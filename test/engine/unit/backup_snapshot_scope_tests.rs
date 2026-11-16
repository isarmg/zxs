//! Real SYSTEM registration, not a `.zxs` name or a fabricated completed row.
use super::require_backup_snapshot_scope;
use crate::persist::snapshot_worktree::{self as codec, NodeAllocation};
use crate::snapshot_worktree::{
    prepare_namespace_host, publication, registry::ReceiptIndex, SnapshotSourceError,
};
use crate::{
    BackupPreparationRequest, BackupTopologyTarget, BackupTransactionRequest,
    CommitRequest, Config, Engine, Error, ErrorCode, Repository,
};
use name_policy::Name;
use std::{collections::BTreeMap, fs, io::Read, path::{Path, PathBuf}, sync::Arc};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use storage_contract::snapshot_worktree::{InitializeSnapshot, SnapshotHandle};
use storage_contract::working_tree::{SnapshotBranchId, SnapshotNodeId, SnapshotRepositoryId};
use storage_contract::{OperationId, Principal};

const BODY: &[u8] = b"independent immutable snapshot body, not a FILE database";

fn op() -> OperationId { OperationId::generate().unwrap() }

fn setup() -> (tempfile::TempDir, Engine, Repository) {
    let temp = tempfile::Builder::new().permissions(fs::Permissions::from_mode(0o700))
        .tempdir().unwrap();
    let config = Config::development(
        ["Primary", "Near archive", "Old far archive"].into_iter()
            .enumerate().map(|(index, name)| {
                (Name::parse(name).unwrap(), temp.path().join(format!("disk-{index}")))
            }),
    ).unwrap();
    let mut engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
    let repository = engine.create_disk_root("P01", "admin", op()).unwrap();
    let workspace = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
    engine.stage_bytes(&workspace.workspace_id, "original.txt", BODY).unwrap();
    engine.commit_workspace(CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: op(), principal: "admin".into(), expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token, message: "real snapshot source".into(),
        request_digest: None,
    }).unwrap();
    // Genuine FILE history alone is supported. It must not be confused with an
    // immutable directory snapshot or inferred from local path spellings.
    require_backup_snapshot_scope(engine.state()).unwrap();
    (temp, engine, repository)
}

fn snapshot_request(engine: &Engine, repository: &Repository) -> InitializeSnapshot {
    let context = engine.snapshot_root_context(repository.repository_id.parse().unwrap()).unwrap();
    InitializeSnapshot { host: context.host, source: context.source, operation_id: op() }
}

fn target(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
    path
}

fn request(engine: &Engine, paths: Vec<PathBuf>) -> BackupPreparationRequest {
    BackupPreparationRequest {
        operation_id: op(), principal: "admin".into(), source_slot_id: "B01".into(),
        target_paths: paths, expected_sequence: engine.state().sequence,
    }
}

type Media = BTreeMap<PathBuf, (u64, u64, u32, Option<Vec<u8>>)>;

fn media(root: &Path) -> Media {
    fn visit(base: &Path, path: &Path, result: &mut Media) {
        let metadata = fs::symlink_metadata(path).unwrap();
        assert!(!metadata.file_type().is_symlink());
        let bytes = if metadata.is_dir() { None } else { Some(fs::read(path).unwrap()) };
        result.insert(path.strip_prefix(base).unwrap().to_path_buf(),
            (metadata.dev(), metadata.ino(), metadata.mode(), bytes));
        if metadata.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                visit(base, &entry.unwrap().path(), result);
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}

fn require_registered(error: Error, publication: OperationId) {
    assert_eq!(error.code(), ErrorCode::UnsupportedOperation);
    assert!(matches!(error, Error::SnapshotSource(ref source)
        if matches!(source.as_ref(), SnapshotSourceError::BackupRegisteredSnapshot(actual)
            if *actual == publication)));
}

fn require_original_body(engine: &Engine, handle: SnapshotHandle) {
    let index = ReceiptIndex::capture(engine.state()).unwrap();
    let held = index.reopen_node(engine, handle).unwrap();
    let entries = held.manifest().entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path.as_str(), "original.txt");
    let crate::working_tree::snapshot::SnapshotEntryContent::File { material, .. } =
        entries[0].content else {
            panic!("real immutable snapshot must contain the original FILE");
        };
    let mut bytes = Vec::new();
    held.open_payload(material.version_id).unwrap().read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, BODY);
}

fn reject_all_starts(engine: &mut Engine, root: &Path, paths: &[PathBuf], handle: SnapshotHandle) {
    let before = Arc::clone(&engine.state);
    let original = media(root);
    let single = request(engine, vec![paths[0].clone()]);
    let single_op = single.operation_id;
    require_registered(engine.start_backup_preparation(
        single, fs::File::open(&paths[0]).unwrap(),
    ).unwrap_err(), handle.publication);
    assert!(engine.operation(single_op).is_none());
    assert_eq!(media(root), original);

    let batch = request(engine, paths.to_vec());
    let batch_op = batch.operation_id;
    require_registered(engine.start_backup_batch_preparation(
        batch, paths.iter().map(|path| fs::File::open(path).unwrap()).collect(),
    ).unwrap_err(), handle.publication);
    assert!(engine.operation(batch_op).is_none());
    assert_eq!(media(root), original);

    let preparation = request(engine, paths.to_vec());
    let transaction_op = preparation.operation_id;
    let transaction = BackupTransactionRequest {
        preparation,
        targets: paths.iter().enumerate().map(|(index, _)| BackupTopologyTarget {
            name: Name::parse(&format!("New archive {index}")).unwrap(),
            disk_id: crate::new_id("disk"), slot_id: format!("C{:02}", index + 2),
            upstream_slot_id: "B01".into(),
        }).collect(),
    };
    require_registered(engine.start_backup_transaction(
        transaction, paths.iter().map(|path| fs::File::open(path).unwrap()).collect(),
    ).unwrap_err(), handle.publication);
    assert!(engine.operation(transaction_op).is_none());
    assert!(engine.backup_preparation_status().is_none());
    assert!(!root.join("system/backup-preparation").exists());
    assert!(Arc::ptr_eq(&before, &engine.state));
    // Complete media includes SYSTEM, all admitted disks and both empty
    // targets. No task intent, root preparation, inode replacement or BODY
    // write is hidden by an operation-result-only assertion.
    assert_eq!(media(root), original);
    require_original_body(engine, handle);
}

#[test]
fn published_snapshot_blocks_whole_backup_before_intent_or_target_writes() {
    let (temp, mut engine, repository) = setup();
    let request = snapshot_request(&engine, &repository);
    let handle = engine.initialize_directory_snapshot(
        &request, &Principal::parse("admin").unwrap(),
    ).unwrap();
    assert_eq!(engine.operation(request.operation_id).unwrap().kind, codec::NODE_RESERVE);
    assert_eq!(engine.operation(handle.publication).unwrap().kind, codec::NODE_COMPLETE);
    require_original_body(&engine, handle);
    let paths = [target(temp.path(), "new-first"), target(temp.path(), "new-second")];
    reject_all_starts(&mut engine, temp.path(), &paths, handle);
    drop(engine);

    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    assert!(!engine.is_read_only());
    assert_eq!(engine.initialize_directory_snapshot(
        &request, &Principal::parse("admin").unwrap(),
    ).unwrap(), handle);
    reject_all_starts(&mut engine, temp.path(), &paths, handle);
}

#[test]
fn reserved_snapshot_is_pending_not_a_published_snapshot() {
    let (temp, mut engine, repository) = setup();
    let request = snapshot_request(&engine, &repository);
    let allocated = NodeAllocation {
        repository: SnapshotRepositoryId::generate().unwrap(),
        branch: SnapshotBranchId::generate().unwrap(),
        node: SnapshotNodeId::generate().unwrap(), completion: op(),
    };
    let host = prepare_namespace_host(
        &mut engine, &request.source, request.host.root_id,
        request.host.expected_root_generation, request.operation_id,
    ).unwrap();
    let (_reserved, _manifest) = publication::reserve(
        &mut engine, &host, &request, allocated, &Principal::parse("admin").unwrap(),
    ).unwrap();
    // This is the actual durable reserve boundary. No node preparation or
    // completion was attempted, and no completion row is fabricated.
    assert_eq!(engine.operation(request.operation_id).unwrap().kind, codec::NODE_RESERVE);
    assert!(engine.operation(allocated.completion).is_none());
    let index = ReceiptIndex::capture(engine.state()).unwrap();
    assert!(index.completions.contains_key(&allocated.completion));
    drop(index);
    let path = target(temp.path(), "unwritten-target");
    let before = Arc::clone(&engine.state);
    let original = media(temp.path());
    let backup = BackupPreparationRequest {
        operation_id: op(), principal: "admin".into(), source_slot_id: "B01".into(),
        target_paths: vec![path.clone()], expected_sequence: engine.state().sequence,
    };
    let error = engine.start_backup_preparation(backup, fs::File::open(&path).unwrap())
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::RecoveryRequired);
    assert!(matches!(error, Error::SnapshotSource(ref source)
        if matches!(source.as_ref(), SnapshotSourceError::BackupRegistration(_))));
    assert!(Arc::ptr_eq(&before, &engine.state));
    assert_eq!(media(temp.path()), original);
    assert!(!temp.path().join("system/backup-preparation").exists());
}

