//! Container-wide names are current state, not a selected content/tree property.
use super::*;
use storage_contract::file_relocation::{NamespaceEntryId, ObjectMoveRequest};
use storage_contract::history_window::HistoryRepositoryId;

#[test]
fn current_container_name_follows_all_aliases_and_old_tree_selection_after_reopen() {
    for action in ["namespace", "batch", "object"] {
        let mut pool = Pool::disk_root();
        let (frozen, selected, original) = selected_aliases(&mut pool);
        let frozen_branch = pool.branch(&frozen);
        let frozen_tree = pool.engine.state().directory_trees[&frozen_branch.head_tree_id].clone();
        let versions = pool.engine.state().versions.clone();
        let before = ["a/report.txt", "b/report.txt"].map(|path| pool.current_entry(path));
        let contexts = ["a/report.txt", "b/report.txt"].map(|path| {
            pool.engine.file_context(&pool.repo.repository_id, path).unwrap()
        });
        let operation = storage_contract::OperationId::generate().unwrap();
        let receipt = match action {
            "namespace" => pool.engine.file_namespace(
                &pool.repo.repository_id,
                NamespaceOperation::parse(
                    storage_contract::NamespaceOperationKind::Rename,
                    "a/report.txt", Some("a/最新.txt"),
                ).unwrap(),
                "admin", operation,
            ).unwrap(),
            "batch" => {
                let item = pool.mapping(&selected, "a/report.txt", &selected, "a/最新.txt");
                pool.engine.directory_namespace_batch(
                    &pool.repo.repository_id,
                    &DirectoryNamespaceBatch {
                        kind: storage_contract::DirectoryBatchKind::Rename,
                        items: vec![item],
                    },
                    "admin", operation,
                ).unwrap()
            }
            "object" => pool.engine.move_object(
                &ObjectMoveRequest {
                    source_repository_id: HistoryRepositoryId::parse(&pool.repo.repository_id).unwrap(),
                    source_entry_id: NamespaceEntryId::parse(&original.entry_id).unwrap(),
                    source_path: name_policy::LookupPath::parse("a/report.txt").unwrap(),
                    target_slot_id: "P01".into(),
                    target_repository_id: Some(
                        HistoryRepositoryId::parse(&pool.repo.repository_id).unwrap(),
                    ),
                    target_path: name_policy::PortablePath::parse("a/最新.txt").unwrap(),
                    replace: false,
                    target_entry_id: None,
                },
                "admin", operation,
            ).unwrap(),
            _ => unreachable!(),
        };
        assert_eq!(pool.engine.state().versions, versions);
        for ((old, context), path) in before.iter().zip(&contexts)
            .zip(["a/最新.txt", "b/最新.txt"])
        {
            let current = pool.current_entry(path);
            assert_eq!(current.file_id, old.file_id);
            assert_eq!(current.version_id, old.version_id);
            assert_eq!(pool.engine.file_context(&pool.repo.repository_id, path).unwrap(), *context);
            unchanged_body_path_fact(&receipt, &pool.repo.repository_id, old, old.path(), path);
        }
        assert_eq!(pool.branch(&frozen), frozen_branch);
        pool.select("", "retained");
        assert_eq!(pool.current_entry("a/最新.txt").version_id, original.version_id);
        assert_eq!(pool.engine.state().directory_trees[&frozen_tree.tree_id], frozen_tree);
        assert_eq!(pool.engine.state().file_names[&original.file_id], "最新.txt");
        assert_eq!(pool.bytes(&original.version_id), b"original");
        let repo = pool.repo.clone();
        let file = original.file_id.clone();
        pool.reopen(|engine| {
            let tree = engine.namespace_tree(&repo.main_branch_id).unwrap();
            assert_eq!(tree.get("a/最新.txt").unwrap().file_id, file);
            assert!(tree.get("a/report.txt").is_none());
            assert_eq!(engine.state().file_names[&file], "最新.txt");
        });
    }
}

#[test]
fn current_container_rename_collision_at_a_different_alias_leaves_state_and_bodies_intact() {
    let mut pool = Pool::disk_root();
    let original = selected_root_aliases(&mut pool);
    pool.save("b/occupied.txt", b"other container");
    let before = pool.engine.state().clone();
    let error = pool.engine.file_namespace(
        &pool.repo.repository_id,
        NamespaceOperation::parse(
            storage_contract::NamespaceOperationKind::Rename,
            "a/report.txt", Some("a/occupied.txt"),
        ).unwrap(),
        "admin", storage_contract::OperationId::generate().unwrap(),
    ).unwrap_err();
    assert_eq!(error.code(), storage_contract::ErrorCode::NameCollision);
    assert_eq!(pool.engine.state(), &before);
    assert_eq!(pool.bytes(&original.version_id), b"original");
    pool.reopen(|engine| assert_eq!(engine.state(), &before));
}
