//! Same public-engine workload for both pinned revisions. Cargo wiring belongs
//! to the disposable validation carrier, never the product commit ancestry.
use json_codec::{Value, json};
use std::{
    error::Error,
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, LineWriter, Write},
    path::{Path, PathBuf},
    time::Instant,
};
use storage_engine::{
    CommitState, Config, CurrentHealth, Engine, Event, OperationId, OperationResult,
    ProtectionAtPublish, Repository, RepositoryName,
};

type BenchResult<T> = Result<T, Box<dyn Error>>;
const ACTOR: &str = "history-scale-bench";
const REPOSITORY_NAME: &str = "Permanent history scale";
const QUERY_REPETITIONS: usize = 25;
const EVENT_PAGE_SIZE: usize = 257;

struct Arguments {
    pool: PathBuf,
    result: PathBuf,
    operations: usize,
    phase: String,
    mode: String,
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
        let mut words = std::env::args().skip(1);
        while let Some(flag) = words.next() {
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
            (2..=1_000_000).contains(&operations),
            "operations must be 2..=1000000 (total receipts, including one setup receipt)",
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

fn load_repository(pool: &Path) -> BenchResult<Repository> {
    Ok(json_codec::from_slice(&fs::read(
        pool.join("harness/repository.json"),
    )?)?)
}

fn verify_active_set(engine: &Engine, repo: &Repository, operations: usize) -> BenchResult<()> {
    let state = engine.state();
    require(!engine.is_read_only(), "healthy fixture became read-only")?;
    require(
        state.sequence == operations as u64
            && state.operations.len() as u64 == operations as u64
            && state.events.len() as u64 == operations as u64
            && state.repositories.len() == 1
            && state.histories.len() == 1
            && state.branches.len() == 1
            && state.commits.len() == 1
            && state.versions.is_empty()
            && state.directory_branches.is_empty()
            && state.directory_trees.is_empty(),
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
            && current.smb_binding_generation == operations as u64
            && current.smb_enabled == ((operations - 1) % 2 == 0)
            && history.generation == operations as u64,
        "live SMB binding or generation differs from the published workload",
    )
}

fn verify_receipt(result: &OperationResult, index: usize, repo: &Repository) -> BenchResult<()> {
    require(
        result.operation_id == operation_id(index)?
            && result.sequence == index as u64 + 1
            && result.commit_state == CommitState::Published
            && result.protection_at_publish == ProtectionAtPublish::NotApplicable
            && result.current_health == CurrentHealth::Healthy
            && result.branch_id.as_deref() == Some(repo.main_branch_id.as_str()),
        "receipt identity, sequence, publication, protection, or branch differs",
    )?;
    if index == 0 {
        require(
            result.kind == "repository.create"
                && result.history_tree_id.as_deref() == Some(repo.history_tree_id.as_str())
                && result.commit_id.is_some(),
            "setup receipt differs",
        )?;
    } else {
        require(
            result.kind == "smb.binding.update"
                && result.commit_id.is_none()
                && result.history_tree_id.is_none()
                && result.result["repository_id"].as_str() == Some(repo.repository_id.as_str())
                && result.result["smb_enabled"].as_bool() == Some(index % 2 == 0)
                && result.result["generation"].as_str() == Some((index + 1).to_string().as_str())
                && result.result["smb_binding_generation"].as_str()
                    == Some((index + 1).to_string().as_str()),
            "metadata receipt payload differs from the original request",
        )?;
    }
    Ok(())
}

fn build(arguments: &Arguments, reporter: &mut Reporter) -> BenchResult<(Engine, Repository)> {
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
    verify_receipt(&original, 0, &repo)?;
    json_codec::to_writer(&mut ledger, &original)?;
    ledger.write_all(b"\n")?;
    reporter.emit(json!({
        "phase": "setup", "verified": true, "result": original,
        "setup_receipts": 1, "setup_events": 1,
        "target_total_receipts": arguments.operations,
        "payload_operations": arguments.operations - 1
    }))?;
    process_snapshot(reporter, "before_append")?;
    for index in 1..arguments.operations {
        let id = operation_id(index)?;
        let started = Instant::now();
        let result = engine.set_smb_binding(
            &repo.repository_id,
            &repo.main_branch_id,
            index % 2 == 0,
            index as u64,
            ACTOR,
            id,
        )?;
        let elapsed = started.elapsed().as_nanos();
        verify_receipt(&result, index, &repo)?;
        verify_active_set(&engine, &repo, index + 1)?;
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
    Ok((engine, repo))
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
    require(samples.len() >= 2, "missing original sample receipts")?;
    Ok(samples)
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
        for repetition in 0..QUERY_REPETITIONS {
            let started = Instant::now();
            let actual = engine.operation_record(expected.operation_id)?;
            let elapsed = started.elapsed().as_nanos();
            require(
                actual.owner() == Some(ACTOR) && actual.result() == expected,
                "queried receipt or original owner binding changed",
            )?;
            reporter.emit(json!({
                "phase": "query", "api": "operation_record", "position": position,
                "index": index, "operation_id": expected.operation_id.to_string(),
                "repetition": repetition, "latency_ns": elapsed, "verified": true,
                "cache_observation": if repetition == 0 { "first_after_admission_or_append" } else { "repeated" }
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
    repo: &Repository,
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
                let creation = engine.operation_record(expected_id)?.into_result();
                (
                    "repository.create",
                    creation
                        .commit_id
                        .ok_or_else(|| invalid("setup commit missing"))?,
                )
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
    verify_active_set(engine, repo, operations)
}

fn verify_all_receipts(
    engine: &Engine,
    repo: &Repository,
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
        verify_receipt(&expected, index, repo)?;
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
        "oracle_memory": "one line and three sampled receipts"
    }))
}

fn verify_retries(
    engine: &mut Engine,
    repo: &Repository,
    samples: &[(usize, OperationResult)],
    operations: usize,
    reporter: &mut Reporter,
) -> BenchResult<()> {
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
                index % 2 == 0,
                *index as u64,
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
                index % 2 != 0,
                *index as u64,
                ACTOR,
                expected.operation_id,
            );
            match mismatch {
                Err(error) if error.code() == storage_engine::ErrorCode::IdempotencyKeyReused => (),
                Err(error) => return Err(error.into()),
                Ok(_) => return Err(invalid("mismatched original ID was accepted").into()),
            }
        }
        verify_active_set(engine, repo, operations)?;
        reporter.emit(json!({
            "phase": "retry", "index": index, "operation_id": expected.operation_id.to_string(),
            "latency_ns": started.elapsed().as_nanos(), "verified": true
        }))?;
    }
    // Replay and mismatch checks must not append any event, including duplicates
    // whose sequence would be hidden by a sequence-based cursor.
    check_events(engine, repo, operations, reporter)
}

fn run(arguments: &Arguments, reporter: &mut Reporter) -> BenchResult<()> {
    reporter.emit(json!({
        "phase": "configuration", "stage": arguments.phase, "mode": arguments.mode,
        "operations": arguments.operations, "verified": true,
        "payload_operations": arguments.operations - 1,
        "metadata_members": 1, "content_versions": 0, "development_mode": true,
        "workload": "alternate SMB binding enabled on one unchanged branch",
        "history_scale_baseline": cfg!(feature = "history-scale-baseline")
    }))?;
    process_snapshot(reporter, "process_start")?;
    let (mut engine, repo) = if arguments.phase == "build" {
        build(arguments, reporter)?
    } else {
        let repo = load_repository(&arguments.pool)?;
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
        verify_active_set(&engine, &repo, arguments.operations)?;
        reporter.emit(json!({
            "phase": arguments.phase, "latency_ns": elapsed, "verified": true,
            "api": if arguments.phase == "reopen" { "Engine::open" } else { "Engine::restore_from_disks" }
        }))?;
        (engine, repo)
    };
    require(
        engine.list_tree(&repo.main_branch_id)?.is_empty(),
        "metadata workload unexpectedly grew its namespace",
    )?;
    let samples = original_samples(&arguments.pool, arguments.operations)?;
    check_queries(&engine, &samples, reporter)?;
    check_events(&engine, &repo, arguments.operations, reporter)?;
    verify_all_receipts(&engine, &repo, arguments, reporter)?;
    verify_retries(&mut engine, &repo, &samples, arguments.operations, reporter)?;
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
