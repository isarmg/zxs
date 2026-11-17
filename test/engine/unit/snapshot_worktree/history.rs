//! Actual initialization, directory publication and independently created
//! historical worktrees; no fabricated node or receipt is installed.
use crate::Engine;
use name_policy::LookupPath;
use std::io::Read;
use std::sync::Arc;
use storage_contract::snapshot_worktree::creation::{AllocateWorktree, WorktreeNaming};
use storage_contract::snapshot_worktree::draft_commit::{
    DirectoryDraftLookup, DirectoryDraftOutcome,
};
use storage_contract::snapshot_worktree::history::{
    DirectorySnapshotHistory, SnapshotHistoryCursor,
};
use storage_contract::snapshot_worktree::{SnapshotHandle, WorktreeHandle};
use storage_contract::{ErrorCode, OperationId, Sequence};

#[path = "draft_gateway/fixture.rs"]
mod fixture;
use fixture::{FIRST_DRAFT, FIRST_ORIGINAL, Fixture, selection, write_draft};

fn observe(
    fixture: &Fixture,
    engine: &Engine,
    cursor: Option<SnapshotHistoryCursor>,
) -> DirectorySnapshotHistory {
    let state = Arc::clone(&engine.state);
    let slot = engine.next_slot;
    let snapshot = engine.snapshot.clone();
    let media = fixture.media();
    let result = engine
        .directory_snapshot_history(fixture.targets[0].repository, cursor)
        .unwrap();
    assert!(Arc::ptr_eq(&state, &engine.state));
    assert_eq!(engine.next_slot, slot);
    assert_eq!(engine.snapshot, snapshot);
    assert_eq!(fixture.media(), media);
    result
}

fn request(fixture: &Fixture, engine: &Engine, source: SnapshotHandle) -> AllocateWorktree {
    let context = engine
        .snapshot_root_context(fixture.targets[0].repository)
        .unwrap();
    AllocateWorktree {
        source,
        parent_directory_id: context.host.directory_id,
        expected_root_generation: context.host.expected_root_generation,
        operation_id: OperationId::generate().unwrap(),
        naming: WorktreeNaming::Automatic,
    }
}

fn read(engine: &Engine, target: WorktreeHandle) -> Vec<u8> {
    let mut content = engine
        .open_directory_worktree_content(target, &LookupPath::parse("first.txt").unwrap())
        .unwrap();
    let mut bytes = Vec::new();
    content.file.read_to_end(&mut bytes).unwrap();
    bytes
}

#[test]
fn actual_history_lists_exact_nodes_and_creates_old_source_without_reselecting_head() {
    let (fixture, mut engine) = Fixture::new();
    let principal = "admin".parse().unwrap();
    let first = observe(&fixture, &engine, None);
    assert_eq!(first.repository_id, fixture.targets[0].repository);
    assert_eq!(first.nodes.len(), 1);
    assert_eq!(first.nodes[0].snapshot, fixture.source);
    assert_eq!(first.nodes[0].branch_name.as_str(), "main");
    assert_eq!(first.nodes[0].parent, None);
    assert!(first.nodes[0].is_head);
    assert!(first.next_cursor.is_none());

    write_draft(&mut engine, &fixture.targets[0], FIRST_DRAFT);
    let selected = [selection(&engine, &fixture.targets[0])];
    let intent = fixture.intent(&selected, &principal);
    let operation = intent.operation;
    engine.commit_directory_working_set(intent).unwrap();
    let result = engine
        .directory_draft_receipt(
            DirectoryDraftLookup {
                source: fixture.source,
                operation_id: operation,
            },
            &principal,
        )
        .unwrap();
    let DirectoryDraftOutcome::Changed { snapshot: changed } = result.result else {
        panic!("a real selected draft must publish its independently bound node");
    };
    let history = observe(&fixture, &engine, None);
    assert_eq!(history.nodes.len(), 2);
    assert_eq!(history.nodes[0].snapshot, changed);
    assert_eq!(history.nodes[0].parent, Some(fixture.source));
    assert!(history.nodes[0].is_head);
    assert_eq!(history.nodes[1].snapshot, fixture.source);
    assert!(!history.nodes[1].is_head);
    let cursor = SnapshotHistoryCursor {
        sequence: Sequence::new(engine.state().sequence),
        after: changed,
    };
    let older = observe(&fixture, &engine, Some(cursor));
    assert_eq!(older.nodes, history.nodes[1..]);
    assert!(older.next_cursor.is_none());

    let old_request = request(&fixture, &engine, history.nodes[1].snapshot);
    let old = engine
        .allocate_directory_worktree(&old_request, &principal)
        .unwrap();
    let head_request = request(&fixture, &engine, history.nodes[0].snapshot);
    let current = engine
        .allocate_directory_worktree(&head_request, &principal)
        .unwrap();
    assert_eq!(
        engine.directory_worktree_status(old).unwrap().original,
        fixture.source
    );
    assert_eq!(
        engine.directory_worktree_status(old).unwrap().name.as_str(),
        "worktree-main-001"
    );
    assert_eq!(
        engine.directory_worktree_status(current).unwrap().original,
        changed
    );
    assert_eq!(
        engine
            .directory_worktree_status(current)
            .unwrap()
            .name
            .as_str(),
        "worktree-main-002"
    );
    assert_ne!(old, current);
    assert_eq!(read(&engine, old), FIRST_ORIGINAL);
    assert_eq!(read(&engine, current), FIRST_DRAFT);
    let before = fixture.media();
    let sequence = engine.state().sequence;
    assert_eq!(
        engine
            .directory_snapshot_history(fixture.targets[0].repository, Some(cursor))
            .unwrap_err()
            .code(),
        ErrorCode::ListingChanged
    );
    assert_eq!(fixture.media(), before);
    assert_eq!(engine.state().sequence, sequence);
    let original = engine
        .directory_worktree_creation_receipt(
            fixture.targets[0].repository,
            old_request.operation_id,
            &principal,
        )
        .unwrap();
    drop(engine);

    let mut engine = Engine::open(&fixture.system).unwrap();
    let history = observe(&fixture, &engine, None);
    assert_eq!(
        history
            .nodes
            .iter()
            .map(|node| node.snapshot)
            .collect::<Vec<_>>(),
        [changed, fixture.source]
    );
    assert_eq!(history.nodes[0].parent, Some(fixture.source));
    assert_eq!(read(&engine, old), FIRST_ORIGINAL);
    assert_eq!(read(&engine, current), FIRST_DRAFT);
    let before = fixture.media();
    let state = Arc::clone(&engine.state);
    assert_eq!(
        engine
            .allocate_directory_worktree(&old_request, &principal)
            .unwrap(),
        old
    );
    assert_eq!(
        engine
            .directory_worktree_creation_receipt(
                fixture.targets[0].repository,
                old_request.operation_id,
                &principal,
            )
            .unwrap(),
        original
    );
    assert!(Arc::ptr_eq(&state, &engine.state));
    assert_eq!(fixture.media(), before);
}
