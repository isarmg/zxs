use super::*;
use std::collections::BTreeSet;
use storage_contract::history_window::{HistoryFileId, HistoryRepositoryId};
use storage_contract::shared_history::{
    SharedHistoryGraph, SharedHistoryNodeKind, SharedHistoryRequest,
};

fn graph_request(pool: &Pool, file: &str) -> SharedHistoryRequest {
    SharedHistoryRequest::parse(
        file,
        pool.engine
            .metadata_view()
            .history_root(&pool.history)
            .unwrap(),
    )
    .unwrap()
}
fn graph(pool: &Pool, file: &str) -> SharedHistoryGraph {
    let request = graph_request(pool, file);
    let read = pool
        .engine
        .capture_shared_history(request.clone(), HistoryWindowCancellation::new())
        .unwrap();
    let result = read.graph().unwrap();
    pool.engine.recheck_shared_history(&read).unwrap();
    result.validate(&request).unwrap();
    result
}
fn members(graph: &SharedHistoryGraph) -> BTreeSet<String> {
    graph
        .files
        .iter()
        .map(|file| file.file_id.to_string())
        .collect()
}
fn delete(pool: &mut Pool, entry: &Entry) {
    pool.engine
        .soft_delete_file(storage_engine::DeleteFileRequest {
            target: storage_engine::FileDeletionTarget {
                repository_id: &HistoryRepositoryId::parse(&pool.repository).unwrap(),
                file_id: &HistoryFileId::parse(&entry.file_id).unwrap(),
                path: &name_policy::LookupPath::parse(entry.path()).unwrap(),
            },
            principal: "admin",
            operation_id: OperationId::generate().unwrap(),
        })
        .unwrap();
}

#[test]
fn shared_history_discovers_transitive_files_without_folding_revisited_bytes_or_empty_files() {
    let mut pool = Pool::new();
    pool.save(&[("a", b"x"), ("b", b"x"), ("empty-a", b""), ("empty-b", b"")]);
    let a = pool.entry("a");
    let b = pool.entry("b");
    let empty = pool.entry("empty-a");
    pool.save(&[("a", b"z"), ("b", b"y"), ("c", b"y")]);
    pool.save(&[("a", b"x")]);
    let latest_a = pool.entry("a");
    let c = pool.entry("c");
    pool.engine
        .file_namespace(
            &pool.repository,
            storage_engine::NamespaceOperation::parse(
                storage_engine::NamespaceOperationKind::Rename,
                "a",
                Some("renamed"),
            )
            .unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let before = storage_contract::canonical_json(pool.engine.state()).unwrap();
    let joined = graph(&pool, &c.file_id);
    assert_eq!(
        members(&joined),
        BTreeSet::from([a.file_id.clone(), b.file_id, c.file_id.clone()])
    );
    assert_eq!(joined.shared_contents.len(), 2);
    assert!(
        joined
            .nodes
            .iter()
            .any(|node| node.kind == SharedHistoryNodeKind::Rename)
    );
    let shared_x = joined
        .shared_contents
        .iter()
        .find(|shared| {
            shared
                .version_ids
                .iter()
                .any(|id| id.as_str() == a.version_id)
        })
        .unwrap();
    assert_eq!(shared_x.version_ids.len(), 3);
    assert_ne!(a.version_id, latest_a.version_id);
    assert!(
        joined
            .nodes
            .iter()
            .any(|node| node.node_id.as_str() == a.version_id)
    );
    assert!(
        joined
            .nodes
            .iter()
            .any(|node| node.node_id.as_str() == latest_a.version_id)
    );
    let alone = graph(&pool, &empty.file_id);
    assert_eq!(alone.files.len(), 1);
    assert!(alone.shared_contents.is_empty());
    assert_eq!(
        storage_contract::canonical_json(pool.engine.state()).unwrap(),
        before
    );
    let root = pool.root.clone();
    drop(pool.engine);
    pool.engine = Engine::open(&root).unwrap();
    assert_eq!(graph(&pool, &c.file_id), joined);
}

#[test]
fn shared_history_new_file_joins_soft_deleted_history_and_remains_writable_after_restart() {
    let mut pool = Pool::new();
    pool.save(&[("old", b"shared")]);
    let old = pool.entry("old");
    container_tags_fixture::edit(
        &mut pool.engine, &old.file_id, &["keep".into()], &[], "admin",
        OperationId::generate().unwrap(),
    ).unwrap();
    delete(&mut pool, &old);
    pool.save(&[("new", b"shared")]);
    let new = pool.entry("new");
    let joined = graph(&pool, &new.file_id);
    assert_eq!(
        members(&joined),
        BTreeSet::from([old.file_id.clone(), new.file_id.clone()])
    );
    let deletion = joined
        .nodes
        .iter()
        .find(|node| node.kind == SharedHistoryNodeKind::Deletion)
        .unwrap();
    assert!(deletion.version_id.is_none());
    assert_eq!(deletion.file_id.as_str(), old.file_id);
    assert!(!deletion.parent_node_ids.is_empty());
    assert_eq!(container_tags_fixture::tags(&pool.engine, &old.file_id),
        BTreeSet::from(["keep".into()]));
    assert!(container_tags_fixture::tags(&pool.engine, &new.file_id).is_empty());
    assert_eq!(
        graph(&pool, &old.file_id).shared_contents,
        joined.shared_contents
    );
    pool.save(&[("new", b"new remains editable")]);
    let edited = pool.entry("new");
    assert_eq!(edited.file_id, new.file_id);
    let current = graph(&pool, &new.file_id);
    assert_eq!(members(&current), members(&joined));
    let root = pool.root.clone();
    drop(pool.engine);
    pool.engine = Engine::open(&root).unwrap();
    assert_eq!(graph(&pool, &new.file_id), current);
    let output = pool._temp.path().join("new.bin");
    pool.engine
        .export_version(&edited.version_id, &output)
        .unwrap();
    assert_eq!(std::fs::read(output).unwrap(), b"new remains editable");
}

#[test]
fn shared_history_purging_bodies_keeps_all_shared_nodes_and_component_members() {
    let mut pool = Pool::new();
    pool.save(&[("a", b"shared"), ("b", b"shared")]);
    let a = pool.entry("a");
    let b = pool.entry("b");
    pool.save(&[("a", b"new a"), ("b", b"new b")]);
    let before = graph(&pool, &a.file_id);
    let plan = pool
        .engine
        .create_purge_plan(
            &pool.history,
            vec![a.version_id.clone(), b.version_id.clone()],
            "admin",
        )
        .unwrap();
    pool.engine
        .commit_plan(&plan.plan_id, "admin", OperationId::generate().unwrap())
        .unwrap();
    let after = graph(&pool, &a.file_id);
    assert_eq!(members(&after), members(&before));
    assert_eq!(after.shared_contents, before.shared_contents);
    for id in [&a.version_id, &b.version_id] {
        assert_eq!(
            after
                .versions
                .iter()
                .find(|v| v.version_id.as_str() == id)
                .unwrap()
                .content_state,
            Some(storage_engine::ContentState::Purged)
        );
    }
    let root = pool.root.clone();
    drop(pool.engine);
    pool.engine = Engine::open(&root).unwrap();
    assert_eq!(graph(&pool, &a.file_id), after);
}

#[test]
fn shared_history_root_cancellation_and_history_boundary_are_exact() {
    let mut pool = Pool::new();
    pool.save(&[("a", b"shared"), ("b", b"shared")]);
    let a = pool.entry("a");
    let foreign = pool
        .engine
        .create_repository(
            &storage_contract::RepositoryName::parse("foreign").unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let workspace = pool
        .engine
        .create_workspace(&foreign.main_branch_id, "admin")
        .unwrap();
    pool.engine
        .stage_bytes(&workspace.workspace_id, "elsewhere", b"shared")
        .unwrap();
    pool.engine.commit_workspace(commit(workspace)).unwrap();
    assert_eq!(graph(&pool, &a.file_id).files.len(), 2);
    let request = graph_request(&pool, &a.file_id);
    let token = HistoryWindowCancellation::new();
    let cancelled = pool
        .engine
        .capture_shared_history(request.clone(), token.clone())
        .unwrap();
    token.cancel();
    assert_eq!(
        cancelled.graph().unwrap_err().code(),
        ErrorCode::OperationAborted
    );
    let read = pool
        .engine
        .capture_shared_history(request.clone(), HistoryWindowCancellation::new())
        .unwrap();
    read.graph().unwrap();
    pool.save(&[("c", b"shared")]);
    assert_eq!(
        pool.engine
            .recheck_shared_history(&read)
            .unwrap_err()
            .code(),
        ErrorCode::HistoryChanged
    );
    assert_eq!(
        pool.engine
            .capture_shared_history(request, HistoryWindowCancellation::new())
            .err()
            .unwrap()
            .code(),
        ErrorCode::HistoryChanged
    );
}

#[test]
fn shared_history_oversized_component_fails_instead_of_claiming_incomplete_membership() {
    let mut pool = Pool::new();
    let paths = (0..=storage_contract::shared_history::MAX_SHARED_HISTORY_FILES)
        .map(|n| format!("file-{n}"))
        .collect::<Vec<_>>();
    let files = paths
        .iter()
        .map(|path| (path.as_str(), b"same".as_slice()))
        .collect::<Vec<_>>();
    pool.save(&files);
    let a = pool.entry(&paths[0]);
    let before = storage_contract::canonical_json(pool.engine.state()).unwrap();
    let read = pool
        .engine
        .capture_shared_history(
            graph_request(&pool, &a.file_id),
            HistoryWindowCancellation::new(),
        )
        .unwrap();
    assert_eq!(read.graph().unwrap_err().code(), ErrorCode::ResourceLimit);
    assert_eq!(
        storage_contract::canonical_json(pool.engine.state()).unwrap(),
        before
    );
}

fn undo_deletion(pool: &mut Pool, entry: &Entry) {
    let node = pool.engine.file_deletion(&entry.file_id).unwrap().clone();
    pool.engine
        .undo_file_deletion(storage_engine::UndoFileDeletionRequest {
            file_id: &HistoryFileId::parse(&entry.file_id).unwrap(),
            deletion_node_id: &node.node_id,
            principal: "admin",
            operation_id: OperationId::generate().unwrap(),
        })
        .unwrap();
}

#[test]
fn shared_history_undoing_one_file_deletion_keeps_both_parallel_content_connections() {
    let mut pool = Pool::new();
    pool.save(&[("a", b"x"), ("b", b"x")]);
    pool.save(&[("a", b"y"), ("b", b"y")]);
    let a = pool.entry("a");
    let b = pool.entry("b");
    let before = graph(&pool, &a.file_id);
    assert_eq!(before.shared_contents.len(), 2);
    let versions = pool.engine.state().versions.clone();
    delete(&mut pool, &a);
    let deleted = graph(&pool, &b.file_id);
    assert_eq!(members(&deleted), members(&before));
    assert_eq!(deleted.shared_contents, before.shared_contents);
    assert!(deleted.nodes.iter().any(|node| {
        node.file_id.as_str() == a.file_id && node.kind == SharedHistoryNodeKind::Deletion
    }));
    undo_deletion(&mut pool, &a);
    let restored = graph(&pool, &a.file_id);
    assert_eq!(members(&restored), members(&before));
    assert_eq!(restored.shared_contents, before.shared_contents);
    assert_eq!(pool.engine.state().versions, versions);
    assert!(!restored.nodes.iter().any(|node| {
        node.file_id.as_str() == a.file_id && node.kind == SharedHistoryNodeKind::Deletion
    }));
    assert!(pool.engine.file_deletion(&b.file_id).is_none());
}

#[test]
fn shared_history_internal_undo_keeps_transitive_members_and_invalidates_the_old_root() {
    let mut pool = Pool::new();
    pool.save(&[("a", b"x"), ("b", b"x")]);
    let a = pool.entry("a");
    pool.save(&[("b", b"y"), ("c", b"y")]);
    let b = pool.entry("b");
    let c = pool.entry("c");
    delete(&mut pool, &b);
    let before = graph(&pool, &a.file_id);
    let frozen = pool
        .engine
        .capture_shared_history(
            graph_request(&pool, &a.file_id),
            HistoryWindowCancellation::new(),
        )
        .unwrap();
    frozen.graph().unwrap();
    undo_deletion(&mut pool, &b);
    assert_eq!(
        pool.engine
            .recheck_shared_history(&frozen)
            .unwrap_err()
            .code(),
        ErrorCode::HistoryChanged
    );
    drop(frozen);
    for file in [&a.file_id, &b.file_id, &c.file_id] {
        let current = graph(&pool, file);
        assert_eq!(members(&current), members(&before));
        assert_eq!(current.shared_contents, before.shared_contents);
    }
    let root = pool.root.clone();
    drop(pool.engine);
    pool.engine = Engine::open(&root).unwrap();
    for file in [&a.file_id, &b.file_id, &c.file_id] {
        let reopened = graph(&pool, file);
        assert_eq!(members(&reopened), members(&before));
        assert_eq!(reopened.shared_contents, before.shared_contents);
    }
}
