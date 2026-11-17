//! Extend the existing real Automatic FILE/worktree flow. Both cuts run the
//! production writer; no synthetic operation receipt or node binding is used.
use crate::directory_worktree::ledger::LedgerError;
use crate::persist::layout::worktree as layout;
use crate::persist::snapshot_worktree::{self as codec, Record};
use crate::snapshot_worktree::{RegistrationError, commit_worktree};
use crate::Engine;
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use storage_contract::snapshot_worktree::CommitWorktree;
use storage_contract::{CommitState, OperationId, Principal};

#[derive(Debug, Eq, PartialEq)]
enum Image {
    Directory { device: u64, inode: u64 },
    File { device: u64, inode: u64, bytes: Vec<u8> },
}

fn inventory(root: &Path) -> BTreeMap<PathBuf, Image> {
    fn visit(base: &Path, path: &Path, out: &mut BTreeMap<PathBuf, Image>) {
        let metadata = fs::symlink_metadata(path).unwrap();
        let image = if metadata.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                visit(base, &entry.unwrap().path(), out);
            }
            Image::Directory { device: metadata.dev(), inode: metadata.ino() }
        } else {
            assert!(metadata.is_file());
            Image::File {
                device: metadata.dev(), inode: metadata.ino(), bytes: fs::read(path).unwrap(),
            }
        };
        out.insert(path.strip_prefix(base).unwrap().to_path_buf(), image);
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}

fn unchanged_rejection(
    engine: &mut Engine, system: &Path, disks: &[PathBuf],
    request: &CommitWorktree, principal: &Principal,
) -> RegistrationError {
    let source = Arc::clone(&engine.state);
    let slot = engine.next_slot;
    let snapshot = engine.snapshot.clone();
    let system_before = inventory(system);
    let disks_before: Vec<_> = disks.iter().map(|disk| inventory(disk)).collect();
    let error = commit_worktree(engine, request, principal).unwrap_err();
    assert!(Arc::ptr_eq(&source, &engine.state));
    assert_eq!(engine.next_slot, slot);
    assert_eq!(engine.snapshot, snapshot);
    assert_eq!(inventory(system), system_before);
    assert_eq!(disks.iter().map(|disk| inventory(disk)).collect::<Vec<_>>(), disks_before);
    error
}

fn require_cut(error: RegistrationError, point: &str) {
    let RegistrationError::Publication(error) = error else {
        panic!("the real worktree publisher did not reach {point}");
    };
    assert_eq!(error.code(), storage_contract::ErrorCode::InjectedFailure);
    let expected = format!("interrupted at {point}");
    assert_eq!(error.message().as_ref(), expected.as_str());
}

fn copy_tree(source: &Path, target: &Path) {
    fs::create_dir(target).unwrap();
    fs::set_permissions(target, fs::Permissions::from_mode(0o700)).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let next = target.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &next);
        } else {
            assert!(entry.file_type().unwrap().is_file());
            fs::copy(entry.path(), &next).unwrap();
            fs::set_permissions(next, fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
}

pub(super) fn interrupt_and_resume(
    mut engine: Engine, system: &Path, disks: &[PathBuf],
    request: &CommitWorktree, principal: &Principal,
) -> Engine {
    engine.inject_fault_once("after_worktree_node_publication");
    require_cut(commit_worktree(&mut engine, request, principal).unwrap_err(),
        "after_worktree_node_publication");
    let reserve = engine.operation(request.operation_id).unwrap();
    let Record::ReserveCommit(plan) = codec::decode(&reserve).unwrap().unwrap().record else {
        panic!("the original request must have a genuine permanent reserve");
    };
    assert_eq!(plan.request, *request);
    let file = engine.operation(plan.file_operation).unwrap();
    let node = engine.operation(plan.node_publication).unwrap();
    assert_eq!(file.kind, "workspace.commit");
    assert_eq!(file.commit_state, CommitState::Published);
    assert_eq!(node.kind, codec::COMMIT_NODE);
    assert_eq!(node.commit_state, CommitState::Published);
    assert!(reserve.sequence < file.sequence && file.sequence < node.sequence);
    assert!(!engine.state().operations.contains_key(&plan.completion));
    let operations = engine.state().operations.clone();
    let versions = engine.state().versions.clone();
    let branches = engine.state().branches.clone();
    let (ledger, node_path, node_binding) = {
        let index = engine.snapshot_receipts().unwrap();
        let original = index.reopen_node(&engine, plan.source).unwrap();
        let published = index.reopen_node(&engine, plan.handle()).unwrap();
        (
            original.worktrees_directory().path().join(
                layout::ledger(request.target.worktree_id).as_path(),
            ),
            published.worktrees_directory().path().parent().unwrap().to_path_buf(),
            published.binding(),
        )
    };
    let committing = plan.before.sequence.next().unwrap();
    let finished = committing.next().unwrap();
    let commit_frame = ledger.join(layout::frame(committing).as_path());
    let final_frame = ledger.join(layout::frame(finished).as_path());
    let commit_bytes = fs::read(&commit_frame).unwrap();
    assert!(!final_frame.exists());
    let node_before = inventory(&node_path);
    drop(engine);
    engine = Engine::open(system).unwrap();
    assert!(!engine.is_read_only());
    assert_eq!(engine.state().operations, operations);
    assert_eq!(engine.state().versions, versions);
    assert_eq!(engine.state().branches, branches);
    assert_eq!(inventory(&node_path), node_before);

    let mut other = request.clone();
    other.message.push_str(" different request");
    assert!(matches!(unchanged_rejection(&mut engine, system, disks, &other, principal),
        RegistrationError::Digest(operation) if operation == request.operation_id));
    assert!(matches!(unchanged_rejection(&mut engine, system, disks, request,
        &"another principal".parse().unwrap()),
        RegistrationError::Digest(operation) if operation == request.operation_id));

    // Matching bytes in another inode are not the independently bound ledger.
    let parked = system.with_file_name("published-tail-ledger-parked");
    fs::rename(&ledger, &parked).unwrap();
    copy_tree(&parked, &ledger);
    assert_ne!(fs::metadata(&ledger).unwrap().ino(), fs::metadata(&parked).unwrap().ino());
    assert!(matches!(unchanged_rejection(&mut engine, system, disks, request, principal),
        RegistrationError::Ledger(LedgerError::Directory(
            crate::disk::DirectoryIoError::IdentityChanged(_),
        ))));
    fs::remove_dir_all(&ledger).unwrap();
    fs::rename(&parked, &ledger).unwrap();

    // Nor can an equal-byte replacement of the already published node be adopted.
    let parked = system.with_file_name("published-tail-node-parked");
    fs::rename(&node_path, &parked).unwrap();
    copy_tree(&parked, &node_path);
    assert!(matches!(unchanged_rejection(&mut engine, system, disks, request, principal),
        RegistrationError::NamespaceHost(_)));
    fs::remove_dir_all(&node_path).unwrap();
    fs::rename(&parked, &node_path).unwrap();

    // This is valid canonical JSON for a different Commit operation, not just
    // damaged bytes. Its new hash cannot become this reserve's expected tail.
    let mut frame: json_codec::Value = json_codec::from_slice(&commit_bytes).unwrap();
    assert_eq!(frame["body"]["record"]["kind"], "commit");
    frame["body"]["record"]["operation"] =
        json_codec::Value::String(OperationId::generate().unwrap().to_string());
    fs::write(&commit_frame, storage_contract::canonical_json(&frame).unwrap()).unwrap();
    assert!(matches!(unchanged_rejection(&mut engine, system, disks, request, principal),
        RegistrationError::Ledger(LedgerError::Chain)));
    fs::write(&commit_frame, &commit_bytes).unwrap();

    // Every preceding frame is still required, and no later frame is ignored.
    let genesis = ledger.join(layout::frame(storage_contract::Sequence::new(0)).as_path());
    let genesis_bytes = fs::read(&genesis).unwrap();
    fs::write(&genesis, [genesis_bytes.as_slice(), b"\n"].concat()).unwrap();
    assert!(matches!(unchanged_rejection(&mut engine, system, disks, request, principal),
        RegistrationError::Ledger(LedgerError::Chain)));
    fs::write(&genesis, &genesis_bytes).unwrap();
    let unknown = ledger.join(layout::frame(finished.next().unwrap()).as_path());
    fs::write(&unknown, &commit_bytes).unwrap();
    assert!(matches!(unchanged_rejection(&mut engine, system, disks, request, principal),
        RegistrationError::Ledger(LedgerError::Chain)));
    fs::remove_file(&unknown).unwrap();
    let parked = system.with_file_name("published-tail-frame-parked");
    fs::rename(&commit_frame, &parked).unwrap();
    assert!(matches!(unchanged_rejection(&mut engine, system, disks, request, principal),
        RegistrationError::Ledger(LedgerError::Chain)));
    fs::rename(&parked, &commit_frame).unwrap();

    engine.inject_fault_once("after_worktree_ledger_completion");
    require_cut(commit_worktree(&mut engine, request, principal).unwrap_err(),
        "after_worktree_ledger_completion");
    assert!(final_frame.is_file());
    assert_eq!(engine.state().operations, operations);
    assert_eq!(engine.state().versions, versions);
    assert_eq!(engine.state().branches, branches);
    assert_eq!(engine.operation(plan.file_operation).unwrap(), file);
    assert_eq!(engine.operation(plan.node_publication).unwrap(), node);
    assert_eq!(inventory(&node_path), node_before);
    let completed_ledger = inventory(&ledger);
    drop(engine);
    engine = Engine::open(system).unwrap();
    assert!(!engine.is_read_only());
    assert_eq!(engine.state().operations, operations);
    assert_eq!(inventory(&ledger), completed_ledger);
    let result = commit_worktree(&mut engine, request, principal).unwrap();
    assert_eq!(result, plan.handle());
    assert_eq!(engine.state().operations.len(), operations.len() + 1);
    let completion = engine.operation(plan.completion).unwrap();
    assert_eq!(completion.commit_state, CommitState::Published);
    assert!(node.sequence < completion.sequence);
    let Record::CompleteCommit(record) = codec::decode(&completion).unwrap().unwrap().record else {
        panic!("the original reserved operation must publish the final completion");
    };
    assert_eq!(record.reserve, request.operation_id);
    assert_eq!(record.checkpoint.sequence, finished);
    assert_eq!(engine.operation(plan.file_operation).unwrap(), file);
    assert_eq!(engine.operation(plan.node_publication).unwrap(), node);
    assert_eq!(engine.state().versions, versions);
    assert_eq!(engine.state().branches, branches);
    assert_eq!(inventory(&node_path), node_before);
    assert_eq!(inventory(&ledger), completed_ledger);
    assert_eq!(engine.snapshot_receipts().unwrap()
        .reopen_node(&engine, result).unwrap().binding(), node_binding);
    let source = Arc::clone(&engine.state);
    let slot = engine.next_slot;
    assert_eq!(commit_worktree(&mut engine, request, principal).unwrap(), result);
    assert!(Arc::ptr_eq(&source, &engine.state));
    assert_eq!(engine.next_slot, slot);
    assert_eq!(engine.operation(plan.completion).unwrap(), completion);
    assert_eq!(inventory(&ledger), completed_ledger);
    engine
}
