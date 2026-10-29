//! Same public-engine workload for both pinned revisions. Cargo wiring belongs
//! to the disposable validation carrier, never the product commit ancestry.
use json_codec::{Value, json};
use std::{
    error::Error,
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, LineWriter, Read, Write},
    path::{Path, PathBuf},
    time::Instant,
};
use storage_engine::{
    CommitRequest, CommitState, Config, CurrentHealth, Engine, Event, OperationId, OperationResult,
    ProtectionAtPublish, Repository, RepositoryName,
};

type BenchResult<T> = Result<T, Box<dyn Error>>;
const ACTOR: &str = "history-scale-bench";
const REPOSITORY_NAME: &str = "Permanent history scale";
const QUERY_REPETITIONS: usize = 25;
const EVENT_PAGE_SIZE: usize = 257;
const SETUP_RECEIPTS: usize = 2;
const CONTENT_PATH: &str = "fixture.bin";
const CONTENT_BYTES: usize = 4096;
const CONTENT_FILL_BYTE: u8 = 0x5a;

#[cfg(all(feature = "history-scale-baseline", feature = "history-scale-current"))]
compile_error!("history scale driver revision adapters are mutually exclusive");
#[cfg(not(any(feature = "history-scale-baseline", feature = "history-scale-current")))]
compile_error!("select an explicit history scale driver revision adapter");

struct Arguments {
    pool: PathBuf,
    result: PathBuf,
    operations: usize,
    phase: String,
    mode: String,
    scale_authorized: bool,
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn require(condition: bool, message: &str) -> BenchResult<()> {
    if !condition {
        return Err(invalid(message).into());
    }
    Ok(())
}

impl Arguments {
    fn parse() -> BenchResult<Self> {
        let mut flags = std::collections::BTreeMap::new();
        let mut scale_authorized = false;
        let mut words = std::env::args().skip(1);
        while let Some(flag) = words.next() {
            if flag == "--scale-authorized" {
                require(!scale_authorized, "duplicate scale authorization flag")?;
                scale_authorized = true;
                continue;
            }
            require(
                matches!(
                    flag.as_str(),
                    "--pool-root" | "--result" | "--operations" | "--phase" | "--mode"
                ),
                "unknown driver argument",
            )?;
            let value = words.next().ok_or_else(|| invalid("missing flag value"))?;
            require(flags.insert(flag, value).is_none(), "duplicate argument")?;
        }
        let mut take = |name: &str| {
            flags
                .remove(name)
                .ok_or_else(|| invalid(format!("missing argument {name}")))
        };
        let operations = take("--operations")?.parse::<usize>()?;
        require(
            (4..=1_000_000).contains(&operations),
            "operations must be 4..=1000000 (including two setup receipts)",
        )?;
        require(
            operations <= 16 || scale_authorized,
            "more than 16 operations requires --scale-authorized after the runner freeze gate",
        )?;
        let phase = take("--phase")?;
        require(
            matches!(phase.as_str(), "build" | "reopen" | "recovery"),
            "phase must be build, reopen, or recovery",
        )?;
        let mode = take("--mode")?;
        require(
            matches!(mode.as_str(), "timing" | "counts"),
            "mode must be timing or counts",
        )?;
        Ok(Self {
            pool: take("--pool-root")?.into(),
            result: take("--result")?.into(),
            operations,
            phase,
            mode,
            scale_authorized,
        })
    }
}

struct Reporter(LineWriter<File>);

impl Reporter {
    fn new(path: &Path) -> BenchResult<Self> {
        Ok(Self(LineWriter::new(
            OpenOptions::new().write(true).create_new(true).open(path)?,
        )))
    }

    fn emit(&mut self, value: Value) -> BenchResult<()> {
        json_codec::to_writer(&mut self.0, &value)?;
        self.0.write_all(b"\n")?;
        Ok(())
    }
}

// Multiplication by an odd number permutes u128: reproducible, distributed IDs
// with no resident ID table, and exactly the same request identities in each pool.
fn operation_id(index: usize) -> BenchResult<OperationId> {
    let bits = (index as u128 + 1).wrapping_mul(0x9e3779b97f4a7c15_d6e8feb86659fd93);
    Ok(format!("op_{bits:032x}").parse()?)
}

fn proc_counter(text: &str, name: &str) -> BenchResult<u64> {
    let value = text
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| *key == name)
        .and_then(|(_, value)| value.split_whitespace().next())
        .ok_or_else(|| invalid(format!("missing /proc counter {name}")))?;
    Ok(value.parse()?)
}

fn process_snapshot(reporter: &mut Reporter, point: &str) -> BenchResult<()> {
    let status = fs::read_to_string("/proc/self/status")?;
    let io = fs::read_to_string("/proc/self/io")?;
    reporter.emit(json!({
        "phase": "telemetry", "point": point,
        "vmrss_kib": proc_counter(&status, "VmRSS")?,
        "vmhwm_kib": proc_counter(&status, "VmHWM")?,
        "rchar": proc_counter(&io, "rchar")?,
        "wchar": proc_counter(&io, "wchar")?,
        "read_bytes": proc_counter(&io, "read_bytes")?,
        "write_bytes": proc_counter(&io, "write_bytes")?,
        "scope": "whole driver process, including oracle and result I/O"
    }))
}

struct Fixture {
    repo: Repository,
    request: CommitRequest,
    seed_result: OperationResult,
    version_id: String,
    file_id: String,
    active_shape: Value,
    history_records: Value,
    version_descriptor: Value,
    history_generation: u64,
    binding_generation: u64,
}

fn active_shape(engine: &Engine, repo: &Repository) -> BenchResult<Value> {
    let state = engine.state();
    let branch = state
        .branches
        .get(&repo.main_branch_id)
        .ok_or_else(|| invalid("fixture branch missing"))?;
    Ok(json!({
        "repositories": state.repositories.len(), "histories": state.histories.len(),
        "branches": state.branches.len(), "commits": state.commits.len(),
        "versions": state.versions.len(), "placements": state.placements.len(),
        "directory_branches": state.directory_branches.len(),
        "directory_trees": state.directory_trees.len(),
        "file_locations": state.file_locations.len(),
        "main_branch_head": branch.head_commit_id,
        "main_branch_generation": branch.generation
    }))
}

fn history_records(engine: &Engine, repo: &Repository) -> BenchResult<Value> {
    let graph = engine.graph(&repo.history_tree_id)?;
    Ok(json!({
        "commits": graph.commits, "branches": graph.branches,
        "merge_links": graph.merge_links, "boundary_marks": graph.boundary_marks
    }))
}

fn oracle_string(value: &Value, key: &str) -> BenchResult<String> {
    Ok(value[key]
        .as_str()
        .ok_or_else(|| invalid(format!("fixture string {key} missing")))?
        .to_owned())
}

fn oracle_number(value: &Value, key: &str) -> BenchResult<u64> {
    Ok(value[key]
        .as_u64()
        .ok_or_else(|| invalid(format!("fixture number {key} missing")))?)
}

impl Fixture {
    fn capture(
        engine: &Engine,
        repo: Repository,
        request: CommitRequest,
        seed_result: OperationResult,
    ) -> BenchResult<Self> {
        let entries = engine.list_tree(&repo.main_branch_id)?;
        require(entries.len() == 1, "fixed fixture must contain one file")?;
        let entry = &entries[0];
        require(
            entry.path() == CONTENT_PATH && entry.size == CONTENT_BYTES as u64,
            "fixed fixture path or size differs",
        )?;
        let versions = engine.file_history(&entry.file_id)?;
        require(versions.len() == 1, "fixed fixture must have one version")?;
        let history_generation = engine.state().histories[&repo.history_tree_id].generation;
        let binding_generation = engine.state().repositories[&repo.repository_id].smb_binding_generation;
        Ok(Self {
            active_shape: active_shape(engine, &repo)?,
            history_records: history_records(engine, &repo)?,
            version_descriptor: json_codec::to_value(&versions[0])?,
            history_generation,
            binding_generation,
            version_id: entry.version_id.clone(),
            file_id: entry.file_id.clone(),
            repo,
            request,
            seed_result,
        })
    }

    fn save(&self, pool: &Path) -> BenchResult<()> {
        fs::write(
            pool.join("harness/content-fixture.json"),
            json_codec::to_vec(&json!({
                "fixture_mode": "fixed-body", "content_bytes": CONTENT_BYTES,
                "fill_byte": CONTENT_FILL_BYTE, "path": CONTENT_PATH,
                "commit_request": self.request, "seed_result": self.seed_result,
                "version_id": self.version_id, "file_id": self.file_id,
                "active_shape": self.active_shape, "history_records": self.history_records,
                "version_descriptor": self.version_descriptor,
                "history_generation": self.history_generation,
                "binding_generation": self.binding_generation
            }))?,
        )?;
        Ok(())
    }

    fn load(pool: &Path) -> BenchResult<Self> {
        let repo = json_codec::from_slice(&fs::read(pool.join("harness/repository.json"))?)?;
        let value: Value = json_codec::from_slice(&fs::read(pool.join("harness/content-fixture.json"))?)?;
        require(
            value["fixture_mode"].as_str() == Some("fixed-body")
                && oracle_number(&value, "content_bytes")? == CONTENT_BYTES as u64
                && oracle_number(&value, "fill_byte")? == CONTENT_FILL_BYTE as u64
                && value["path"].as_str() == Some(CONTENT_PATH),
            "fixed content oracle parameters differ",
        )?;
        let request: CommitRequest = json_codec::from_value(value["commit_request"].clone())?;
        require(
            request.operation_id == operation_id(1)? && request.principal == ACTOR,
            "fixed content request identity differs",
        )?;
        Ok(Self {
            repo,
            request,
            seed_result: json_codec::from_value(value["seed_result"].clone())?,
            version_id: oracle_string(&value, "version_id")?,
            file_id: oracle_string(&value, "file_id")?,
            active_shape: value["active_shape"].clone(),
            history_records: value["history_records"].clone(),
            version_descriptor: value["version_descriptor"].clone(),
            history_generation: oracle_number(&value, "history_generation")?,
            binding_generation: oracle_number(&value, "binding_generation")?,
        })
    }

    fn history_generation_at(&self, total: usize) -> BenchResult<u64> {
        Ok(self.history_generation
            .checked_add((total - SETUP_RECEIPTS) as u64)
            .ok_or_else(|| invalid("fixture history generation overflow"))?)
    }

    fn binding_generation_at(&self, total: usize) -> BenchResult<u64> {
        Ok(self.binding_generation
            .checked_add((total - SETUP_RECEIPTS) as u64)
            .ok_or_else(|| invalid("fixture binding generation overflow"))?)
    }
}

fn binding_enabled(index: usize) -> bool {
    (index - SETUP_RECEIPTS) % 2 == 1
}

fn verify_active_set(engine: &Engine, fixture: &Fixture, operations: usize) -> BenchResult<()> {
    let state = engine.state();
    let repo = &fixture.repo;
    require(!engine.is_read_only(), "healthy fixture became read-only")?;
    require(
        state.sequence == operations as u64
            && state.operations.len() as u64 == operations as u64
            && state.events.len() as u64 == operations as u64
            && state.repositories.len() == 1
            && state.histories.len() == 1
            && state.versions.len() == 1
            && active_shape(engine, repo)? == fixture.active_shape,
        "operation sequence or constant active-set invariant differs",
    )?;
    let current = state
        .repositories
        .get(&repo.repository_id)
        .ok_or_else(|| invalid("fixture repository missing"))?;
    let history = state
        .histories
        .get(&repo.history_tree_id)
        .ok_or_else(|| invalid("fixture history missing"))?;
    require(
        current.main_branch_id == repo.main_branch_id
            && current.smb_binding_generation == fixture.binding_generation_at(operations)?
            && current.smb_enabled == ((operations - SETUP_RECEIPTS) % 2 == 0)
            && history.generation == fixture.history_generation_at(operations)?,
        "live SMB binding or generation differs from the published workload",
    )
}

fn verify_receipt_envelope(
    result: &OperationResult,
    index: usize,
    repo: &Repository,
) -> BenchResult<()> {
    let protection = if index == 1 {
        ProtectionAtPublish::TargetMet
    } else {
        ProtectionAtPublish::NotApplicable
    };
    require(
        result.operation_id == operation_id(index)?
            && result.sequence == index as u64 + 1
            && result.commit_state == CommitState::Published
            && result.protection_at_publish == protection
            && result.current_health == CurrentHealth::Healthy
            && result.branch_id.as_deref() == Some(repo.main_branch_id.as_str()),
        "receipt identity, sequence, publication, protection, or branch differs",
    )
}

fn verify_receipt(result: &OperationResult, index: usize, fixture: &Fixture) -> BenchResult<()> {
    let repo = &fixture.repo;
    verify_receipt_envelope(result, index, repo)?;
    if index == 0 {
        require(
            result.kind == "repository.create"
                && result.history_tree_id.as_deref() == Some(repo.history_tree_id.as_str())
                && result.commit_id.as_deref() == Some(fixture.request.expected_head.as_str()),
            "setup receipt differs",
        )?;
    } else if index == 1 {
        require(
            result == &fixture.seed_result
                && result.kind == "workspace.commit"
                && result.history_tree_id.as_deref() == Some(repo.history_tree_id.as_str())
                && result.commit_id.is_some(),
            "original fixed content publication differs",
        )?;
    } else {
        require(
            result.kind == "smb.binding.update"
                && result.commit_id.is_none()
                && result.history_tree_id.is_none()
                && result.result["repository_id"].as_str() == Some(repo.repository_id.as_str())
                && result.result["smb_enabled"].as_bool() == Some(binding_enabled(index))
                && result.result["generation"].as_str()
                    == Some(fixture.history_generation_at(index + 1)?.to_string().as_str())
                && result.result["smb_binding_generation"].as_str()
                    == Some(fixture.binding_generation_at(index + 1)?.to_string().as_str()),
            "metadata receipt payload differs from the original request",
        )?;
    }
    Ok(())
}

fn build(arguments: &Arguments, reporter: &mut Reporter) -> BenchResult<(Engine, Fixture)> {
    fs::create_dir_all(&arguments.pool)?;
    // create_new admission preserves all earlier evidence if a pool is reused.
    fs::create_dir(arguments.pool.join("harness"))?;
    let mut ledger = LineWriter::new(
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(arguments.pool.join("harness/receipts.jsonl"))?,
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
    let original = engine.operation_record(operation_id(0)?)?.into_result();
    verify_receipt_envelope(&original, 0, &repo)?;
    require(
        original.kind == "repository.create"
            && original.history_tree_id.as_deref() == Some(repo.history_tree_id.as_str())
            && original.commit_id.as_deref()
                == Some(engine.state().branches[&repo.main_branch_id].head_commit_id.as_str()),
        "original repository setup result differs",
    )?;
    json_codec::to_writer(&mut ledger, &original)?;
    ledger.write_all(b"\n")?;
    reporter.emit(json!({
        "phase": "setup", "index": 0, "verified": true, "result": original,
        "setup_receipts": 1, "setup_events": 1,
        "target_total_receipts": arguments.operations,
        "payload_operations": arguments.operations - SETUP_RECEIPTS
    }))?;
    let fixture = seed_content(&mut engine, repo)?;
    fixture.save(&arguments.pool)?;
    verify_receipt(&original, 0, &fixture)?;
    verify_receipt(&fixture.seed_result, 1, &fixture)?;
    verify_active_set(&engine, &fixture, SETUP_RECEIPTS)?;
    json_codec::to_writer(&mut ledger, &fixture.seed_result)?;
    ledger.write_all(b"\n")?;
    reporter.emit(json!({
        "phase": "setup", "index": 1, "verified": true, "result": fixture.seed_result,
        "setup_receipts": SETUP_RECEIPTS, "setup_events": SETUP_RECEIPTS,
        "target_total_receipts": arguments.operations,
        "payload_operations": arguments.operations - SETUP_RECEIPTS,
        "commit_request": fixture.request, "seed_version_id": fixture.version_id,
        "seed_file_id": fixture.file_id, "seed_path": CONTENT_PATH,
        "content_bytes": CONTENT_BYTES, "fill_byte": CONTENT_FILL_BYTE,
        "active_shape": fixture.active_shape,
        "base_history_generation": fixture.history_generation,
        "base_binding_generation": fixture.binding_generation
    }))?;
    process_snapshot(reporter, "before_append")?;
    for index in SETUP_RECEIPTS..arguments.operations {
        let id = operation_id(index)?;
        let generation = fixture.history_generation_at(index)?;
        let enabled = binding_enabled(index);
        let started = Instant::now();
        let result = engine.set_smb_binding(
            &fixture.repo.repository_id,
            &fixture.repo.main_branch_id,
            enabled,
            generation,
            ACTOR,
            id,
        )?;
        let elapsed = started.elapsed().as_nanos();
        verify_receipt(&result, index, &fixture)?;
        verify_active_set(&engine, &fixture, index + 1)?;
        json_codec::to_writer(&mut ledger, &result)?;
        ledger.write_all(b"\n")?;
        reporter.emit(json!({
            "phase": "append", "index": index,
            "total_receipts": index + 1, "total_events": index + 1,
            "operation_id": id.to_string(), "latency_ns": elapsed,
            "verified": true, "result": result
        }))?;
    }
    ledger.flush()?;
    process_snapshot(reporter, "after_append")?;
    Ok((engine, fixture))
}

fn seed_content(engine: &mut Engine, repo: Repository) -> BenchResult<Fixture> {
    let workspace = engine.create_workspace(&repo.main_branch_id, ACTOR)?;
    engine.stage_bytes(
        &workspace.workspace_id, CONTENT_PATH, &[CONTENT_FILL_BYTE; CONTENT_BYTES],
    )?;
    let request = CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: operation_id(1)?,
        principal: ACTOR.to_owned(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "Fixed 4KiB content recovery oracle".to_owned(),
        request_digest: None,
    };
    let result = engine.commit_workspace(request.clone())?;
    Fixture::capture(engine, repo, request, result)
}

fn original_samples(pool: &Path, operations: usize) -> BenchResult<Vec<(usize, OperationResult)>> {
    let selected = [0, operations / 2, operations - 1];
    let mut samples = Vec::with_capacity(3);
    for (index, line) in BufReader::new(File::open(pool.join("harness/receipts.jsonl"))?)
        .lines()
        .enumerate()
    {
        let line = line?;
        if selected.contains(&index) {
            samples.push((index, json_codec::from_str(&line)?));
        }
    }
    require(samples.len() == 3, "missing original sample receipts")?;
    Ok(samples)
}

#[cfg(feature = "history-scale-current")]
type QueryOutcome = storage_engine::OperationOutcome;
#[cfg(feature = "history-scale-baseline")]
type QueryOutcome = OperationResult;

fn permanent_outcome(result: OperationResult) -> QueryOutcome {
    // The old revision has no OperationOutcome domain type. Its immutable
    // operation_record is the corresponding receipt-only local consumer.
    #[cfg(feature = "history-scale-current")]
    { result.into() }
    #[cfg(feature = "history-scale-baseline")]
    { result }
}

fn outcome_api() -> &'static str {
    if cfg!(feature = "history-scale-current") {
        "Engine::operation_record+OperationOutcome::from"
    } else {
        "Engine::operation_record (legacy permanent receipt)"
    }
}

fn check_queries(
    engine: &Engine,
    samples: &[(usize, OperationResult)],
    reporter: &mut Reporter,
) -> BenchResult<()> {
    for (sample, (index, expected)) in samples.iter().enumerate() {
        let position = if sample == 0 {
            "earliest"
        } else if sample + 1 == samples.len() {
            "recent"
        } else {
            "middle"
        };
        let expected_outcome = permanent_outcome(expected.clone());
        for repetition in 0..QUERY_REPETITIONS {
            let started = Instant::now();
            let record = engine.operation_record(expected.operation_id)?;
            let owner_matches = record.owner() == Some(ACTOR);
            let actual = permanent_outcome(record.into_result());
            let elapsed = started.elapsed().as_nanos();
            require(
                owner_matches && actual == expected_outcome,
                "queried receipt or original owner binding changed",
            )?;
            reporter.emit(json!({
                "phase": "query", "api": outcome_api(), "position": position,
                "query_kind": "permanent_outcome",
                "index": index, "operation_id": expected.operation_id.to_string(),
                "repetition": repetition, "latency_ns": elapsed, "verified": true,
                "cache_observation": if repetition == 0 { "first_after_admission_or_append" } else { "repeated" }
            }))?;
        }
    }
    Ok(())
}

fn observe_health(
    engine: &Engine,
    id: OperationId,
) -> BenchResult<(storage_engine::OperationRecord, Option<CurrentHealth>)> {
    let record = engine.operation_record(id)?;
    #[cfg(feature = "history-scale-current")]
    {
        let observation = engine.prepare_operation_health(record).observe();
        let health = observation.health();
        Ok((observation.into_record(), health))
    }
    #[cfg(feature = "history-scale-baseline")]
    {
        // check() performs the old real commit/payload diagnostic. It exposes
        // no NotObserved type: scope classification here belongs to this fixed
        // fixture, never a claim about the old public endpoint representation.
        let has_commit_scope = record.result().commit_id.is_some();
        let observed = engine.prepare_operation_health(record).check();
        let health = has_commit_scope.then_some(observed.result().current_health);
        Ok((observed, health))
    }
}

fn check_health_queries(
    engine: &Engine,
    fixture: &Fixture,
    samples: &[(usize, OperationResult)],
    reporter: &mut Reporter,
) -> BenchResult<()> {
    let selected = [
        ("content_fixture", 1, &fixture.seed_result),
        ("middle", samples[1].0, &samples[1].1),
        ("recent", samples[2].0, &samples[2].1),
    ];
    for (position, index, expected) in selected {
        let expected_outcome = permanent_outcome(expected.clone());
        let expected_health = (index == 1).then_some(CurrentHealth::Healthy);
        for repetition in 0..QUERY_REPETITIONS {
            let started = Instant::now();
            let (record, health) = observe_health(engine, expected.operation_id)?;
            let elapsed = started.elapsed().as_nanos();
            require(
                record.owner() == Some(ACTOR)
                    && permanent_outcome(record.into_result()) == expected_outcome
                    && health == expected_health,
                "content diagnostic, publication facts, or unobserved metadata scope differs",
            )?;
            reporter.emit(json!({
                "phase": "health_query", "query_kind": "current_health",
                "position": position, "index": index,
                "operation_id": expected.operation_id.to_string(),
                "repetition": repetition, "latency_ns": elapsed, "verified": true,
                "health_observed": health.is_some(),
                "health_state": health.map(|value| value.as_str()),
                "scope_versions": if index == 1 { 1 } else { 0 },
                "observation_contract": if cfg!(feature = "history-scale-current") {
                    "native_observe"
                } else {
                    "legacy_commit_check"
                },
                "api": if cfg!(feature = "history-scale-current") {
                    "operation_record+prepare_operation_health.observe"
                } else {
                    "operation_record+prepare_operation_health.check"
                },
                "cache_observation": if repetition == 0 {
                    "first_after_outcome_queries"
                } else {
                    "repeated"
                }
            }))?;
        }
    }
    Ok(())
}

#[cfg(feature = "history-scale-baseline")]
fn events_page(engine: &Engine, after: u64) -> storage_engine::Result<Vec<Event>> {
    Ok(engine
        .events_after(after)
        .take(EVENT_PAGE_SIZE)
        .cloned()
        .collect())
}

#[cfg(not(feature = "history-scale-baseline"))]
fn events_page(engine: &Engine, after: u64) -> storage_engine::Result<Vec<Event>> {
    engine.events_page(after, EVENT_PAGE_SIZE)
}

fn check_events(
    engine: &Engine,
    fixture: &Fixture,
    operations: usize,
    reporter: &mut Reporter,
) -> BenchResult<()> {
    let mut cursor = 0;
    let mut ordinal = 0;
    loop {
        let requested_after = cursor;
        let started = Instant::now();
        let page = events_page(engine, cursor)?;
        let elapsed = started.elapsed().as_nanos();
        for event in &page {
            require(ordinal < operations, "extra or repeated event")?;
            let expected_id = operation_id(ordinal)?;
            let (kind, object) = if ordinal == 0 {
                ("repository.create", fixture.request.expected_head.clone())
            } else if ordinal == 1 {
                ("workspace.commit", fixture.seed_result.commit_id.clone()
                    .ok_or_else(|| invalid("content fixture commit missing"))?)
            } else {
                ("smb.binding.update", expected_id.to_string())
            };
            require(
                event.sequence == ordinal as u64 + 1
                    && event.sequence > cursor
                    && event.operation_id == expected_id
                    && event.kind == kind
                    && event.object_id == object,
                "event identity, publication order, kind, or object binding differs",
            )?;
            cursor = event.sequence;
            ordinal += 1;
        }
        reporter.emit(json!({
            "phase": "events", "after_sequence": requested_after,
            "next_sequence": cursor,
            "page_size": EVENT_PAGE_SIZE, "records": page.len(),
            "verified_events": ordinal, "latency_ns": elapsed, "verified": true
        }))?;
        if page.is_empty() {
            break;
        }
    }
    require(ordinal == operations, "missing published events")?;
    verify_active_set(engine, fixture, operations)
}

fn verify_all_receipts(
    engine: &Engine,
    fixture: &Fixture,
    arguments: &Arguments,
    reporter: &mut Reporter,
) -> BenchResult<()> {
    let started = Instant::now();
    let mut count = 0;
    for (index, line) in BufReader::new(File::open(arguments.pool.join("harness/receipts.jsonl"))?)
        .lines()
        .enumerate()
    {
        let expected: OperationResult = json_codec::from_str(&line?)?;
        verify_receipt(&expected, index, fixture)?;
        let actual = engine.operation_record(expected.operation_id)?;
        require(
            actual.owner() == Some(ACTOR) && actual.result() == &expected,
            "permanent original ID/result/owner binding changed",
        )?;
        count += 1;
    }
    require(
        count == arguments.operations,
        "original ledger length differs",
    )?;
    reporter.emit(json!({
        "phase": "verify_receipts", "records": count,
        "latency_ns": started.elapsed().as_nanos(), "verified": true,
        "oracle_memory": "one line, three sampled receipts, and one fixed content/history fixture"
    }))
}

fn verify_retries(
    engine: &mut Engine,
    fixture: &Fixture,
    samples: &[(usize, OperationResult)],
    operations: usize,
    reporter: &mut Reporter,
) -> BenchResult<()> {
    let repo = &fixture.repo;
    for (index, expected) in samples {
        let started = Instant::now();
        if *index == 0 {
            let actual = engine.create_repository(
                &RepositoryName::parse(REPOSITORY_NAME)?,
                "P01",
                1,
                ACTOR,
                expected.operation_id,
            )?;
            require(
                actual == *repo,
                "creation retry changed its original response",
            )?;
        } else {
            let actual = engine.set_smb_binding(
                &repo.repository_id,
                &repo.main_branch_id,
                binding_enabled(*index),
                fixture.history_generation_at(*index)?,
                ACTOR,
                expected.operation_id,
            )?;
            require(
                actual == *expected,
                "same-request retry changed its permanent result",
            )?;
            let mismatch = engine.set_smb_binding(
                &repo.repository_id,
                &repo.main_branch_id,
                !binding_enabled(*index),
                fixture.history_generation_at(*index)?,
                ACTOR,
                expected.operation_id,
            );
            match mismatch {
                Err(error) if error.code() == storage_engine::ErrorCode::IdempotencyKeyReused => (),
                Err(error) => return Err(error.into()),
                Ok(_) => return Err(invalid("mismatched original ID was accepted").into()),
            }
        }
        verify_active_set(engine, fixture, operations)?;
        reporter.emit(json!({
            "phase": "retry", "index": index, "operation_id": expected.operation_id.to_string(),
            "latency_ns": started.elapsed().as_nanos(), "verified": true
        }))?;
    }
    Ok(())
}

fn verify_content(
    engine: &Engine,
    fixture: &Fixture,
    after_retry: bool,
    reporter: &mut Reporter,
) -> BenchResult<()> {
    let started = Instant::now();
    let entries = engine.list_tree(&fixture.repo.main_branch_id)?;
    require(entries.len() == 1, "fixed active file count changed")?;
    let entry = &entries[0];
    require(
        entry.path() == CONTENT_PATH
            && entry.file_id == fixture.file_id
            && entry.version_id == fixture.version_id
            && entry.size == CONTENT_BYTES as u64,
        "recovered namespace no longer names the exact original content version",
    )?;
    let versions = engine.file_history(&fixture.file_id)?;
    require(
        versions.len() == 1
            && json_codec::to_value(&versions[0])? == fixture.version_descriptor
            && history_records(engine, &fixture.repo)? == fixture.history_records,
        "original version description or complete fixed history graph changed",
    )?;
    // open_content performs the real proof and full-payload verification.
    // This extra tool oracle is deliberately outside Engine::open timing/RSS.
    let mut opened = engine.open_content(&fixture.version_id)?;
    require(opened.size == CONTENT_BYTES as u64, "opened content length differs")?;
    let mut bytes = Vec::with_capacity(CONTENT_BYTES + 1);
    (&mut opened.file).take((CONTENT_BYTES + 1) as u64).read_to_end(&mut bytes)?;
    require(
        bytes.as_slice() == [CONTENT_FILL_BYTE; CONTENT_BYTES],
        "full original 4KiB content bytes changed",
    )?;
    reporter.emit(json!({
        "phase": "verify_content", "verified": true,
        "file_id": fixture.file_id, "version_id": fixture.version_id,
        "path": CONTENT_PATH, "content_bytes": CONTENT_BYTES,
        "fill_byte": CONTENT_FILL_BYTE, "after_retry": after_retry,
        "history_versions": versions.len(), "history_records_verified": true,
        "latency_ns": started.elapsed().as_nanos()
    }))
}

fn verify_content_retry(
    engine: &mut Engine,
    fixture: &Fixture,
    operations: usize,
    reporter: &mut Reporter,
) -> BenchResult<()> {
    let started = Instant::now();
    let actual = engine.commit_workspace(fixture.request.clone())?;
    require(actual == fixture.seed_result, "original content request replay changed its result")?;
    let mut mismatch = fixture.request.clone();
    mismatch.message.push_str(" mismatched retry");
    match engine.commit_workspace(mismatch) {
        Err(error) if error.code() == storage_engine::ErrorCode::IdempotencyKeyReused => (),
        Err(error) => return Err(error.into()),
        Ok(_) => return Err(invalid("changed original content request was accepted").into()),
    }
    verify_active_set(engine, fixture, operations)?;
    reporter.emit(json!({
        "phase": "retry_content", "index": 1,
        "operation_id": fixture.request.operation_id.to_string(),
        "result": actual, "latency_ns": started.elapsed().as_nanos(),
        "verified": true, "no_new_publication": true
    }))
}

fn run(arguments: &Arguments, reporter: &mut Reporter) -> BenchResult<()> {
    reporter.emit(json!({
        "phase": "configuration", "stage": arguments.phase, "mode": arguments.mode,
        "operations": arguments.operations, "verified": true,
        "payload_operations": arguments.operations - SETUP_RECEIPTS,
        "setup_receipts": SETUP_RECEIPTS, "setup_events": SETUP_RECEIPTS,
        "metadata_members": 1, "content_versions": 1, "development_mode": true,
        "content_bytes": CONTENT_BYTES, "fill_byte": CONTENT_FILL_BYTE,
        "fixture_mode": "fixed-body", "query_protocol": "separate-outcome-health",
        "workload": "alternate SMB binding after one fixed 4KiB content publication",
        "history_scale_baseline": cfg!(feature = "history-scale-baseline"),
        "history_scale_current": cfg!(feature = "history-scale-current"),
        "scale_authorized": arguments.scale_authorized,
        "measurement_scope": "trusted local Engine consumers; excludes HTTP transport and authentication",
        "query_order": ["permanent_outcome", "current_health", "events"],
        "scrub_schedule_registered": false
    }))?;
    process_snapshot(reporter, "process_start")?;
    let (mut engine, fixture) = if arguments.phase == "build" {
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
        // Capture RSS before the oracle scan or point queries warm anything.
        process_snapshot(reporter, "after_admission_before_oracle")?;
        let fixture = Fixture::load(&arguments.pool)?;
        verify_active_set(&engine, &fixture, arguments.operations)?;
        reporter.emit(json!({
            "phase": arguments.phase, "latency_ns": elapsed, "verified": true,
            "api": if arguments.phase == "reopen" { "Engine::open" } else { "Engine::restore_from_disks" }
        }))?;
        (engine, fixture)
    };
    let samples = original_samples(&arguments.pool, arguments.operations)?;
    check_queries(&engine, &samples, reporter)?;
    check_health_queries(&engine, &fixture, &samples, reporter)?;
    check_events(&engine, &fixture, arguments.operations, reporter)?;
    verify_all_receipts(&engine, &fixture, arguments, reporter)?;
    verify_content(&engine, &fixture, false, reporter)?;
    verify_retries(&mut engine, &fixture, &samples, arguments.operations, reporter)?;
    verify_content_retry(&mut engine, &fixture, arguments.operations, reporter)?;
    verify_content(&engine, &fixture, true, reporter)?;
    // The second full event walk detects extra publication from any replay,
    // including duplicates hidden by a sequence-only cursor.
    check_events(&engine, &fixture, arguments.operations, reporter)?;
    process_snapshot(reporter, "after_verification")?;
    let started = Instant::now();
    drop(engine);
    reporter.emit(json!({
        "phase": "close", "latency_ns": started.elapsed().as_nanos(), "verified": true
    }))?;
    process_snapshot(reporter, "after_close")?;
    reporter.emit(json!({
        "phase": "complete", "stage": arguments.phase,
        "operations": arguments.operations, "actual_total_receipts": arguments.operations,
        "actual_total_events": arguments.operations, "verified": true
    }))
}

fn main() -> BenchResult<()> {
    require(
        matches!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true")),
        "history scale driver executes only in GitHub Actions",
    )?;
    let arguments = Arguments::parse()?;
    let mut reporter = Reporter::new(&arguments.result)?;
    match run(&arguments, &mut reporter) {
        Ok(()) => Ok(()),
        Err(error) => {
            reporter.emit(json!({
                "phase": "failure", "stage": arguments.phase,
                "verified": false, "error": error.to_string()
            }))?;
            Err(error)
        }
    }
}
