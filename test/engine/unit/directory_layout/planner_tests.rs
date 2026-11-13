use super::*;
use storage_contract::{
    Branch, BranchKind, BranchState, Commit, ContentState, DirectoryTree, HistoryState,
    HistoryTree, Repository,
    directory_git::{DirectoryGitBranch, DirectoryGitRepository},
};

fn id(prefix: &str, n: u64) -> String {
    format!("{prefix}_{n:032x}")
}
fn root() -> String {
    id("repo", 1)
}
fn history() -> String {
    id("history", 1)
}
fn fixture_state() -> State {
    let mut state = State::default();
    state.repositories.insert(
        root(),
        Repository {
            repository_id: root(),
            name: "root".into(),
            history_tree_id: history(),
            main_branch_id: id("br", 1),
            primary_slot_id: "P01".into(),
            target_copies: 1,
            name_policy: name_policy::NAME_POLICY.into(),
            unicode_version: name_policy::UNICODE_VERSION.into(),
            smb_enabled: false,
            smb_binding_generation: 0,
            file_manager_root: true,
        },
    );
    state.histories.insert(
        history(),
        HistoryTree {
            history_tree_id: history(),
            repository_id: root(),
            root_commit_id: id("commit", 1),
            main_branch_id: Some(id("br", 1)),
            generation: 1,
            state: HistoryState::Active,
            provenance: None,
        },
    );
    let current = retained_commit(&mut state, 100_000, vec![]);
    state.branches.insert(
        id("br", 1),
        Branch {
            branch_id: id("br", 1),
            history_tree_id: history(),
            name: "namespace".into(),
            head_commit_id: current.clone(),
            fork_commit_id: current,
            branch_kind: BranchKind::Full,
            file_id: None,
            file_node_id: None,
            pending_file_versions: BTreeMap::new(),
            generation: 1,
            state: BranchState::Active,
        },
    );
    state
}
fn directory(n: u64, path: &str) -> Entry {
    Entry {
        entry_id: id("entry", n),
        path: PortablePath::parse(path).unwrap(),
        kind: NamespaceEntryKind::Directory,
        file_id: String::new(),
        version_id: String::new(),
        size: 0,
    }
}
fn file(state: &mut State, n: u64, path: &str) -> Entry {
    let version = FileVersion {
        history_tree_id: history(),
        version_id: id("ver", n),
        file_id: id("file", n),
        creation_primary_slot_id: "P01".into(),
        logical_size: 1,
        version_sequence: n,
        content_hash: "0".repeat(64).parse().unwrap(),
        checksum_chunk_bytes: 4096,
        checksum_manifest_hash: "0".repeat(64),
        creation_name: path.rsplit('/').next().unwrap().into(),
        storage_id: storage_contract::VersionStorageId::from_version(
            id("ver", n).parse().unwrap(),
        ),
        created_at: "2026-11-06T12:00:00Z".into(),
        source_version_ids: vec![],
        created_as_full_checkpoint: false,
    };
    state
        .content_states
        .insert(version.version_id.clone(), ContentState::Available);
    state
        .versions
        .insert(version.version_id.clone(), version.clone());
    Entry {
        entry_id: id("entry", 100 + n),
        path: PortablePath::parse(path).unwrap(),
        kind: NamespaceEntryKind::File,
        file_id: version.file_id,
        version_id: version.version_id,
        size: 1,
    }
}
fn branch(
    state: &mut State,
    n: u64,
    owner: &str,
    path: &str,
    entries: Vec<Entry>,
    main: bool,
) -> String {
    let branch_id = id("dbr", n);
    let tree_id = id("tree", n);
    state.directory_branches.insert(
        branch_id.clone(),
        DirectoryBranch {
            directory_branch_id: branch_id.clone(),
            repository_id: root(),
            directory_id: owner.into(),
            path: path.into(),
            name: format!("branch-{n}"),
            parent_branch_id: None,
            head_tree_id: tree_id.clone(),
            commit_id: id("commit", n),
            entries: entries.clone(),
            file_branches: BTreeMap::new(),
            file_contexts: BTreeMap::new(),
            inherited_files: BTreeMap::new(),
            directory_refs: BTreeMap::new(),
            materialized: true,
            sequence: n,
        },
    );
    state.directory_trees.insert(
        tree_id.clone(),
        DirectoryTree {
            tree_id: tree_id.clone(),
            directory_branch_id: branch_id.clone(),
            parent_tree_id: None,
            commit_id: id("commit", n),
            path: path.into(),
            entries,
            file_branches: BTreeMap::new(),
            file_contexts: BTreeMap::new(),
            inherited_files: BTreeMap::new(),
            directory_refs: BTreeMap::new(),
            sequence: n,
        },
    );
    if main {
        state.directory_git_repositories.insert(
            owner.into(),
            DirectoryGitRepository {
                deletions: std::collections::BTreeMap::new(),
                repository_id: root(),
                directory_id: owner.into(),
                initialized_sequence: 1,
                main_branch_id: branch_id.clone(),
                current_branch_id: branch_id.clone(),
                branches: BTreeMap::new(),
            },
        );
    }
    state
        .directory_git_repositories
        .get_mut(owner)
        .unwrap()
        .branches
        .insert(
            branch_id.clone(),
            DirectoryGitBranch {
                head_tree_id: None,
                index_tree_id: tree_id,
                index_generation: 1,
                index_boundaries: BTreeMap::new(),
                index_children: BTreeMap::new(),
            },
        );
    branch_id
}

#[test]
fn main_saved_workspace_and_full_identity_flat_names() {
    let mut state = fixture_state();
    let a = file(&mut state, 1, "left/report.txt");
    let b = file(&mut state, 2, "right/report.txt");
    let c = file(&mut state, 3, "later/report.txt");
    let d = file(&mut state, 4, "deleted/report.txt");
    let main = branch(
        &mut state,
        1,
        &root(),
        "",
        vec![
            directory(1, "left"),
            directory(2, "right"),
            directory(3, "empty"),
            a.clone(),
            b.clone(),
        ],
        true,
    );
    let mut alias = a.clone();
    alias.relocate(PortablePath::parse("topic/report.txt").unwrap());
    let topic = branch(
        &mut state,
        2,
        &root(),
        "",
        vec![
            directory(4, "topic"),
            directory(5, "later"),
            directory(6, "deleted"),
            alias,
            c.clone(),
            d.clone(),
        ],
        false,
    );
    state
        .directory_git_repositories
        .get_mut(&root())
        .unwrap()
        .current_branch_id = topic;
    state
        .content_states
        .insert(d.version_id.clone(), ContentState::Purged);
    let before = state.clone();
    let plan = plan(&state, &root(), ".zxs-extra").unwrap();
    assert_eq!(plan.state_sequence(), state.sequence);
    assert_eq!(
        plan.repositories()[root().as_str()]
            .main
            .directory_branch_id,
        main
    );
    assert!(
        plan.repositories()[root().as_str()]
            .directories
            .contains_key("empty")
    );
    assert_eq!(plan.files().len(), 4);
    assert_eq!(
        plan.files()[&key(&state, &a).unwrap()].path.as_str(),
        "left/report.txt"
    );
    assert_eq!(
        plan.files()[&key(&state, &b).unwrap()].path.as_str(),
        "right/report.txt"
    );
    for e in [&c, &d] {
        let layout = &plan.files()[&key(&state, e).unwrap()];
        assert_eq!(
            layout.path.as_str(),
            format!(".zxs-extra/f-{}-{}", history(), e.file_id)
        );
        assert!(layout.main.is_none());
        assert_eq!(layout.versions.len(), 1);
    }
    assert_eq!(state, before);
}

#[test]
fn recursive_children_use_their_own_main_and_not_parent_pins_or_scope_paths() {
    let mut state = fixture_state();
    let child = directory(10, "old-child");
    let grand = directory(11, "current-child/grand");
    let child_file = file(&mut state, 1, "current-child/own");
    let grand_file = file(&mut state, 2, "somewhere/grand-file");
    let extra = file(&mut state, 3, "current-child/extra");
    branch(&mut state, 1, &root(), "", vec![child.clone()], true);
    branch(
        &mut state,
        2,
        &child.entry_id,
        "current-child",
        vec![child_file.clone(), grand.clone()],
        true,
    );
    branch(
        &mut state,
        3,
        &child.entry_id,
        "current-child",
        vec![extra.clone()],
        false,
    );
    branch(
        &mut state,
        4,
        &grand.entry_id,
        "somewhere",
        vec![grand_file.clone()],
        true,
    );
    let plan = plan(&state, &root(), ".zxs-extra").unwrap();
    assert_eq!(
        plan.repositories()[child.entry_id.as_str()]
            .root
            .as_ref()
            .unwrap()
            .as_str(),
        "old-child"
    );
    assert_eq!(
        plan.repositories()[grand.entry_id.as_str()]
            .root
            .as_ref()
            .unwrap()
            .as_str(),
        "old-child/grand"
    );
    assert_eq!(
        plan.files()[&key(&state, &child_file).unwrap()]
            .path
            .as_str(),
        "own"
    );
    assert_eq!(
        plan.files()[&key(&state, &grand_file).unwrap()]
            .path
            .as_str(),
        "grand-file"
    );
    assert_eq!(
        plan.files()[&key(&state, &extra).unwrap()].directory_id,
        child.entry_id
    );
    assert!(
        plan.files()[&key(&state, &extra).unwrap()]
            .path
            .as_str()
            .starts_with(".zxs-extra/")
    );
    assert_eq!(plan.repositories()[root().as_str()].children.len(), 1);
}

#[test]
fn parent_historical_ordinary_content_is_owned_by_its_stable_child_boundary() {
    let mut state = fixture_state();
    let child = directory(10, "child");
    let a = file(&mut state, 1, "child/a");
    branch(
        &mut state,
        1,
        &root(),
        "",
        vec![child.clone(), a.clone()],
        true,
    );
    branch(
        &mut state,
        2,
        &child.entry_id,
        "child",
        vec![a.clone()],
        true,
    );
    let plan = plan(&state, &root(), ".zxs-extra").unwrap();
    assert_eq!(plan.files().len(), 1);
    assert_eq!(
        plan.files()[&key(&state, &a).unwrap()].directory_id,
        child.entry_id
    );
}

#[test]
fn same_main_aliases_are_not_silently_discarded() {
    let mut state = fixture_state();
    let a = file(&mut state, 1, "a");
    let mut b = a.clone();
    b.relocate(PortablePath::parse("b").unwrap());
    branch(&mut state, 1, &root(), "", vec![a, b], true);
    assert!(
        matches!(plan(&state, &root(), ".zxs-extra"), Err(LayoutError::MultipleMainOccurrences { paths, .. }) if paths.len() == 2)
    );
}

#[test]
fn cross_repository_history_without_a_current_home_is_an_explicit_conflict() {
    let mut state = fixture_state();
    let child = directory(10, "child");
    let a = file(&mut state, 1, "a");
    let mut alias = a.clone();
    alias.relocate(PortablePath::parse("child/a").unwrap());
    branch(&mut state, 1, &root(), "", vec![child.clone()], true);
    branch(&mut state, 2, &child.entry_id, "child", vec![], true);
    branch(&mut state, 3, &root(), "", vec![a], false);
    branch(&mut state, 4, &child.entry_id, "child", vec![alias], false);
    assert!(
        matches!(plan(&state, &root(), ".zxs-extra"), Err(LayoutError::FileOwnerAmbiguous { directory_ids, .. }) if directory_ids.len() == 2)
    );
}

#[test]
fn detached_child_and_unproven_retained_file_cannot_disappear() {
    let mut state = fixture_state();
    branch(&mut state, 1, &root(), "", vec![], true);
    let retained = file(&mut state, 1, "deleted-before-initialization");
    assert!(matches!(plan(&state, &root(), ".zxs-extra"),
        Err(LayoutError::FileOwnerMissing { file_id, .. }) if file_id == retained.file_id));
    let detached = id("entry", 2);
    branch(&mut state, 2, &detached, "deleted-child", vec![], true);
    let (registered, roots) = forest_roots(&state, &root()).unwrap();
    assert_eq!(
        registered,
        BTreeSet::from([root().as_str(), detached.as_str()])
    );
    assert_eq!(roots.iter().copied().collect::<BTreeSet<_>>(), registered);
    for directory in &roots {
        assert!(
            matches!(plan_root(&state, &root(), directory, ".zxs-extra"),
            Err(LayoutError::FileOwnerMissing { file_id, .. }) if file_id == retained.file_id)
        );
    }

    // A retained occurrence supplies file ownership; an absent historical mount
    // never supplies a current physical parent for the independent repository.
    retained_commit(&mut state, 90, vec![retained.clone()]);
    let root_plan = plan(&state, &root(), ".zxs-extra").unwrap();
    assert_eq!(root_plan.repositories().len(), 1);
    assert!(root_plan.repositories().contains_key(root().as_str()));
    assert_eq!(root_plan.files().len(), 1);
    let file = &root_plan.files()[&key(&state, &retained).unwrap()];
    assert_eq!(file.directory_id, root());
    assert!(file.main.is_none());
    assert!(root_plan.outside_files().is_empty());

    let child_plan = plan_root(&state, &root(), &detached, ".zxs-extra").unwrap();
    assert_eq!(child_plan.repositories().len(), 1);
    assert!(child_plan.repositories()[detached.as_str()].root.is_none());
    assert!(child_plan.files().is_empty());
    assert_eq!(child_plan.outside_files().len(), 1);
    assert_eq!(
        child_plan.outside_files()[&key(&state, &retained).unwrap()].owners,
        BTreeSet::from([LayoutOwner::Repository(root().as_str())])
    );
}

#[test]
fn main_mount_graph_rejects_multiple_parents_and_disconnected_cycles() {
    let mut state = fixture_state();
    let a = directory(10, "a");
    let b = directory(11, "b");
    let shared = directory(12, "shared");
    branch(&mut state, 1, &root(), "", vec![a.clone(), b.clone()], true);
    branch(&mut state, 2, &a.entry_id, "", vec![shared.clone()], true);
    branch(&mut state, 3, &b.entry_id, "", vec![shared.clone()], true);
    branch(&mut state, 4, &shared.entry_id, "", vec![], true);
    assert!(matches!(
        plan(&state, &root(), ".zxs-extra"),
        Err(LayoutError::MultipleRepositoryMounts { .. })
    ));
    let mut state = fixture_state();
    branch(&mut state, 1, &root(), "", vec![], true);
    branch(&mut state, 2, &a.entry_id, "", vec![b.clone()], true);
    branch(&mut state, 3, &b.entry_id, "", vec![a.clone()], true);
    assert!(matches!(
        plan(&state, &root(), ".zxs-extra"),
        Err(LayoutError::RepositoryCycle { .. })
    ));
}

#[test]
fn inherited_different_identity_is_not_a_duplicate_body_or_a_merged_file() {
    let mut state = fixture_state();
    let mut a = file(&mut state, 1, "a");
    a.file_id = id("file", 2);
    branch(&mut state, 1, &root(), "", vec![a], true);
    assert!(matches!(
        plan(&state, &root(), ".zxs-extra"),
        Err(LayoutError::UnwrittenFileIdentity { .. })
    ));
}

#[test]
fn metadata_budgets_stop_without_partial_results() {
    let mut budget = Budget {
        steps: MAX_STEPS,
        bytes: 0,
    };
    assert!(matches!(budget.charge(0), Err(LayoutError::ResourceLimit)));
    let mut budget = Budget {
        steps: 0,
        bytes: MAX_PATH_BYTES,
    };
    assert!(matches!(budget.charge(1), Err(LayoutError::ResourceLimit)));
}

#[test]
fn file_container_cannot_be_another_target_ancestor() {
    let mut state = fixture_state();
    let a = file(&mut state, 1, "a");
    let nested = file(&mut state, 2, "a/container.json");
    branch(&mut state, 1, &root(), "", vec![a, nested], true);
    assert!(matches!(plan(&state, &root(), ".zxs-extra"),
        Err(LayoutError::MainStructureConflict { parent, .. }) if parent == "a"));
}

#[test]
fn missing_structure_and_file_parent_of_child_mount_are_rejected() {
    let mut state = fixture_state();
    let a = file(&mut state, 1, "missing/a");
    branch(&mut state, 1, &root(), "", vec![a], true);
    assert!(matches!(
        plan(&state, &root(), ".zxs-extra"),
        Err(LayoutError::MainStructureConflict { .. })
    ));
    let mut state = fixture_state();
    let a = file(&mut state, 1, "a");
    let child = directory(10, "a/child");
    branch(&mut state, 1, &root(), "", vec![a, child.clone()], true);
    branch(&mut state, 2, &child.entry_id, "a/child", vec![], true);
    assert!(matches!(
        plan(&state, &root(), ".zxs-extra"),
        Err(LayoutError::MainStructureConflict { .. })
    ));
}

#[test]
fn main_parent_spelling_preserves_logical_file_identity_and_leaf() {
    for (directory_name, referenced_parent) in [("ẞ", "SS"), ("Straße", "STRASSE")] {
        let mut state = fixture_state();
        let logical = format!("{referenced_parent}/REPORT.txt");
        let entry = file(&mut state, 1, &logical);
        let main = branch(
            &mut state,
            1,
            &root(),
            "",
            vec![entry.clone(), directory(1, directory_name)],
            true,
        );
        state
            .directory_branches
            .get_mut(&main)
            .unwrap()
            .file_contexts
            .insert(
                entry.lookup_key().into(),
                FileContext {
                    node_id: Some(id("node", 1)),
                    selection_generation: 7,
                    branch_id: id("fbr", 1),
                    inherited_from: None,
                },
            );
        let before = state.clone();
        let plan = plan(&state, &root(), ".zxs-extra").unwrap();
        let projected = &plan.files()[&key(&state, &entry).unwrap()];
        assert_eq!(
            projected.path.as_str(),
            format!("{directory_name}/REPORT.txt")
        );
        let original = &state.directory_branches[&main].entries[0];
        let context = &state.directory_branches[&main].file_contexts[entry.lookup_key()];
        assert!(std::ptr::eq(
            projected.main.as_ref().unwrap().entry,
            original
        ));
        assert!(std::ptr::eq(
            projected.main.as_ref().unwrap().context.unwrap(),
            context
        ));
        assert_eq!(original.path(), logical);
        assert_eq!(state, before);
    }
}

#[test]
fn main_parent_spelling_keeps_child_mounts_and_scopes_independent() {
    let mut state = fixture_state();
    let child = directory(10, "STRASSE/Child");
    let grand = directory(11, "SS/ÜBER/Grand");
    let a = file(&mut state, 1, "SS/ÜBER/report.txt");
    let b = file(&mut state, 2, "old-scope/leaf");
    branch(
        &mut state,
        1,
        &root(),
        "",
        vec![child.clone(), directory(1, "Straße")],
        true,
    );
    branch(
        &mut state,
        2,
        &child.entry_id,
        "ẞ",
        vec![
            grand.clone(),
            a.clone(),
            directory(2, "ss/Über"),
            directory(3, "SS/ÜBER/Empty"),
        ],
        true,
    );
    branch(
        &mut state,
        3,
        &grand.entry_id,
        "old-scope",
        vec![b.clone()],
        true,
    );
    let before = state.clone();
    let plan = plan(&state, &root(), ".zxs-extra").unwrap();
    let child_layout = &plan.repositories()[child.entry_id.as_str()];
    assert_eq!(child_layout.root.as_ref().unwrap().as_str(), "Straße/Child");
    assert!(child_layout.directories.contains_key("Über/Empty"));
    assert_eq!(
        child_layout.children[grand.entry_id.as_str()].as_str(),
        "Über/Grand"
    );
    assert_eq!(
        plan.repositories()[grand.entry_id.as_str()]
            .root
            .as_ref()
            .unwrap()
            .as_str(),
        "Straße/Child/Über/Grand"
    );
    assert_eq!(
        plan.files()[&key(&state, &a).unwrap()].path.as_str(),
        "Über/report.txt"
    );
    assert_eq!(
        plan.files()[&key(&state, &b).unwrap()].path.as_str(),
        "leaf"
    );
    assert_eq!(state, before);
}

#[test]
fn flat_directory_file_collision_still_uses_fixed_keys_inside_child_scope() {
    for nested in [false, true] {
        let mut state = fixture_state();
        if nested {
            let child = directory(10, "child");
            let a = file(&mut state, 1, "old-scope/STRASSE");
            branch(&mut state, 1, &root(), "", vec![child.clone()], true);
            branch(&mut state, 2, &child.entry_id, "old-scope", vec![a], true);
        } else {
            let a = file(&mut state, 1, "STRASSE");
            branch(&mut state, 1, &root(), "", vec![a], true);
        }
        assert!(matches!(
            plan(&state, &root(), "Straße"),
            Err(LayoutError::ExtraDirectoryCollision { .. })
        ));
    }
}

#[test]
fn main_parent_spelling_keeps_projected_container_internal_path_budget() {
    for suffix in [
        "container.json",
        "versions/v000001_20261106T120000Z_00000000.txt",
    ] {
        let mut state = fixture_state();
        let leaf = "a".repeat(200 - "Straße/".encode_utf16().count() - 1 - suffix.len());
        let logical = format!("Straße/{leaf}");
        assert!(PortablePath::parse(&format!("{logical}/{suffix}")).is_ok());
        let a = file(&mut state, 1, &logical);
        branch(
            &mut state,
            1,
            &root(),
            "",
            vec![directory(1, "STRASSE"), a],
            true,
        );
        let expected = format!("STRASSE/{leaf}/{suffix}");
        assert!(
            matches!(plan(&state, &root(), ".zxs-extra"), Err(LayoutError::PhysicalPath { path, .. }) if path == expected)
        );
    }
}

fn retained_commit(state: &mut State, n: u64, entries: Vec<Entry>) -> String {
    let commit_id = id("commit", n);
    state.commits.insert(
        commit_id.clone(),
        Commit {
            commit_id: commit_id.clone(),
            history_tree_id: history(),
            kind: "workspace".into(),
            parent_commit_ids: Vec::new(),
            tree_id: id("namespace", n),
            sequence: n,
            operation_id: id("op", n).parse().unwrap(),
            author: "fixture".into(),
            message: "retained ancestor evidence".into(),
            entries,
            namespace_deletions: None,
            pending_file_versions: BTreeMap::new(),
            provenance: None,
        },
    );
    commit_id
}

#[test]
fn initialized_subdirectory_partitions_uninitialized_outer_history() {
    let mut state = fixture_state();
    let child = directory(10, "child");
    let current = file(&mut state, 1, "child/current");
    let deleted = file(&mut state, 2, "child/deleted");
    let outside = file(&mut state, 3, "outside");
    let gone = file(&mut state, 4, "gone");
    branch(
        &mut state,
        1,
        &child.entry_id,
        "child",
        vec![current.clone()],
        true,
    );
    retained_commit(
        &mut state,
        90,
        vec![
            child.clone(),
            current.clone(),
            deleted.clone(),
            outside.clone(),
            gone.clone(),
        ],
    );
    state
        .content_states
        .insert(gone.version_id.clone(), ContentState::Purged);
    let planned = plan_root(&state, &root(), &child.entry_id, ".zxs-extra").unwrap();
    assert_eq!(planned.repository_id(), root());
    assert_eq!(planned.root_directory_id(), child.entry_id);
    assert!(
        planned.repositories()[child.entry_id.as_str()]
            .root
            .is_none()
    );
    assert_eq!(planned.files().len(), 2);
    assert_eq!(
        planned.files()[&key(&state, &current).unwrap()]
            .path
            .as_str(),
        "current"
    );
    assert!(
        planned.files()[&key(&state, &deleted).unwrap()]
            .main
            .is_none()
    );
    assert_eq!(planned.outside_files().len(), 2);
    for entry in [&outside, &gone] {
        let exterior = &planned.outside_files()[&key(&state, entry).unwrap()];
        assert_eq!(exterior.owners, BTreeSet::from([LayoutOwner::Unmanaged]));
        assert_eq!(exterior.versions.len(), 1);
    }
    assert!(matches!(
        plan(&state, &root(), ".zxs-extra"),
        Err(LayoutError::RootNotInitialized { .. })
    ));
}

#[test]
fn sibling_initialized_root_is_exterior_to_selected_main_tree() {
    let mut state = fixture_state();
    let a = directory(10, "a");
    let b = directory(11, "b");
    let child = directory(12, "a/child");
    let own = file(&mut state, 1, "a/own");
    let nested = file(&mut state, 2, "a/child/nested");
    let exterior = file(&mut state, 3, "b/file");
    let unrelated_extra = directory(13, "b/.ZXS-EXTRA");
    branch(
        &mut state,
        1,
        &a.entry_id,
        "a",
        vec![child.clone(), own.clone(), nested.clone()],
        true,
    );
    branch(
        &mut state,
        2,
        &child.entry_id,
        "a/child",
        vec![nested.clone()],
        true,
    );
    branch(
        &mut state,
        3,
        &b.entry_id,
        "b",
        vec![exterior.clone(), unrelated_extra.clone()],
        true,
    );
    retained_commit(
        &mut state,
        90,
        vec![
            a.clone(),
            b.clone(),
            child.clone(),
            own.clone(),
            nested.clone(),
            exterior.clone(),
            unrelated_extra,
        ],
    );
    let planned = plan_root(&state, &root(), &a.entry_id, ".zxs-extra").unwrap();
    assert_eq!(planned.repositories().len(), 2);
    assert_eq!(
        planned.repositories()[child.entry_id.as_str()]
            .root
            .as_ref()
            .unwrap()
            .as_str(),
        "child"
    );
    assert_eq!(
        planned.files()[&key(&state, &nested).unwrap()].directory_id,
        child.entry_id
    );
    let outside = &planned.outside_files()[&key(&state, &exterior).unwrap()];
    assert_eq!(
        outside.owners,
        BTreeSet::from([LayoutOwner::Repository(b.entry_id.as_str())])
    );
}

#[test]
fn later_child_initialization_reinterprets_all_retained_boundaries() {
    let mut state = fixture_state();
    let child = directory(10, "child");
    let content = file(&mut state, 1, "child/content");
    branch(
        &mut state,
        1,
        &root(),
        "",
        vec![child.clone(), content.clone()],
        true,
    );
    retained_commit(&mut state, 90, vec![child.clone(), content.clone()]);
    branch(&mut state, 2, &child.entry_id, "child", vec![], true);
    branch(
        &mut state,
        3,
        &child.entry_id,
        "child",
        vec![content.clone()],
        false,
    );
    state
        .directory_git_repositories
        .get_mut(&child.entry_id)
        .unwrap()
        .initialized_sequence = 100;
    let planned = plan_root(&state, &root(), &child.entry_id, ".zxs-extra").unwrap();
    assert_eq!(planned.files().len(), 1);
    assert_eq!(
        planned.files()[&key(&state, &content).unwrap()].directory_id,
        child.entry_id
    );
    assert!(planned.outside_files().is_empty());
}

#[test]
fn cross_root_alias_and_missing_occurrence_are_not_unmanaged() {
    let mut state = fixture_state();
    let child = directory(10, "child");
    let content = file(&mut state, 1, "child/content");
    let mut alias = content.clone();
    alias.relocate(PortablePath::parse("outside").unwrap());
    branch(&mut state, 1, &child.entry_id, "child", vec![], true);
    retained_commit(&mut state, 90, vec![child.clone(), content, alias]);
    assert!(matches!(
        plan_root(&state, &root(), &child.entry_id, ".zxs-extra"),
        Err(LayoutError::FileOwnerCrossesRoot {
            unmanaged: true,
            ..
        })
    ));

    let mut state = fixture_state();
    branch(&mut state, 1, &child.entry_id, "child", vec![], true);
    let orphan = file(&mut state, 1, "never-retained");
    retained_commit(&mut state, 90, vec![child.clone()]);
    assert!(
        matches!(plan_root(&state, &root(), &child.entry_id, ".zxs-extra"),
        Err(LayoutError::FileOwnerMissing { file_id, .. }) if file_id == orphan.file_id)
    );
}

#[test]
fn omitted_child_and_ordinary_scope_require_exact_historical_evidence() {
    let mut state = fixture_state();
    let a = directory(10, "a");
    let child = directory(11, "a/child");
    let content = file(&mut state, 1, "a/child/content");
    branch(&mut state, 1, &a.entry_id, "a", vec![], true);
    branch(
        &mut state,
        2,
        &child.entry_id,
        "a/child",
        vec![content.clone()],
        true,
    );
    retained_commit(
        &mut state,
        90,
        vec![a.clone(), child.clone(), content.clone()],
    );
    let (registered, roots) = forest_roots(&state, &root()).unwrap();
    assert_eq!(
        registered,
        BTreeSet::from([a.entry_id.as_str(), child.entry_id.as_str()])
    );
    assert_eq!(roots.iter().copied().collect::<BTreeSet<_>>(), registered);
    let parent = plan_root(&state, &root(), &a.entry_id, ".zxs-extra").unwrap();
    assert_eq!(parent.repositories().len(), 1);
    assert!(
        parent.repositories()[a.entry_id.as_str()]
            .children
            .is_empty()
    );
    assert!(parent.files().is_empty());
    assert_eq!(parent.outside_files().len(), 1);
    assert_eq!(
        parent.outside_files()[&key(&state, &content).unwrap()].owners,
        BTreeSet::from([LayoutOwner::Repository(child.entry_id.as_str())])
    );
    let child_plan = plan_root(&state, &root(), &child.entry_id, ".zxs-extra").unwrap();
    assert_eq!(child_plan.repositories().len(), 1);
    assert!(
        child_plan.repositories()[child.entry_id.as_str()]
            .root
            .is_none()
    );
    assert_eq!(child_plan.files().len(), 1);
    let child_file = &child_plan.files()[&key(&state, &content).unwrap()];
    assert_eq!(child_file.directory_id, child.entry_id);
    assert_eq!(child_file.path.as_str(), "content");
    assert!(child_plan.outside_files().is_empty());

    let mut state = fixture_state();
    let ordinary = directory(12, "a/ordinary");
    let content = file(&mut state, 1, "a/ordinary/content");
    branch(&mut state, 1, &a.entry_id, "a", vec![], true);
    branch(
        &mut state,
        90,
        &ordinary.entry_id,
        "a/ordinary",
        vec![content.clone()],
        true,
    );
    state.directory_git_repositories.remove(&ordinary.entry_id);
    assert!(matches!(
        plan_root(&state, &root(), &a.entry_id, ".zxs-extra"),
        Err(LayoutError::SnapshotScopeUnproven { .. })
    ));
    retained_commit(&mut state, 90, vec![a.clone(), ordinary, content.clone()]);
    let planned = plan_root(&state, &root(), &a.entry_id, ".zxs-extra").unwrap();
    assert_eq!(
        planned.files()[&key(&state, &content).unwrap()].directory_id,
        a.entry_id
    );
    assert!(
        planned.files()[&key(&state, &content).unwrap()]
            .main
            .is_none()
    );
    drop(planned);
    let tree_id = id("tree", 90);
    let mut incomplete = state.directory_trees[&tree_id].clone();
    incomplete.entries[0].relocate(PortablePath::parse("a/ordinary/missing/content").unwrap());
    state.directory_trees.insert(tree_id, incomplete);
    assert!(matches!(
        plan_root(&state, &root(), &a.entry_id, ".zxs-extra"),
        Err(LayoutError::SnapshotStructureUnproven { .. })
    ));
}

fn current_namespace(state: &mut State, n: u64, entries: Vec<Entry>) {
    let head = retained_commit(state, n, entries);
    state.branches.get_mut(&id("br", 1)).unwrap().head_commit_id = head;
}

#[test]
fn moving_an_unmanaged_file_into_the_unique_stable_main_selects_that_home() {
    let mut state = fixture_state();
    let a = directory(10, "a");
    let old = file(&mut state, 1, "loose");
    retained_commit(&mut state, 80, vec![old.clone()]);
    let mut moved = old.clone();
    moved.relocate(PortablePath::parse("a/saved").unwrap());
    branch(&mut state, 1, &a.entry_id, "a", vec![moved.clone()], true);
    current_namespace(&mut state, 90, vec![a.clone(), moved.clone()]);
    let planned = plan_root(&state, &root(), &a.entry_id, ".zxs-extra").unwrap();
    let file = &planned.files()[&key(&state, &old).unwrap()];
    assert_eq!(file.directory_id, a.entry_id);
    assert_eq!(file.path.as_str(), "saved");
    assert_eq!(file.main.as_ref().unwrap().entry.file_id, old.file_id);
    assert_eq!(file.versions.len(), 1);
    assert!(planned.outside_files().is_empty());
}

#[test]
fn moving_out_without_a_user_commit_overrides_old_internal_checkpoints() {
    let mut state = fixture_state();
    let a = directory(10, "a");
    let old = file(&mut state, 1, "a/saved");
    let main = branch(&mut state, 1, &a.entry_id, "a", vec![old.clone()], true);
    let old_tree = state.directory_branches[&main].head_tree_id.clone();
    let mut saved = state.directory_trees[&old_tree].clone();
    saved.tree_id = id("tree", 2);
    saved.parent_tree_id = Some(old_tree);
    saved.entries.clear();
    saved.sequence = 2;
    let next = saved.tree_id.clone();
    state.directory_trees.insert(next.clone(), saved);
    let working = state.directory_branches.get_mut(&main).unwrap();
    working.entries.clear();
    working.head_tree_id = next;
    working.sequence = 2;
    let mut moved = old.clone();
    moved.relocate(PortablePath::parse("loose").unwrap());
    current_namespace(&mut state, 90, vec![a.clone(), moved]);
    let planned = plan_root(&state, &root(), &a.entry_id, ".zxs-extra").unwrap();
    assert!(planned.files().is_empty());
    let outside = &planned.outside_files()[&key(&state, &old).unwrap()];
    assert_eq!(outside.owners, BTreeSet::from([LayoutOwner::Unmanaged]));
    assert_eq!(outside.versions.len(), 1);
    assert!(
        state.directory_git_repositories[&a.entry_id].branches[&main]
            .head_tree_id
            .is_none()
    );
}

#[test]
fn different_branch_paths_of_one_file_do_not_override_its_unique_main() {
    let mut state = fixture_state();
    let a = directory(10, "a");
    let original = file(&mut state, 1, "a/main.txt");
    let main = branch(
        &mut state,
        1,
        &a.entry_id,
        "a",
        vec![original.clone()],
        true,
    );
    let mut topic_entry = original.clone();
    topic_entry.relocate(PortablePath::parse("a/topic/renamed.txt").unwrap());
    let ordinary = directory(11, "a/topic");
    let topic = branch(
        &mut state,
        2,
        &a.entry_id,
        "a",
        vec![ordinary.clone(), topic_entry.clone()],
        false,
    );
    state
        .directory_git_repositories
        .get_mut(&a.entry_id)
        .unwrap()
        .current_branch_id = topic;
    current_namespace(&mut state, 90, vec![a.clone(), ordinary, topic_entry]);
    let planned = plan_root(&state, &root(), &a.entry_id, ".zxs-extra").unwrap();
    assert_eq!(planned.files().len(), 1);
    let file = &planned.files()[&key(&state, &original).unwrap()];
    assert_eq!(file.path.as_str(), "main.txt");
    assert_eq!(file.main.as_ref().unwrap().directory_branch_id, main);
    assert_eq!(file.versions.len(), 1);
}

#[test]
fn current_main_and_explicit_unmanaged_alias_conflict_without_fallback() {
    let mut state = fixture_state();
    let a = directory(10, "a");
    let content = file(&mut state, 1, "a/saved");
    branch(&mut state, 1, &a.entry_id, "a", vec![content.clone()], true);
    let mut alias = content.clone();
    alias.relocate(PortablePath::parse("loose").unwrap());
    current_namespace(&mut state, 90, vec![a.clone(), content, alias]);
    assert!(
        matches!(plan_root(&state, &root(), &a.entry_id, ".zxs-extra"),
        Err(LayoutError::CurrentMainUnmanagedConflict { unmanaged_paths, .. }) if unmanaged_paths == vec!["loose"])
    );
}

#[test]
fn absent_exact_outer_current_namespace_is_not_treated_as_unmanaged() {
    let mut state = fixture_state();
    let a = directory(10, "a");
    branch(&mut state, 1, &a.entry_id, "a", vec![], true);
    state.branches.remove(&id("br", 1));
    assert!(matches!(
        plan_root(&state, &root(), &a.entry_id, ".zxs-extra"),
        Err(LayoutError::CurrentNamespaceUnproven {
            commit_id: None,
            ..
        })
    ));
}
