//! Real version growth, deliberately separate from the fixed-body receipt workload.
use super::*;

const PROTOCOL: &str = "full-version-history-v1";

fn body(index: usize) -> [u8; CONTENT_BYTES] {
    let mut bytes = [CONTENT_FILL_BYTE; CONTENT_BYTES];
    bytes[..8].copy_from_slice(&(index as u64).to_le_bytes());
    bytes
}

fn verify_shape(engine: &Engine, repo: &Repository, total: usize) -> BenchResult<()> {
    let state = engine.state();
    require(
        !engine.is_read_only()
            && state.sequence == total as u64
            && state.operations.len() == total
            && state.events.len() == total
            && state.repositories.len() == 1
            && state.histories.len() == 1
            && state.versions.len() == total - 1
            && state.commits.len() == total,
        "real-version history cardinality or sequence differs",
    )?;
    let entries = engine.list_tree(&repo.main_branch_id)?;
    require(
        entries.len() == 1 && entries[0].path() == CONTENT_PATH,
        "real-version history must retain one live logical file",
    )
}

fn write_record(ledger: &mut LineWriter<File>, record: &Value) -> BenchResult<()> {
    json_codec::to_writer(&mut *ledger, record)?;
    ledger.write_all(b"\n")?;
    Ok(())
}

fn build(arguments: &Arguments, reporter: &mut Reporter) -> BenchResult<(Engine, Repository)> {
    fs::create_dir_all(&arguments.pool)?;
    fs::create_dir(arguments.pool.join("harness"))?;
    let mut ledger = LineWriter::new(
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(arguments.pool.join("harness/full-versions.jsonl"))?,
    );
    let mut engine = Engine::initialize(
        &arguments.pool.join("system"),
        Config::development([arguments.pool.join("disk")]),
    )?;
    let repo = engine.create_repository(
        &RepositoryName::parse(REPOSITORY_NAME)?,
        "P01",
        1,
        ACTOR,
        operation_id(0)?,
    )?;
    fs::write(
        arguments.pool.join("harness/repository.json"),
        json_codec::to_vec(&repo)?,
    )?;
    let result = engine.operation_record(operation_id(0)?)?.into_result();
    write_record(&mut ledger, &json!({"index":0,"result":result}))?;
    reporter.emit(json!({"phase":"history_setup","verified":true,
        "operation_id":operation_id(0)?.to_string(),"total_receipts":1}))?;
    for index in 1..arguments.operations {
        let workspace = engine.create_workspace(&repo.main_branch_id, ACTOR)?;
        engine.stage_bytes(&workspace.workspace_id, CONTENT_PATH, &body(index))?;
        let request = CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: operation_id(index)?,
            principal: ACTOR.to_owned(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: format!("Real FULL version {index}"),
            request_digest: None,
        };
        let started = Instant::now();
        let result = engine.commit_workspace(request.clone())?;
        let elapsed = started.elapsed().as_nanos();
        let entries = engine.list_tree(&repo.main_branch_id)?;
        require(entries.len() == 1, "publication must retain one live file")?;
        let entry = &entries[0];
        let version = engine
            .state()
            .versions
            .get(&entry.version_id)
            .ok_or_else(|| invalid("published real version missing"))?;
        require(
            result.operation_id == operation_id(index)?
                && result.sequence == index as u64 + 1
                && result.commit_state == CommitState::Published
                && result.protection_at_publish == ProtectionAtPublish::TargetMet
                && result.current_health == CurrentHealth::Healthy,
            "FULL publication contract differs",
        )?;
        verify_shape(&engine, &repo, index + 1)?;
        write_record(
            &mut ledger,
            &json!({"index":index,"request":request,
            "result":result,"version":version}),
        )?;
        reporter.emit(
            json!({"phase":"history_append","verified":true,"index":index,
            "operation_id":result.operation_id.to_string(),"version_id":version.version_id,
            "file_id":version.file_id,"total_receipts":index+1,"history_versions":index,
            "latency_ns":elapsed}),
        )?;
    }
    ledger.flush()?;
    Ok((engine, repo))
}

fn verify_version(
    engine: &Engine,
    expected: &storage_engine::FileVersion,
    index: usize,
    file_id: &str,
) -> BenchResult<()> {
    let actual = engine
        .state()
        .versions
        .get(&expected.version_id)
        .ok_or_else(|| invalid("original version missing"))?;
    require(
        actual == expected
            && expected.file_id == file_id
            && expected.logical_size == CONTENT_BYTES as u64
            && engine.state().content_states.get(&expected.version_id)
                == Some(&storage_engine::ContentState::Available),
        "original FULL version description, file identity or availability changed",
    )?;
    let mut opened = engine.open_content(&expected.version_id)?;
    require(
        opened.size == CONTENT_BYTES as u64,
        "original version length changed",
    )?;
    let mut bytes = Vec::with_capacity(CONTENT_BYTES + 1);
    (&mut opened.file)
        .take((CONTENT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    require(
        bytes.as_slice() == body(index),
        "original FULL version payload changed",
    )
}

fn verify_ledger(
    engine: &mut Engine,
    repo: &Repository,
    arguments: &Arguments,
    reporter: &mut Reporter,
) -> BenchResult<()> {
    let entries = engine.list_tree(&repo.main_branch_id)?;
    require(entries.len() == 1, "expected exactly one current file")?;
    let file_id = entries[0].file_id.clone();
    let history = engine.file_history(&file_id)?;
    require(
        history.len() == arguments.operations - 1,
        "history API version count differs",
    )?;
    // Only the existing API response holds the version list. The ledger and body
    // oracle stream one original record and one bounded 4KiB body at a time.
    let mut history = history.into_iter();
    let mut count = 0;
    for (index, line) in BufReader::new(File::open(
        arguments.pool.join("harness/full-versions.jsonl"),
    )?)
    .lines()
    .enumerate()
    {
        require(index < arguments.operations, "extra original ledger record")?;
        let value: Value = json_codec::from_str(&line?)?;
        require(
            oracle_number(&value, "index")? == index as u64,
            "original ledger index differs",
        )?;
        let expected: OperationResult = json_codec::from_value(value["result"].clone())?;
        require(
            expected.operation_id == operation_id(index)? && expected.sequence == index as u64 + 1,
            "original ledger operation identity differs",
        )?;
        let record = engine.operation_record(operation_id(index)?)?;
        require(
            record.owner() == Some(ACTOR) && record.result() == &expected,
            "original permanent operation result or owner changed",
        )?;
        if index == 0 {
            require(
                engine.create_repository(
                    &RepositoryName::parse(REPOSITORY_NAME)?,
                    "P01",
                    1,
                    ACTOR,
                    operation_id(0)?,
                )? == *repo,
                "repository retry changed identity",
            )?;
        } else {
            let request: CommitRequest = json_codec::from_value(value["request"].clone())?;
            require(
                request.operation_id == operation_id(index)? && request.principal == ACTOR,
                "original publication request identity differs",
            )?;
            let version: storage_engine::FileVersion =
                json_codec::from_value(value["version"].clone())?;
            require(
                history.next().as_ref() == Some(&version),
                "history API changed original version identity or order",
            )?;
            verify_version(engine, &version, index, &file_id)?;
            require(
                engine.commit_workspace(request.clone())? == expected,
                "original FULL request replay changed receipt",
            )?;
            let mut changed = request;
            changed.message.push_str(" changed retry");
            match engine.commit_workspace(changed) {
                Err(error) if error.code() == storage_engine::ErrorCode::IdempotencyKeyReused => (),
                Err(error) => return Err(error.into()),
                Ok(_) => return Err(invalid("mismatched original FULL request accepted").into()),
            }
            verify_version(engine, &version, index, &file_id)?;
        }
        reporter.emit(
            json!({"phase":"history_verify","verified":true,"index":index,
            "operation_id":operation_id(index)?.to_string(),"body_verified":index>0,
            "original_retry_verified":true,"mismatched_retry_rejected":index>0,
            "version_id":value["version"]["version_id"],
            "file_id":if index>0 {Some(file_id.as_str())} else {None}}),
        )?;
        count += 1;
    }
    require(
        count == arguments.operations,
        "missing original ledger record",
    )?;
    require(history.next().is_none(), "extra history API version")?;
    let mut events = 0;
    for (index, event) in engine.events_after(0).enumerate() {
        require(
            index < arguments.operations
                && event.operation_id == operation_id(index)?
                && event.sequence == index as u64 + 1
                && event.kind
                    == if index == 0 {
                        "repository.create"
                    } else {
                        "workspace.commit"
                    },
            "event identity/order changed or duplicate publication",
        )?;
        events += 1;
    }
    require(events == arguments.operations, "missing original event")?;
    verify_shape(engine, repo, arguments.operations)?;
    reporter.emit(
        json!({"phase":"history_oracle","verified":true,"records":count,
        "history_versions":count-1,"full_bodies_verified":count-1,
        "events":events,"no_new_publication":true}),
    )
}

pub(super) fn run(arguments: &Arguments, reporter: &mut Reporter) -> BenchResult<()> {
    reporter.emit(json!({"phase":"configuration","verified":true,"stage":arguments.phase,
        "mode":arguments.mode,"operations":arguments.operations,"content_versions":arguments.operations-1,
        "metadata_members":1,"content_bytes":CONTENT_BYTES,"fixture_mode":"full-versions",
        "query_protocol":PROTOCOL,"workload":"one logical file with successive distinct FULL versions",
        "execution_environment":if arguments.local_calibration {"local-calibration"} else {"github-actions"},
        "history_scale_current":true,"history_scale_baseline":false,
        "scale_authorized":arguments.scale_authorized,"scrub_schedule_registered":false,
        "measurement_scope":"trusted Engine; explicit history version growth, not HTTP or physical disk acceptance"}))?;
    process_snapshot(reporter, "process_start")?;
    let (mut engine, repo) = if arguments.phase == "build" {
        build(arguments, reporter)?
    } else {
        let started = Instant::now();
        let engine = if arguments.phase == "reopen" {
            Engine::open(&arguments.pool.join("system"))?
        } else {
            Engine::restore_from_disks(
                &arguments.pool.join("recovered-system"),
                &[arguments.pool.join("disk")],
            )?
        };
        let elapsed = started.elapsed().as_nanos();
        process_snapshot(reporter, "after_admission_before_oracle")?;
        let repo =
            json_codec::from_slice(&fs::read(arguments.pool.join("harness/repository.json"))?)?;
        verify_shape(&engine, &repo, arguments.operations)?;
        reporter.emit(json!({"phase":arguments.phase,"verified":true,"latency_ns":elapsed}))?;
        (engine, repo)
    };
    verify_ledger(&mut engine, &repo, arguments, reporter)?;
    process_snapshot(reporter, "after_verification")?;
    let started = Instant::now();
    drop(engine);
    reporter
        .emit(json!({"phase":"close","verified":true,"latency_ns":started.elapsed().as_nanos()}))?;
    process_snapshot(reporter, "after_close")?;
    reporter.emit(json!({"phase":"complete","verified":true,"stage":arguments.phase,
        "operations":arguments.operations,"actual_total_receipts":arguments.operations,
        "actual_total_events":arguments.operations,"actual_history_versions":arguments.operations-1}))
}

#[cfg(test)]
#[path = "../../test/history-scale-bench/full_versions.rs"]
mod tests;
