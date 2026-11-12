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
    assert_eq!(entries[0].path.as_str(), FILE);
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
    assert_eq!(naming_entries[0].path.as_str(), FILE);
    assert_ne!(naming_entries[0].file_id, entries[0].file_id);
    let mut content = engine.open_content(&naming_entries[0].version_id).unwrap();
    let mut bytes = Vec::new();
    content.file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, PAYLOADS[0].as_bytes());
    let naming_git = engine
        .directory_git_status(&naming.repository_id, "")
        .unwrap();
    assert!(naming_git.initialized);
    assert_eq!(naming_git.repository_id, naming.repository_id);
    assert_eq!(naming_git.directory_id, naming.repository_id);
    assert_eq!(naming_git.path, "");
    assert!(naming_git.directory_branch_id.is_some());
    assert!(naming_git.head_tree_id.is_some());
    assert!(
        !naming_git.staged && !naming_git.unstaged && !naming_git.needs_restage,
        "batch naming must start from the same committed source without Web initialization"
    );
    drop(content);
    drop(engine);
    let error = seed_files(&temporary).unwrap_err();
    assert!(error.to_string().contains("fresh disposable pool"));
    let engine = Engine::open(&temporary.path().join("pool")).unwrap();
    assert_eq!(engine.file_history(&entries[0].file_id).unwrap().len(), 2);
    assert_eq!(
        engine.directory_git_status(&naming_git.repository_id, "").unwrap(),
        naming_git
    );
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
