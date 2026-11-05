//! Public directory labels do not materialize raw historical entries.
use super::*;

fn directory_branch(fixture: &mut Fixture) -> String {
    crate::directory_git_fixture::checkpoint(&mut fixture.engine, &fixture.repo, "dir")
        .directory_branch_id
        .unwrap()
}

#[test]
fn membership_registry_current_directory_label_reuses_the_exact_owned_projection() {
    let mut fixture = Fixture::new(true);
    let branch_id = directory_branch(&mut fixture);
    let before = fixture.engine.state.directory_branches[&branch_id].clone();
    let old_tree = fixture.engine.state.directory_trees[&before.head_tree_id].clone();
    fixture.project(false);
    let scope = Arc::clone(
        &fixture
            .engine
            .current_namespaces
            .as_ref()
            .unwrap()
            .directories[&branch_id],
    );
    let name = BranchName::parse("Visible label").unwrap();
    let operation = op();
    let result = fixture
        .engine
        .rename_directory_branch(
            &fixture.repo.repository_id,
            &branch_id,
            &name,
            "admin",
            operation,
        )
        .unwrap();
    let after = &fixture.engine.state.directory_branches[&branch_id];
    assert_eq!(after.name, name.as_str());
    assert_eq!(after.entries, before.entries);
    assert_eq!(after.head_tree_id, before.head_tree_id);
    assert_eq!(
        fixture.engine.state.directory_trees[&before.head_tree_id],
        old_tree
    );
    assert!(Arc::ptr_eq(
        &scope,
        &fixture
            .engine
            .current_namespaces
            .as_ref()
            .unwrap()
            .directories[&branch_id]
    ));
    assert!(
        fixture
            .engine
            .current_directory_entries(&branch_id)
            .unwrap()
            .iter()
            .all(|entry| entry.path() != "dir/a")
    );
    let state = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(
        fixture
            .engine
            .rename_directory_branch(
                &fixture.repo.repository_id,
                &branch_id,
                &name,
                "admin",
                operation
            )
            .unwrap(),
        result
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), state);
}

#[test]
fn membership_registry_current_directory_label_rejects_reused_path_identity() {
    let mut fixture = Fixture::new(true);
    let branch_id = directory_branch(&mut fixture);
    let old_directory = fixture.engine.state.directory_branches[&branch_id]
        .directory_id
        .clone();
    fixture.project(false);
    let name = BranchName::parse("Before removal").unwrap();
    let operation = op();
    let receipt = fixture
        .engine
        .rename_directory_branch(
            &fixture.repo.repository_id,
            &branch_id,
            &name,
            "admin",
            operation,
        )
        .unwrap();
    fixture
        .engine
        .file_namespace(
            &fixture.repo.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::DeleteTree, "dir", None).unwrap(),
            "admin",
            op(),
        )
        .unwrap();
    save(
        &mut fixture.engine,
        &fixture.repo.main_branch_id,
        "dir/new",
        b"new scope",
    );
    assert_ne!(
        fixture
            .engine
            .namespace_tree(&fixture.repo.main_branch_id)
            .unwrap()
            .get("dir")
            .unwrap()
            .entry_id,
        old_directory
    );
    let state = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(
        fixture
            .engine
            .rename_directory_branch(
                &fixture.repo.repository_id,
                &branch_id,
                &name,
                "admin",
                op()
            )
            .unwrap_err()
            .code(),
        ErrorCode::DirectoryBranchNotFound
    );
    assert_eq!(
        fixture
            .engine
            .rename_directory_branch(
                &fixture.repo.repository_id,
                &branch_id,
                &name,
                "admin",
                operation
            )
            .unwrap(),
        receipt
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), state);
}

#[test]
fn membership_registry_current_directory_label_recovery_requires_unchanged_tree_contents() {
    let mut fixture = Fixture::new(true);
    let branch_id = directory_branch(&mut fixture);
    fixture.project(false);
    let before = fixture.engine.state();
    let view = fixture.engine.current_namespaces.as_ref().unwrap();
    let mut changed = before.clone();
    changed.directory_branches.get_mut(&branch_id).unwrap().name = "Recovered label".into();
    let decoded: State = json_codec::from_slice(&canonical_json(&changed).unwrap()).unwrap();
    let recovered = view.advance(before, &decoded).unwrap();
    assert!(Arc::ptr_eq(
        &view.directories[&branch_id],
        &recovered.directories[&branch_id]
    ));
    let head = changed.directory_branches[&branch_id].head_tree_id.clone();
    changed
        .directory_trees
        .get_mut(&head)
        .unwrap()
        .entries
        .clear();
    assert_eq!(
        view.advance(before, &changed).err().unwrap().code(),
        ErrorCode::PublicationInvalid
    );
    let mut imported = decoded;
    imported
        .directory_branches
        .get_mut(&branch_id)
        .unwrap()
        .entries
        .push(fixture.removed.clone());
    assert!(view.advance(before, &imported).is_err());
}

#[test]
fn membership_registry_current_directory_label_retains_disabled_empty_roots() {
    for disable in [false, true] {
        let mut fixture = Fixture::new(false);
        let root = fixture
            .engine
            .create_disk_root("P01", "admin", op())
            .unwrap();
        let branch = crate::directory_git_fixture::checkpoint(&mut fixture.engine, &root, "")
            .directory_branch_id
            .unwrap();
        fixture.project(disable);
        let result = fixture.engine.rename_directory_branch(
            &root.repository_id,
            &branch,
            &BranchName::parse("Root label").unwrap(),
            "admin",
            op(),
        );
        if disable {
            result.unwrap();
        } else {
            assert_eq!(
                result.unwrap_err().code(),
                ErrorCode::DirectoryBranchNotFound
            );
        }
        assert!(
            fixture
                .engine
                .current_directory_entries(&branch)
                .unwrap()
                .is_empty()
        );
    }
}
