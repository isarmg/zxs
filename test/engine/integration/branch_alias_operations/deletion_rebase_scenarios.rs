use super::*;

#[test]
fn active_frozen_batch_deletion_records_live_rebased_path_exactly_once() {
    let mut pool = Pool::new();
    pool.save("Straße/report.txt", b"retained historical content");
    let frozen = pool.register("Straße", "frozen");
    let original = pool.entry(&frozen, "Straße/report.txt");
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
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    assert_eq!(pool.branch(&frozen).path, "Straße");
    pool.select("renamed", "frozen");
    assert_eq!(pool.branch(&frozen).path, "Straße");
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

    let operation = pool.batch(
        "delete",
        vec![DirectoryNamespaceItem {
            source: pool.source(&frozen, "Straße/report.txt"),
            target: None,
        }],
    );
    let main_branch = pool.repo.main_branch_id.clone();
    let inspect = |engine: &Engine| {
        let owners = engine
            .state()
            .commits
            .values()
            .filter(|commit| commit.operation_id == operation.operation_id)
            .filter(|commit| commit.namespace_deletions.is_some())
            .collect::<Vec<_>>();
        assert_eq!(owners.len(), 1);
        assert_eq!(Some(&owners[0].commit_id), operation.commit_id.as_ref());
        let facts = owners[0].namespace_deletions.as_ref().unwrap();
        assert_eq!(facts.paths.len(), 1);
        assert_eq!(facts.paths[0].path, "renamed/report.txt");
        assert_eq!(
            facts.paths[0].kind,
            storage_engine::NamespaceEntryKind::File
        );
        assert!(
            !engine
                .list_tree(&main_branch)
                .unwrap()
                .iter()
                .any(|entry| entry.path == "renamed/report.txt")
        );
    };
    inspect(&pool.engine);
    pool.reopen(inspect);
}
