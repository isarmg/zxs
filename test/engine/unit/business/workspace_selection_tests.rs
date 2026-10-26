//! Selection fencing stays path-scoped without scanning the pool per generation.
use super::*;

fn fixture() -> (tempfile::TempDir, Engine, Repository) {
    let temporary = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temporary.path().join("system"),
        Config::development([temporary.path().join("primary")]),
    )
    .unwrap();
    let repo = engine
        .create_repository(
            "Workspace selections",
            "P01",
            1,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    for path in ["Folder/Child/file.txt", "Folderish/file.txt"] {
        engine
            .stage_bytes(&workspace.workspace_id, path, b"original")
            .unwrap();
    }
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "selection fixture".into(),
            request_digest: None,
        })
        .unwrap();
    (temporary, engine, repo)
}

// Reference predicate from the generation-first implementation. Keep the
// original traversal here to check the new traversal's complete selection set.
fn generation_first_conflict(state: &State, workspace: &Workspace, path: &str) -> bool {
    let branch = &state.branches[&workspace.branch_id];
    let base = &state.commits[&workspace.base_head];
    let current = &state.commits[&branch.head_commit_id];
    let path = name_policy::path_key(path);
    state
        .directory_selection_generations
        .iter()
        .any(|(id, generation)| {
            let relevant = state.repositories.values().any(|repo| {
                repo.history_tree_id == branch.history_tree_id && repo.repository_id == *id
            }) || base.entries.iter().chain(&current.entries).any(|entry| {
                entry.kind == NamespaceEntryKind::Directory
                    && entry.entry_id == *id
                    && (path == entry.lookup_key()
                        || path.starts_with(&format!("{}/", entry.lookup_key())))
            });
            relevant
                && workspace
                    .directory_selection_generations
                    .get(id)
                    .copied()
                    .unwrap_or(0)
                    != *generation
        })
}

fn directory(path: &str) -> Entry {
    Entry {
        entry_id: new_id("entry"),
        path: name_policy::PortablePath::parse(path).unwrap(),
        kind: NamespaceEntryKind::Directory,
        file_id: String::new(),
        version_id: String::new(),
        size: 0,
    }
}

#[test]
fn directory_generation_checks_match_both_views_roots_and_component_boundaries() {
    let (_temporary, mut engine, repo) = fixture();
    let mut workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    let mut state = engine.state().clone();
    let mut current = state.commits[&workspace.base_head].clone();
    current.commit_id = new_id("commit");
    let old = directory("Old");
    let future = directory("Future");
    state
        .commits
        .get_mut(&workspace.base_head)
        .unwrap()
        .entries
        .push(old.clone());
    current.entries.push(future.clone());
    state
        .branches
        .get_mut(&workspace.branch_id)
        .unwrap()
        .head_commit_id = current.commit_id.clone();
    state.commits.insert(current.commit_id.clone(), current);
    let mut ids = state.commits[&workspace.base_head]
        .entries
        .iter()
        .map(|entry| entry.entry_id.clone())
        .collect::<Vec<_>>();
    ids.extend([future.entry_id, repo.repository_id.clone(), new_id("entry")]);
    // Retain all roots in the same history, not merely the main-branch owner.
    for same_history in [true, false] {
        let mut other = repo.clone();
        other.repository_id = new_id("repo");
        if !same_history {
            other.history_tree_id = new_id("history");
        }
        ids.push(other.repository_id.clone());
        state
            .repositories
            .insert(other.repository_id.clone(), other);
    }
    for id in ids {
        for current_generation in [None, Some(0), Some(1), Some(2)] {
            state.directory_selection_generations.clear();
            if let Some(generation) = current_generation {
                state
                    .directory_selection_generations
                    .insert(id.clone(), generation);
            }
            for captured_generation in [None, Some(0), Some(1), Some(2)] {
                workspace.directory_selection_generations.clear();
                if let Some(generation) = captured_generation {
                    workspace
                        .directory_selection_generations
                        .insert(id.clone(), generation);
                }
                for path in [
                    "",
                    "folder",
                    "FOLDER/CHILD/file.txt",
                    "Folderish/file.txt",
                    "Old/new.txt",
                    "Future/new.txt",
                    "unrelated.txt",
                ] {
                    let expected = generation_first_conflict(&state, &workspace, path);
                    let actual = validate_directory_selection_key(
                        &state,
                        &workspace,
                        &name_policy::path_key(path),
                    );
                    assert_eq!(
                        actual.is_err(),
                        expected,
                        "{id}/{current_generation:?}/{captured_generation:?}/{path}"
                    );
                    if let Err(error) = actual {
                        assert_eq!(
                            error.code(),
                            storage_contract::ErrorCode::DirectoryBranchConflict
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn empty_generations_still_validate_workspace_references_in_order() {
    let (_temporary, mut engine, repo) = fixture();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    for missing in ["branch", "base", "current"] {
        let mut state = engine.state().clone();
        state.directory_selection_generations.clear();
        let expected = match missing {
            "branch" => {
                state.branches.remove(&workspace.branch_id);
                "workspace branch missing"
            }
            "base" => {
                state.commits.remove(&workspace.base_head);
                "workspace base missing"
            }
            _ => {
                state
                    .branches
                    .get_mut(&workspace.branch_id)
                    .unwrap()
                    .head_commit_id = new_id("commit");
                "current namespace missing"
            }
        };
        let error = validate_directory_selection_key(&state, &workspace, "new.txt").unwrap_err();
        assert!(error.to_string().contains(expected), "{missing}: {error}");
    }
}

#[test]
fn directory_round_trip_fences_ancestors_but_not_siblings_or_other_repositories() {
    let (_temporary, mut engine, repo) = fixture();
    engine
        .initialize_directory_scope(
            &repo.repository_id,
            "",
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    engine
        .create_directory_branches(
            &repo.repository_id,
            "Folder",
            "draft",
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    let other = engine
        .create_repository(
            "Other",
            "P01",
            1,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    engine
        .create_directory_branches(
            &other.repository_id,
            "",
            "draft",
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    engine
        .select_directory_branches(
            &other.repository_id,
            "",
            "draft",
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    engine
        .validate_workspace_file_selection(&workspace.workspace_id, "Folder/Child/file.txt")
        .unwrap();
    for selection in ["draft", "main"] {
        engine
            .select_directory_branches(
                &repo.repository_id,
                "Folder",
                selection,
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
    }
    for path in ["folder", "FOLDER/CHILD/file.txt", "Folder/Child/new.txt"] {
        assert_eq!(
            engine
                .validate_workspace_file_selection(&workspace.workspace_id, path)
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::DirectoryBranchConflict
        );
    }
    engine
        .validate_workspace_file_selection(&workspace.workspace_id, "Folderish/file.txt")
        .unwrap();
}

#[test]
fn workspace_capture_matches_path_lookup_for_bootstrap_and_inherited_contexts() {
    let (temporary, mut engine, repo) = fixture();
    // A normal repository with no named compositions uses bootstrap contexts.
    assert!(engine.state().file_contexts.is_empty());
    for inherited in [false, true] {
        if inherited {
            engine
                .create_directory_branches(
                    &repo.repository_id,
                    "Folder",
                    "draft",
                    "admin",
                    storage_contract::OperationId::generate(),
                )
                .unwrap();
            engine
                .select_directory_branches(
                    &repo.repository_id,
                    "Folder",
                    "draft",
                    "admin",
                    storage_contract::OperationId::generate(),
                )
                .unwrap();
        }
        let head = &engine.state().branches[&repo.main_branch_id].head_commit_id;
        let expected = engine.state().commits[head]
            .entries
            .iter()
            .filter(|entry| entry.kind == NamespaceEntryKind::File)
            .map(|entry| {
                (
                    directory::context_key_from_key(&repo.repository_id, entry.lookup_key()),
                    directory::context_at_key(
                        engine.state(),
                        &repo.repository_id,
                        entry.lookup_key(),
                    )
                    .unwrap(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        assert_eq!(workspace.selected_file_contexts, expected);
        assert_eq!(
            workspace.selected_directory_branches,
            *engine.state().directory_selections
        );
        assert_eq!(
            workspace.directory_selection_generations,
            *engine.state().directory_selection_generations
        );
        assert_eq!(
            workspace.file_selection_generations,
            *engine.state().file_selection_generations
        );
        if inherited {
            assert!(
                workspace
                    .selected_file_contexts
                    .values()
                    .any(|context| context.inherited_from.is_some())
            );
        }
        let persisted = canonical_json(&workspace).unwrap();
        drop(engine);
        engine = Engine::open(&temporary.path().join("system")).unwrap();
        assert_eq!(
            canonical_json(&engine.workspace(&workspace.workspace_id).unwrap()).unwrap(),
            persisted
        );
    }
}

#[test]
fn workspace_namespace_plan_keeps_frozen_source_across_preparatory_commits() {
    for inherited in [false, true] {
        let (_temporary, mut engine, repo) = fixture();
        if inherited {
            engine
                .create_directory_branches(
                    &repo.repository_id,
                    "Folder",
                    "draft",
                    "admin",
                    storage_contract::OperationId::generate(),
                )
                .unwrap();
            engine
                .select_directory_branches(
                    &repo.repository_id,
                    "Folder",
                    "draft",
                    "admin",
                    storage_contract::OperationId::generate(),
                )
                .unwrap();
        }
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        for operation in [
            NamespaceOperation::parse(NamespaceOperationKind::Rename, "FOLDER", Some("Moved"))
                .unwrap(),
            NamespaceOperation::parse(NamespaceOperationKind::Delete, "Folderish/file.txt", None)
                .unwrap(),
        ] {
            engine
                .namespace_operation(&workspace.workspace_id, operation)
                .unwrap();
        }
        let before = canonical_json(engine.state()).unwrap();
        let source = &engine.state().commits[&workspace.base_head];
        let plan = engine
            .plan_workspace_commit_inner(
                CommitRequest {
                    workspace_id: workspace.workspace_id,
                    operation_id: storage_contract::OperationId::generate(),
                    principal: "admin".into(),
                    expected_head: workspace.base_head.clone(),
                    fencing_token: workspace.fencing_token,
                    message: "plan ordered namespace changes".into(),
                    request_digest: None,
                },
                None,
            )
            .unwrap();
        let MutationPlan::Prepared { next, result, .. } = plan.mutation else {
            panic!("new namespace operation must produce a candidate");
        };
        if !inherited {
            assert!(
                next.commits.len() > engine.state().commits.len() + 1,
                "directory preparation must insert commits before the final namespace commit"
            );
        }
        assert!(std::ptr::eq(&next.commits[&workspace.base_head], source));
        let committed = &next.commits[result.commit_id.as_ref().unwrap()];
        assert_eq!(committed.parent_commit_ids, [workspace.base_head]);
        assert_eq!(
            committed
                .entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            ["Folderish", "Moved", "Moved/Child", "Moved/Child/file.txt"]
        );
        let moved = find_unordered(&committed.entries, "Moved/Child/file.txt").unwrap();
        let original = find_unordered(&source.entries, "Folder/Child/file.txt").unwrap();
        if inherited {
            assert!(source.pending_file_versions.contains_key(&original.file_id));
        }
        assert_eq!(moved.entry_id, original.entry_id);
        assert_eq!(moved.file_id, original.file_id);
        assert_eq!(moved.version_id, original.version_id);
        assert_eq!(
            committed.pending_file_versions,
            source
                .pending_file_versions
                .iter()
                .filter(|(file, _)| **file == original.file_id)
                .map(|(file, version)| (file.clone(), version.clone()))
                .collect()
        );
        let deleted = &committed.namespace_deletions.as_ref().unwrap().paths;
        assert_eq!(deleted.len(), 1);
        assert_eq!(deleted[0].path, "Folderish/file.txt");
        assert_eq!(canonical_json(engine.state()).unwrap(), before);
        drop(next);
        assert_eq!(canonical_json(engine.state()).unwrap(), before);
    }
}
