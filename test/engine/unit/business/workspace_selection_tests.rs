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
            &storage_contract::RepositoryName::parse("Workspace selections").unwrap(),
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
fn generation_first_conflict(
    state: &State,
    workspace: &Workspace,
    path: &str,
    base_entries: &[Entry],
    current_entries: &[Entry],
) -> bool {
    let branch = &state.branches[&workspace.branch_id];
    let path = name_policy::path_key(path);
    state
        .directory_selection_generations
        .iter()
        .any(|(id, generation)| {
            let relevant = state.repositories.values().any(|repo| {
                repo.history_tree_id == branch.history_tree_id && repo.repository_id == *id
            }) || base_entries.iter().chain(current_entries).any(|entry| {
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
fn pending_alias_liveness_walks_missing_physical_hints_together() {
    use crate::namespace::NamespaceSource;

    let mut entries = vec![directory("visible")];
    let mut pending = BTreeSet::new();
    let mut locations = BTreeMap::new();
    for index in 0..384 {
        let entry = Entry {
            entry_id: new_id("entry"),
            path: name_policy::PortablePath::parse(&format!("visible/file-{index:04}.txt"))
                .unwrap(),
            kind: NamespaceEntryKind::File,
            file_id: new_id("file"),
            version_id: new_id("ver"),
            size: 1,
        };
        if [0, 129, 257].contains(&index) {
            pending.insert(entry.file_id.clone());
            locations.insert(entry.file_id.clone(), format!("owner/file-{index:04}.txt"));
            let mut alias = entry.clone();
            alias.relocate(
                name_policy::PortablePath::parse(&format!("visible/alias-{index:04}.txt")).unwrap(),
            );
            entries.push(alias);
        }
        entries.push(entry);
    }
    let absent = new_id("file");
    let candidates = pending
        .iter()
        .map(String::as_str)
        .chain(std::iter::once(absent.as_str()))
        .collect();
    let mut objects = crate::namespace::NamespaceObjects::new();
    let namespace = crate::namespace::build_from_entries(&entries, &mut objects).unwrap();
    let missing = crate::namespace::LoaderSource(
        |_: &storage_contract::NamespaceReference| -> Result<crate::namespace::NamespaceNode> {
            Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "fixture object missing",
            ))
        },
    );
    let draft = crate::namespace::DraftSource {
        fresh: &objects,
        base: &missing,
    };
    let reads = std::cell::RefCell::new(BTreeMap::<String, usize>::new());
    let counted =
        crate::namespace::LoaderSource(|reference: &storage_contract::NamespaceReference| {
            *reads
                .borrow_mut()
                .entry(reference.hash.clone())
                .or_default() += 1;
            draft.node(reference)
        });
    let live = live_pending_files(&namespace, candidates, &locations, &counted).unwrap();
    assert_eq!(live, pending);
    // Every missing physical path probes only the one-page root. The one
    // fallback walk reads the indexed visible directory exactly once, even
    // with several pending aliases and an identity that has really vanished.
    let observed = reads.borrow();
    assert_eq!(observed[&namespace.root.hash], locations.len() + 1);
    let descendant_reads = observed
        .iter()
        .filter(|(hash, _)| *hash != &namespace.root.hash)
        .map(|(_, count)| *count)
        .collect::<Vec<_>>();
    assert!(descendant_reads.len() >= 4);
    assert!(descendant_reads.iter().all(|count| *count == 1));
    let full_walk_reads = descendant_reads.len();
    drop(observed);

    reads.borrow_mut().clear();
    let candidates = pending.iter().map(String::as_str).collect();
    assert_eq!(
        live_pending_files(&namespace, candidates, &locations, &counted).unwrap(),
        pending
    );
    // All live aliases are at the front: do not read unrelated later pages
    // once every candidate has been found.
    assert!(reads.borrow().len() - 1 < full_walk_reads);
}

#[test]
fn pending_liveness_preserves_point_and_fallback_read_errors() {
    let mut objects = crate::namespace::NamespaceObjects::new();
    let namespace = crate::namespace::build_from_entries(&[], &mut objects).unwrap();
    let file = new_id("file");
    let failed = crate::namespace::LoaderSource(
        |_: &storage_contract::NamespaceReference| -> Result<crate::namespace::NamespaceNode> {
            Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "pending source failed",
            ))
        },
    );
    for locations in [
        BTreeMap::from([(file.clone(), "missing.txt".into())]),
        BTreeMap::new(),
    ] {
        let error = live_pending_files(
            &namespace,
            BTreeSet::from([file.as_str()]),
            &locations,
            &failed,
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::ObjectCorrupt);
        assert!(error.to_string().contains("pending source failed"));
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
    // Path-copy both synthetic heads; their fresh pages resolve before the
    // published store throughout this test.
    let mut built = crate::namespace::NamespaceObjects::new();
    let (base_ns, current_ns) = engine
        .with_namespace_source(|base| {
            let mut editor = crate::namespace::NamespaceEditor::open(&current.namespace, base)?;
            editor.upsert_directory(&old)?;
            let base_ns = editor.seal(&mut built)?;
            let mut editor = crate::namespace::NamespaceEditor::open(&current.namespace, base)?;
            editor.upsert_directory(&future)?;
            let current_ns = editor.seal(&mut built)?;
            Ok((base_ns, current_ns))
        })
        .unwrap();
    state
        .commits
        .get_mut(&workspace.base_head)
        .unwrap()
        .namespace = base_ns;
    current.namespace = current_ns;
    state
        .branches
        .get_mut(&workspace.branch_id)
        .unwrap()
        .head_commit_id = current.commit_id.clone();
    state.commits.insert(current.commit_id.clone(), current);
    let (base_entries, current_entries) = engine
        .with_namespace_source(|base| {
            let source = crate::namespace::DraftSource {
                fresh: &built,
                base,
            };
            let base_entries = crate::namespace::NamespaceView::new(
                &state.commits[&workspace.base_head].namespace,
                &source,
            )
            .collect_entries()?;
            let branch = &state.branches[&workspace.branch_id];
            let current_entries = crate::namespace::NamespaceView::new(
                &state.commits[&branch.head_commit_id].namespace,
                &source,
            )
            .collect_entries()?;
            Ok((base_entries, current_entries))
        })
        .unwrap();
    let mut ids = base_entries
        .iter()
        .filter(|entry| entry.kind == NamespaceEntryKind::Directory)
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
                    let expected = generation_first_conflict(
                        &state,
                        &workspace,
                        path,
                        &base_entries,
                        &current_entries,
                    );
                    let actual = engine
                        .with_namespace_source(|base| {
                            let source = crate::namespace::DraftSource {
                                fresh: &built,
                                base,
                            };
                            Ok(validate_directory_selection_key(
                                &state,
                                &workspace,
                                &name_policy::path_key(path),
                                &source,
                            ))
                        })
                        .unwrap();
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
        let error = engine
            .with_namespace_source(|source| {
                Ok(validate_directory_selection_key(
                    &state, &workspace, "new.txt", source,
                ))
            })
            .unwrap()
            .unwrap_err();
        assert!(error.to_string().contains(expected), "{missing}: {error}");
    }
}

#[test]
fn directory_round_trip_fences_ancestors_but_not_siblings_or_other_repositories() {
    let (_temporary, mut engine, repo) = fixture();
    engine
        .initialize_directory_scope(
            &repo.repository_id,
            None,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    engine
        .create_directory_branches(
            &repo.repository_id,
            Some(&name_policy::LookupPath::parse("Folder").unwrap()),
            &storage_contract::BranchName::parse("draft").unwrap(),
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    let other = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Other").unwrap(),
            "P01",
            1,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    engine
        .create_directory_branches(
            &other.repository_id,
            None,
            &storage_contract::BranchName::parse("draft").unwrap(),
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    engine
        .select_directory_branches(
            &other.repository_id,
            None,
            &storage_contract::BranchName::parse("draft").unwrap(),
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    engine
        .validate_workspace_file_selection(
            &workspace.workspace_id,
            &name_policy::PortablePath::parse("Folder/Child/file.txt").unwrap(),
        )
        .unwrap();
    for selection in ["draft", "main"] {
        engine
            .select_directory_branches(
                &repo.repository_id,
                Some(&name_policy::LookupPath::parse("Folder").unwrap()),
                &storage_contract::BranchName::parse(selection).unwrap(),
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
    }
    for path in ["folder", "FOLDER/CHILD/file.txt", "Folder/Child/new.txt"] {
        assert_eq!(
            engine
                .validate_workspace_file_selection(
                    &workspace.workspace_id,
                    &name_policy::PortablePath::parse(path).unwrap()
                )
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::DirectoryBranchConflict
        );
    }
    engine
        .validate_workspace_file_selection(
            &workspace.workspace_id,
            &name_policy::PortablePath::parse("Folderish/file.txt").unwrap(),
        )
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
                    Some(&name_policy::LookupPath::parse("Folder").unwrap()),
                    &storage_contract::BranchName::parse("draft").unwrap(),
                    "admin",
                    storage_contract::OperationId::generate(),
                )
                .unwrap();
            engine
                .select_directory_branches(
                    &repo.repository_id,
                    Some(&name_policy::LookupPath::parse("Folder").unwrap()),
                    &storage_contract::BranchName::parse("draft").unwrap(),
                    "admin",
                    storage_contract::OperationId::generate(),
                )
                .unwrap();
        }
        let head = &engine.state().branches[&repo.main_branch_id].head_commit_id;
        let expected = engine
            .with_namespace_source(|source| {
                let entries = crate::namespace::NamespaceView::new(
                    &engine.state().commits[head].namespace,
                    source,
                )
                .collect_entries()?;
                let mut map = BTreeMap::new();
                for entry in entries
                    .iter()
                    .filter(|entry| entry.kind == NamespaceEntryKind::File)
                {
                    map.insert(
                        directory::context_key_from_key(&repo.repository_id, entry.lookup_key()),
                        directory::context_at_key(
                            engine.state(),
                            &repo.repository_id,
                            entry.lookup_key(),
                            source,
                        )?
                        .unwrap(),
                    );
                }
                Ok(map)
            })
            .unwrap();
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
                    Some(&name_policy::LookupPath::parse("Folder").unwrap()),
                    &storage_contract::BranchName::parse("draft").unwrap(),
                    "admin",
                    storage_contract::OperationId::generate(),
                )
                .unwrap();
            engine
                .select_directory_branches(
                    &repo.repository_id,
                    Some(&name_policy::LookupPath::parse("Folder").unwrap()),
                    &storage_contract::BranchName::parse("draft").unwrap(),
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
        let MutationPlan::Prepared {
            next,
            result,
            namespace,
            ..
        } = plan.mutation
        else {
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
        // The plan's fresh namespace objects resolve before the published
        // store, exactly as publication installs them.
        let committed_entries = engine
            .with_namespace_source(|base| {
                let planned = crate::namespace::PreparedSource::new(&namespace, base)?;
                crate::namespace::NamespaceView::new(&committed.namespace, &planned)
                    .collect_entries()
            })
            .unwrap();
        let source_entries = engine.namespace_entries(&source.namespace).unwrap();
        assert_eq!(
            committed_entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            ["Folderish", "Moved", "Moved/Child", "Moved/Child/file.txt"]
        );
        let moved = find_unordered(&committed_entries, "Moved/Child/file.txt").unwrap();
        let original = find_unordered(&source_entries, "Folder/Child/file.txt").unwrap();
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

#[test]
fn workspace_namespace_deletions_keep_rename_slots_until_commit() {
    let (_temporary, mut engine, repo) = fixture();
    let upload = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    for path in ["p/a", "p/z"] {
        engine
            .stage_bytes(&upload.workspace_id, path, b"original")
            .unwrap();
    }
    engine
        .commit_workspace(CommitRequest {
            workspace_id: upload.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "admin".into(),
            expected_head: upload.base_head,
            fencing_token: upload.fencing_token,
            message: "deletion ordering fixture".into(),
            request_digest: None,
        })
        .unwrap();

    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    for operation in [
        NamespaceOperation::parse(NamespaceOperationKind::Rename, "p/z", Some("p/0")).unwrap(),
        NamespaceOperation::parse(NamespaceOperationKind::DeleteTree, "p", None).unwrap(),
    ] {
        engine
            .namespace_operation(&workspace.workspace_id, operation)
            .unwrap();
    }
    let plan = engine
        .plan_workspace_commit_inner(
            CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: storage_contract::OperationId::generate(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "rename then delete tree".into(),
                request_digest: None,
            },
            None,
        )
        .unwrap();
    let MutationPlan::Prepared { next, result, .. } = plan.mutation else {
        panic!("namespace deletion must produce a candidate");
    };
    let committed = &next.commits[result.commit_id.as_ref().unwrap()];
    let deleted = &committed.namespace_deletions.as_ref().unwrap().paths;
    assert_eq!(
        deleted
            .iter()
            .map(|fact| (fact.path.as_str(), fact.kind))
            .collect::<Vec<_>>(),
        [
            ("p", NamespaceEntryKind::Directory),
            ("p/a", NamespaceEntryKind::File),
            ("p/0", NamespaceEntryKind::File),
        ]
    );
}

#[test]
fn workspace_first_writes_clear_only_written_pending_file_identities() {
    let (_temporary, mut engine, repo) = fixture();
    let source = engine.state().branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let branch = engine
        .create_branch(
            &source,
            &storage_contract::BranchName::parse("pending").unwrap(),
            "admin",
            OperationId::generate(),
        )
        .unwrap();
    assert!(repository::main_for_branch(engine.state(), &branch.branch_id).is_none());
    let workspace = engine.create_workspace(&branch.branch_id, "admin").unwrap();
    let source = &engine.state().commits[&workspace.base_head];
    let source_entries = engine.namespace_entries(&source.namespace).unwrap();
    let written = find_unordered(&source_entries, "Folder/Child/file.txt")
        .unwrap()
        .clone();
    let retained = find_unordered(&source_entries, "Folderish/file.txt")
        .unwrap()
        .clone();
    assert_eq!(source.pending_file_versions.len(), 2);
    // Even identical bytes must materialize inherited first-write intent.
    engine
        .stage_bytes(&workspace.workspace_id, written.path(), b"original")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "New.txt", b"new")
        .unwrap();
    let before = canonical_json(engine.state()).unwrap();
    let plan = engine
        .plan_workspace_commit_inner(
            CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: OperationId::generate(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "first writes".into(),
                request_digest: None,
            },
            None,
        )
        .unwrap();
    let MutationPlan::Prepared {
        next,
        result,
        namespace,
        ..
    } = plan.mutation
    else {
        panic!("first writes must produce a candidate");
    };
    let commit = &next.commits[result.commit_id.as_ref().unwrap()];
    let expected = BTreeMap::from([(retained.file_id.clone(), retained.version_id.clone())]);
    assert_eq!(commit.pending_file_versions, expected);
    assert_eq!(
        next.branches[&branch.branch_id].pending_file_versions,
        expected
    );
    let commit_entries = engine
        .with_namespace_source(|base| {
            let planned = crate::namespace::PreparedSource::new(&namespace, base)?;
            crate::namespace::NamespaceView::new(&commit.namespace, &planned).collect_entries()
        })
        .unwrap();
    let materialized = find_unordered(&commit_entries, written.path()).unwrap();
    assert_eq!(materialized.file_id, written.file_id);
    assert_ne!(materialized.version_id, written.version_id);
    assert_eq!(
        find_unordered(&commit_entries, retained.path())
            .unwrap()
            .version_id,
        retained.version_id
    );
    assert!(find_unordered(&commit_entries, "New.txt").is_some());
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
}

#[path = "workspace_namespace_path_budget_tests.rs"]
mod namespace_path_budget_tests;
