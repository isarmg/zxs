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
fn membership_registry_current_directory_label_path_reuse_keeps_id_addressing() {
    let mut fixture = Fixture::new(true);
    let branch_id = directory_branch(&mut fixture);
    let old_directory = fixture.engine.state.directory_branches[&branch_id].directory_id.clone();
    let old_tree = fixture.engine.state.directory_trees[
        &fixture.engine.state.directory_branches[&branch_id].head_tree_id
    ].clone();
    fixture.project(false);
    // A directory owned by D04 survives loss of its P01 file. Explicitly move
    // it before reusing the path; this is not delete/recreate or D3 retirement.
    fixture.engine.file_namespace(
        &fixture.repo.repository_id,
        NamespaceOperation::parse(NamespaceOperationKind::Rename, "dir", Some("relocated"))
            .unwrap(),
        "admin",
        op(),
    ).unwrap();
    save(&mut fixture.engine, &fixture.repo.main_branch_id, "dir/new", b"new scope");
    let replacement = directory_branch(&mut fixture);
    let new_branch = fixture.engine.state.directory_branches[&replacement].clone();
    let new_tree = fixture.engine.state.directory_trees[&new_branch.head_tree_id].clone();
    assert_ne!(new_branch.directory_id, old_directory);
    assert_ne!(replacement, branch_id);
    let name = BranchName::parse("Relocated label").unwrap();
    let mut expected_old = fixture.engine.state.directory_branches[&branch_id].clone();
    expected_old.name = name.as_str().into();
    let operation = op();
    let receipt = fixture.engine.rename_directory_branch(
        &fixture.repo.repository_id, &branch_id, &name, "admin", operation,
    ).unwrap();
    let renamed = fixture.engine.state.directory_branches[&branch_id].clone();
    assert_eq!(renamed, expected_old);
    assert_eq!(renamed.directory_id, old_directory);
    assert_eq!(renamed.path, "relocated");
    assert_eq!(renamed.name, name.as_str());
    assert_eq!(fixture.engine.state.directory_branches[&replacement], new_branch);
    assert_eq!(fixture.engine.state.directory_trees[&new_tree.tree_id], new_tree);
    assert_eq!(fixture.engine.state.directory_trees[&old_tree.tree_id], old_tree);
    let before = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(fixture.engine.rename_directory_branch(
        &fixture.repo.repository_id, &branch_id, &name, "admin", operation,
    ).unwrap(), receipt);
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);

    drop(fixture.engine);
    fixture.engine = Engine::open(&fixture._temp.path().join("system")).unwrap();
    // This fixture reinstalls a verified projection; it does not persist a D3 exit.
    fixture.project(false);
    let namespace = fixture.engine.namespace_tree(&fixture.repo.main_branch_id).unwrap();
    assert_eq!(namespace.get("relocated").unwrap().entry_id, old_directory);
    assert_eq!(namespace.get("dir").unwrap().entry_id, new_branch.directory_id);
    assert_eq!(fixture.engine.state.directory_branches[&branch_id], renamed);
    assert_eq!(fixture.engine.state.directory_branches[&replacement], new_branch);
    assert_eq!(fixture.engine.state.directory_trees[&old_tree.tree_id], old_tree);
    assert_eq!(fixture.engine.state.directory_trees[&new_tree.tree_id], new_tree);
    let before = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(fixture.engine.rename_directory_branch(
        &fixture.repo.repository_id, &branch_id, &name, "admin", operation,
    ).unwrap(), receipt);
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
}

#[test]
fn membership_registry_current_directory_label_rejects_absent_owned_identity() {
    let mut fixture = Fixture::new(true);
    let retired = fixture.engine.create_repository(
        &RepositoryName::parse("retired owner").unwrap(), "P01", 1, "admin", op(),
    ).unwrap();
    save(&mut fixture.engine, &retired.main_branch_id, "dir/only", b"old scope");
    let branch_id = crate::directory_git_fixture::checkpoint(
        &mut fixture.engine, &retired, "dir",
    ).directory_branch_id.unwrap();
    let old_branch = fixture.engine.state.directory_branches[&branch_id].clone();
    let old_tree = fixture.engine.state.directory_trees[&old_branch.head_tree_id].clone();
    for reopen in [false, true] {
        if reopen {
            drop(fixture.engine);
            fixture.engine = Engine::open(&fixture._temp.path().join("system")).unwrap();
        }
        // Unlike a D04 directory with a P01 file, this namespace owner is P01.
        // Re-derive the admitted Current projection, not a physical D3 exit.
        fixture.project(false);
        assert!(fixture.engine.namespace_tree(&retired.main_branch_id).unwrap()
            .get("dir").is_none());
        let before = canonical_json(fixture.engine.state()).unwrap();
        let refused = op();
        assert_eq!(fixture.engine.rename_directory_branch(
            &retired.repository_id, &branch_id,
            &BranchName::parse("Unavailable label").unwrap(), "admin", refused,
        ).unwrap_err().code(), ErrorCode::DirectoryBranchNotFound);
        assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
        assert!(!fixture.engine.state.operations.contains_key(&refused));
        assert_eq!(fixture.engine.state.directory_branches[&branch_id], old_branch);
        assert_eq!(fixture.engine.state.directory_trees[&old_tree.tree_id], old_tree);
    }
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
