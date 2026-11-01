//! Isolated Development-mode wall-clock probe through public Engine phases.
//! execute() includes planning, payload work and metadata preparation; this tool
//! cannot attribute its time to one of those operations or measure lock wait.
use json_codec::json;
use std::{hint::black_box, io::Read, path::Path, time::Instant};
use storage_engine::{CommitRequest, Config, Engine, OperationId, OperationResult};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Clone, Copy)]
enum Workload {
    Fixed,
    Growing,
}

impl Workload {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "fixed" => Ok(Self::Fixed),
            "growing" => Ok(Self::Growing),
            _ => Err("mode must be fixed or growing".into()),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Fixed => "fixed",
            Self::Growing => "growing",
        }
    }

    fn path(self, iteration: usize) -> String {
        match self {
            Self::Fixed => "sample.bin".into(),
            Self::Growing => format!("sample-{iteration:04}.bin"),
        }
    }

    fn live_files(self, iteration: usize) -> usize {
        match self {
            Self::Fixed => 1,
            Self::Growing => iteration,
        }
    }
}

struct Options {
    workload: Workload,
    iterations: usize,
    payload_bytes: usize,
}

impl Options {
    fn read() -> Result<Self> {
        let args: Vec<_> = std::env::args().skip(1).collect();
        if args.len() > 3 {
            return Err("usage: resource-phases [fixed|growing] [1..128] [64|128]".into());
        }
        let workload = Workload::parse(args.first().map(String::as_str).unwrap_or("fixed"))?;
        let iterations = args.get(1).map(String::as_str).unwrap_or("64").parse()?;
        let payload_bytes = args.get(2).map(String::as_str).unwrap_or("128").parse()?;
        if !(1..=128).contains(&iterations) {
            return Err("iterations must be 1..128".into());
        }
        if ![64, 128].contains(&payload_bytes) {
            return Err("payload bytes must be 64 or 128".into());
        }
        Ok(Self {
            workload,
            iterations,
            payload_bytes,
        })
    }
}

struct OldestSave {
    branch_id: String,
    request: CommitRequest,
    result: OperationResult,
    version_id: String,
    bytes: Vec<u8>,
}

fn measured<T>(operation: impl FnOnce() -> storage_engine::Result<T>) -> Result<(T, u128)> {
    let start = Instant::now();
    let value = operation()?;
    Ok((value, start.elapsed().as_nanos()))
}

fn save(
    engine: &mut Engine,
    branch: &str,
    options: &Options,
    iteration: usize,
) -> Result<(CommitRequest, OperationResult, Vec<u8>)> {
    let path = options.workload.path(iteration);
    let mut bytes = vec![(iteration % 251) as u8; options.payload_bytes];
    bytes[..8].copy_from_slice(&(iteration as u64).to_le_bytes());
    let operation_id = OperationId::generate()?;
    let total_start = Instant::now();
    let (workspace, create_ns) = measured(|| engine.create_workspace(branch, "admin"))?;
    let (_, stage_ns) = measured(|| engine.stage_bytes(&workspace.workspace_id, &path, &bytes))?;
    let request = CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id,
        principal: "admin".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "Resource phase workload".into(),
        request_digest: None,
    };
    let replay_request = request.clone();
    let (preparation, capture_ns) = measured(|| engine.prepare_workspace_commit(request, None))?;
    let (prepared, execute_ns) = measured(|| preparation.execute())?;
    let (result, finish_ns) = measured(|| engine.finish_workspace_commit(prepared))?;
    let total_ns = total_start.elapsed().as_nanos();
    println!(
        "{}",
        json!({
            "event":"transaction",
            "mode":options.workload.label(),
            "history":iteration,
            "payload_bytes":options.payload_bytes,
            "create_workspace_ns":create_ns,
            "stage_bytes_ns":stage_ns,
            "prepare_capture_ns":capture_ns,
            "prepare_execute_ns":execute_ns,
            "finish_publication_ns":finish_ns,
            "create_through_finish_ns":total_ns,
            "operations":engine.state().operations.len(),
            "events":engine.state().events.len(),
            "retained_versions":engine.state().versions.len(),
        })
    );
    Ok((replay_request, result, bytes))
}

fn checkpoint(
    mut engine: Engine,
    root: &Path,
    oldest: &OldestSave,
    options: &Options,
    iteration: usize,
) -> Result<Engine> {
    // The clone stays alive across the clock read; destructor/equality/canonical
    // encoding are excluded. No append or actual mutation planning is measured.
    let clone_start = Instant::now();
    let cloned: storage_engine::State = black_box(engine.state()).clone();
    black_box(&cloned);
    let clone_ns = clone_start.elapsed().as_nanos();
    assert_eq!(&cloned, engine.state(), "State clone must preserve values");
    drop(cloned);
    let before = storage_engine::canonical_json(engine.state())?;
    drop(engine);
    let (reopened, open_ns) = measured(|| Engine::open(root))?;
    engine = reopened;
    assert_eq!(storage_engine::canonical_json(engine.state())?, before);
    assert_eq!(
        engine
            .operation_record(oldest.request.operation_id)?
            .result(),
        &oldest.result
    );
    assert_eq!(
        engine
            .operation_owner(oldest.request.operation_id)?
            .as_deref(),
        Some("admin")
    );
    let replay = engine
        .prepare_workspace_commit(oldest.request.clone(), None)?
        .execute()?;
    assert_eq!(engine.finish_workspace_commit(replay)?, oldest.result);
    assert_eq!(storage_engine::canonical_json(engine.state())?, before);
    let mut opened = engine.open_content(&oldest.version_id)?;
    let mut recovered = Vec::new();
    opened.file.read_to_end(&mut recovered)?;
    assert_eq!(
        recovered, oldest.bytes,
        "oldest retained body must be exact"
    );
    drop(opened);
    let expected_live = options.workload.live_files(iteration);
    assert_eq!(engine.list_tree(&oldest.branch_id)?.len(), expected_live);
    let (report, check_ns) = measured(|| engine.check())?;
    assert!(
        report.issues.is_empty(),
        "integrity check failed: {:?}",
        report.issues
    );
    println!(
        "{}",
        json!({
            "event":"checkpoint",
            "mode":options.workload.label(),
            "history":iteration,
            "state_clone_only_ns":clone_ns,
            "engine_open_ns":open_ns,
            "integrity_check_ns":check_ns,
            "state_canonical_bytes":before.len(),
            "operations":engine.state().operations.len(),
            "events":engine.state().events.len(),
            "retained_versions":engine.state().versions.len(),
            "live_files":expected_live,
            "checks":{
                "clone_value_equality":true,
                "canonical_restart_state":true,
                "oldest_receipt_and_owner":true,
                "oldest_request_replay_no_state_mutation":true,
                "oldest_content_exact":true,
                "live_namespace_count":true,
                "engine_check_no_issues":true,
            },
        })
    );
    Ok(engine)
}

fn main() -> Result<()> {
    let options = Options::read()?;
    let temporary = tempfile::Builder::new()
        .prefix("zxs-resource-phases-")
        .tempdir()?;
    let root = temporary.path().join("system");
    let config = Config::development([
        temporary.path().join("primary"),
        temporary.path().join("backup"),
        temporary.path().join("tertiary"),
    ])?;
    let mut engine = Engine::initialize(&root, config)?;
    let repo = engine.create_repository(
        &storage_engine::RepositoryName::parse("Resource phase probe")?,
        "P01",
        3,
        "admin",
        OperationId::generate()?,
    )?;
    let branch = repo.main_branch_id;
    println!(
        "{}",
        json!({
            "event":"probe_start",
            "scope":"isolated-development-cloud-proxy",
            "temporary_root":temporary.path().display().to_string(),
            "storage_topology":"three copy directories on the selected temporary filesystem; no physical-device independence claim",
            "mode":options.workload.label(),
            "iterations":options.iterations,
            "payload_bytes":options.payload_bytes,
            "copies":3,
            "concurrency":1,
            "cache":"process-warm; no explicit OS page-cache drop; reopen checkpoints",
            "source_commit_binding":"external provenance required",
            "measurement":"single-process public Engine wall-clock phases",
            "phase_boundaries":{
                "prepare_capture_ns":"prepare_workspace_commit(request,None)",
                "prepare_execute_ns":"execute(): planning + payload + metadata preparation",
                "finish_publication_ns":"finish_workspace_commit(): recheck + publication + workspace completion",
                "create_through_finish_ns":"create start through finish return; includes request construction and timer overhead",
                "state_clone_only_ns":"black_box State::clone; excludes drop/equality/encoding; no allocation or planning attribution",
            },
        })
    );
    let mut oldest = None;
    for iteration in 1..=options.iterations {
        let (request, result, bytes) = save(&mut engine, &branch, &options, iteration)?;
        if iteration == 1 {
            let path = options.workload.path(iteration);
            let version_id = engine
                .list_tree(&branch)?
                .into_iter()
                .find(|entry| entry.path == path)
                .ok_or("first saved file missing")?
                .version_id;
            oldest = Some(OldestSave {
                branch_id: branch.clone(),
                request,
                result,
                version_id,
                bytes,
            });
        }
        if iteration == 1 || iteration == options.iterations / 2 || iteration == options.iterations
        {
            engine = checkpoint(
                engine,
                &root,
                oldest.as_ref().expect("invariant: first save is recorded"),
                &options,
                iteration,
            )?;
        }
    }
    println!(
        "{}",
        json!({"event":"probe_complete","all_checks_passed":true})
    );
    Ok(())
}
