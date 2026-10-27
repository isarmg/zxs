//! Broken persisted references are recoverable errors, not mutex-poisoning panics.
use super::*;

fn fixture() -> (tempfile::TempDir, Engine, Repository, Entry, String) {
    let temporary = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temporary.path().join("system"),
        Config::development([
            temporary.path().join("primary"),
            temporary.path().join("backup"),
            temporary.path().join("tertiary"),
        ]),
    )
    .unwrap();
    let repo = engine
        .create_disk_root("P01", "admin", storage_contract::OperationId::generate())
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "folder/file.txt", b"data")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "fixture".into(),
            request_digest: None,
        })
        .unwrap();
    let entry = engine.state.commits[&engine.state.branches[&repo.main_branch_id].head_commit_id]
        .entries
        .iter()
        .find(|entry| entry.kind == NamespaceEntryKind::File)
        .unwrap()
        .clone();
    let branch = engine
        .create_file_branch(
            &entry.file_id,
            &entry.version_id,
            &storage_contract::BranchName::parse("draft").unwrap(),
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap()
        .result["branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    (temporary, engine, repo, entry, branch)
}

#[test]
fn file_selection_exposes_broken_admitted_heads_without_publication() {
    for missing in ["main_branch", "main_head", "file_head", "history"] {
        for scoped in [false, true] {
            let (_temporary, mut engine, repo, entry, branch) = fixture();
            match missing {
                "main_branch" => {
                    std::sync::Arc::make_mut(&mut engine.state)
                        .branches
                        .remove(&repo.main_branch_id);
                }
                "main_head" => {
                    let head = engine.state.branches[&repo.main_branch_id]
                        .head_commit_id
                        .clone();
                    std::sync::Arc::make_mut(&mut engine.state)
                        .commits
                        .remove(&head);
                }
                "file_head" => {
                    std::sync::Arc::make_mut(&mut engine.state)
                        .branches
                        .get_mut(&branch)
                        .unwrap()
                        .head_commit_id = "missing".into();
                }
                "history" => {
                    std::sync::Arc::make_mut(&mut engine.state)
                        .histories
                        .remove(&repo.history_tree_id);
                }
                _ => unreachable!(),
            }
            let before = canonical_json(engine.state()).unwrap();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if scoped {
                    engine.select_file_branch_at(
                        &repo.repository_id,
                        &entry.path,
                        &entry.file_id,
                        &branch,
                        "admin",
                        storage_contract::OperationId::generate(),
                    )
                } else {
                    engine.select_file_branch(
                        &entry.file_id,
                        &branch,
                        "admin",
                        storage_contract::OperationId::generate(),
                    )
                }
            }));
            if matches!(missing, "main_branch" | "main_head") {
                assert!(
                    result.is_err(),
                    "{missing}/{scoped}: admitted root must exist"
                );
                assert_eq!(canonical_json(engine.state()).unwrap(), before);
                continue;
            }
            let error = result.unwrap().unwrap_err();
            assert!(
                matches!(
                    error.code(),
                    storage_contract::ErrorCode::StateInvalid
                        | storage_contract::ErrorCode::BranchNotFound
                        | storage_contract::ErrorCode::HistoryNotFound
                ),
                "{missing}/{scoped}: {error}"
            );
            assert_eq!(
                canonical_json(engine.state()).unwrap(),
                before,
                "{missing}/{scoped}"
            );
        }
    }
}

#[test]
fn workspace_missing_head_returns_error_without_creating_workspace() {
    let (_temporary, mut engine, repo, _, _) = fixture();
    let head = engine.state.branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    std::sync::Arc::make_mut(&mut engine.state)
        .commits
        .remove(&head);
    let shared = std::sync::Mutex::new(engine);
    let mut engine = shared.lock().unwrap();
    let before = canonical_json(engine.state()).unwrap();
    let workspaces = engine.workspaces.len();
    let error = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap_err();
    assert_eq!(error.code(), storage_contract::ErrorCode::StateInvalid);
    assert_eq!(engine.workspaces.len(), workspaces);
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    drop(engine);
    assert!(
        shared.lock().is_ok(),
        "recoverable corruption must not poison the shared engine"
    );
}

#[test]
fn batch_rejects_missing_references_without_publication() {
    for missing in [
        "main_branch",
        "main_head",
        "scope_commit",
        "owner",
        "path_context",
    ] {
        let (_temporary, mut engine, repo, entry, _) = fixture();
        let result = engine
            .create_directory_branches(
                &repo.repository_id,
                Some(&name_policy::LookupPath::parse("folder").unwrap()),
                &storage_contract::BranchName::parse("draft").unwrap(),
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
        let branch_id = result.result["directory_branch_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let branch = engine.state.directory_branches[&branch_id].clone();
        let request = DirectoryNamespaceBatch {
            kind: storage_contract::DirectoryBatchKind::Rename,
            items: vec![DirectoryNamespaceItem {
                source: DirectoryNamespaceSource {
                    directory_branch_id: branch_id.clone(),
                    expected_tree_id: branch.head_tree_id.clone(),
                    path: entry.path.into_string(),
                    entry_id: entry.entry_id,
                },
                target: Some(DirectoryNamespaceTarget {
                    directory_branch_id: branch_id.clone(),
                    expected_tree_id: branch.head_tree_id,
                    path: "folder/renamed.txt".into(),
                }),
            }],
        };
        match missing {
            "main_branch" => {
                std::sync::Arc::make_mut(&mut engine.state)
                    .branches
                    .remove(&repo.main_branch_id);
            }
            "main_head" => {
                let head = engine.state.branches[&repo.main_branch_id]
                    .head_commit_id
                    .clone();
                std::sync::Arc::make_mut(&mut engine.state)
                    .commits
                    .remove(&head);
            }
            "scope_commit" => {
                std::sync::Arc::make_mut(&mut engine.state)
                    .directory_branches
                    .get_mut(&branch_id)
                    .unwrap()
                    .commit_id = "missing".into();
            }
            "path_context" => {
                std::sync::Arc::make_mut(&mut engine.state)
                    .directory_branches
                    .get_mut(&branch_id)
                    .unwrap()
                    .file_contexts
                    .clear();
            }
            "owner" => {
                let branch = std::sync::Arc::make_mut(&mut engine.state)
                    .directory_branches
                    .get_mut(&branch_id)
                    .unwrap();
                branch.file_contexts.clear();
                branch.file_branches.clear();
            }
            _ => unreachable!(),
        }
        let before = canonical_json(engine.state()).unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            engine.directory_namespace_batch(
                &repo.repository_id,
                &request,
                "admin",
                storage_contract::OperationId::generate(),
            )
        }));
        if matches!(missing, "main_branch" | "main_head") {
            assert!(result.is_err(), "{missing}: admitted root must exist");
            assert_eq!(canonical_json(engine.state()).unwrap(), before);
            continue;
        }
        let error = result.unwrap().unwrap_err();
        assert_eq!(
            error.code(),
            storage_contract::ErrorCode::StateInvalid,
            "{missing}"
        );
        assert_eq!(canonical_json(engine.state()).unwrap(), before, "{missing}");
    }
}

#[test]
fn file_node_publication_exposes_missing_admitted_main_without_leaking_new_nodes() {
    for missing in ["main_branch", "main_head"] {
        let (_temporary, mut engine, repo, entry, branch) = fixture();
        match missing {
            "main_branch" => {
                std::sync::Arc::make_mut(&mut engine.state)
                    .branches
                    .remove(&repo.main_branch_id);
            }
            "main_head" => {
                let head = engine.state.branches[&repo.main_branch_id]
                    .head_commit_id
                    .clone();
                std::sync::Arc::make_mut(&mut engine.state)
                    .commits
                    .remove(&head);
            }
            _ => unreachable!(),
        }
        let before = canonical_json(engine.state()).unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            engine.update_file_node_tags(crate::FileNodeTagsUpdateRequest {
                file_id: &entry.file_id,
                targets: &[FileTagTarget {
                    branch_id: branch,
                    source_node_id: entry.version_id.clone(),
                    expected_node_id: entry.version_id,
                }],
                add: &["tag".into()],
                remove: &[],
                location: None,
                principal: "admin",
                operation_id: storage_contract::OperationId::generate(),
            })
        }));
        assert!(result.is_err(), "{missing}: admitted root must exist");
        assert_eq!(canonical_json(engine.state()).unwrap(), before, "{missing}");
    }
}

#[test]
fn file_node_publication_requires_frozen_path_contexts_without_publication() {
    for missing in ["frozen_tree", "pending_projection"] {
        let (_temporary, mut engine, repo, entry, _) = fixture();
        let file_branch = engine
            .file_context(&repo.repository_id, &entry.path)
            .unwrap()
            .branch_id
            .clone();
        let result = engine
            .create_directory_branches(
                &repo.repository_id,
                Some(&name_policy::LookupPath::parse("folder").unwrap()),
                &storage_contract::BranchName::parse("pending").unwrap(),
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
        let directory = result.result["directory_branch_id"].as_str().unwrap();
        let pending = engine.state.directory_branches[directory].clone();
        assert!(!pending.materialized);
        let state = std::sync::Arc::make_mut(&mut engine.state);
        let contexts = match missing {
            "frozen_tree" => {
                &mut state
                    .directory_trees
                    .get_mut(&pending.head_tree_id)
                    .unwrap()
                    .file_contexts
            }
            "pending_projection" => {
                &mut state
                    .directory_branches
                    .get_mut(directory)
                    .unwrap()
                    .file_contexts
            }
            _ => unreachable!(),
        };
        assert!(contexts.remove(entry.lookup_key()).is_some());
        let before = canonical_json(engine.state()).unwrap();
        let sequence = engine.state.sequence;
        let slot = engine.next_slot;
        let operation = storage_contract::OperationId::generate();
        let error = engine
            .update_file_node_tags(crate::FileNodeTagsUpdateRequest {
                file_id: &entry.file_id,
                targets: &[FileTagTarget {
                    branch_id: file_branch,
                    source_node_id: entry.version_id.clone(),
                    expected_node_id: entry.version_id,
                }],
                add: &["tag".into()],
                remove: &[],
                location: None,
                principal: "admin",
                operation_id: operation,
            })
            .unwrap_err();
        assert_eq!(
            error.code(),
            storage_contract::ErrorCode::StateInvalid,
            "{missing}"
        );
        assert_eq!(canonical_json(engine.state()).unwrap(), before, "{missing}");
        assert_eq!(engine.state.sequence, sequence, "{missing}");
        assert_eq!(engine.next_slot, slot, "{missing}");
        assert!(
            !engine.state.operations.contains_key(&operation),
            "{missing}"
        );
    }
}

#[test]
fn missing_file_nodes_and_bodies_return_errors_without_publication() {
    for missing in ["node", "body"] {
        let (_temporary, mut engine, _, entry, branch) = fixture();
        let head = if missing == "node" {
            std::sync::Arc::make_mut(&mut engine.state)
                .branches
                .get_mut(&branch)
                .unwrap()
                .file_node_id = Some("missing".into());
            "missing".to_owned()
        } else {
            let node = new_id("fnode");
            let history_tree_id = engine.state.branches[&branch].history_tree_id.clone();
            let sequence = engine.state.sequence;
            std::sync::Arc::make_mut(&mut engine.state)
                .file_path_nodes
                .insert(
                    node.clone(),
                    FilePathNode {
                        node_id: node.clone(),
                        history_tree_id,
                        file_id: entry.file_id.clone(),
                        version_id: "missing".into(),
                        parent_node_ids: vec![entry.version_id],
                        branch_id: branch.clone(),
                        path: entry.path.as_str().to_owned(),
                        previous_path: entry.path.into_string(),
                        sequence,
                        created_at: now().unwrap(),
                        operation_id: storage_contract::OperationId::generate(),
                        author: "admin".into(),
                    },
                );
            std::sync::Arc::make_mut(&mut engine.state)
                .branches
                .get_mut(&branch)
                .unwrap()
                .file_node_id = Some(node.clone());
            node
        };
        if missing == "body" {
            assert!(
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    engine.file_node_version(&entry.file_id, &head)
                }))
                .is_err()
            );
            continue;
        }
        let before = canonical_json(engine.state()).unwrap();
        let error = engine
            .update_file_node_tags(crate::FileNodeTagsUpdateRequest {
                file_id: &entry.file_id,
                targets: &[FileTagTarget {
                    branch_id: branch,
                    source_node_id: head.clone(),
                    expected_node_id: head.clone(),
                }],
                add: &["tag".into()],
                remove: &[],
                location: None,
                principal: "admin",
                operation_id: storage_contract::OperationId::generate(),
            })
            .unwrap_err();
        assert!(
            matches!(
                error.code(),
                storage_contract::ErrorCode::StateInvalid
                    | storage_contract::ErrorCode::FileNodeNotFound
            ),
            "{missing}: {error}"
        );
        assert_eq!(canonical_json(engine.state()).unwrap(), before, "{missing}");
        let error = engine
            .create_file_branch_from_node(
                &entry.file_id,
                "missing",
                &head,
                &storage_contract::BranchName::parse("broken source").unwrap(),
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap_err();
        assert_eq!(
            error.code(),
            if missing == "body" {
                storage_contract::ErrorCode::StateInvalid
            } else {
                storage_contract::ErrorCode::FileNodeNotFound
            }
        );
        assert_eq!(
            canonical_json(engine.state()).unwrap(),
            before,
            "source/{missing}"
        );
    }
}

#[test]
fn exhausted_branch_generations_reject_metadata_mutations_atomically() {
    for action in [
        "select",
        "select_at",
        "namespace",
        "tags_file",
        "tags_main",
        "archive",
        "merge_survivor",
        "merge_alias",
    ] {
        let (_temporary, mut engine, repo, entry, draft) = fixture();
        let owner = engine.state.file_selections[&entry.file_id].clone();
        let saturated = match action {
            "tags_file" | "archive" | "merge_alias" => &draft,
            "merge_survivor" => &owner,
            _ => &repo.main_branch_id,
        };
        std::sync::Arc::make_mut(&mut engine.state)
            .branches
            .get_mut(saturated)
            .unwrap()
            .generation = u64::MAX;
        let before = canonical_json(engine.state()).unwrap();
        let operation = storage_contract::OperationId::generate();
        let result = match action {
            "select" => engine.select_file_branch(&entry.file_id, &draft, "admin", operation),
            "select_at" => engine.select_file_branch_at(
                &repo.repository_id,
                &entry.path,
                &entry.file_id,
                &draft,
                "admin",
                operation,
            ),
            "namespace" => engine.file_namespace(
                &repo.repository_id,
                NamespaceOperation::parse(
                    NamespaceOperationKind::Rename,
                    &(entry.path.clone()),
                    Some("folder/renamed.txt"),
                )
                .unwrap(),
                "admin",
                operation,
            ),
            "tags_file" | "tags_main" => {
                engine.update_file_node_tags(crate::FileNodeTagsUpdateRequest {
                    file_id: &entry.file_id,
                    targets: &[FileTagTarget {
                        branch_id: if action == "tags_file" {
                            draft.clone()
                        } else {
                            owner.clone()
                        },
                        source_node_id: entry.version_id.clone(),
                        expected_node_id: entry.version_id.clone(),
                    }],
                    add: &["tag".into()],
                    remove: &[],
                    location: None,
                    principal: "admin",
                    operation_id: operation,
                })
            }
            "archive" => engine.archive_branch(&draft, "admin", operation),
            "merge_survivor" | "merge_alias" => {
                engine.merge_file_branches(crate::FileBranchMergeRequest {
                    file_id: &entry.file_id,
                    inputs: &[
                        FileMergeInput {
                            branch_id: owner.clone(),
                            expected_node_id: entry.version_id.clone(),
                        },
                        FileMergeInput {
                            branch_id: draft.clone(),
                            expected_node_id: entry.version_id.clone(),
                        },
                    ],
                    chosen_node_id: &entry.version_id,
                    surviving_branch_id: &owner,
                    location: None,
                    principal: "admin",
                    operation_id: operation,
                })
            }
            _ => unreachable!(),
        };
        assert_eq!(
            result.unwrap_err().code(),
            storage_contract::ErrorCode::ResourceLimit,
            "{action}"
        );
        assert_eq!(canonical_json(engine.state()).unwrap(), before, "{action}");
    }
}

#[test]
fn exhausted_workspace_generations_reject_candidate_without_publication() {
    for saturated_owner in [false, true] {
        let (_temporary, mut engine, repo, entry, _) = fixture();
        let owner = engine.state.file_selections[&entry.file_id].clone();
        std::sync::Arc::make_mut(&mut engine.state)
            .branches
            .get_mut(if saturated_owner {
                &owner
            } else {
                &repo.main_branch_id
            })
            .unwrap()
            .generation = u64::MAX;
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, &entry.path, b"changed body")
            .unwrap();
        let before = canonical_json(engine.state()).unwrap();
        let error = engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: storage_contract::OperationId::generate(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "must not publish".into(),
                request_digest: None,
            })
            .unwrap_err();
        assert_eq!(
            error.code(),
            storage_contract::ErrorCode::ResourceLimit,
            "owner={saturated_owner}"
        );
        assert_eq!(
            canonical_json(engine.state()).unwrap(),
            before,
            "owner={saturated_owner}"
        );
    }
}

#[test]
fn exhausted_main_generation_rejects_active_namespace_batch() {
    let (_temporary, mut engine, repo, entry, _) = fixture();
    let directory = engine
        .state
        .directory_branches
        .values()
        .find(|branch| {
            branch.repository_id == repo.repository_id
                && branch.path == "folder"
                && engine.state.directory_selections.get(&branch.directory_id)
                    == Some(&branch.directory_branch_id)
        })
        .unwrap()
        .clone();
    let request = DirectoryNamespaceBatch {
        kind: storage_contract::DirectoryBatchKind::Rename,
        items: vec![DirectoryNamespaceItem {
            source: DirectoryNamespaceSource {
                directory_branch_id: directory.directory_branch_id.clone(),
                expected_tree_id: directory.head_tree_id.clone(),
                path: entry.path.into_string(),
                entry_id: entry.entry_id,
            },
            target: Some(DirectoryNamespaceTarget {
                directory_branch_id: directory.directory_branch_id,
                expected_tree_id: directory.head_tree_id,
                path: "folder/renamed.txt".into(),
            }),
        }],
    };
    std::sync::Arc::make_mut(&mut engine.state)
        .branches
        .get_mut(&repo.main_branch_id)
        .unwrap()
        .generation = u64::MAX;
    let before = canonical_json(engine.state()).unwrap();
    let error = engine
        .directory_namespace_batch(
            &repo.repository_id,
            &request,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap_err();
    assert_eq!(error.code(), storage_contract::ErrorCode::ResourceLimit);
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
}

#[test]
fn frozen_directory_compositions_reject_missing_path_contexts() {
    for action in ["create", "select"] {
        let (_temporary, mut engine, repo, _, _) = fixture();
        let branch = engine
            .create_directory_branches(
                &repo.repository_id,
                Some(&name_policy::LookupPath::parse("folder").unwrap()),
                &storage_contract::BranchName::parse("draft").unwrap(),
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
        let id = branch.result["directory_branch_id"].as_str().unwrap();
        let head = engine.state.directory_branches[id].head_tree_id.clone();
        if action == "create" {
            std::sync::Arc::make_mut(&mut engine.state)
                .directory_trees
                .get_mut(&head)
                .unwrap()
                .file_contexts
                .clear();
        } else {
            std::sync::Arc::make_mut(&mut engine.state)
                .directory_branches
                .get_mut(id)
                .unwrap()
                .file_contexts
                .clear();
        }
        let before = canonical_json(engine.state()).unwrap();
        let error = if action == "create" {
            engine
                .create_directory_branches(
                    &repo.repository_id,
                    Some(&name_policy::LookupPath::parse("folder").unwrap()),
                    &storage_contract::BranchName::parse("second").unwrap(),
                    "admin",
                    storage_contract::OperationId::generate(),
                )
                .unwrap_err()
        } else {
            engine
                .select_directory_branches(
                    &repo.repository_id,
                    Some(&name_policy::LookupPath::parse("folder").unwrap()),
                    &storage_contract::BranchName::parse("draft").unwrap(),
                    "admin",
                    storage_contract::OperationId::generate(),
                )
                .unwrap_err()
        };
        assert_eq!(
            error.code(),
            storage_contract::ErrorCode::StateInvalid,
            "{action}"
        );
        assert_eq!(canonical_json(engine.state()).unwrap(), before, "{action}");
    }
}
