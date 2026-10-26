use super::*;
use std::{io::Read, os::unix::fs::PermissionsExt};

fn fixture() -> (tempfile::TempDir, Engine, Repository) {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([
        temp.path().join("p"),
        temp.path().join("b"),
        temp.path().join("c"),
    ]);
    let mut engine = Engine::initialize(&temp.path().join("state"), config).unwrap();
    let repo = engine
        .create_repository("项目", "P01", 3, "alice", &new_id("op"))
        .unwrap();
    (temp, engine, repo)
}
fn save(engine: &mut Engine, branch: &str, bytes: &[u8]) -> String {
    let workspace = engine.create_workspace(branch, "alice").unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "资料/文件.txt", bytes)
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: new_id("op"),
            principal: "alice".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "save".into(),
            request_digest: None,
        })
        .unwrap()
        .commit_id
        .unwrap()
}
fn assert_graph_closed(engine: &Engine, history: &str) {
    let graph = engine.graph(history).unwrap();
    let commits = graph
        .commits
        .iter()
        .map(|c| c.commit_id.as_str())
        .collect::<BTreeSet<_>>();
    let branches = graph
        .branches
        .iter()
        .map(|b| b.branch_id.as_str())
        .collect::<BTreeSet<_>>();
    for commit in &graph.commits {
        for parent in &commit.parent_commit_ids {
            assert!(commits.contains(parent.as_str()));
        }
    }
    for branch in &graph.branches {
        assert!(commits.contains(branch.head_commit_id.as_str()));
        assert!(commits.contains(branch.fork_commit_id.as_str()));
    }
    for link in &graph.merge_links {
        assert!(commits.contains(link.source_commit_id.as_str()));
        assert!(commits.contains(link.target_commit_id.as_str()));
        assert!(branches.contains(link.source_branch_id.as_str()));
        assert!(branches.contains(link.target_branch_id.as_str()));
    }
    for boundary in &graph.boundary_marks {
        assert!(commits.contains(boundary.local_commit_id.as_str()));
        if let Some(branch) = &boundary.local_branch_id {
            assert!(branches.contains(branch.as_str()));
        }
        assert!(!boundary.external_commit_id.is_empty());
    }
}

#[test]
fn split_closes_source_policy_scopes_without_dropping_valid_empty_rules() {
    let (temp, mut engine, repo) = fixture();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let configure = |engine: &mut Engine, path: &str, copies: u32| {
        engine
            .configure_backups(BackupConfigurationRequest {
                repository_id: &repo.repository_id,
                path,
                target_copies: copies,
                backup_prefixes: &if copies == 1 {
                    BTreeMap::new()
                } else {
                    BTreeMap::from([("B01".into(), "backup".into())])
                },
                expected_generation: 0,
                principal: "alice",
                operation_id: &new_id("op"),
            })
            .unwrap()
            .result["policy"]["policy_id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let mkdir = |engine: &mut Engine, path: &str| {
        engine
            .file_namespace(
                &repo.repository_id,
                NamespaceOperation::parse(
                    storage_contract::NamespaceOperationKind::Mkdir,
                    path,
                    None,
                )
                .unwrap(),
                "alice",
                &new_id("op"),
            )
            .unwrap();
    };
    mkdir(&mut engine, "prefix-empty");
    let valid_empty = configure(&mut engine, "prefix-empty", 1);
    let cut = save(&mut engine, &repo.main_branch_id, b"split source scope");
    let file = engine
        .list_tree(&repo.main_branch_id)
        .unwrap()
        .into_iter()
        .find(|entry| entry.kind == NamespaceEntryKind::File)
        .unwrap();
    let bound = configure(&mut engine, "资料", 2);
    mkdir(&mut engine, "moved-empty");
    let moved_empty = configure(&mut engine, "moved-empty", 1);
    let plan = engine
        .create_split_plan(&repo.history_tree_id, &cut, "alice")
        .unwrap();
    let result = engine
        .commit_plan(&plan.plan_id, "alice", &new_id("op"))
        .unwrap();
    let destination = result.history_tree_id.unwrap();
    let source_policy = &engine.state.protection_policies[&bound];
    assert!(!source_policy.inheritable);
    assert_eq!(source_policy.scope_directory_id, None);
    assert_eq!(source_policy.target_copies, 2);
    assert_eq!(engine.state.file_backup_policies[&file.file_id], bound);
    assert!(engine.state.protection_policies[&valid_empty].inheritable);
    assert!(
        engine.state.protection_policies[&valid_empty]
            .scope_directory_id
            .is_some()
    );
    assert!(!engine.state.protection_policies.contains_key(&moved_empty));
    let copied = engine
        .state
        .repositories
        .values()
        .find(|repository| repository.history_tree_id == destination)
        .unwrap();
    for (path, copies) in [("资料", 2), ("prefix-empty", 1), ("moved-empty", 1)] {
        let configuration = engine
            .backup_configuration(&copied.repository_id, path)
            .unwrap();
        assert_eq!(configuration["target_copies"], copies);
        let policy = configuration["policy_id"].as_str().unwrap();
        assert!(engine.state.protection_policies[policy].inheritable);
    }
    assert_graph_closed(&engine, &repo.history_tree_id);
    assert_graph_closed(&engine, &destination);
    for (history, name) in [
        (&repo.history_tree_id, "source"),
        (&destination, "destination"),
    ] {
        engine
            .export_history_package(history, &temp.path().join(format!("package-{name}")))
            .unwrap();
    }
    drop(engine);
    let engine = Engine::open(&temp.path().join("state")).unwrap();
    assert!(!engine.state.protection_policies[&bound].inheritable);
    assert!(engine.state.protection_policies[&valid_empty].inheritable);
    assert!(!engine.state.protection_policies.contains_key(&moved_empty));
    assert_graph_closed(&engine, &repo.history_tree_id);
    assert_graph_closed(&engine, &destination);
}

#[test]
fn split_preserves_bound_settings_when_the_original_scope_is_outside_the_cut() {
    for inheritable in [false, true] {
        let (temp, mut engine, repo) = fixture();
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let mut policies = Vec::new();
        for path in ["old", "unused"] {
            engine
                .file_namespace(
                    &repo.repository_id,
                    NamespaceOperation::parse(
                        storage_contract::NamespaceOperationKind::Mkdir,
                        path,
                        None,
                    )
                    .unwrap(),
                    "alice",
                    &new_id("op"),
                )
                .unwrap();
            let configured = engine
                .configure_backups(BackupConfigurationRequest {
                    repository_id: &repo.repository_id,
                    path,
                    target_copies: 2,
                    backup_prefixes: &BTreeMap::from([("B01".into(), path.into())]),
                    expected_generation: 0,
                    principal: "alice",
                    operation_id: &new_id("op"),
                })
                .unwrap();
            policies.push(
                configured.result["policy"]["policy_id"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            );
            engine
                .file_namespace(
                    &repo.repository_id,
                    NamespaceOperation::parse(
                        storage_contract::NamespaceOperationKind::Delete,
                        path,
                        None,
                    )
                    .unwrap(),
                    "alice",
                    &new_id("op"),
                )
                .unwrap();
        }
        engine
            .configure_backups(BackupConfigurationRequest {
                repository_id: &repo.repository_id,
                path: "",
                target_copies: 3,
                backup_prefixes: &BTreeMap::from([
                    ("B01".into(), "root".into()),
                    ("C01".into(), "cold".into()),
                ]),
                expected_generation: 0,
                principal: "alice",
                operation_id: &new_id("op"),
            })
            .unwrap();
        let cut = save(&mut engine, &repo.main_branch_id, b"split body");
        let file = engine
            .list_tree(&repo.main_branch_id)
            .unwrap()
            .into_iter()
            .find(|entry| entry.kind == NamespaceEntryKind::File)
            .unwrap();
        // Real directory/path-node histories are deliberately not splittable.
        // Publish only the valid binding prerequisite into an ordinary history
        // to exercise policy remapping without weakening that admission rule.
        engine
            .mutate(
                "fixture.bound_settings",
                "alice",
                &new_id("op"),
                json!({}),
                |_, state, _| {
                    state
                        .protection_policies
                        .get_mut(&policies[0])
                        .unwrap()
                        .inheritable = inheritable;
                    state
                        .file_backup_policies
                        .insert(file.file_id.clone(), policies[0].clone());
                    state
                        .file_backup_paths
                        .get_mut(&file.file_id)
                        .unwrap()
                        .insert("B01".into(), "old/文件.txt".into());
                    Ok(json!({}))
                },
            )
            .unwrap();
        let plan = engine
            .create_split_plan(&repo.history_tree_id, &cut, "alice")
            .unwrap();
        let split = engine
            .commit_plan(&plan.plan_id, "alice", &new_id("op"))
            .unwrap();
        let history = split.history_tree_id.unwrap();
        let copied = engine
            .state
            .repositories
            .values()
            .find(|repo| repo.history_tree_id == history)
            .unwrap()
            .clone();
        let file = engine
            .list_tree(&copied.main_branch_id)
            .unwrap()
            .into_iter()
            .find(|entry| entry.kind == NamespaceEntryKind::File)
            .unwrap();
        let bound = engine.state.file_backup_policies[&file.file_id].clone();
        let retained = &engine.state.protection_policies[&bound];
        assert!(!retained.inheritable);
        assert_eq!(retained.scope_directory_id, None);
        assert_eq!(retained.scope_file_id, None);
        assert_eq!(retained.target_copies, 2);
        assert_eq!(retained.generation, 1);
        assert_eq!(
            engine
                .state
                .protection_policies
                .values()
                .filter(|policy| policy.repository_id == copied.repository_id)
                .count(),
            2
        );
        assert_eq!(
            engine
                .new_file_placement(&copied.repository_id, "old/future.txt")
                .unwrap()
                .1,
            3
        );
        assert_eq!(
            engine
                .backup_configuration(&copied.repository_id, &file.path)
                .unwrap()["target_copies"],
            2
        );
        assert_eq!(
            engine.state.file_backup_paths[&file.file_id]["B01"],
            "old/文件.txt"
        );
        save(&mut engine, &copied.main_branch_id, b"after split");
        let latest = engine
            .list_tree(&copied.main_branch_id)
            .unwrap()
            .into_iter()
            .find(|entry| entry.kind == NamespaceEntryKind::File)
            .unwrap();
        assert_eq!(engine.state.placements[&latest.version_id].target_copies, 2);
        assert_graph_closed(&engine, &history);
        let package = temp.path().join("package");
        engine.export_history_package(&history, &package).unwrap();
        let destination = temp.path().join("restored");
        let mut restored = Engine::initialize(
            &destination.join("state"),
            Config::development([
                destination.join("p"),
                destination.join("b"),
                destination.join("c"),
            ]),
        )
        .unwrap();
        restored
            .restore_history_package(&package, "P01", 2, "alice", &new_id("op"))
            .unwrap();
        assert!(!restored.state.protection_policies[&bound].inheritable);
        assert_eq!(restored.state.file_backup_policies[&file.file_id], bound);
        drop(restored);
        Engine::open(&destination.join("state")).unwrap();
        drop(engine);
        Engine::open(&temp.path().join("state")).unwrap();
    }
}

#[test]
fn all_named_directory_heads_pin_sources_until_every_head_advances() {
    let (_temporary, mut engine, repo) = fixture();
    let source_commit = save(&mut engine, &repo.main_branch_id, b"frozen source");
    let source = engine
        .list_tree(&repo.main_branch_id)
        .unwrap()
        .into_iter()
        .find(|entry| entry.kind == NamespaceEntryKind::File)
        .unwrap();
    save(&mut engine, &repo.main_branch_id, b"current main");
    let shared = engine
        .create_file_branch(
            &source.file_id,
            &source.version_id,
            "shared",
            "alice",
            &new_id("op"),
        )
        .unwrap()
        .branch_id
        .unwrap();
    save(&mut engine, &shared, b"shared own content");
    // Only the frozen composition still needs the original body; both
    // ordinary branch HEADs have moved to their own newer versions.
    engine
        .create_purge_plan(
            &repo.history_tree_id,
            vec![source.version_id.clone()],
            "alice",
        )
        .unwrap();
    let record = new_id("dbr");
    let parent_record = new_id("dbr");
    let parent_tree = new_id("dtree");
    engine
        .mutate(
            "fixture.directory_composition",
            "alice",
            &new_id("op"),
            json!({}),
            |_, state, _| {
                let parent = DirectoryBranch {
                    directory_branch_id: parent_record.clone(),
                    repository_id: repo.repository_id.clone(),
                    directory_id: repo.repository_id.clone(),
                    path: String::new(),
                    name: "frozen".into(),
                    parent_branch_id: None,
                    head_tree_id: parent_tree.clone(),
                    commit_id: source_commit.clone(),
                    entries: vec![source.clone()],
                    file_branches: BTreeMap::from([(source.file_id.clone(), shared.clone())]),
                    file_contexts: BTreeMap::from([(
                        source.lookup_key().to_owned(),
                        FileContext {
                            node_id: None,
                            selection_generation: 0,
                            branch_id: shared.clone(),
                            inherited_from: None,
                        },
                    )]),
                    inherited_files: BTreeMap::new(),
                    directory_refs: BTreeMap::new(),
                    materialized: true,
                    sequence: state.sequence,
                };
                state.directory_trees.insert(
                    parent_tree.clone(),
                    DirectoryTree {
                        tree_id: parent_tree.clone(),
                        directory_branch_id: parent_record.clone(),
                        parent_tree_id: None,
                        commit_id: parent.commit_id.clone(),
                        path: parent.path.clone(),
                        entries: parent.entries.clone(),
                        file_branches: parent.file_branches.clone(),
                        file_contexts: parent.file_contexts.clone(),
                        inherited_files: parent.inherited_files.clone(),
                        directory_refs: parent.directory_refs.clone(),
                        sequence: parent.sequence,
                    },
                );
                let mut pending = parent.clone();
                pending.directory_branch_id = record.clone();
                pending.name = "pending".into();
                pending.parent_branch_id = Some(parent_record.clone());
                pending.materialized = false;
                state
                    .directory_branches
                    .insert(parent_record.clone(), parent);
                state.directory_branches.insert(record.clone(), pending);
                Ok(json!({}))
            },
        )
        .unwrap();
    assert_eq!(
        engine
            .create_purge_plan(
                &repo.history_tree_id,
                vec![source.version_id.clone()],
                "alice"
            )
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::ContentPinned
    );
    assert_eq!(
        engine
            .create_split_plan(&repo.history_tree_id, &source_commit, "alice")
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::DirectoryHistorySplitUnsupported
    );
    engine
        .mutate(
            "fixture.directory_complete",
            "alice",
            &new_id("op"),
            json!({}),
            |_, state, _| {
                let branch = state.directory_branches.get_mut(&record).unwrap();
                let tree = new_id("dtree");
                state.directory_trees.insert(
                    tree.clone(),
                    DirectoryTree {
                        tree_id: tree.clone(),
                        directory_branch_id: record.clone(),
                        parent_tree_id: Some(branch.head_tree_id.clone()),
                        commit_id: branch.commit_id.clone(),
                        path: branch.path.clone(),
                        entries: branch.entries.clone(),
                        file_branches: branch.file_branches.clone(),
                        file_contexts: branch.file_contexts.clone(),
                        inherited_files: branch.inherited_files.clone(),
                        directory_refs: branch.directory_refs.clone(),
                        sequence: state.sequence,
                    },
                );
                branch.head_tree_id = tree;
                branch.materialized = true;
                branch.sequence = state.sequence;
                Ok(json!({}))
            },
        )
        .unwrap();
    assert_eq!(
        engine
            .create_purge_plan(
                &repo.history_tree_id,
                vec![source.version_id.clone()],
                "alice"
            )
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::ContentPinned
    );
    engine
        .mutate(
            "fixture.directory_select",
            "alice",
            &new_id("op"),
            json!({}),
            |_, state, _| {
                state
                    .directory_selections
                    .insert(repo.repository_id.clone(), record.clone());
                Ok(json!({}))
            },
        )
        .unwrap();
    assert_eq!(
        engine
            .create_purge_plan(
                &repo.history_tree_id,
                vec![source.version_id.clone()],
                "alice"
            )
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::ContentPinned
    );
    engine
        .mutate(
            "fixture.directory_unselect",
            "alice",
            &new_id("op"),
            json!({}),
            |_, state, _| {
                state.directory_selections.remove(&repo.repository_id);
                Ok(json!({}))
            },
        )
        .unwrap();
    assert_eq!(
        engine
            .create_purge_plan(
                &repo.history_tree_id,
                vec![source.version_id.clone()],
                "alice"
            )
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::ContentPinned
    );
    // A branch remains a readable snapshot when it is not selected. Only
    // advancing every retained named HEAD releases its former content.
    let latest = engine.state.commits[&engine.state.branches[&shared].head_commit_id].clone();
    engine
        .mutate(
            "fixture.directory_advance",
            "alice",
            &new_id("op"),
            json!({}),
            |_, state, _| {
                for id in [&parent_record, &record] {
                    let branch = state.directory_branches.get_mut(id).unwrap();
                    let tree = new_id("dtree");
                    state.directory_trees.insert(
                        tree.clone(),
                        DirectoryTree {
                            tree_id: tree.clone(),
                            directory_branch_id: id.clone(),
                            parent_tree_id: Some(branch.head_tree_id.clone()),
                            commit_id: latest.commit_id.clone(),
                            path: String::new(),
                            entries: latest.entries.clone(),
                            file_branches: branch.file_branches.clone(),
                            file_contexts: branch.file_contexts.clone(),
                            inherited_files: BTreeMap::new(),
                            directory_refs: BTreeMap::new(),
                            sequence: state.sequence,
                        },
                    );
                    branch.head_tree_id = tree;
                    branch.commit_id = latest.commit_id.clone();
                    branch.entries = latest.entries.clone();
                    branch.materialized = true;
                    branch.sequence = state.sequence;
                }
                Ok(json!({}))
            },
        )
        .unwrap();
    let plan = engine
        .create_purge_plan(
            &repo.history_tree_id,
            vec![source.version_id.clone()],
            "alice",
        )
        .unwrap();
    engine
        .commit_plan(&plan.plan_id, "alice", &new_id("op"))
        .unwrap();
    assert_eq!(
        engine
            .open_content(&source.version_id)
            .err()
            .unwrap()
            .code(),
        storage_contract::ErrorCode::ContentPurged
    );
    assert_eq!(
        engine.state.directory_trees[&parent_tree].entries[0].version_id,
        source.version_id
    );
    let own = engine
        .list_tree(&shared)
        .unwrap()
        .into_iter()
        .find(|entry| entry.kind == NamespaceEntryKind::File)
        .unwrap();
    assert!(engine.open_content(&own.version_id).is_ok());
}

#[test]
fn inherited_branches_and_their_workspaces_pin_sources_until_first_own_content() {
    let (_temporary, mut engine, repo) = fixture();
    let source = save(&mut engine, &repo.main_branch_id, b"old source");
    let old = engine
        .list_tree(&repo.main_branch_id)
        .unwrap()
        .into_iter()
        .find(|entry| entry.kind == NamespaceEntryKind::File)
        .unwrap();
    let pending = engine
        .create_branch(&source, "pending", "alice", &new_id("op"))
        .unwrap();
    let workspace = engine
        .create_workspace(&pending.branch_id, "alice")
        .unwrap();
    // Inheritance is frozen at registration; main advancing does not change
    // this branch's visible source or release its active read dependency.
    save(&mut engine, &repo.main_branch_id, b"current main");
    assert_eq!(
        engine
            .create_purge_plan(&repo.history_tree_id, vec![old.version_id.clone()], "alice")
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::ContentPinned
    );
    let inherited = engine
        .list_tree(&pending.branch_id)
        .unwrap()
        .into_iter()
        .find(|entry| entry.kind == NamespaceEntryKind::File)
        .unwrap();
    assert_eq!(inherited.version_id, old.version_id);
    assert_eq!(inherited.size, old.size);
    engine
        .stage_bytes(&workspace.workspace_id, &old.path, b"first own body")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: new_id("op"),
            principal: "alice".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "first own content releases inherited source".into(),
            request_digest: None,
        })
        .unwrap();
    assert!(
        engine.state.branches[&pending.branch_id]
            .pending_file_versions
            .is_empty()
    );
    let current = engine
        .list_tree(&pending.branch_id)
        .unwrap()
        .into_iter()
        .find(|entry| entry.kind == NamespaceEntryKind::File)
        .unwrap();
    assert_eq!(
        engine.state.versions[&current.version_id].source_version_ids,
        vec![old.version_id.clone()]
    );
    let plan = engine
        .create_purge_plan(&repo.history_tree_id, vec![old.version_id.clone()], "alice")
        .unwrap();
    engine
        .commit_plan(&plan.plan_id, "alice", &new_id("op"))
        .unwrap();
    assert_eq!(
        engine.open_content(&old.version_id).err().unwrap().code(),
        storage_contract::ErrorCode::ContentPurged
    );
    assert!(engine.open_content(&current.version_id).is_ok());
}

#[test]
fn selected_inherited_file_pins_source_through_directory_main_until_materialized() {
    let (_temporary, mut engine, repo) = fixture();
    save(&mut engine, &repo.main_branch_id, b"old source");
    let old = engine
        .list_tree(&repo.main_branch_id)
        .unwrap()
        .into_iter()
        .find(|entry| entry.kind == NamespaceEntryKind::File)
        .unwrap();
    save(&mut engine, &repo.main_branch_id, b"owned main body");
    let created = engine
        .create_file_branch(
            &old.file_id,
            &old.version_id,
            "pending",
            "alice",
            &new_id("op"),
        )
        .unwrap();
    let branch = created.branch_id.unwrap();
    engine
        .select_file_branch(&old.file_id, &branch, "alice", &new_id("op"))
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "alice")
        .unwrap();
    assert_eq!(
        engine
            .create_purge_plan(&repo.history_tree_id, vec![old.version_id.clone()], "alice")
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::ContentPinned
    );
    engine
        .stage_bytes(
            &workspace.workspace_id,
            &old.path,
            b"first selected branch bytes",
        )
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: new_id("op"),
            principal: "alice".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "activate selected pending".into(),
            request_digest: None,
        })
        .unwrap();
    assert!(
        engine.state.branches[&branch]
            .pending_file_versions
            .is_empty()
    );
    let current = engine
        .list_tree(&branch)
        .unwrap()
        .into_iter()
        .find(|entry| entry.kind == NamespaceEntryKind::File)
        .unwrap();
    assert_eq!(
        engine.state.versions[&current.version_id].source_version_ids,
        vec![old.version_id.clone()]
    );
    let plan = engine
        .create_purge_plan(&repo.history_tree_id, vec![old.version_id.clone()], "alice")
        .unwrap();
    engine
        .commit_plan(&plan.plan_id, "alice", &new_id("op"))
        .unwrap();
    assert_eq!(
        engine.open_content(&old.version_id).err().unwrap().code(),
        storage_contract::ErrorCode::ContentPurged
    );
    assert!(engine.open_content(&current.version_id).is_ok());
}

#[test]
fn split_remaps_inherited_references_without_creating_placeholder_files() {
    let (_temporary, mut engine, repo) = fixture();
    let cut = save(&mut engine, &repo.main_branch_id, b"first");
    let original = engine
        .list_tree(&repo.main_branch_id)
        .unwrap()
        .into_iter()
        .find(|entry| entry.kind == NamespaceEntryKind::File)
        .unwrap();
    let pending = engine
        .create_branch(&cut, "pending", "alice", &new_id("op"))
        .unwrap();
    save(&mut engine, &repo.main_branch_id, b"second");
    let plan = engine
        .create_split_plan(&repo.history_tree_id, &cut, "alice")
        .unwrap();
    let result = engine
        .commit_plan(&plan.plan_id, "alice", &new_id("op"))
        .unwrap();
    let history = result.history_tree_id.unwrap();
    let moved = engine
        .state
        .branches
        .values()
        .find(|branch| branch.history_tree_id == history && branch.name == "pending")
        .unwrap();
    assert_eq!(moved.pending_file_versions.len(), 1);
    let (file, source) = moved.pending_file_versions.first_key_value().unwrap();
    assert_ne!(file, &original.file_id);
    assert_ne!(source, &original.version_id);
    assert_eq!(engine.state.versions[source].file_id, *file);
    assert_eq!(engine.state.versions[source].history_tree_id, history);
    assert!(
        engine.state.commits[&moved.head_commit_id]
            .entries
            .iter()
            .any(|entry| &entry.file_id == file && &entry.version_id == source)
    );
    assert_eq!(
        engine.state.commits[&moved.head_commit_id].pending_file_versions,
        moved.pending_file_versions
    );
    assert!(
        engine.state.branches[&pending.branch_id]
            .pending_file_versions
            .is_empty()
    );
    for disk in &engine.config.disks {
        assert!(
            !disk
                .root
                .join("histories")
                .join(&repo.history_tree_id)
                .join("file-containers")
                .join(&original.file_id)
                .join(original.name())
                .join("pending-branches")
                .join(format!("{}.pending", pending.branch_id))
                .exists()
        );
        assert!(
            !disk
                .root
                .join("histories")
                .join(&history)
                .join("file-containers")
                .join(file)
                .join(original.name())
                .join("pending-branches")
                .exists()
        );
        let cache = disk
            .root
            .join("histories")
            .join(&history)
            .join("file-containers")
            .join(file)
            .join("namespace")
            .join(&engine.state.file_locations[file])
            .join("container.json");
        assert!(!cache.parent().unwrap().join("pending-branches").exists());
        let metadata: json_codec::Value = read_json(&cache).unwrap();
        assert_eq!(
            metadata["inherited_branches"][0]["branch_id"],
            moved.branch_id
        );
        assert_eq!(
            metadata["inherited_branches"][0]["source_version_id"],
            *source
        );
        assert_eq!(metadata["inherited_branches"][0]["version_id"], *source);
        assert!(metadata["inherited_branches"][0]["own_version_id"].is_null());
    }
    assert_graph_closed(&engine, &history);
    assert_graph_closed(&engine, &repo.history_tree_id);
}

#[test]
fn split_keeps_pending_prefix_snapshots_and_remaps_mixed_snapshot_reservations() {
    let (_temporary, mut engine, repo) = fixture();
    let source = save(&mut engine, &repo.main_branch_id, b"lineage source");
    let original = engine
        .list_tree(&repo.main_branch_id)
        .unwrap()
        .into_iter()
        .find(|entry| entry.kind == NamespaceEntryKind::File)
        .unwrap();
    let pending = engine
        .create_branch(&source, "pending", "alice", &new_id("op"))
        .unwrap();
    let prefix = pending.head_commit_id.clone();
    let workspace = engine
        .create_workspace(&pending.branch_id, "alice")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "other.txt", b"only real new file")
        .unwrap();
    let cut = engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: new_id("op"),
            principal: "alice".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "mixed snapshot".into(),
            request_digest: None,
        })
        .unwrap()
        .commit_id
        .unwrap();
    let plan = engine
        .create_split_plan(&repo.history_tree_id, &cut, "alice")
        .unwrap();
    let result = engine
        .commit_plan(&plan.plan_id, "alice", &new_id("op"))
        .unwrap();
    let history = result.history_tree_id.unwrap();
    let moved = engine
        .state
        .branches
        .values()
        .find(|branch| branch.history_tree_id == history && branch.name == "pending")
        .unwrap();
    let remapped = &engine.state.commits[&moved.head_commit_id];
    assert_eq!(remapped.pending_file_versions.len(), 1);
    assert_eq!(remapped.pending_file_versions, moved.pending_file_versions);
    let (file, source) = remapped.pending_file_versions.first_key_value().unwrap();
    assert_ne!(file, &original.file_id);
    assert_ne!(source, &original.version_id);
    assert!(
        remapped
            .entries
            .iter()
            .any(|entry| &entry.file_id == file && &entry.version_id == source)
    );
    let old = &engine.state.branches[&pending.branch_id];
    assert_eq!(old.head_commit_id, prefix);
    assert_eq!(
        old.pending_file_versions[&original.file_id],
        original.version_id
    );
    assert_eq!(
        engine.state.commits[&prefix].pending_file_versions,
        old.pending_file_versions
    );
    assert_graph_closed(&engine, &history);
    assert_graph_closed(&engine, &repo.history_tree_id);
}

#[test]
fn split_preserves_identity_per_file_and_requires_explicit_rebinding() {
    let (temp, mut engine, repo) = fixture();
    let cut = save(&mut engine, &repo.main_branch_id, b"first");
    let original = engine
        .list_tree(&repo.main_branch_id)
        .unwrap()
        .into_iter()
        .find(|e| e.kind == NamespaceEntryKind::File)
        .unwrap();
    save(&mut engine, &repo.main_branch_id, b"second");
    let plan = engine
        .create_split_plan(&repo.history_tree_id, &cut, "alice")
        .unwrap();
    let operation = new_id("op");
    let result = engine
        .commit_plan(&plan.plan_id, "alice", &operation)
        .unwrap();
    assert_eq!(
        engine
            .commit_plan(&plan.plan_id, "alice", &operation)
            .unwrap()
            .sequence,
        result.sequence
    );
    let history = result.history_tree_id.as_ref().unwrap();
    let new_repo_id = result.result["repository_id"].as_str().unwrap();
    let new_repo = &engine.state.repositories[new_repo_id];
    assert_ne!(new_repo_id, repo.repository_id);
    assert!(!new_repo.smb_enabled);
    assert_eq!(new_repo.smb_binding_generation, 1);
    assert!(!engine.state.repositories[&repo.repository_id].smb_enabled);
    assert_eq!(
        engine.state.repositories[&repo.repository_id].smb_binding_generation,
        repo.smb_binding_generation + 1
    );
    assert_eq!(
        engine.state.repositories[&repo.repository_id].history_tree_id,
        repo.history_tree_id
    );
    assert_eq!(
        engine.state.branches[&repo.main_branch_id].state.as_str(),
        "migrated"
    );
    let graph = engine.graph(history).unwrap();
    let files = graph
        .commits
        .iter()
        .flat_map(|c| &c.entries)
        .filter(|e| e.kind == NamespaceEntryKind::File)
        .collect::<Vec<_>>();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].entry_id, files[1].entry_id);
    assert_eq!(files[0].file_id, files[1].file_id);
    assert_ne!(files[0].file_id, original.file_id);
    assert_ne!(files[0].entry_id, original.entry_id);
    for file in files {
        let version = &engine.state.versions[&file.version_id];
        assert_eq!(version.file_id, file.file_id);
        assert!(version.source_version_ids.is_empty());
        let mut content = engine.open_content(&file.version_id).unwrap();
        let mut bytes = Vec::new();
        content.file.read_to_end(&mut bytes).unwrap();
        assert!([b"first".as_slice(), b"second".as_slice()].contains(&bytes.as_slice()));
    }
    assert_eq!(
        engine.graph(&repo.history_tree_id).unwrap().commits.len(),
        1
    );
    assert_graph_closed(&engine, &repo.history_tree_id);
    assert_graph_closed(&engine, history);
    drop(engine);
    let reopened = Engine::open(&temp.path().join("state")).unwrap();
    assert_graph_closed(&reopened, &repo.history_tree_id);
    assert_graph_closed(&reopened, history);
}

#[test]
fn split_merge_links_have_local_branch_stubs_and_explicit_crossing_boundaries() {
    let (_temp, mut engine, repo) = fixture();
    let root = engine.state.histories[&repo.history_tree_id]
        .root_commit_id
        .clone();
    let outside = engine
        .create_branch(&root, "前缀分支", "alice", &new_id("op"))
        .unwrap();
    let cut = save(&mut engine, &repo.main_branch_id, b"first");
    let last = save(&mut engine, &repo.main_branch_id, b"second");
    // Legacy metadata could name an endpoint outside the recorded branch.
    // New requests reject it; split still closes all references when such
    // old data is encountered instead of emitting an empty branch id.
    let legacy = MergeLink {
        merge_link_id: new_id("link"),
        history_tree_id: repo.history_tree_id.clone(),
        source_branch_id: outside.branch_id.clone(),
        source_commit_id: cut.clone(),
        target_branch_id: repo.main_branch_id.clone(),
        target_commit_id: last.clone(),
        author: "alice".into(),
        operation_id: new_id("op"),
    };
    std::sync::Arc::make_mut(&mut engine.state)
        .merge_links
        .insert(legacy.merge_link_id.clone(), legacy);
    engine
        .add_merge_link(
            &outside.branch_id,
            &outside.head_commit_id,
            &repo.main_branch_id,
            &cut,
            "alice",
            &new_id("op"),
        )
        .unwrap();
    let plan = engine
        .create_split_plan(&repo.history_tree_id, &cut, "alice")
        .unwrap();
    let result = engine
        .commit_plan(&plan.plan_id, "alice", &new_id("op"))
        .unwrap();
    let history = result.history_tree_id.unwrap();
    assert_graph_closed(&engine, &repo.history_tree_id);
    assert_graph_closed(&engine, &history);
    let graph = engine.graph(&history).unwrap();
    assert_eq!(graph.merge_links.len(), 1);
    assert!(
        graph
            .branches
            .iter()
            .any(|b| b.state == storage_contract::BranchState::Archived)
    );
    assert!(
        graph
            .boundary_marks
            .iter()
            .any(|b| b.source_merge_link_id.is_some())
    );
    assert!(
        engine
            .graph(&repo.history_tree_id)
            .unwrap()
            .merge_links
            .is_empty()
    );
}

#[test]
fn split_preserves_archived_branches_and_selects_an_active_main() {
    for scenario in ["archived_side", "archived_main", "active_survivor"] {
        let (temp, mut engine, repo) = fixture();
        let root = engine.state.histories[&repo.history_tree_id]
            .root_commit_id
            .clone();
        let side = engine
            .create_branch(&root, "side", "alice", &new_id("op"))
            .unwrap();
        let archived = if scenario == "archived_main" {
            engine
                .set_smb_binding(
                    &repo.repository_id,
                    &side.branch_id,
                    true,
                    1,
                    "alice",
                    &new_id("op"),
                )
                .unwrap();
            repo.main_branch_id.clone()
        } else {
            side.branch_id.clone()
        };
        let cut = save(&mut engine, &archived, b"archived body");
        let active = (scenario == "active_survivor").then(|| {
            engine
                .create_branch(&cut, "live", "alice", &new_id("op"))
                .unwrap()
                .branch_id
        });
        engine
            .archive_branch(&archived, "alice", &new_id("op"))
            .unwrap();
        let plan = engine
            .create_split_plan(&repo.history_tree_id, &cut, "alice")
            .unwrap();
        let result = engine
            .commit_plan(&plan.plan_id, "alice", &new_id("op"))
            .unwrap();
        let history = result.history_tree_id.as_ref().unwrap();
        let main = result.branch_id.as_ref().unwrap();
        let mapped_archived = result.result["branch_mapping"][&archived].as_str().unwrap();
        assert_eq!(
            engine.state.branches[mapped_archived].state.as_str(),
            "archived"
        );
        assert_eq!(
            engine.state.branches[main].state.as_str(),
            "active",
            "{scenario}"
        );
        assert!(engine.state.branches[main].file_id.is_none());
        if scenario == "archived_main" {
            assert_eq!(engine.state.branches[main].name, "main-2");
        }
        let graph = engine.graph(history).unwrap();
        let mapped_count = result.result["branch_mapping"].as_object().unwrap().len();
        if let Some(active) = active {
            assert_eq!(
                main,
                result.result["branch_mapping"][active].as_str().unwrap()
            );
            assert_eq!(
                graph.branches.len(),
                mapped_count,
                "do not add an unnecessary main"
            );
        } else {
            assert_ne!(main, mapped_archived);
            assert_eq!(graph.branches.len(), mapped_count + 1);
            assert_eq!(
                engine.state.branches[main].head_commit_id,
                graph.history.root_commit_id
            );
        }
        let names = graph
            .branches
            .iter()
            .map(|branch| name_policy::lookup_key(&branch.name))
            .collect::<BTreeSet<_>>();
        assert_eq!(
            names.len(),
            graph.branches.len(),
            "new main must not duplicate an archived name"
        );
        engine
            .export_history_package(history, &temp.path().join("exports/package"))
            .unwrap();
        let new_repo = result.result["repository_id"].as_str().unwrap().to_owned();
        let main = main.clone();
        let mapped_archived = mapped_archived.to_owned();
        drop(engine);
        let mut engine = Engine::open(&temp.path().join("state")).unwrap();
        assert_eq!(
            engine.state.branches[&mapped_archived].state.as_str(),
            "archived"
        );
        engine
            .set_smb_binding(&new_repo, &main, true, 1, "alice", &new_id("op"))
            .unwrap();
        let workspace = engine.create_workspace(&main, "alice").unwrap();
        engine
            .abort_workspace(&workspace.workspace_id, "alice")
            .unwrap();
    }
}

#[test]
fn split_rejects_root_and_rechecks_active_read_before_publication() {
    let (_temp, mut engine, repo) = fixture();
    let root = engine.state.histories[&repo.history_tree_id]
        .root_commit_id
        .clone();
    assert_eq!(
        engine
            .create_split_plan(&repo.history_tree_id, &root, "alice")
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::SplitRootForbidden
    );
    let cut = save(&mut engine, &repo.main_branch_id, b"first");
    let plan = engine
        .create_split_plan(&repo.history_tree_id, &cut, "alice")
        .unwrap();
    let version = engine
        .list_tree(&repo.main_branch_id)
        .unwrap()
        .into_iter()
        .find(|e| e.kind == NamespaceEntryKind::File)
        .unwrap()
        .version_id;
    let read = engine.open_content(&version).unwrap();
    let generation = engine.state.business_generation;
    assert_eq!(
        engine
            .commit_plan(&plan.plan_id, "alice", &new_id("op"))
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::ContentPinned
    );
    assert_eq!(engine.state.business_generation, generation);
    drop(read);
    engine
        .commit_plan(&plan.plan_id, "alice", &new_id("op"))
        .unwrap();
}

#[test]
fn split_at_deletion_preserves_facts_without_copying_deleted_content() {
    let (temp, mut engine, repo) = fixture();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    save(
        &mut engine,
        &repo.main_branch_id,
        b"not needed by split history",
    );
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "alice")
        .unwrap();
    engine
        .namespace_operation(
            &workspace.workspace_id,
            NamespaceOperation::parse(
                storage_contract::NamespaceOperationKind::DeleteTree,
                "资料",
                None,
            )
            .unwrap(),
        )
        .unwrap();
    let cut = engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: new_id("op"),
            principal: "alice".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "delete before split".into(),
            request_digest: None,
        })
        .unwrap()
        .commit_id
        .unwrap();
    let facts = engine.state.commits[&cut]
        .namespace_deletions
        .clone()
        .unwrap();
    let plan = engine
        .create_split_plan(&repo.history_tree_id, &cut, "alice")
        .unwrap();
    let result = engine
        .commit_plan(&plan.plan_id, "alice", &new_id("op"))
        .unwrap();
    let history = result.history_tree_id.unwrap();
    let graph = engine.graph(&history).unwrap();
    let root = graph
        .commits
        .iter()
        .find(|commit| commit.kind == "split_root")
        .unwrap();
    assert!(root.entries.is_empty());
    assert!(root.parent_commit_ids.is_empty());
    assert_eq!(root.namespace_deletions.as_ref(), Some(&facts));
    assert!(
        !engine
            .state
            .versions
            .values()
            .any(|version| version.history_tree_id == history)
    );
    let package = temp.path().join("deletion-split-package");
    let report = engine.export_history_package(&history, &package).unwrap();
    assert_eq!(report.versions, 0);
    assert_eq!(report.full_bytes, 0);
    crate::history_package::offline_check_history_package(&package).unwrap();
    let (_target, mut restored, _) = fixture();
    restored
        .restore_history_package(&package, "P01", 1, "alice", &new_id("op"))
        .unwrap();
    assert_eq!(
        restored.state.commits[&root.commit_id]
            .namespace_deletions
            .as_ref(),
        Some(&facts)
    );
}
