//! A durable sparse leaf and local frame are not an independent completion.
#[path = "draft_gateway/fixture.rs"]
mod fixture;
use crate::persist::layout::worktree as layout;
use crate::persist::snapshot_worktree::{self as codec, Record};
use crate::{Engine, ReadFile};
use fixture::Fixture;
use std::{fs, io::{Seek, Write}, sync::Arc};
use storage_contract::snapshot_worktree::creation::{AllocateWorktree, WorktreeNaming};
use storage_contract::snapshot_worktree::EditWorktree;
use storage_contract::{ContentHash, ErrorCode, OperationId, Principal};

fn upload(bytes: &[u8]) -> ReadFile {
    let mut held = tempfile::tempfile().unwrap();
    held.write_all(bytes).unwrap();
    held.rewind().unwrap();
    held.into()
}

#[test]
fn lost_edit_completion_reports_its_original_without_adopting_local_bytes_or_frames() {
    let (fixture, mut engine) = Fixture::new();
    let principal: Principal = "admin".parse().unwrap();
    let context = engine.snapshot_root_context(fixture.targets[0].repository).unwrap();
    let target = engine.allocate_directory_worktree(&AllocateWorktree {
        source: fixture.source, parent_directory_id: context.host.directory_id,
        expected_root_generation: context.host.expected_root_generation,
        operation_id: OperationId::generate().unwrap(), naming: WorktreeNaming::Automatic,
    }, &principal).unwrap();
    let bytes = b"this edit's exact retained body";
    let request = EditWorktree {
        target, path: fixture.targets[0].path.clone(),
        expected: engine.directory_worktree_status(target).unwrap().revision,
        operation_id: OperationId::generate().unwrap(), size: bytes.len() as u64,
        content_hash: ContentHash::from_digest(blake3::hash(bytes)),
    };
    let directory = engine.snapshot_receipts().unwrap().reopen_node(&engine, fixture.source)
        .unwrap().worktrees_directory().path().to_owned();
    let initial_versions = engine.state().versions.clone();
    engine.inject_fault_once("after_worktree_edit_ledger");
    let error = engine.edit_directory_worktree(&request, &principal, upload(bytes)).unwrap_err();
    assert_eq!(error.code(), ErrorCode::RecoveryRequired);
    assert_eq!(error.published_operation(), Some(request.operation_id));
    let original = engine.operation(request.operation_id).unwrap();
    let Record::ReserveEdit(plan) = codec::decode(&original).unwrap().unwrap().record else {
        panic!("the original is an actual edit reservation");
    };
    let leaf = directory.join(layout::instance(target.worktree_id).as_path())
        .join(layout::edited_file(plan.version, &request.path).as_path());
    let local_frame = directory.join(layout::ledger(target.worktree_id).as_path())
        .join(layout::frame(plan.before.sequence.next().unwrap()).as_path());
    let frame_bytes = fs::read(&local_frame).unwrap();
    assert_eq!(fs::read(&leaf).unwrap().as_slice(), bytes);
    assert!(engine.operation(plan.completion).is_err());
    drop(engine);

    let mut engine = Engine::open(&fixture.system).unwrap();
    let media = fixture.media();
    let state = Arc::clone(&engine.state);
    let pending = engine.edit_directory_worktree(&request, &principal, upload(b"")).unwrap_err();
    assert_eq!(pending.code(), ErrorCode::RecoveryRequired);
    assert_eq!(pending.published_operation(), Some(request.operation_id));
    let mut changed = request.clone();
    changed.size += 1;
    for error in [
        engine.edit_directory_worktree(&changed, &principal, upload(b"")).unwrap_err(),
        engine.edit_directory_worktree(&request, &"other".parse().unwrap(), upload(b"")).unwrap_err(),
    ] {
        assert_eq!(error.code(), ErrorCode::IdempotencyKeyReused);
        assert_eq!(error.published_operation(), None);
    }
    let mut fresh = request.clone();
    fresh.operation_id = OperationId::generate().unwrap();
    let fresh_error = engine.edit_directory_worktree(&fresh, &principal, upload(bytes)).unwrap_err();
    assert_eq!(fresh_error.code(), ErrorCode::RecoveryRequired);
    assert_eq!(fresh_error.published_operation(), None);
    assert!(engine.operation(fresh.operation_id).is_err());
    assert!(engine.operation(plan.completion).is_err());
    assert!(engine.directory_worktree_status(target).is_err());
    assert!(engine.open_directory_worktree_content(target, &(&request.path).into()).is_err());
    assert_eq!(engine.operation(request.operation_id).unwrap(), original);
    assert_eq!(engine.state().versions, initial_versions);
    assert!(Arc::ptr_eq(&state, &engine.state));
    assert_eq!(fs::read(&local_frame).unwrap(), frame_bytes);
    assert_eq!(fs::read(&leaf).unwrap().as_slice(), bytes);
    assert_eq!(fixture.media(), media);
}
