//! Fixed/growing Development-mode workloads measuring complete engine recovery and startup.
use json_codec::json;
use std::{
    fs,
    io::{Read, Write},
    sync::{Arc, Mutex},
    time::Instant,
};
use storage_engine::{CommitRequest, Config, Engine, OperationId};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Clone, Copy)]
enum Workload {
    Fixed,
    Growing,
}
impl std::str::FromStr for Workload {
    type Err = &'static str;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "fixed" => Ok(Self::Fixed),
            "growing" => Ok(Self::Growing),
            _ => Err("mode must be fixed or growing"),
        }
    }
}
impl Workload {
    fn label(self) -> &'static str {
        match self {
            Self::Fixed => "fixed",
            Self::Growing => "growing",
        }
    }
}

#[derive(Clone, Default)]
struct TraceOutput(Arc<Mutex<Vec<u8>>>);
impl Write for TraceOutput {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("trace output poisoned")
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn reopen(root: &std::path::Path) -> Result<(Engine, u128, json_codec::Value)> {
    let output = TraceOutput::default();
    let writer = output.clone();
    let subscriber = tracing::collector::fmt()
        .json()
        .without_time()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let guard = tracing::subscriber::set_default(subscriber);
    let (engine, ns) = elapsed(|| Engine::open(root));
    drop(guard);
    let bytes = std::mem::take(&mut *output.0.lock().expect("trace output poisoned"));
    std::io::stderr().write_all(&bytes)?;
    let engine = engine?;
    let events: Vec<json_codec::Value> = std::str::from_utf8(&bytes)?
        .lines()
        .map(json_codec::from_str)
        .collect::<std::result::Result<_, _>>()?;
    let mut checkpoints = events
        .into_iter()
        .filter(|event| event["target"] == "storage_engine::recovery_checkpoint");
    let checkpoint = checkpoints
        .next()
        .ok_or("checkpoint selection event absent")?;
    assert!(
        checkpoints.next().is_none(),
        "one selection event per engine open"
    );
    Ok((engine, ns, checkpoint))
}

fn elapsed<T>(f: impl FnOnce() -> T) -> (T, u128) {
    let start = Instant::now();
    let value = f();
    (value, start.elapsed().as_nanos())
}
fn rss() -> Result<json_codec::Value> {
    let status = fs::read_to_string("/proc/self/status")?;
    let read = |key: &str| -> Result<u64> {
        Ok(status
            .lines()
            .find(|line| line.starts_with(key))
            .ok_or("RSS field absent")?
            .split_whitespace()
            .nth(1)
            .ok_or("RSS value absent")?
            .parse()?)
    };
    Ok(json!({"rss_kib":read("VmRSS:")?,"peak_rss_kib":read("VmHWM:")?}))
}
fn warning(error: Option<storage_engine::Error>) -> json_codec::Value {
    let Some(error) = error else {
        return json_codec::Value::Null;
    };
    let mut sources = Vec::new();
    let mut source = std::error::Error::source(&error);
    while let Some(cause) = source {
        sources.push(cause.to_string());
        source = cause.source();
    }
    json!({"code":error.code(),"message":error.to_string(),"sources":sources})
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let mode: Workload = args.get(1).map(String::as_str).unwrap_or("fixed").parse()?;
    let maximum: usize = args.get(2).map(String::as_str).unwrap_or("1024").parse()?;
    if ![64, 256, 1024].contains(&maximum) {
        return Err("maximum must be 64, 256 or 1024".into());
    }
    let temp = tempfile::Builder::new()
        .prefix("zxs-recovery-startup-")
        .tempdir()?;
    let root = temp.path().join("system");
    let config = Config::development([
        temp.path().join("primary"),
        temp.path().join("backup"),
        temp.path().join("tertiary"),
    ]);
    let mut engine = Engine::initialize(&root, config)?;
    let oldest = OperationId::generate();
    let repo = engine.create_repository(
        &storage_engine::RepositoryName::parse("State sharing probe")?,
        "P01",
        3,
        "admin",
        oldest,
    )?;
    let branch = repo.main_branch_id.clone();
    let mut transactions = Vec::new();
    let mut commits = Vec::new();
    let mut oldest_version = None;
    let mut oldest_bytes = Vec::new();
    for n in 1..=maximum {
        let mut bytes = vec![0u8; 128];
        bytes[..8].copy_from_slice(&(n as u64).to_le_bytes());
        let path = match mode {
            Workload::Fixed => "sample.bin".to_owned(),
            Workload::Growing => format!("sample-{n:04}.bin"),
        };
        let start = Instant::now();
        let w = engine.create_workspace(&branch, "admin")?;
        engine.stage_bytes(&w.workspace_id, &path, &bytes)?;
        let request = CommitRequest {
            workspace_id: w.workspace_id,
            operation_id: OperationId::generate(),
            principal: "admin".into(),
            expected_head: w.base_head,
            fencing_token: w.fencing_token,
            message: "State sharing workload".into(),
            request_digest: None,
        };
        let (result, commit_ns) = elapsed(|| engine.commit_workspace(request));
        result?;
        transactions.push(start.elapsed().as_nanos());
        commits.push(commit_ns);
        if n == 1 {
            oldest_version = Some(
                engine
                    .list_tree(&branch)?
                    .iter()
                    .find(|entry| entry.path == path)
                    .expect("committed file exists")
                    .version_id
                    .clone(),
            );
            oldest_bytes = bytes;
        }
        if ![64, 256, 1024].contains(&n) {
            continue;
        }
        let transaction_memory = rss()?;
        let count = engine.state().operations.len();
        let event_count = engine.state().events.len();
        let sequence = engine.state().sequence;
        let before = storage_engine::canonical_json(engine.state())?;
        let mut restarts = Vec::new();
        let mut restart_evidence = Vec::new();
        for restart in 1..=3 {
            drop(engine);
            let (reopened, ns, checkpoint) = reopen(&root)?;
            engine = reopened;
            restarts.push(ns);
            let warnings = engine.take_recovery_warnings();
            let checkpoint_hit = checkpoint["fields"]["checkpoint_selected"]
                .as_bool()
                .expect("checkpoint event records its selection decision");
            let evidence = json!({
                "event":"engine_restart",
                "mode":mode.label(),
                "history":n,
                "restart":restart,
                "engine_open_ns":ns,
                "checkpoint_hit":checkpoint_hit,
                "checkpoint_hit_evidence":"recovery_checkpoint_selection_event",
                "checkpoint_sequence":checkpoint["fields"].get("checkpoint_sequence"),
                "checkpoint_candidate_hash":checkpoint["fields"].get("checkpoint_candidate_hash"),
                "checkpoint_snapshot_hash":checkpoint["fields"].get("checkpoint_snapshot_hash"),
                "checkpoint_load_warning":warning(warnings.checkpoint_load),
                "checkpoint_save_warning":warning(warnings.checkpoint_save),
            });
            eprintln!("{evidence}");
            restart_evidence.push(evidence);
            assert_eq!(storage_engine::canonical_json(engine.state())?, before);
            assert_eq!(
                engine.create_repository(
                    &storage_engine::RepositoryName::parse("State sharing probe")?,
                    "P01",
                    3,
                    "admin",
                    oldest
                )?,
                repo
            );
            assert_eq!(engine.state().sequence, sequence);
            assert_eq!(engine.state().operations.len(), count);
            assert_eq!(storage_engine::canonical_json(engine.state())?, before);
        }
        let mut content =
            engine.open_content(oldest_version.as_deref().expect("oldest version recorded"))?;
        let mut recovered = Vec::new();
        content.file.read_to_end(&mut recovered)?;
        assert_eq!(recovered, oldest_bytes);
        drop(content);
        let expected_live = match mode {
            Workload::Fixed => 1,
            Workload::Growing => n,
        };
        assert_eq!(engine.list_tree(&branch)?.len(), expected_live);
        println!(
            "{}",
            json!({
                "mode":mode.label(),
                "history":n,
                "copies":3,
                "development":true,
                "operations":count,
                "events":event_count,
                "live_files":expected_live,
                "retained_versions":engine.state().versions.len(),
                "state_json_bytes":before.len(),
                "transactions_ns":&transactions[transactions.len()-32..],
                "commit_ns":&commits[commits.len()-32..],
                "restart_ns":restarts,
                "restart_evidence":restart_evidence,
                "transaction_memory":transaction_memory,
                "post_probe_memory":rss()?,
                "checks":{
                    "canonical_restart_state":true,
                    "oldest_receipt_replay_no_mutation":true,
                    "oldest_content_read":true,
                    "live_namespace":true,
                },
            })
        );
    }
    assert!(engine.check()?.issues.is_empty(), "integrity check failed");
    Ok(())
}
