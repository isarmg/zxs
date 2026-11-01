//! Isolated Development-mode namespace costs through the public Engine API.
//! Fixture construction and correctness oracles are outside measured commit phases.
use json_codec::json;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    io::Read,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};
use storage_engine::{
    CommitRequest, Config, Engine, ErrorCode, NamespaceEntryKind, NamespaceOperation,
    NamespaceOperationKind, OperationId, OperationResult, Workspace,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

struct CountedSystem;

static ALLOCATION_CALLS: AtomicU64 = AtomicU64::new(0);
static REQUESTED_BYTES: AtomicU64 = AtomicU64::new(0);

fn count_allocation(pointer: *mut u8, size: usize) {
    if !pointer.is_null() {
        ALLOCATION_CALLS.fetch_add(1, Ordering::Relaxed);
        REQUESTED_BYTES.fetch_add(size as u64, Ordering::Relaxed);
    }
}

// SAFETY: Each operation forwards the caller's original pointer, layout and size
// unchanged to System. The counters never dereference pointers or allocate.
unsafe impl GlobalAlloc for CountedSystem {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: The GlobalAlloc caller supplies the required valid layout.
        let pointer = unsafe { System.alloc(layout) };
        count_allocation(pointer, layout.size());
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: The GlobalAlloc caller supplies the required valid layout.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        count_allocation(pointer, layout.size());
        pointer
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: The caller supplies a live System allocation, its original
        // layout and a new size satisfying GlobalAlloc::realloc's contract.
        let result = unsafe { System.realloc(pointer, layout, size) };
        count_allocation(result, size);
        result
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: The caller supplies the live allocation and its original layout.
        unsafe { System.dealloc(pointer, layout) };
    }
}

#[global_allocator]
static ALLOCATOR: CountedSystem = CountedSystem;

struct PhaseTimer {
    started: Instant,
    allocation_calls: u64,
    requested_bytes: u64,
}

#[derive(Clone, Copy)]
struct Phase {
    ns: u128,
    allocation_calls: u64,
    requested_bytes: u64,
}

impl PhaseTimer {
    fn start() -> Self {
        Self {
            started: Instant::now(),
            allocation_calls: ALLOCATION_CALLS.load(Ordering::Relaxed),
            requested_bytes: REQUESTED_BYTES.load(Ordering::Relaxed),
        }
    }

    fn finish(self) -> Phase {
        Phase {
            ns: self.started.elapsed().as_nanos(),
            allocation_calls: ALLOCATION_CALLS.load(Ordering::Relaxed) - self.allocation_calls,
            requested_bytes: REQUESTED_BYTES.load(Ordering::Relaxed) - self.requested_bytes,
        }
    }
}

impl Phase {
    fn allocations(self) -> json_codec::Value {
        json!({"allocation_calls":self.allocation_calls,"requested_bytes":self.requested_bytes})
    }
}

#[derive(Clone, Copy)]
enum Case {
    Empty,
    ContentOne,
    ContentNew,
    ContentFour,
    Rename,
    Delete,
    RenameConflict,
    DeleteConflict,
}

impl Case {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "empty" => Ok(Self::Empty),
            "content-one" => Ok(Self::ContentOne),
            "content-new" => Ok(Self::ContentNew),
            "content-four" => Ok(Self::ContentFour),
            "rename" => Ok(Self::Rename),
            "delete" => Ok(Self::Delete),
            "rename-conflict" => Ok(Self::RenameConflict),
            "delete-conflict" => Ok(Self::DeleteConflict),
            _ => Err("unknown namespace case".into()),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::ContentOne => "content-one",
            Self::ContentNew => "content-new",
            Self::ContentFour => "content-four",
            Self::Rename => "rename",
            Self::Delete => "delete",
            Self::RenameConflict => "rename-conflict",
            Self::DeleteConflict => "delete-conflict",
        }
    }

    fn rejects(self) -> bool {
        matches!(self, Self::RenameConflict | Self::DeleteConflict)
    }

    fn staged_files(self) -> usize {
        match self {
            Self::ContentOne | Self::ContentNew => 1,
            Self::ContentFour => 4,
            Self::Empty
            | Self::Rename
            | Self::Delete
            | Self::RenameConflict
            | Self::DeleteConflict => 0,
        }
    }

    fn namespace_change(self) -> Result<Option<NamespaceOperation>> {
        let operation = match self {
            Self::Rename | Self::RenameConflict => Some(NamespaceOperation::parse(
                NamespaceOperationKind::Rename,
                "group",
                Some("moved"),
            )?),
            Self::Delete | Self::DeleteConflict => Some(NamespaceOperation::parse(
                NamespaceOperationKind::DeleteTree,
                "group",
                None,
            )?),
            Self::Empty | Self::ContentOne | Self::ContentNew | Self::ContentFour => None,
        };
        Ok(operation)
    }
}

struct SeedFile {
    entry_id: String,
    file_id: String,
    version_id: String,
    path: String,
    bytes: Vec<u8>,
}

struct Receipt {
    request: CommitRequest,
    result: OperationResult,
}

fn elapsed<T>(operation: impl FnOnce() -> T) -> (T, Phase) {
    let timer = PhaseTimer::start();
    let result = operation();
    (result, timer.finish())
}

fn bytes(index: usize, generation: u64) -> Vec<u8> {
    let mut value = vec![((index as u64 + generation) % 251) as u8; 128];
    value[..8].copy_from_slice(&(index as u64).to_le_bytes());
    value[8..16].copy_from_slice(&generation.to_le_bytes());
    value
}

fn path(index: usize) -> String {
    format!("group/file-{index:04}.bin")
}

fn request(workspace: Workspace, message: &str) -> Result<CommitRequest> {
    Ok(CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: OperationId::generate()?,
        principal: "admin".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: message.into(),
        request_digest: None,
    })
}

fn seed(engine: &mut Engine, branch: &str, count: usize) -> Result<(Vec<SeedFile>, Receipt)> {
    let timer = PhaseTimer::start();
    let workspace = engine.create_workspace(branch, "admin")?;
    for index in 0..count {
        engine.stage_bytes(&workspace.workspace_id, &path(index), &bytes(index, 1))?;
    }
    let staging = Phase {
        ns: timer.started.elapsed().as_nanos(),
        allocation_calls: ALLOCATION_CALLS.load(Ordering::Relaxed) - timer.allocation_calls,
        requested_bytes: REQUESTED_BYTES.load(Ordering::Relaxed) - timer.requested_bytes,
    };
    let request = request(workspace, "namespace fixture: one batched seed commit")?;
    let (result, commit) = elapsed(|| engine.commit_workspace(request.clone()));
    let result = result?;
    let total = timer.finish();
    let entries = engine.list_tree(branch)?;
    assert_eq!(
        entries.len(),
        count + 1,
        "seed files plus one group directory"
    );
    let mut files = Vec::with_capacity(count);
    for index in 0..count {
        let expected_path = path(index);
        let entry = entries
            .iter()
            .find(|entry| entry.path() == expected_path)
            .ok_or("seed file missing")?;
        assert_eq!(entry.kind, NamespaceEntryKind::File);
        files.push(SeedFile {
            entry_id: entry.entry_id.clone(),
            file_id: entry.file_id.clone(),
            version_id: entry.version_id.clone(),
            path: expected_path,
            bytes: bytes(index, 1),
        });
    }
    println!(
        "{}",
        json!({"event":"fixture_seed","live_files":count,"entries":entries.len(),
            "payload_bytes_per_file":128,"staged_files":count,"commits":1,
            "create_and_all_staging_ns":staging.ns,"commit_ns":commit.ns,
            "create_through_commit_ns":total.ns,"measured_operation":false,
            "allocations":{"create_and_all_staging":staging.allocations(),
                "commit":commit.allocations(),"create_through_commit":total.allocations()}}),
    );
    Ok((files, Receipt { request, result }))
}

fn competing_save(engine: &mut Engine, branch: &str, count: usize) -> Result<Receipt> {
    let timer = PhaseTimer::start();
    let workspace = engine.create_workspace(branch, "admin")?;
    engine.stage_bytes(
        &workspace.workspace_id,
        &path(count - 1),
        &bytes(count - 1, 3),
    )?;
    let request = request(workspace, "fixture winner changes final subtree entry")?;
    let result = engine.commit_workspace(request.clone())?;
    let phase = timer.finish();
    println!(
        "{}",
        json!({"event":"fixture_competing_save","changed_path":path(count-1),
            "create_through_commit_ns":phase.ns,"allocations":phase.allocations(),
            "measured_operation":false}),
    );
    Ok(Receipt { request, result })
}

fn measure(
    engine: &mut Engine,
    request: &CommitRequest,
    case: Case,
    count: usize,
) -> Result<Option<Receipt>> {
    let (preparation, capture) = elapsed(|| engine.prepare_workspace_commit(request.clone(), None));
    let preparation = preparation?;
    let (prepared, execute) = elapsed(|| preparation.execute());
    let (receipt, finish, rejected_code) = match prepared {
        Ok(prepared) => {
            assert!(!case.rejects(), "conflicting namespace commit must reject");
            let (result, finish) = elapsed(|| engine.finish_workspace_commit(prepared));
            let result = result?;
            (
                Some(Receipt {
                    request: request.clone(),
                    result,
                }),
                Some(finish),
                None,
            )
        }
        Err(error) => {
            assert!(case.rejects(), "unexpected preparation error: {error}");
            assert_eq!(error.code(), ErrorCode::NamespaceConflict);
            (None, None, Some(error.code().to_string()))
        }
    };
    println!(
        "{}",
        json!({"event":"measured_commit","case":case.label(),"live_files_seed":count,"expected_rejection":case.rejects(),
            "actual_rejection":receipt.is_none(),"rejected_code":rejected_code,
            "prepare_capture_ns":capture.ns,"prepare_execute_ns":execute.ns,
            "finish_publication_ns":finish.map(|phase|phase.ns),"staged_files":case.staged_files(),
            "allocations":{"prepare_capture":capture.allocations(),"prepare_execute":execute.allocations(),
                "finish_publication":finish.map(Phase::allocations)},
            "namespace_operations":usize::from(case.namespace_change()?.is_some()),
            "operations_after":engine.state().operations.len(),"events_after":engine.state().events.len(),
            "versions_after":engine.state().versions.len()}),
    );
    Ok(receipt)
}

fn receipt_and_replay(engine: &mut Engine, receipt: &Receipt) -> Result<()> {
    let before = storage_engine::canonical_json(engine.state())?;
    let record = engine.operation_record(receipt.request.operation_id)?;
    assert_eq!(record.result(), &receipt.result);
    assert_eq!(record.owner(), Some("admin"));
    assert_eq!(
        engine.commit_workspace(receipt.request.clone())?,
        receipt.result
    );
    assert_eq!(storage_engine::canonical_json(engine.state())?, before);
    Ok(())
}

fn absent_receipt(engine: &Engine, request: &CommitRequest) -> Result<()> {
    match engine.operation_record(request.operation_id) {
        Err(error) => assert_eq!(error.code(), ErrorCode::OperationNotFound),
        Ok(_) => return Err("rejected operation unexpectedly has a receipt".into()),
    }
    assert_eq!(engine.operation_owner(request.operation_id)?, None);
    Ok(())
}

fn body(engine: &Engine, version: &str, expected: &[u8]) -> Result<()> {
    let mut opened = engine.open_content(version)?;
    let mut actual = Vec::new();
    opened.file.read_to_end(&mut actual)?;
    assert_eq!(actual, expected, "exact complete body readback");
    Ok(())
}

fn verify_files(engine: &Engine, branch: &str, files: &[SeedFile], case: Case) -> Result<()> {
    let entries = engine.list_tree(branch)?;
    if matches!(case, Case::Delete) {
        assert!(
            entries.is_empty(),
            "delete_tree removes group and all live children"
        );
    } else {
        assert_eq!(
            entries.len(),
            files.len() + 1 + usize::from(matches!(case, Case::ContentNew))
        );
        for (index, seed) in files.iter().enumerate() {
            let expected_path = if matches!(case, Case::Rename) {
                seed.path.replacen("group/", "moved/", 1)
            } else {
                seed.path.clone()
            };
            let entry = entries
                .iter()
                .find(|entry| entry.path() == expected_path)
                .ok_or("expected live file missing")?;
            assert_eq!(entry.entry_id, seed.entry_id);
            assert_eq!(entry.file_id, seed.file_id);
            let generation = if (matches!(case, Case::ContentOne) && index + 1 == files.len())
                || (matches!(case, Case::ContentFour) && index < 4)
            {
                2
            } else if case.rejects() && index + 1 == files.len() {
                3
            } else {
                1
            };
            if generation == 1 {
                assert_eq!(entry.version_id, seed.version_id);
            } else {
                assert_ne!(entry.version_id, seed.version_id);
            }
            body(engine, &entry.version_id, &bytes(index, generation))?;
        }
        if matches!(case, Case::ContentNew) {
            let new_path = path(files.len());
            let entry = entries
                .iter()
                .find(|entry| entry.path() == new_path)
                .ok_or("new content key is missing")?;
            assert!(files.iter().all(|seed| seed.file_id != entry.file_id));
            body(engine, &entry.version_id, &bytes(files.len(), 2))?;
        }
    }
    // Deleted and overwritten files still retain their original independent FULL version.
    for seed in files {
        body(engine, &seed.version_id, &seed.bytes)?;
    }
    Ok(())
}

fn verify_after_restart(
    mut engine: Engine,
    root: &Path,
    branch: &str,
    files: &[SeedFile],
    case: Case,
    request: &CommitRequest,
    receipts: &[Receipt],
) -> Result<()> {
    let canonical = storage_engine::canonical_json(engine.state())?;
    let workspace = storage_engine::canonical_json(&engine.workspace(&request.workspace_id)?)?;
    drop(engine);
    let (reopened, reopen) = elapsed(|| Engine::open(root));
    engine = reopened?;
    assert_eq!(storage_engine::canonical_json(engine.state())?, canonical);
    assert_eq!(
        storage_engine::canonical_json(&engine.workspace(&request.workspace_id)?)?,
        workspace,
    );
    if case.rejects() {
        absent_receipt(&engine, request)?;
        assert_eq!(
            engine.workspace(&request.workspace_id)?.state.as_str(),
            "open"
        );
    }
    for receipt in receipts {
        receipt_and_replay(&mut engine, receipt)?;
    }
    verify_files(&engine, branch, files, case)?;
    let (report, check) = elapsed(|| engine.check());
    let report = report?;
    assert!(
        report.issues.is_empty(),
        "integrity issues: {:?}",
        report.issues
    );
    println!(
        "{}",
        json!({"event":"correctness_checkpoint","case":case.label(),"live_files_seed":files.len(),
            "engine_open_ns":reopen.ns,"integrity_check_ns":check.ns,"measured_operation":false,
            "allocations":{"engine_open":reopen.allocations(),"integrity_check":check.allocations()},
            "checks":{"canonical_restart_state":true,"workspace_restart_state":true,
                "all_published_receipts_and_owners":true,"replay_preserves_canonical_state":true,
                "live_namespace_and_complete_bodies":true,"retained_seed_bodies_exact":true,
                "rejected_receipt_absent_when_expected":true,"engine_check_no_issues":true}}),
    );
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        return Err("usage: namespace-costs <4..256 files> <empty|content-one|content-new|content-four|rename|delete|rename-conflict|delete-conflict>".into());
    }
    let count: usize = args[0].parse()?;
    if !(4..=256).contains(&count) {
        return Err("fixture files must be 4..256".into());
    }
    let case = Case::parse(&args[1])?;
    let temporary = tempfile::Builder::new()
        .prefix("zxs-namespace-costs-")
        .tempdir()?;
    let root = temporary.path().join("system");
    let config = Config::development([
        temporary.path().join("primary"),
        temporary.path().join("backup"),
        temporary.path().join("tertiary"),
    ])?;
    let mut engine = Engine::initialize(&root, config)?;
    let repo = engine.create_repository(
        &storage_engine::RepositoryName::parse("Namespace costs fixture")?,
        "P01",
        3,
        "admin",
        OperationId::generate()?,
    )?;
    let branch = repo.main_branch_id;
    println!(
        "{}",
        json!({"event":"probe_start","case":case.label(),"live_files_seed":count,
        "scope":"isolated-development-cloud-proxy","target_host_measurement":false,
        "physical_redundancy_evidence":false,"copies":3,"payload_bytes_per_file":128,
        "temporary_root":temporary.path().display().to_string(),"concurrency":1,
        "cache":"fixture and canonical oracle warm metadata; OS cache uncontrolled; no cache drop",
        "source_commit_binding":"external centralized build and binary provenance required",
        "allocation_scope":"process-wide successful alloc/alloc_zeroed/realloc requests between phase snapshots; full new realloc size; dealloc excluded; atomic observer overhead included; not peak/live/resident memory",
        "phase_boundaries":{"prepare_capture_ns":"public prepare_workspace_commit",
            "prepare_execute_ns":"public execute: planning plus payload and metadata preparation",
            "finish_publication_ns":"public finish_workspace_commit: recheck, publication and completion",
            "fixture":"one batch of N stages and one commit; excluded from measured_commit phases",
            "rejection":"stale group source: winner replaces final sorted file; rejection occurs in execute"},
        "timings_are_gates":false})
    );
    let (files, seed_receipt) = seed(&mut engine, &branch, count)?;
    let mut receipts = vec![seed_receipt];
    let (workspace, create) = elapsed(|| engine.create_workspace(&branch, "admin"));
    let workspace = workspace?;
    let stage_timer = PhaseTimer::start();
    for offset in 0..case.staged_files() {
        let index = match case {
            Case::ContentOne => count - 1,
            Case::ContentNew => count,
            _ => offset,
        };
        engine.stage_bytes(&workspace.workspace_id, &path(index), &bytes(index, 2))?;
    }
    if let Some(operation) = case.namespace_change()? {
        engine.namespace_operation(&workspace.workspace_id, operation)?;
    }
    let stage = stage_timer.finish();
    let request = request(workspace, "measured namespace cost case")?;
    if case.rejects() {
        receipts.push(competing_save(&mut engine, &branch, count)?);
    }
    let before = storage_engine::canonical_json(engine.state())?;
    let before_namespace = storage_engine::canonical_json(&engine.list_tree(&branch)?)?;
    let before_workspace =
        storage_engine::canonical_json(&engine.workspace(&request.workspace_id)?)?;
    println!(
        "{}",
        json!({"event":"operation_setup","case":case.label(),"measured_operation":false,
        "create_workspace_ns":create.ns,"stage_and_queue_namespace_ns":stage.ns,
        "allocations":{"create_workspace":create.allocations(),"stage_and_queue_namespace":stage.allocations()},
        "entries_before":engine.list_tree(&branch)?.len(),"staged_files":case.staged_files(),
        "versions_before":engine.state().versions.len(),"canonical_state_bytes":before.len()})
    );
    if let Some(receipt) = measure(&mut engine, &request, case, count)? {
        if matches!(case, Case::Empty) {
            assert_eq!(receipt.result.result["no_change"], json!(true));
            assert_eq!(
                storage_engine::canonical_json(&engine.list_tree(&branch)?)?,
                before_namespace
            );
        }
        receipts.push(receipt);
    } else {
        assert_eq!(storage_engine::canonical_json(engine.state())?, before);
        assert_eq!(
            storage_engine::canonical_json(&engine.list_tree(&branch)?)?,
            before_namespace
        );
        assert_eq!(
            storage_engine::canonical_json(&engine.workspace(&request.workspace_id)?)?,
            before_workspace
        );
        absent_receipt(&engine, &request)?;
    }
    verify_after_restart(engine, &root, &branch, &files, case, &request, &receipts)?;
    println!(
        "{}",
        json!({"event":"probe_complete","case":case.label(),"live_files_seed":count,
        "expected_rejection":case.rejects(),"all_checks_passed":true})
    );
    Ok(())
}
