//! Ordinary rename consumers and the durable replay entrypoint. The source view
//! is installed by the parent fixture; these tests do not certify actual D3/I/O.
use super::*;

fn rename(fixture: &mut Fixture, source: &str, destination: &str) -> OperationResult {
    fixture
        .engine
        .file_namespace(
            &fixture.repo.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::Rename, source, Some(destination))
                .unwrap(),
            "admin",
            op(),
        )
        .unwrap()
}

fn current(fixture: &Fixture, path: &str) -> Entry {
    fixture
        .engine
        .namespace_tree(&fixture.repo.main_branch_id)
        .unwrap()
        .get(path)
        .unwrap()
        .clone()
}

fn stage_operation(
    fixture: &mut Fixture,
    workspace: &Workspace,
    kind: NamespaceOperationKind,
    source: &str,
    destination: Option<&str>,
) {
    fixture
        .engine
        .namespace_operation(
            &workspace.workspace_id,
            NamespaceOperation::parse(kind, source, destination).unwrap(),
        )
        .unwrap();
}

fn assert_hidden_absent(fixture: &Fixture) {
    let view = fixture.engine.current_namespaces.as_ref().unwrap();
    for scope in view.branches.values().chain(view.directories.values()) {
        assert!(
            scope
                .entries
                .iter()
                .all(|entry| entry.entry_id != fixture.removed.entry_id)
        );
    }
    let root = &fixture.engine.state.branches[&fixture.repo.main_branch_id].head_commit_id;
    assert!(
        fixture.engine.state.commits[root]
            .entries
            .iter()
            .all(|entry| entry.entry_id != fixture.removed.entry_id)
    );
}

#[test]
fn membership_registry_current_rename_to_hidden_name_keeps_source_identity_and_history() {
    for file_manager in [false, true] {
        let mut fixture = Fixture::new(file_manager);
        let commits = fixture.engine.state.commits.clone();
        let trees = fixture.engine.state.directory_trees.clone();
        fixture.project(false);
        rename(&mut fixture, "dir/b", "dir/a");
        let moved = current(&fixture, "dir/a");
        assert_eq!(moved.entry_id, fixture.retained.entry_id);
        assert_eq!(moved.file_id, fixture.retained.file_id);
        assert_eq!(moved.version_id, fixture.retained.version_id);
        assert!(
            fixture
                .engine
                .namespace_tree(&fixture.repo.main_branch_id)
                .unwrap()
                .get("dir/b")
                .is_none()
        );
        assert_hidden_absent(&fixture);
        for (id, commit) in commits.iter() {
            assert_eq!(fixture.engine.state.commits.get(id), Some(commit));
        }
        for (id, tree) in trees.iter() {
            assert_eq!(fixture.engine.state.directory_trees.get(id), Some(tree));
        }
        let workspace = fixture
            .engine
            .create_workspace(&fixture.repo.main_branch_id, "admin")
            .unwrap();
        let upload = fixture._temp.path().join("upload-after-rename");
        std::fs::write(&upload, b"must not reuse hidden item").unwrap();
        assert_eq!(
            fixture
                .engine
                .stage_file_as(
                    &workspace.workspace_id,
                    &fixture.removed.entry_id,
                    "dir/a",
                    &upload
                )
                .unwrap_err()
                .code(),
            ErrorCode::WorkspaceChanged
        );
        fixture
            .engine
            .stage_bytes(&workspace.workspace_id, "dir/a", b"saved after rename")
            .unwrap();
        fixture
            .engine
            .commit_workspace(request(&workspace))
            .unwrap();
        let saved = current(&fixture, "dir/a");
        assert_eq!(saved.entry_id, moved.entry_id);
        assert_eq!(saved.file_id, moved.file_id);
        assert_ne!(saved.version_id, moved.version_id);
        assert_hidden_absent(&fixture);
    }
}

#[test]
fn membership_registry_current_nested_and_case_only_renames_refresh_selected_children() {
    let mut fixture = Fixture::new(true);
    save(
        &mut fixture.engine,
        &fixture.repo.main_branch_id,
        "dir/nested/keep",
        b"nested",
    );
    fixture
        .engine
        .file_namespace(
            &fixture.repo.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::Mkdir, "dir/empty", None).unwrap(),
            "admin",
            op(),
        )
        .unwrap();
    let nested = current(&fixture, "dir/nested/keep");
    let directory = current(&fixture, "dir");
    let old_trees = fixture.engine.state.directory_trees.clone();
    fixture.project(false);
    rename(&mut fixture, "DIR", "Dir");
    assert_eq!(current(&fixture, "Dir/b").path(), "Dir/b");
    rename(&mut fixture, "Dir", "outer/Moved");
    assert_eq!(
        current(&fixture, "outer/Moved").entry_id,
        directory.entry_id
    );
    assert_eq!(
        current(&fixture, "outer/Moved/nested/keep").version_id,
        nested.version_id
    );
    assert_hidden_absent(&fixture);
    let root = current(&fixture, "outer/Moved");
    let selected = &fixture.engine.state.directory_selections[&root.entry_id];
    let branch = &fixture.engine.state.directory_branches[selected];
    assert_eq!(branch.path, "outer/Moved");
    let tree = &fixture.engine.state.directory_trees[&branch.head_tree_id];
    assert_eq!(
        tree.entries,
        fixture.engine.current_directory_entries(selected).unwrap()
    );
    for reference in tree.directory_refs.values() {
        let child = &fixture.engine.state.directory_trees[&reference.tree_id];
        assert!(child.path.starts_with("outer/Moved/"));
        assert!(
            child
                .entries
                .iter()
                .all(|entry| entry.entry_id != fixture.removed.entry_id)
        );
    }
    for (id, tree) in old_trees.iter() {
        assert_eq!(fixture.engine.state.directory_trees.get(id), Some(tree));
    }
}

#[test]
fn membership_registry_current_rename_updates_the_exact_independent_file_context() {
    let mut fixture = Fixture::new(true);
    let context = fixture
        .engine
        .file_context(&fixture.repo.repository_id, "dir/b")
        .unwrap();
    fixture.project(false);
    rename(&mut fixture, "dir/b", "elsewhere/renamed");
    let moved = current(&fixture, "elsewhere/renamed");
    let after = fixture
        .engine
        .file_context(&fixture.repo.repository_id, "elsewhere/renamed")
        .unwrap();
    assert_eq!(after.branch_id, context.branch_id);
    let node = &fixture.engine.state.file_path_nodes[after.node_id.as_ref().unwrap()];
    assert_eq!(node.previous_path, "dir/b");
    assert_eq!(node.path, "elsewhere/renamed");
    assert_eq!(node.version_id, moved.version_id);
    assert!(
        fixture
            .engine
            .file_context(&fixture.repo.repository_id, "dir/b")
            .is_none()
    );
    assert_eq!(
        fixture
            .engine
            .namespace_tree(&after.branch_id)
            .unwrap()
            .get("elsewhere/renamed"),
        Some(&moved)
    );
    let workspace = fixture
        .engine
        .create_workspace(&after.branch_id, "admin")
        .unwrap();
    fixture
        .engine
        .stage_bytes(
            &workspace.workspace_id,
            "elsewhere/renamed",
            b"independent save",
        )
        .unwrap();
    fixture
        .engine
        .commit_workspace(request(&workspace))
        .unwrap();
    let saved = current(&fixture, "elsewhere/renamed");
    assert_eq!(saved.entry_id, moved.entry_id);
    assert_eq!(saved.file_id, moved.file_id);
    assert_ne!(saved.version_id, moved.version_id);
    assert_hidden_absent(&fixture);
}

#[test]
fn membership_registry_current_rename_keeps_another_root_context_and_selected_tree() {
    let mut fixture = Fixture::new(true);
    let other = fixture
        .engine
        .create_disk_root("P01", "admin", op())
        .unwrap();
    fixture
        .engine
        .update_placement_rule(PlacementRuleUpdate {
            rule_id: &new_id("rule").unwrap(),
            repository_id: &other.repository_id,
            path_prefix: "dir/b",
            primary_slot_id: "D04",
            expected_generation: 0,
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    save(
        &mut fixture.engine,
        &other.main_branch_id,
        "dir/b",
        b"independent root",
    );
    let old = fixture
        .engine
        .file_context(&other.repository_id, "dir/b")
        .unwrap();
    let other_branch = fixture.engine.state.branches[&other.main_branch_id].clone();
    let selected = fixture.engine.state.directory_selections[&other.repository_id].clone();
    let other_tree = fixture.engine.state.directory_branches[&selected].clone();
    fixture.project(false);
    rename(&mut fixture, "dir", "moved");
    assert_eq!(
        fixture.engine.file_context(&other.repository_id, "dir/b"),
        Some(old)
    );
    assert_eq!(
        fixture.engine.state.branches[&other.main_branch_id],
        other_branch
    );
    assert_eq!(
        fixture.engine.state.directory_branches[&selected],
        other_tree
    );
    assert!(
        fixture
            .engine
            .namespace_tree(&other.main_branch_id)
            .unwrap()
            .get("dir/b")
            .is_some()
    );
}

#[test]
fn membership_registry_current_rename_keeps_alias_versions_and_contexts_independent() {
    let mut fixture = Fixture::new(true);
    save(
        &mut fixture.engine,
        &fixture.repo.main_branch_id,
        "alias/keep",
        b"keep",
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
    let alias = current(&fixture, "alias");
    let target = fixture.engine.state.directory_selections[&alias.entry_id].clone();
    let request = DirectoryNamespaceBatch {
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
        .directory_namespace_batch(&fixture.repo.repository_id, &request, "admin", op())
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
    let alias = current(&fixture, "alias/c");
    let source = current(&fixture, "dir/b");
    assert_eq!(alias.entry_id, source.entry_id);
    assert_eq!(alias.file_id, source.file_id);
    assert_ne!(alias.version_id, source.version_id);
    let alias_context = fixture
        .engine
        .file_context(&fixture.repo.repository_id, "alias/c")
        .unwrap();
    let source_context = fixture
        .engine
        .file_context(&fixture.repo.repository_id, "dir/b")
        .unwrap();
    assert_ne!(alias_context.branch_id, source_context.branch_id);
    fixture.project(false);
    rename(&mut fixture, "dir/b", "dir/renamed");
    assert_eq!(current(&fixture, "alias/c"), alias);
    assert_eq!(
        fixture
            .engine
            .file_context(&fixture.repo.repository_id, "alias/c"),
        Some(alias_context)
    );
    let moved = current(&fixture, "dir/renamed");
    assert_eq!(moved.entry_id, source.entry_id);
    assert_eq!(moved.version_id, source.version_id);
    assert_eq!(
        fixture
            .engine
            .file_context(&fixture.repo.repository_id, "dir/renamed")
            .unwrap()
            .branch_id,
        source_context.branch_id
    );
    assert_hidden_absent(&fixture);
}

#[test]
fn membership_registry_current_workspace_rename_orders_deletions_and_unrelated_uploads() {
    for file_manager in [false, true] {
        let mut fixture = Fixture::new(file_manager);
        save(
            &mut fixture.engine,
            &fixture.repo.main_branch_id,
            "discard/old",
            b"discard",
        );
        fixture.project(false);
        let workspace = fixture
            .engine
            .create_workspace(&fixture.repo.main_branch_id, "admin")
            .unwrap();
        fixture
            .engine
            .stage_bytes(&workspace.workspace_id, "unrelated/new", b"staged")
            .unwrap();
        stage_operation(
            &mut fixture,
            &workspace,
            NamespaceOperationKind::DeleteTree,
            "discard",
            None,
        );
        stage_operation(
            &mut fixture,
            &workspace,
            NamespaceOperationKind::Rename,
            "dir",
            Some("discard"),
        );
        stage_operation(
            &mut fixture,
            &workspace,
            NamespaceOperationKind::Rename,
            "discard/b",
            Some("discard/final"),
        );
        stage_operation(
            &mut fixture,
            &workspace,
            NamespaceOperationKind::Mkdir,
            "dir",
            None,
        );
        fixture
            .engine
            .commit_workspace(request(&workspace))
            .unwrap();
        assert_eq!(
            current(&fixture, "discard/final").file_id,
            fixture.retained.file_id
        );
        assert_eq!(current(&fixture, "dir").kind, NamespaceEntryKind::Directory);
        assert!(
            fixture
                .engine
                .namespace_tree(&fixture.repo.main_branch_id)
                .unwrap()
                .get("unrelated/new")
                .is_some()
        );
        assert!(
            fixture
                .engine
                .namespace_tree(&fixture.repo.main_branch_id)
                .unwrap()
                .get("discard/old")
                .is_none()
        );
        assert_hidden_absent(&fixture);
        let next = fixture
            .engine
            .create_workspace(&fixture.repo.main_branch_id, "admin")
            .unwrap();
        stage_operation(
            &mut fixture,
            &next,
            NamespaceOperationKind::Rename,
            "discard",
            Some("deleted"),
        );
        stage_operation(
            &mut fixture,
            &next,
            NamespaceOperationKind::DeleteTree,
            "deleted",
            None,
        );
        fixture.engine.commit_workspace(request(&next)).unwrap();
        let head = &fixture.engine.state.branches[&fixture.repo.main_branch_id].head_commit_id;
        let deleted = &fixture.engine.state.commits[head]
            .namespace_deletions
            .as_ref()
            .unwrap()
            .paths;
        assert!(deleted.iter().any(|entry| entry.path == "deleted/final"));
        assert!(deleted.iter().all(|entry| entry.path != "deleted/a"));
        assert_hidden_absent(&fixture);
    }
}

#[test]
fn membership_registry_current_rename_conflicts_and_overlapping_upload_preserve_staging() {
    let mut fixture = Fixture::new(true);
    save(
        &mut fixture.engine,
        &fixture.repo.main_branch_id,
        "occupied",
        b"collision",
    );
    fixture.project(false);
    let before = canonical_json(fixture.engine.state()).unwrap();
    for (source, destination, expected) in [
        ("dir/a", "missing", ErrorCode::EntryNotFound),
        ("dir/b", "occupied", ErrorCode::NameCollision),
        ("dir", "dir/inside", ErrorCode::InvalidMove),
    ] {
        assert_eq!(
            fixture
                .engine
                .file_namespace(
                    &fixture.repo.repository_id,
                    NamespaceOperation::parse(
                        NamespaceOperationKind::Rename,
                        source,
                        Some(destination)
                    )
                    .unwrap(),
                    "admin",
                    op()
                )
                .unwrap_err()
                .code(),
            expected
        );
        assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    }
    let workspace = fixture
        .engine
        .create_workspace(&fixture.repo.main_branch_id, "admin")
        .unwrap();
    let staged = fixture
        .engine
        .stage_bytes(&workspace.workspace_id, "dir/b", b"must remain staged")
        .unwrap();
    stage_operation(
        &mut fixture,
        &workspace,
        NamespaceOperationKind::Rename,
        "dir",
        Some("moved"),
    );
    let before = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(
        fixture
            .engine
            .commit_workspace(request(&workspace))
            .unwrap_err()
            .code(),
        ErrorCode::WorkspaceDependencyBusy
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    assert_eq!(
        std::fs::read(&staged.source_path).unwrap(),
        b"must remain staged"
    );
}

#[test]
fn membership_registry_current_rename_recovery_revalidates_deserialized_next_state() {
    let mut fixture = Fixture::new(true);
    fixture.project(false);
    let before = Arc::clone(&fixture.engine.state);
    let before_snapshot = fixture.engine.snapshot.clone();
    let source = fixture.engine.current_namespaces.clone();
    rename(&mut fixture, "dir", "recovered");
    let state: State =
        json_codec::from_slice(&canonical_json(fixture.engine.state()).unwrap()).unwrap();
    let snapshot = fixture.engine.snapshot.clone().unwrap();
    let moved = current(&fixture, "recovered/b");
    fixture.engine.state = before;
    fixture.engine.snapshot = before_snapshot.clone();
    fixture.engine.current_namespaces = source;
    let mut forged = state.clone();
    let head = forged.branches[&fixture.repo.main_branch_id]
        .head_commit_id
        .clone();
    let mut commit = forged.commits[&head].clone();
    let mut hidden = fixture.removed.clone();
    hidden.relocate(name_policy::PortablePath::parse("recovered/a").unwrap());
    commit.entries.push(hidden);
    commit
        .entries
        .sort_by(|left, right| left.lookup_key().cmp(right.lookup_key()));
    forged.commits.insert(head, commit);
    assert_eq!(
        fixture
            .engine
            .install_recovered_namespace_state(forged, snapshot.clone())
            .unwrap_err()
            .code(),
        ErrorCode::WorkspaceChanged
    );
    assert_eq!(fixture.engine.snapshot, before_snapshot);
    fixture
        .engine
        .install_recovered_namespace_state(state, snapshot)
        .unwrap();
    assert_eq!(current(&fixture, "recovered/b"), moved);
    assert_hidden_absent(&fixture);
    save(
        &mut fixture.engine,
        &fixture.repo.main_branch_id,
        "recovered/b",
        b"after replay",
    );
    assert_eq!(current(&fixture, "recovered/b").file_id, moved.file_id);
    assert_hidden_absent(&fixture);
}

#[test]
fn membership_registry_current_staged_rename_conflicts_with_a_changed_source() {
    let mut fixture = Fixture::new(true);
    fixture.project(false);
    let workspace = fixture
        .engine
        .create_workspace(&fixture.repo.main_branch_id, "admin")
        .unwrap();
    stage_operation(
        &mut fixture,
        &workspace,
        NamespaceOperationKind::Rename,
        "dir/b",
        Some("staged"),
    );
    rename(&mut fixture, "dir/b", "other");
    let before = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(
        fixture
            .engine
            .commit_workspace(request(&workspace))
            .unwrap_err()
            .code(),
        ErrorCode::NamespaceConflict
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    assert_eq!(current(&fixture, "other").file_id, fixture.retained.file_id);
}

#[test]
fn membership_registry_current_rename_admission_rejects_copy_and_hidden_version_revival() {
    let mut fixture = Fixture::new(false);
    fixture.project(false);
    let view = fixture.engine.current_namespaces.as_ref().unwrap();
    let mut entries = fixture
        .engine
        .list_tree(&fixture.repo.main_branch_id)
        .unwrap();
    let mut duplicate = fixture.retained.clone();
    duplicate.relocate(name_policy::PortablePath::parse("copied").unwrap());
    entries.push(duplicate);
    entries.sort_by(|a, b| a.lookup_key().cmp(b.lookup_key()));
    assert_eq!(
        view.validate_draft(&fixture.repo.main_branch_id, &entries)
            .unwrap_err()
            .code(),
        ErrorCode::WorkspaceChanged
    );
    let mut entries = fixture
        .engine
        .list_tree(&fixture.repo.main_branch_id)
        .unwrap();
    let moved = entries
        .iter_mut()
        .find(|entry| entry.entry_id == fixture.retained.entry_id)
        .unwrap();
    moved.relocate(name_policy::PortablePath::parse("moved").unwrap());
    moved.version_id = fixture.removed.version_id.clone();
    entries.sort_by(|a, b| a.lookup_key().cmp(b.lookup_key()));
    assert_eq!(
        view.validate_draft(&fixture.repo.main_branch_id, &entries)
            .unwrap_err()
            .code(),
        ErrorCode::WorkspaceChanged
    );
}
