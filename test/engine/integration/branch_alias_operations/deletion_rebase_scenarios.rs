use super::*;

#[test]
fn active_frozen_batch_deletion_records_one_d_at_the_live_rebased_path() {
    let mut pool = Pool::disk_root();
    pool.save("Straße/report.txt", b"retained historical content");
    let frozen = pool.register("Straße", "frozen");
    let original = pool.entry(&frozen, "Straße/report.txt");
    let frozen_branch = pool.branch(&frozen);
    let committed = pool.engine.state().directory_git_repositories[&frozen_branch.directory_id]
        .branches[&frozen]
        .head_tree_id
        .clone()
        .unwrap();
    pool.engine
        .file_namespace(
            &pool.repo.repository_id,
            NamespaceOperation::parse(
                storage_contract::NamespaceOperationKind::Rename,
                "STRASSE",
                Some("renamed"),
            )
            .unwrap(),
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    assert_eq!(pool.branch(&frozen).path, "Straße");
    pool.select("renamed", "frozen");
    // Checkout rebases only the working composition. Its committed Git tree
    // retains the historical spelling and stable identity.
    assert_eq!(pool.branch(&frozen).path, "renamed");
    assert_eq!(
        pool.engine.state().directory_trees[&committed].path,
        "Straße"
    );
    assert_eq!(
        pool.engine.state().directory_git_repositories[&frozen_branch.directory_id].branches
            [&frozen]
            .head_tree_id
            .as_ref(),
        Some(&committed)
    );
    assert_eq!(
        pool.current_entry("renamed/report.txt").file_id,
        original.file_id
    );
    let branch = pool.branch(&frozen);
    assert_eq!(
        pool.engine.state().directory_selections[&branch.directory_id],
        frozen
    );
    assert_eq!(
        pool.engine.state().directory_tree_selections[&branch.directory_id],
        branch.head_tree_id
    );

    let before = pool.engine.state().clone();
    let operation = pool.batch(
        storage_contract::DirectoryBatchKind::Delete,
        vec![DirectoryNamespaceItem {
            source: pool.source(&frozen, "renamed/report.txt"),
            target: None,
        }],
    );
    let main_branch = pool.repo.main_branch_id.clone();
    let inspect = |engine: &Engine| {
        assert_eq!(operation.kind, "file.delete.batch");
        let nodes = operation.result["deletion_nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 1);
        let node = engine.file_deletion(&original.file_id).unwrap();
        assert_eq!(node.operation_id, operation.operation_id);
        assert_eq!(node.path.as_str(), "renamed/report.txt");
        assert_eq!(json_codec::to_value(node).unwrap(), nodes[0]);
        assert_eq!(engine.state().commits, before.commits);
        assert_eq!(engine.state().directory_branches, before.directory_branches);
        assert_eq!(engine.state().file_contexts, before.file_contexts);
        assert!(
            engine
                .namespace_tree(&main_branch)
                .unwrap()
                .get("renamed/report.txt")
                .is_none()
        );
    };
    inspect(&pool.engine);
    pool.reopen(inspect);
}
