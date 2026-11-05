use super::*;
use storage_engine::{
    BranchKind, BranchState, Config,
    history_window::{
        HistoryCollection, HistoryFileId, HistoryPageLimit, HistoryWindowCancellation,
        HistoryWindowPage, HistoryWindowRequest,
    },
};

fn pool() -> tempfile::TempDir {
    let temporary = tempfile::tempdir().unwrap();
    let disks = ["P01", "B01", "C01"].map(|slot| temporary.path().join(slot));
    drop(
        Engine::initialize(
            &temporary.path().join("pool"),
            Config::development(disks).unwrap(),
        )
        .unwrap(),
    );
    temporary
}

#[test]
fn test_only_fixture_persists_two_versions_and_refuses_reuse() {
    let temporary = pool();
    seed_files(&temporary).unwrap();
    let engine = Engine::open(&temporary.path().join("pool")).unwrap();
    let repository = engine
        .state()
        .repositories
        .values()
        .find(|repository| repository.name == "browser-fixtures")
        .unwrap();
    assert_eq!(repository.name, "browser-fixtures");
    let root_git = engine
        .directory_git_status(&repository.repository_id, "")
        .unwrap();
    assert!(root_git.initialized && root_git.head_tree_id.is_some());
    assert!(
        !root_git.staged && !root_git.needs_restage,
        "the notice lifecycle must start from a committed, clean independent repository: {root_git:?}"
    );
    let entries = engine.list_tree(&repository.main_branch_id).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path, FILE);
    let versions = engine.file_history(&entries[0].file_id).unwrap();
    assert_eq!(versions.len(), 2);
    for (version, expected) in versions.iter().zip(PAYLOADS) {
        let mut content = engine.open_content(&version.version_id).unwrap();
        let mut bytes = Vec::new();
        content.file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, expected.as_bytes());
    }
    let request = HistoryWindowRequest {
        file_id: HistoryFileId::parse(&entries[0].file_id).unwrap(),
        collection: HistoryCollection::Nodes,
        location: None,
        history_root: None,
        cursor: None,
        anchor_node_id: None,
        limit: HistoryPageLimit::new(32).unwrap(),
    };
    let HistoryWindowPage::Nodes {
        identity, nodes, ..
    } = engine
        .capture_history_window(request.clone(), HistoryWindowCancellation::new())
        .unwrap()
        .page()
        .unwrap()
    else {
        panic!("node request must produce a node page");
    };
    assert_eq!(nodes.len(), 2);
    let directory = storage_engine::directory_history::DirectoryHistoryRequest::parse(
        &repository.repository_id,
        &repository.repository_id,
        storage_engine::directory_history::DirectoryHistoryCollection::Nodes,
        std::collections::BTreeMap::new(),
    )
    .unwrap();
    let directory = engine
        .capture_directory_history(directory, HistoryWindowCancellation::new())
        .unwrap()
        .page()
        .unwrap();
    assert!(
        directory.nodes.len() >= 3,
        "the root scope and both writes have explicit user directory commits"
    );
    let ids = directory
        .nodes
        .iter()
        .map(|node| node.tree_id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let edges = directory
        .nodes
        .iter()
        .filter_map(|node| {
            node.parent_tree_id
                .as_deref()
                .map(|parent| (parent, node.tree_id.as_str()))
        })
        .collect::<Vec<_>>();
    assert!(
        edges.len() >= 2,
        "the browser fixture must exercise actual SVG parent edges"
    );
    assert!(edges.iter().all(|(parent, _)| ids.contains(parent)));
    for node in &directory.nodes {
        assert_eq!(
            node.parent_tree_id,
            user_commit_parent(&engine, &node.tree_id)
        );
    }
    let HistoryWindowPage::Branches { branches, page, .. } = engine
        .capture_history_window(
            HistoryWindowRequest {
                collection: HistoryCollection::Branches,
                history_root: Some(identity.history_root),
                ..request
            },
            HistoryWindowCancellation::new(),
        )
        .unwrap()
        .page()
        .unwrap()
    else {
        panic!("branch request must produce a branch page");
    };
    assert_eq!(branches.len(), 2);
    assert!(page.previous_cursor.is_none());
    assert!(page.next_cursor.is_none());
    for name in ["main", FILE_BRANCH] {
        let branch = branches.iter().find(|branch| branch.name == name).unwrap();
        assert_eq!(branch.state, BranchState::Active);
        assert_eq!(branch.branch_kind, BranchKind::File);
        assert!(!branch.read_only);
        assert!(branch.alias_of.is_none());
        let head = branch.node_id.as_ref().unwrap();
        assert!(nodes.iter().any(|node| node.node_id == *head));
        assert_eq!(head.as_str(), entries[0].version_id);
    }
    let naming = engine
        .state()
        .repositories
        .values()
        .find(|repository| repository.name == NAMING_REPOSITORY)
        .unwrap();
    assert_ne!(naming.repository_id, repository.repository_id);
    let naming_entries = engine.list_tree(&naming.main_branch_id).unwrap();
    assert_eq!(naming_entries.len(), 1);
    assert_eq!(naming_entries[0].path, FILE);
    assert_ne!(naming_entries[0].file_id, entries[0].file_id);
    let mut content = engine.open_content(&naming_entries[0].version_id).unwrap();
    let mut bytes = Vec::new();
    content.file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, PAYLOADS[0].as_bytes());
    let naming_git = engine
        .directory_git_status(&naming.repository_id, "")
        .unwrap();
    assert!(
        !naming_git.initialized,
        "the manual initialization browser flow must start uninitialized"
    );
    assert!(naming_git.directory_branch_id.is_none());
    assert!(naming_git.head_tree_id.is_none());
    assert_real_readonly_directory_history(&engine);
    drop(content);
    drop(engine);
    let error = seed_files(&temporary).unwrap_err();
    assert!(error.to_string().contains("fresh disposable pool"));
    let engine = Engine::open(&temporary.path().join("pool")).unwrap();
    assert_eq!(engine.file_history(&entries[0].file_id).unwrap().len(), 2);
}

fn assert_real_readonly_directory_history(engine: &Engine) {
    use storage_engine::directory_history::{DirectoryHistoryCollection, DirectoryHistoryRequest};
    let repository = engine
        .state()
        .repositories
        .values()
        .find(|repository| repository.name == DIRECTORY_REPOSITORY)
        .unwrap();
    let entries = engine.list_tree(&repository.main_branch_id).unwrap();
    let child = entries
        .iter()
        .find(|entry| entry.path() == DIRECTORY_PATH)
        .unwrap();
    let history = |collection| {
        engine
            .capture_directory_history(
                DirectoryHistoryRequest::parse(
                    &repository.repository_id,
                    &child.entry_id,
                    collection,
                    std::collections::BTreeMap::new(),
                )
                .unwrap(),
                HistoryWindowCancellation::new(),
            )
            .unwrap()
            .page()
            .unwrap()
    };
    let nodes = history(DirectoryHistoryCollection::Nodes).nodes;
    assert!(
        nodes.len() >= 3,
        "the real preview child needs three explicit user directory commits"
    );
    let ids = nodes
        .iter()
        .map(|node| node.tree_id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert!(
        nodes
            .iter()
            .filter(|node| node.parent_tree_id.is_some())
            .count()
            >= 2,
        "the selected read-only child must retain actual parent connectors"
    );
    for node in &nodes {
        assert_eq!(
            node.parent_tree_id,
            user_commit_parent(engine, &node.tree_id)
        );
        if let Some(parent) = &node.parent_tree_id {
            assert!(ids.contains(parent.as_str()));
        }
    }
    let branches = history(DirectoryHistoryCollection::Branches).branches;
    assert_eq!(branches.len(), 2);
    assert_eq!(branches[0].committed_tree_id, branches[1].committed_tree_id);
    let committed = branches[0].committed_tree_id.as_ref().unwrap();
    assert!(ids.contains(committed.as_str()));
    let historical = nodes
        .iter()
        .find(|node| &node.tree_id != committed)
        .unwrap();
    let frozen = engine
        .preview_directory_branch(
            &repository.repository_id,
            &historical.directory_branch_id,
            &historical.tree_id,
        )
        .unwrap();
    assert_eq!(frozen.branch().head_tree_id, historical.tree_id);
    assert_eq!(frozen.path(), DIRECTORY_PATH);
    assert_eq!(
        frozen.branch().entries,
        engine.state().directory_trees[&historical.tree_id].entries
    );
    let parent = engine
        .state()
        .directory_branches
        .values()
        .find(|branch| {
            branch.repository_id == repository.repository_id && branch.name == DIRECTORY_PREVIEW
        })
        .unwrap();
    assert_eq!(parent.directory_id, repository.repository_id);
    let preview = engine
        .preview_directory_branch(
            &repository.repository_id,
            &parent.directory_branch_id,
            &parent.head_tree_id,
        )
        .unwrap();
    let index = preview
        .branch()
        .entries
        .iter()
        .position(|entry| entry.entry_id == child.entry_id)
        .unwrap();
    assert_eq!(preview.path(), "");
    assert_eq!(preview.entry(index).unwrap().path(), DIRECTORY_PATH);
    assert_eq!(preview.source_path(DIRECTORY_PATH).unwrap(), DIRECTORY_PATH);
}

fn user_commit_parent(engine: &Engine, tree_id: &str) -> Option<String> {
    let state = engine.state();
    state.directory_trees[tree_id]
        .parent_tree_id
        .as_ref()
        .and_then(|parent| {
            storage_engine::directory_staging::committed_directory_ancestor(
                state,
                &state.directory_trees[parent],
            )
        })
        .map(|parent| parent.tree_id.clone())
}

#[test]
fn fixture_requires_explicit_initialization() {
    let temporary = tempfile::tempdir().unwrap();
    assert!(seed_files(&temporary).is_err());
    assert!(!temporary.path().join("pool/config.json").exists());
}

#[test]
fn management_fixture_publishes_explicit_authority_that_survives_reopen() {
    let temporary = tempfile::tempdir().unwrap();
    let password = "fixture-admin-password";
    initialize_management(&temporary, password).unwrap();
    let pool = temporary.path().join("pool");
    assert!(!pool.join("auth.json").exists());
    assert!(pool.join("management-authority.json").is_file());
    assert!(pool.join("management/management-auth.json").is_file());
    let (backend, mut auth) =
        storage_server::management::open_service(&pool, &pool.join("management")).unwrap();
    assert!(auth.login(PRINCIPAL, password).is_ok());
    drop(backend);
    drop(auth);
    seed_files(&temporary).unwrap();
    assert!(initialize_management(&temporary, password).is_err());
}
