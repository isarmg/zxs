use super::*;

#[test]
fn membership_registry_current_new_repository_and_disk_root_are_writable() {
    let mut fixture = Fixture::new(false);
    fixture.project(false);
    let repository = fixture
        .engine
        .create_repository(
            &RepositoryName::parse("after-exit").unwrap(),
            "D04",
            1,
            "admin",
            op(),
        )
        .unwrap();
    assert!(
        fixture
            .engine
            .namespace_tree(&repository.main_branch_id)
            .unwrap()
            .entries()
            .is_empty()
    );
    save(
        &mut fixture.engine,
        &repository.main_branch_id,
        "dir/a",
        b"new repository identity",
    );
    let created = fixture
        .engine
        .namespace_tree(&repository.main_branch_id)
        .unwrap()
        .get("dir/a")
        .unwrap();
    assert_ne!(created.entry_id, fixture.removed.entry_id);
    assert_ne!(created.file_id, fixture.removed.file_id);

    let root = fixture
        .engine
        .create_disk_root("D04", "admin", op())
        .unwrap();
    assert!(root.file_manager_root);
    save(
        &mut fixture.engine,
        &root.main_branch_id,
        "new/file",
        b"new root content",
    );
    assert!(
        fixture
            .engine
            .namespace_tree(&root.main_branch_id)
            .unwrap()
            .get("new/file")
            .is_some()
    );
    assert!(
        fixture
            .engine
            .namespace_tree(&fixture.repo.main_branch_id)
            .unwrap()
            .get("dir/a")
            .is_none()
    );
    assert_eq!(
        fixture.engine.state.commits[&fixture.original_head]
            .entries
            .iter()
            .find(|entry| entry.path() == "dir/a"),
        Some(&fixture.removed)
    );
}

#[test]
fn membership_registry_current_root_creation_refuses_unregistered_owner_but_replays_receipt() {
    for disable in [false, true] {
        let mut fixture = Fixture::new(false);
        let operation = op();
        let original = fixture
            .engine
            .create_disk_root("P01", "admin", operation)
            .unwrap();
        let repository_operation = op();
        let name = RepositoryName::parse("original").unwrap();
        let repository = fixture
            .engine
            .create_repository(&name, "P01", 1, "admin", repository_operation)
            .unwrap();
        fixture.project(disable);
        let before = canonical_json(fixture.engine.state()).unwrap();
        assert_eq!(
            fixture
                .engine
                .create_disk_root("P01", "admin", op())
                .unwrap_err()
                .code(),
            ErrorCode::PlacementInvalid
        );
        assert_eq!(
            fixture
                .engine
                .create_repository(
                    &RepositoryName::parse("refused").unwrap(),
                    "P01",
                    1,
                    "admin",
                    op()
                )
                .unwrap_err()
                .code(),
            ErrorCode::PlacementInvalid
        );
        assert_eq!(
            fixture
                .engine
                .create_disk_root("P01", "admin", operation)
                .unwrap(),
            original
        );
        assert_eq!(
            fixture
                .engine
                .create_repository(&name, "P01", 1, "admin", repository_operation)
                .unwrap(),
            repository
        );
        assert_eq!(
            fixture
                .engine
                .create_disk_root("P01", "other", operation)
                .unwrap_err()
                .code(),
            ErrorCode::IdempotencyKeyReused
        );
        assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    }
}

#[test]
fn membership_registry_current_new_root_admission_rejects_content_or_detached_owner() {
    let mut fixture = Fixture::new(false);
    fixture.project(false);
    let before = Arc::clone(&fixture.engine.state);
    let prior = Arc::clone(fixture.engine.current_namespaces.as_ref().unwrap());
    let repository = fixture
        .engine
        .create_repository(
            &RepositoryName::parse("new-root").unwrap(),
            "D04",
            1,
            "admin",
            op(),
        )
        .unwrap();
    let next = fixture.engine.state();
    let branch = &next.branches[&repository.main_branch_id];
    let commit = &next.commits[&branch.head_commit_id];
    assert!(
        prior
            .new_branch_writable(&before, next, branch, commit)
            .unwrap()
    );
    let mut populated = commit.clone();
    populated.entries.push(fixture.retained.clone());
    assert_eq!(
        prior
            .new_branch_writable(&before, next, branch, &populated)
            .unwrap_err()
            .code(),
        ErrorCode::WorkspaceChanged
    );
    let mut borrowed_owner = next.clone();
    borrowed_owner
        .repositories
        .get_mut(&repository.repository_id)
        .unwrap()
        .main_branch_id = fixture.repo.main_branch_id.clone();
    assert_eq!(
        prior
            .new_branch_writable(&before, &borrowed_owner, branch, commit)
            .unwrap_err()
            .code(),
        ErrorCode::WorkspaceChanged
    );
    let mut target = next.clone();
    target
        .repositories
        .get_mut(&repository.repository_id)
        .unwrap()
        .primary_slot_id = "P01".into();
    assert_eq!(
        prior
            .new_branch_writable(&before, &target, branch, commit)
            .unwrap_err()
            .code(),
        ErrorCode::PlacementInvalid
    );
}
