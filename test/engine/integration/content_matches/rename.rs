//! Composition with explicit-directory history protection, using real commits.
use super::*;
use storage_contract::{NamespaceOperation, NamespaceOperationKind};
use storage_engine::{
    DirectoryCommitRequest,
    directory_git::{DirectoryGitInitializeBody, DirectoryGitStageOwnedBody},
};

fn initialize_and_commit(pool: &mut Pool, path: &str) {
    let head = pool.engine.state().branches[&pool.branch]
        .head_commit_id
        .clone();
    pool.engine
        .initialize_directory_git(
            &pool.repository,
            &DirectoryGitInitializeBody {
                path: path.into(),
                expected_namespace_head: head,
                operation_id: OperationId::generate().unwrap(),
            },
            "admin",
        )
        .unwrap();
    let status = pool
        .engine
        .directory_git_status(&pool.repository, path)
        .unwrap();
    pool.engine
        .stage_directory_git_owned(
            &pool.repository,
            &DirectoryGitStageOwnedBody {
                directory_id: status.directory_id,
                directory_branch_id: status.directory_branch_id.unwrap(),
                expected_tree_id: status.working_tree_id.unwrap(),
                expected_index_generation: status.index_generation,
                operation_id: OperationId::generate().unwrap(),
            },
            "admin",
        )
        .unwrap();
    let status = pool
        .engine
        .directory_git_status(&pool.repository, path)
        .unwrap();
    pool.engine
        .commit_directory_changes(DirectoryCommitRequest {
            repository_id: &pool.repository,
            directory_branch_id: status.directory_branch_id.as_ref().unwrap(),
            expected_tree_id: status.working_tree_id.as_ref().unwrap(),
            expected_index_generation: status.index_generation,
            message: "Protect explicit directory history",
            principal: "admin",
            operation_id: OperationId::generate().unwrap(),
        })
        .unwrap();
}

fn rename(
    pool: &mut Pool,
    from: &str,
    to: &str,
) -> storage_engine::Result<storage_engine::OperationResult> {
    pool.engine.file_namespace(
        &pool.repository,
        NamespaceOperation::parse(NamespaceOperationKind::Rename, from, Some(to)).unwrap(),
        "admin",
        OperationId::generate().unwrap(),
    )
}

#[test]
fn content_matches_committed_export_rejection_preserves_frozen_query_and_cold_result() {
    let mut pool = Pool::new();
    pool.save(&[("box/source", b"same body"), ("peer", b"same body")]);
    initialize_and_commit(&mut pool, "box");
    let source = pool.entry("box/source");
    let peer = pool.entry("peer");
    let request = pool.request(&source, &source.version_id, None, 64);
    let read = pool
        .engine
        .capture_content_matches(request.clone(), HistoryWindowCancellation::new())
        .unwrap();
    let page = read.page().unwrap();
    assert_eq!(page.matches.len(), 1);
    assert_eq!(page.matches[0].file_id.as_str(), peer.file_id);
    let before = storage_contract::canonical_json(pool.engine.state()).unwrap();
    assert_eq!(
        rename(&mut pool, "box/source", "exported")
            .unwrap_err()
            .code(),
        ErrorCode::UnsupportedOperation
    );
    pool.engine.recheck_content_matches(&read).unwrap();
    assert_eq!(read.page().unwrap(), page);
    assert_eq!(pool.entry("box/source"), source);
    assert!(
        pool.engine
            .namespace_tree(&pool.branch)
            .unwrap()
            .get("exported")
            .is_none()
    );
    assert_eq!(
        storage_contract::canonical_json(pool.engine.state()).unwrap(),
        before
    );
    drop(read);
    let root = pool.root.clone();
    drop(pool.engine);
    pool.engine = Engine::open(&root).unwrap();
    assert_eq!(pool.page(request), page);
}

#[test]
fn content_matches_legal_renames_keep_version_candidates_and_invalidate_old_root() {
    let mut pool = Pool::new();
    pool.save(&[("box/source", b"same body"), ("peer", b"same body")]);
    initialize_and_commit(&mut pool, "box");
    let source = pool.entry("box/source");
    let peer = pool.entry("peer");
    let old = pool
        .engine
        .capture_content_matches(
            pool.request(&source, &source.version_id, None, 64),
            HistoryWindowCancellation::new(),
        )
        .unwrap();
    old.page().unwrap();
    // A committed file stays inside its stable owner. Entering that owner is
    // also legal for a file not committed in a different directory repository.
    let source_context = pool.engine.file_context(&pool.repository, "box/source").unwrap();
    let source_head = pool.engine.file_branch_head(&source_context.branch_id).unwrap();
    let moved_receipt = rename(&mut pool, "box/source", "box/renamed").unwrap();
    let imported_receipt = rename(&mut pool, "peer", "box/imported").unwrap();
    for (receipt, from, to, file) in [
        (&moved_receipt, "box/source", "box/renamed", &source),
        (&imported_receipt, "peer", "box/imported", &peer),
    ] {
        let facts = receipt.result["path_changes"].as_array().unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0]["operation_id"], receipt.operation_id.to_string());
        assert_eq!(facts[0]["file_id"], file.file_id);
        assert_eq!(facts[0]["before"]["path"], from);
        assert_eq!(facts[0]["after"]["path"], to);
        assert_eq!(facts[0]["before_version_id"], file.version_id);
        assert_eq!(facts[0]["after_version_id"], file.version_id);
    }
    assert_eq!(
        pool.engine
            .recheck_content_matches(&old)
            .unwrap_err()
            .code(),
        ErrorCode::HistoryChanged
    );
    drop(old);
    let moved = pool.entry("box/renamed");
    let imported = pool.entry("box/imported");
    assert_eq!(
        (&moved.file_id, &moved.entry_id, &moved.version_id),
        (&source.file_id, &source.entry_id, &source.version_id)
    );
    assert_eq!(
        (&imported.file_id, &imported.entry_id, &imported.version_id),
        (&peer.file_id, &peer.entry_id, &peer.version_id)
    );
    assert_eq!(pool.engine.state().versions.len(), 2);
    let current_node = pool
        .engine
        .file_context(&pool.repository, "box/renamed")
        .unwrap()
        .node_id
        .unwrap();
    assert_eq!(current_node, source.version_id);
    assert_eq!(pool.engine.file_branch_head(&source_context.branch_id).unwrap(), source_head);
    assert!(pool.engine.state().file_path_nodes.is_empty());
    let request = pool.request(&moved, &current_node, None, 64);
    let before_query = storage_contract::canonical_json(pool.engine.state()).unwrap();
    let page = pool.page(request.clone());
    assert_eq!(page.source_version_id.as_str(), source.version_id);
    assert_eq!(
        page.matches.len(),
        1,
        "path events must not duplicate content versions"
    );
    assert_eq!(page.matches[0].file_id.as_str(), peer.file_id);
    assert_eq!(page.matches[0].version_id.as_str(), peer.version_id);
    assert_eq!(
        page.matches[0].creation_name, "peer",
        "creation name is not the new path"
    );
    assert_eq!(
        storage_contract::canonical_json(pool.engine.state()).unwrap(),
        before_query
    );
    let root = pool.root.clone();
    drop(pool.engine);
    pool.engine = Engine::open(&root).unwrap();
    assert_eq!(pool.page(request), page);
}
