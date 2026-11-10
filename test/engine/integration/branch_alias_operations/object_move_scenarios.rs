//! One real alias composition checks movement by occurrence, not shared identity.
use super::*;
use storage_contract::{
    file_relocation::{NamespaceEntryId, ObjectMoveRequest},
    history_window::HistoryRepositoryId,
};

#[test]
fn moving_one_directory_preserves_outside_alias_and_each_inside_context() {
    let mut pool = Pool::new();
    let original = selected_root_aliases(&mut pool);
    // Move from an unselected directory branch into the selected composition.
    // Its existing occurrence remains: this is not a new-identity file copy.
    let frozen = pool.register("", "second alias source");
    let selected = pool.engine.state().directory_selections[&pool.repo.repository_id].clone();
    pool.batch(
        storage_contract::DirectoryBatchKind::Move,
        vec![pool.mapping(&frozen, "a/report.txt", &selected, "a/second.txt")],
    );
    let branch = pool
        .engine
        .create_file_branch(
            &original.file_id,
            &original.version_id,
            &storage_contract::BranchName::parse("second alias work").unwrap(),
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap()
        .branch_id
        .unwrap();
    pool.engine
        .select_file_branch_at(
            &pool.repo.repository_id,
            &name_policy::PortablePath::parse("a/second.txt").unwrap(),
            &original.file_id,
            &branch,
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    pool.save("a/second.txt", b"second alias contents");

    let inside = ["a/report.txt", "a/second.txt"].map(|path| {
        (
            pool.current_entry(path),
            pool.engine
                .file_context(&pool.repo.repository_id, path)
                .unwrap(),
        )
    });
    let outside = pool.current_entry("b/report.txt");
    let outside_context = pool
        .engine
        .file_context(&pool.repo.repository_id, "b/report.txt")
        .unwrap();
    let outside_branch = pool.engine.state().branches[&outside_context.branch_id].clone();
    for (entry, _) in &inside {
        assert_eq!(entry.entry_id, outside.entry_id);
        assert_eq!(entry.file_id, outside.file_id);
    }
    let contents = inside.each_ref().map(|(entry, _)| pool.bytes(&entry.version_id));
    assert_ne!(inside[0].0.version_id, inside[1].0.version_id);
    assert_ne!(inside[0].1.branch_id, inside[1].1.branch_id);
    assert_ne!(inside[0].1.branch_id, outside_context.branch_id);
    let directory = pool.current_entry("a");
    let request = ObjectMoveRequest {
        source_repository_id: HistoryRepositoryId::parse(&pool.repo.repository_id).unwrap(),
        source_entry_id: NamespaceEntryId::parse(&directory.entry_id).unwrap(),
        source_path: name_policy::LookupPath::parse("a").unwrap(),
        target_slot_id: "P01".into(),
        target_repository_id: Some(HistoryRepositoryId::parse(&pool.repo.repository_id).unwrap()),
        target_path: name_policy::PortablePath::parse("moved").unwrap(),
        replace: false,
        target_entry_id: None,
    };
    pool.engine
        .move_object(
            &request,
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();

    assert_eq!(pool.current_entry("b/report.txt"), outside);
    assert_eq!(
        pool.engine
            .file_context(&pool.repo.repository_id, "b/report.txt")
            .unwrap(),
        outside_context
    );
    assert_eq!(
        pool.engine.state().branches[&outside_context.branch_id],
        outside_branch
    );
    let contexts = ["moved/report.txt", "moved/second.txt"].map(|path| {
        pool.engine
            .file_context(&pool.repo.repository_id, path)
            .unwrap()
    });
    for (index, path) in ["moved/report.txt", "moved/second.txt"].into_iter().enumerate() {
        let moved = pool.current_entry(path);
        let (original, original_context) = &inside[index];
        assert_eq!(moved.entry_id, original.entry_id);
        assert_eq!(moved.file_id, original.file_id);
        assert_eq!(moved.version_id, original.version_id);
        assert_eq!(contexts[index].branch_id, original_context.branch_id);
        let node_id = contexts[index].node_id.as_ref().unwrap();
        let node = &pool.engine.state().file_path_nodes[node_id];
        assert_eq!(node.path, moved.path());
        assert_eq!(node.previous_path, original.path());
        assert_eq!(node.version_id, original.version_id);
        assert_eq!(node.branch_id, original_context.branch_id);
        assert_eq!(pool.bytes(&moved.version_id), contents[index]);
    }
    assert_ne!(contexts[0].node_id, contexts[1].node_id);
    let namespace = pool
        .engine
        .namespace_tree(&pool.repo.main_branch_id)
        .unwrap();
    assert!(namespace.get("a").is_none());
    assert!(namespace.get("a/report.txt").is_none());
    assert!(namespace.get("a/second.txt").is_none());
    assert!(pool.engine.file_deletion(&original.file_id).is_none());
}

#[test]
fn moving_non_owner_alias_keeps_the_outside_physical_owner() {
    let mut pool = Pool::new();
    let original = selected_root_aliases(&mut pool);
    let owner = pool.engine.state().file_locations[&original.file_id].clone();
    assert_eq!(owner, "b/report.txt");
    let directory = pool.current_entry("a");
    let request = ObjectMoveRequest {
        source_repository_id: HistoryRepositoryId::parse(&pool.repo.repository_id).unwrap(),
        source_entry_id: NamespaceEntryId::parse(&directory.entry_id).unwrap(),
        source_path: name_policy::LookupPath::parse("a").unwrap(),
        target_slot_id: "P01".into(),
        target_repository_id: Some(HistoryRepositoryId::parse(&pool.repo.repository_id).unwrap()),
        target_path: name_policy::PortablePath::parse("moved").unwrap(),
        replace: false,
        target_entry_id: None,
    };
    pool.engine
        .move_object(
            &request,
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();

    assert_eq!(pool.current_entry("moved/report.txt").file_id, original.file_id);
    assert_eq!(pool.current_entry("b/report.txt").file_id, original.file_id);
    assert_eq!(pool.engine.state().file_locations[&original.file_id], owner);
}
