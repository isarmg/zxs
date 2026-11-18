//! Real saves and independent cold reopening; no reconstructed ledger fixture.
use crate::{Engine, ReadFile};
use name_policy::{LookupPath, PortablePath};
use std::{fs, io::Read, path::Path};
use storage_contract::{ContentHash, OperationId, Principal};
use storage_contract::snapshot_worktree::{EditWorktree, WorktreeHandle};
use storage_contract::snapshot_worktree::creation::{AllocateWorktree, WorktreeNaming};

#[path = "draft_gateway/fixture.rs"]
mod fixture;
use fixture::{FIRST_ORIGINAL, Fixture, SECOND_ORIGINAL};

fn save(
    engine: &mut Engine, target: WorktreeHandle, principal: &Principal,
    scratch: &Path, path: &str, body: &[u8],
) -> EditWorktree {
    let operation_id = OperationId::generate().unwrap();
    let request = EditWorktree {
        target, path: PortablePath::parse(path).unwrap(),
        expected: engine.directory_worktree_status(target).unwrap().revision,
        operation_id, size: body.len() as u64,
        content_hash: ContentHash::from_digest(blake3::hash(body)),
    };
    let input = scratch.join(operation_id.to_string());
    fs::write(&input, body).unwrap();
    engine.edit_directory_worktree(&request, principal, ReadFile::from(fs::File::open(input).unwrap()))
        .unwrap();
    request
}

fn bytes(engine: &Engine, target: WorktreeHandle, path: &str) -> Vec<u8> {
    let mut content = engine.open_directory_worktree_content(
        target, &LookupPath::parse(path).unwrap(),
    ).unwrap();
    let mut bytes = Vec::new();
    content.file.read_to_end(&mut bytes).unwrap();
    bytes
}

#[test]
fn siblings_survive_resave_and_cold_reopen_without_publishing_either_file() {
    let (fixture, mut engine) = Fixture::with_nested_directory(true);
    let scratch = tempfile::tempdir().unwrap();
    let principal: Principal = "admin".parse().unwrap();
    let root = engine.snapshot_root_context(fixture.targets[0].repository).unwrap();
    let allocate = || AllocateWorktree {
        source: fixture.source, parent_directory_id: root.host.directory_id,
        expected_root_generation: root.host.expected_root_generation,
        operation_id: OperationId::generate().unwrap(), naming: WorktreeNaming::Automatic,
    };
    let instance = engine.allocate_directory_worktree(&allocate(), &principal).unwrap();
    let other = engine.allocate_directory_worktree(&allocate(), &principal).unwrap();
    let branches = engine.state().branches.clone();
    let versions = engine.state().versions.clone();
    let first = save(&mut engine, instance, &principal, scratch.path(), "first.txt", b"first save");
    let nested = save(&mut engine, instance, &principal, scratch.path(), "Folder/Child/existing.txt",
        b"unchanged nested original");
    let again = save(&mut engine, instance, &principal, scratch.path(), "first.txt", b"resaved");
    let revision = engine.directory_worktree_status(instance).unwrap().revision;
    assert_ne!(again.expected, first.expected);
    let check = |engine: &Engine| {
        assert_eq!(engine.state().branches, branches);
        assert_eq!(engine.state().versions, versions);
        assert_eq!(engine.directory_worktree_status(instance).unwrap().revision, revision);
        assert_eq!(bytes(engine, instance, "first.txt"), b"resaved");
        assert_eq!(bytes(engine, instance, "Folder/Child/existing.txt"), b"unchanged nested original");
        assert_eq!(bytes(engine, instance, "second.txt"), SECOND_ORIGINAL);
        assert_eq!(bytes(engine, other, "first.txt"), FIRST_ORIGINAL);
        let receipts = engine.snapshot_receipts().unwrap();
        let active = receipts.active_edits(instance);
        assert_eq!(active.len(), 2);
        for (edit, expected) in active.iter().zip([&again, &nested]) {
            let (plan, published) = receipts.original_edit(instance, edit).unwrap();
            assert_eq!(plan.request, *expected);
            assert_eq!(published.reserve, expected.operation_id);
            assert_eq!(published.edit, *edit);
        }
        assert!(receipts.completed_edit(instance).is_err(),
            "the independent single-file route may not silently commit only the last writer");
    };
    check(&engine);
    drop(engine);
    check(&Engine::open(&fixture.system).unwrap());
}

#[path = "content_plan.rs"]
mod plan;

#[path = "content_publish.rs"]
mod publish;
