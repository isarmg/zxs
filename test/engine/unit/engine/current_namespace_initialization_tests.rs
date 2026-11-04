//! Explicit current-scope initialization. The parent fixture verifies namespace
//! semantics, not durable D3 publication or actual target I/O retirement.
use super::*;

fn initialize(
    fixture: &mut Fixture,
    path: Option<&str>,
    operation: OperationId,
) -> Result<OperationResult> {
    let path = path.map(|path| name_policy::LookupPath::parse(path).unwrap());
    fixture.engine.initialize_directory_scope(
        &fixture.repo.repository_id,
        path.as_ref(),
        "admin",
        operation,
    )
}

fn initialized_tree<'a>(engine: &'a Engine, result: &OperationResult) -> &'a DirectoryTree {
    let branch = result.result["directory_branch_id"].as_str().unwrap();
    let tree = result.result["head_tree_id"].as_str().unwrap();
    assert_eq!(engine.state.directory_branches[branch].head_tree_id, tree);
    &engine.state.directory_trees[tree]
}

fn assert_current_tree(fixture: &Fixture, tree: &DirectoryTree) {
    let current = fixture
        .engine
        .namespace_tree(&fixture.repo.main_branch_id)
        .unwrap();
    let prefix = format!("{}/", name_policy::path_key(&tree.path));
    let expected = current
        .entries()
        .iter()
        .filter(|entry| tree.path.is_empty() || entry.lookup_key().starts_with(&prefix))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(tree.entries, expected);
    let files = tree
        .entries
        .iter()
        .filter(|entry| entry.kind == NamespaceEntryKind::File)
        .collect::<Vec<_>>();
    assert_eq!(tree.file_contexts.len(), files.len());
    for file in files {
        let context = fixture
            .engine
            .file_context(&fixture.repo.repository_id, file.path())
            .unwrap();
        assert_eq!(tree.file_contexts[file.lookup_key()], context);
        let branch = &fixture.engine.state.branches[&tree.file_branches[&file.file_id]];
        assert_eq!(branch.file_id.as_deref(), Some(file.file_id.as_str()));
    }
    for (directory, reference) in &tree.directory_refs {
        assert!(tree.entries.iter().any(|entry| {
            entry.kind == NamespaceEntryKind::Directory && &entry.entry_id == directory
        }));
        assert_eq!(
            fixture.engine.state.directory_tree_selections[directory],
            reference.tree_id
        );
        let child = &fixture.engine.state.directory_trees[&reference.tree_id];
        assert_current_tree(fixture, child);
    }
}

#[test]
fn membership_registry_current_initialize_freezes_only_current_occurrences_and_contexts() {
    for file_manager in [false, true] {
        let mut fixture = Fixture::new(file_manager);
        let commits = fixture.engine.state.commits.clone();
        let trees = fixture.engine.state.directory_trees.clone();
        let old_context = fixture
            .engine
            .state
            .file_contexts
            .get(&crate::business::context_key_from_key(
                &fixture.repo.repository_id,
                "dir/b",
            ))
            .cloned();
        fixture.project(false);
        let directory = fixture
            .engine
            .namespace_tree(&fixture.repo.main_branch_id)
            .unwrap()
            .get("dir")
            .unwrap()
            .entry_id
            .clone();
        let initialized = initialize(&mut fixture, Some("DIR"), op()).unwrap();
        let tree = initialized_tree(&fixture.engine, &initialized);
        assert_eq!(tree.path, "dir");
        assert_eq!(tree.entries, vec![fixture.retained.clone()]);
        let branch = &fixture.engine.state.directory_branches[&tree.directory_branch_id];
        assert_eq!(branch.directory_id, directory);
        assert_current_tree(&fixture, tree);
        if let Some(context) = old_context {
            assert_eq!(tree.file_contexts["dir/b"], context);
        }

        let root = initialize(&mut fixture, None, op()).unwrap();
        let root_tree = initialized_tree(&fixture.engine, &root);
        assert_eq!(root_tree.path, "");
        assert_current_tree(&fixture, root_tree);
        assert_eq!(
            fixture.engine.state.directory_branches[&root_tree.directory_branch_id].directory_id,
            fixture.repo.repository_id
        );
        assert_eq!(
            root_tree.directory_refs[&directory].tree_id,
            initialized.result["head_tree_id"].as_str().unwrap()
        );
        let count = fixture.engine.state.directory_trees.len();
        let again = initialize(&mut fixture, None, op()).unwrap();
        for key in ["directory_branch_id", "head_tree_id"] {
            assert_eq!(again.result[key], root.result[key]);
        }
        assert_eq!(fixture.engine.state.directory_trees.len(), count);
        for (id, commit) in commits.iter() {
            assert_eq!(fixture.engine.state.commits.get(id), Some(commit));
        }
        for (id, tree) in trees.iter() {
            assert_eq!(fixture.engine.state.directory_trees.get(id), Some(tree));
        }
        for branch in fixture.engine.state.branches.values() {
            if !commits.contains_key(&branch.head_commit_id) {
                assert!(
                    fixture.engine.state.commits[&branch.head_commit_id]
                        .entries
                        .iter()
                        .all(|entry| entry.entry_id != fixture.removed.entry_id)
                );
            }
        }
    }
}

#[test]
fn membership_registry_current_initialize_refreshes_selected_descendants_before_root() {
    let mut fixture = Fixture::new(true);
    save(
        &mut fixture.engine,
        &fixture.repo.main_branch_id,
        "dir/child/keep",
        b"retained child",
    );
    let child_context = fixture
        .engine
        .file_context(&fixture.repo.repository_id, "dir/child/keep")
        .unwrap();
    let trees = fixture.engine.state.directory_trees.clone();
    let root_branch =
        fixture.engine.state.directory_selections[&fixture.repo.repository_id].clone();
    let old_root = fixture.engine.state.directory_branches[&root_branch]
        .head_tree_id
        .clone();
    fixture.project(false);
    let result = initialize(&mut fixture, None, op()).unwrap();
    let tree = initialized_tree(&fixture.engine, &result);
    assert_eq!(tree.directory_branch_id, root_branch);
    assert_ne!(tree.tree_id, old_root);
    assert_eq!(tree.parent_tree_id.as_deref(), Some(old_root.as_str()));
    assert_eq!(tree.file_contexts["dir/child/keep"], child_context);
    assert_current_tree(&fixture, tree);
    for (id, tree) in trees.iter() {
        assert_eq!(fixture.engine.state.directory_trees.get(id), Some(tree));
    }
}

#[test]
fn membership_registry_current_initialize_preserves_alias_versions_and_path_contexts() {
    let mut fixture = Fixture::new(true);
    save(
        &mut fixture.engine,
        &fixture.repo.main_branch_id,
        "alias/keep",
        b"alias directory",
    );
    let frozen = fixture
        .engine
        .create_directory_branches(
            &fixture.repo.repository_id,
            Some(&name_policy::LookupPath::parse("dir").unwrap()),
            &BranchName::parse("frozen source").unwrap(),
            "admin",
            op(),
        )
        .unwrap()
        .result["directory_branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let alias = fixture
        .engine
        .namespace_tree(&fixture.repo.main_branch_id)
        .unwrap()
        .get("alias")
        .unwrap();
    let target = fixture.engine.state.directory_selections[&alias.entry_id].clone();
    let batch = DirectoryNamespaceBatch {
        kind: DirectoryBatchKind::Move,
        items: vec![DirectoryNamespaceItem {
            source: DirectoryNamespaceSource {
                directory_branch_id: frozen.clone(),
                expected_tree_id: fixture.engine.state.directory_branches[&frozen]
                    .head_tree_id
                    .clone(),
                path: "dir/b".into(),
                entry_id: fixture.retained.entry_id.clone(),
            },
            target: Some(DirectoryNamespaceTarget {
                directory_branch_id: target.clone(),
                expected_tree_id: fixture.engine.state.directory_branches[&target]
                    .head_tree_id
                    .clone(),
                path: "alias/c".into(),
            }),
        }],
    };
    fixture
        .engine
        .directory_namespace_batch(&fixture.repo.repository_id, &batch, "admin", op())
        .unwrap();
    let branch = fixture
        .engine
        .create_file_branch(
            &fixture.retained.file_id,
            &fixture.retained.version_id,
            &BranchName::parse("alias version").unwrap(),
            "admin",
            op(),
        )
        .unwrap()
        .result["branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    fixture
        .engine
        .select_file_branch_at(
            &fixture.repo.repository_id,
            &name_policy::PortablePath::parse("alias/c").unwrap(),
            &fixture.retained.file_id,
            &branch,
            "admin",
            op(),
        )
        .unwrap();
    save(
        &mut fixture.engine,
        &fixture.repo.main_branch_id,
        "alias/c",
        b"independent alias version",
    );
    let expected = ["dir/b", "alias/c"].map(|path| {
        (
            fixture
                .engine
                .namespace_tree(&fixture.repo.main_branch_id)
                .unwrap()
                .get(path)
                .unwrap()
                .clone(),
            fixture
                .engine
                .file_context(&fixture.repo.repository_id, path)
                .unwrap(),
        )
    });
    assert_eq!(expected[0].0.file_id, expected[1].0.file_id);
    assert_ne!(expected[0].0.version_id, expected[1].0.version_id);
    assert_ne!(expected[0].1.branch_id, expected[1].1.branch_id);
    let frozen_branch = fixture.engine.state.directory_branches[&frozen].clone();
    fixture.project(false);
    let result = initialize(&mut fixture, None, op()).unwrap();
    let tree = initialized_tree(&fixture.engine, &result);
    assert_current_tree(&fixture, tree);
    for (entry, context) in expected {
        assert!(tree.entries.contains(&entry));
        assert_eq!(tree.file_contexts[entry.lookup_key()], context);
    }
    assert_eq!(
        fixture.engine.state.directory_branches[&frozen],
        frozen_branch
    );
}

#[test]
fn membership_registry_current_initialize_reused_directory_path_gets_new_identity() {
    let mut fixture = Fixture::new(true);
    let old_operation = op();
    let receipt = initialize(&mut fixture, Some("dir"), old_operation).unwrap();
    let old_branch_id = receipt.result["directory_branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let old_branch = fixture.engine.state.directory_branches[&old_branch_id].clone();
    let old_tree = fixture.engine.state.directory_trees[&old_branch.head_tree_id].clone();
    fixture.project(false);
    fixture
        .engine
        .file_namespace(
            &fixture.repo.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::DeleteTree, "dir", None).unwrap(),
            "admin",
            op(),
        )
        .unwrap();
    let deleted = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(
        initialize(&mut fixture, Some("dir"), op())
            .unwrap_err()
            .code(),
        ErrorCode::DirectoryNotFound
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), deleted);
    save(
        &mut fixture.engine,
        &fixture.repo.main_branch_id,
        "DIR/new",
        b"new directory identity",
    );
    let replacement = fixture
        .engine
        .namespace_tree(&fixture.repo.main_branch_id)
        .unwrap()
        .get("dir")
        .unwrap()
        .clone();
    assert_ne!(replacement.entry_id, old_branch.directory_id);
    let result = initialize(&mut fixture, Some("dir"), op()).unwrap();
    let tree = initialized_tree(&fixture.engine, &result);
    let branch = &fixture.engine.state.directory_branches[&tree.directory_branch_id];
    assert_ne!(branch.directory_branch_id, old_branch_id);
    assert_eq!(branch.directory_id, replacement.entry_id);
    assert_eq!(branch.path, "DIR");
    assert_current_tree(&fixture, tree);
    assert_eq!(
        fixture.engine.state.directory_branches[&old_branch_id],
        old_branch
    );
    assert_eq!(
        fixture.engine.state.directory_trees[&old_tree.tree_id],
        old_tree
    );

    let before_replay = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(
        initialize(&mut fixture, Some("dir"), old_operation).unwrap(),
        receipt
    );
    assert_eq!(
        initialize(&mut fixture, None, old_operation)
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyKeyReused
    );
    assert_eq!(
        fixture
            .engine
            .initialize_directory_scope(
                &fixture.repo.repository_id,
                Some(&name_policy::LookupPath::parse("dir").unwrap()),
                "other",
                old_operation,
            )
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyKeyReused
    );
    assert_eq!(
        canonical_json(fixture.engine.state()).unwrap(),
        before_replay
    );
}

#[test]
fn membership_registry_current_initialize_requires_writable_owner_even_for_empty_root() {
    for disable in [false, true] {
        let mut fixture = Fixture::new(false);
        let owned = fixture
            .engine
            .create_repository(
                &RepositoryName::parse("target-owned").unwrap(),
                "P01",
                2,
                "admin",
                op(),
            )
            .unwrap();
        save(
            &mut fixture.engine,
            &owned.main_branch_id,
            "dir/shared",
            b"shared",
        );
        let empty = fixture
            .engine
            .create_repository(
                &RepositoryName::parse("empty-target-owned").unwrap(),
                "P01",
                1,
                "admin",
                op(),
            )
            .unwrap();
        let retained = fixture
            .engine
            .create_repository(
                &RepositoryName::parse("empty-retained-owned").unwrap(),
                "D04",
                1,
                "admin",
                op(),
            )
            .unwrap();
        let operation = op();
        let receipt = fixture
            .engine
            .initialize_directory_scope(&owned.repository_id, None, "admin", operation)
            .unwrap();
        fixture.project(disable);
        assert!(
            fixture
                .engine
                .namespace_tree(&owned.main_branch_id)
                .unwrap()
                .get("dir/shared")
                .is_some()
        );
        let before = canonical_json(fixture.engine.state()).unwrap();
        for (repository, path) in [
            (&owned.repository_id, None),
            (&owned.repository_id, Some("dir")),
            (&empty.repository_id, None),
        ] {
            let path = path.map(|path| name_policy::LookupPath::parse(path).unwrap());
            assert_eq!(
                fixture
                    .engine
                    .initialize_directory_scope(repository, path.as_ref(), "admin", op())
                    .unwrap_err()
                    .code(),
                ErrorCode::BranchNotActive
            );
        }
        assert_eq!(
            fixture
                .engine
                .initialize_directory_scope(&owned.repository_id, None, "admin", operation)
                .unwrap(),
            receipt
        );
        assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
        let result = fixture
            .engine
            .initialize_directory_scope(&retained.repository_id, None, "admin", op())
            .unwrap();
        let tree = initialized_tree(&fixture.engine, &result);
        assert!(tree.entries.is_empty());
        assert_eq!(
            fixture.engine.state.directory_branches[&tree.directory_branch_id].directory_id,
            retained.repository_id
        );
    }
}

#[test]
fn membership_registry_current_initialize_recovery_revalidates_exact_owner_entries() {
    let mut fixture = Fixture::new(false);
    fixture.project(false);
    let before = Arc::clone(&fixture.engine.state);
    let before_snapshot = fixture.engine.snapshot.clone();
    let source = fixture.engine.current_namespaces.clone();
    let result = initialize(&mut fixture, Some("dir"), op()).unwrap();
    let state: State =
        json_codec::from_slice(&canonical_json(fixture.engine.state()).unwrap()).unwrap();
    let snapshot = fixture.engine.snapshot.clone().unwrap();
    fixture.engine.state = before;
    fixture.engine.snapshot = before_snapshot.clone();
    fixture.engine.current_namespaces = source;
    let branch_id = result.result["directory_branch_id"].as_str().unwrap();
    let tree_id = result.result["head_tree_id"].as_str().unwrap();
    let mut copied = fixture.retained.clone();
    copied.relocate(name_policy::PortablePath::parse("dir/copied").unwrap());
    for imported in [fixture.removed.clone(), copied] {
        let mut forged = state.clone();
        for entries in [
            &mut forged
                .directory_branches
                .get_mut(branch_id)
                .unwrap()
                .entries,
            &mut forged.directory_trees.get_mut(tree_id).unwrap().entries,
        ] {
            entries.push(imported.clone());
            entries.sort_by(|left, right| left.lookup_key().cmp(right.lookup_key()));
        }
        assert_eq!(
            fixture
                .engine
                .install_recovered_namespace_state(forged, snapshot.clone())
                .unwrap_err()
                .code(),
            ErrorCode::WorkspaceChanged
        );
        assert_eq!(fixture.engine.snapshot, before_snapshot);
    }
    fixture
        .engine
        .install_recovered_namespace_state(state, snapshot)
        .unwrap();
    assert_current_tree(&fixture, initialized_tree(&fixture.engine, &result));
    let before_rejections = canonical_json(fixture.engine.state()).unwrap();
    for kind in ["branch.create", "file.history.merge", "merge-link.create"] {
        assert_eq!(
            fixture
                .engine
                .current_namespaces
                .as_ref()
                .unwrap()
                .require_operation(kind)
                .unwrap_err()
                .code(),
            ErrorCode::RecoveryRequired
        );
    }
    assert_eq!(
        canonical_json(fixture.engine.state()).unwrap(),
        before_rejections
    );
}
