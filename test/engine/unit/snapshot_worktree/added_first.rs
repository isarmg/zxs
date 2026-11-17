//! Actual shared FULL -> FILE copy and node MOVE, with original operation retry.
#[path = "draft_gateway/fixture.rs"]
mod fixture;
use crate::persist::snapshot_worktree::{self as codec, Record};
use crate::{Engine, WorktreeEntryContent};
use fixture::Fixture;
use name_policy::{LookupPath, PortablePath};
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use storage_contract::snapshot_worktree::additions::CreateWorktreeFile;
use storage_contract::snapshot_worktree::creation::{AllocateWorktree, WorktreeNaming};
use storage_contract::snapshot_worktree::{CommitWorktree, SnapshotHandle, WorktreeHandle};
use storage_contract::{OperationId, Principal};
fn principal() -> Principal {
    "admin".parse().unwrap()
}
fn instance(fixture: &Fixture, engine: &mut Engine, source: SnapshotHandle) -> WorktreeHandle {
    let host = engine
        .snapshot_root_context(fixture.targets[0].repository)
        .unwrap()
        .host;
    engine
        .allocate_directory_worktree(
            &AllocateWorktree {
                source,
                parent_directory_id: host.directory_id,
                expected_root_generation: host.expected_root_generation,
                operation_id: OperationId::generate().unwrap(),
                naming: WorktreeNaming::Automatic,
            },
            &principal(),
        )
        .unwrap()
}
fn body(engine: &Engine, target: WorktreeHandle, path: &str) -> (Vec<u8>, (u64, u64)) {
    let mut opened = engine
        .open_directory_worktree_content(target, &LookupPath::parse(path).unwrap())
        .unwrap();
    let metadata = opened.file.metadata().unwrap();
    let mut bytes = Vec::new();
    opened.file.read_to_end(&mut bytes).unwrap();
    (bytes, (metadata.dev(), metadata.ino()))
}
#[test]
fn single_added_first_commit_keeps_deleted_owner_moves_full_and_cold_retries_exactly() {
    let (fixture, mut engine) = Fixture::new();
    let original_branches = engine.state().branches.clone();
    let one = instance(&fixture, &mut engine, fixture.source);
    let two = instance(&fixture, &mut engine, fixture.source);
    let mut entered = Vec::new();
    for target in [one, two] {
        let file = engine
            .create_directory_worktree_file(
                &CreateWorktreeFile {
                    target,
                    path: PortablePath::parse("new.txt").unwrap(),
                    expected: engine.directory_worktree_status(target).unwrap().revision,
                    operation_id: OperationId::generate().unwrap(),
                },
                &principal(),
            )
            .unwrap();
        let mut opened = engine
            .open_directory_worktree_working(&file, &principal(), true)
            .unwrap();
        engine
            .write_native_working(&mut opened, &principal(), 0, b"first")
            .unwrap();
        engine
            .flush_native_working(&mut opened, &principal())
            .unwrap();
        engine
            .write_native_working(&mut opened, &principal(), 0, b"final")
            .unwrap();
        engine
            .write_native_working(&mut opened, &principal(), 9, b"Z")
            .unwrap();
        engine
            .flush_native_working(&mut opened, &principal())
            .unwrap();
        entered.push(file);
    }
    assert_ne!(entered[0].file, entered[1].file);
    let first_owner = engine.state().file_containers[&entered[0].file].clone();
    let second_owner = engine.state().file_containers[&entered[1].file].clone();
    let original_copy = body(&engine, one, "first.txt");
    let old_versions = engine.state().versions.len();
    let original_contexts = engine.state().file_contexts.clone();
    let original_locations = engine.state().file_locations.clone();
    let request = CommitWorktree {
        target: one,
        expected: engine.directory_worktree_status(one).unwrap().revision,
        message: "first Added version".into(),
        operation_id: OperationId::generate().unwrap(),
    };
    let node = engine
        .commit_directory_worktree(&request, &principal())
        .unwrap();
    assert_eq!(node.branch_id, fixture.source.branch_id);
    assert_ne!(node.node_id, fixture.source.node_id);
    let Record::ReserveAddedCommit(plan) =
        codec::decode(&engine.operation(request.operation_id).unwrap())
            .unwrap()
            .unwrap()
            .record
    else {
        panic!("actual original Added reservation");
    };
    let Record::PublishAddedFile(first) =
        codec::decode(&engine.operation(plan.file_operation).unwrap())
            .unwrap()
            .unwrap()
            .record
    else {
        panic!("actual independent first FILE publication");
    };
    assert_eq!(first.reserve, request.operation_id);
    let published = body(&engine, one, "new.txt");
    assert_eq!(published.0, b"final\0\0\0\0Z");
    assert_eq!(
        published.1,
        (first.full.entity.device(), first.full.entity.inode())
    );
    let copy = body(&engine, one, "first.txt");
    assert_eq!(copy.0, original_copy.0);
    assert_ne!(copy.1, original_copy.1);
    assert_eq!(engine.state().versions.len(), old_versions + 1);
    let version = &engine.state().versions[&first.content.version.version_id];
    assert!(version.source_version_ids.is_empty());
    let mut file_body = engine.open_content(&version.version_id).unwrap();
    let metadata = file_body.file.metadata().unwrap();
    assert_ne!((metadata.dev(), metadata.ino()), published.1);
    let mut bytes = Vec::new();
    file_body.file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, published.0);
    drop(file_body);
    let current = &engine.state().file_containers[&entered[0].file];
    assert_eq!(current.namespace, first_owner.namespace);
    assert_eq!(current.replicas, first_owner.replicas);
    assert!(current.working.unborn().is_none());
    let added_key = entered[0].file.to_string();
    assert_eq!(
        engine.state().file_locations[&added_key],
        entered[0].path.as_str(),
    );
    assert_eq!(
        engine.state().file_names[&added_key],
        first_owner.namespace.name.as_str()
    );
    assert_eq!(engine.state().file_contexts, original_contexts);
    assert_eq!(
        engine.state().file_locations.len(),
        original_locations.len() + 1
    );
    for (file, location) in original_locations.iter() {
        assert_eq!(&engine.state().file_locations[file], location);
    }
    assert_eq!(
        engine.state().file_containers[&entered[1].file],
        second_owner
    );
    for (id, branch) in original_branches.iter() {
        assert_eq!(&engine.state().branches[id], branch);
    }
    assert!(matches!(engine.directory_worktree_entry(one,
        &LookupPath::parse("new.txt").unwrap()).unwrap().content,
        WorktreeEntryContent::RetainedFile { target, .. } if target == entered[0]));
    // A later instance references the same FILE without rewriting birth ownership.
    let next = instance(&fixture, &mut engine, node);
    assert_eq!(body(&engine, next, "new.txt").0, published.0);
    assert_eq!(
        engine.state().file_containers[&entered[0].file].namespace,
        first_owner.namespace
    );
    drop(engine);
    let mut engine = Engine::open(&fixture.system).unwrap();
    assert_eq!(
        engine.state().file_locations[&added_key],
        entered[0].path.as_str()
    );
    assert_eq!(
        engine.state().file_names[&added_key],
        first_owner.namespace.name.as_str()
    );
    assert_eq!(engine.state().file_contexts, original_contexts);
    let media = fixture.media();
    let sequence = engine.state().sequence;
    assert_eq!(
        engine
            .commit_directory_worktree(&request, &principal())
            .unwrap(),
        node
    );
    assert_eq!(body(&engine, one, "new.txt"), published);
    let receipt = engine
        .directory_worktree_commit_receipt(
            fixture.targets[0].repository,
            request.operation_id,
            &principal(),
        )
        .unwrap();
    assert_eq!(receipt.request, request);
    assert_eq!(engine.state().sequence, sequence);
    assert_eq!(fixture.media(), media);
    assert_eq!(
        engine.state().file_containers[&entered[1].file],
        second_owner
    );
}
