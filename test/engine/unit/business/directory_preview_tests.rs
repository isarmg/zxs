use super::*;

fn fixture() -> (tempfile::TempDir, Engine, Repository, String) {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temp.path().join("system"),
        Config::development([
            temp.path().join("primary"),
            temp.path().join("backup"),
            temp.path().join("third"),
        ])
        .unwrap(),
    )
    .unwrap();
    let repo = engine
        .create_disk_root("P01", "admin", OperationId::generate().unwrap())
        .unwrap();
    save(&mut engine, &repo, "Café/file.txt", b"before");
    save(&mut engine, &repo, "outside.txt", b"outside");
    let branch = engine
        .create_directory_branches(
            &repo.repository_id,
            Some(&name_policy::LookupPath::parse("Café").unwrap()),
            &BranchName::parse("draft").unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap()
        .result["directory_branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    (temp, engine, repo, branch)
}
fn save(engine: &mut Engine, repo: &Repository, path: &str, bytes: &[u8]) {
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, path, bytes)
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "preview test".into(),
            request_digest: None,
        })
        .unwrap();
}
fn set(
    engine: &mut Engine,
    repo: &Repository,
    branch: &str,
    tree: &str,
    head: &str,
    operation: OperationId,
) -> Result<OperationResult> {
    engine.set_directory_default(DirectoryDefaultSelection {
        repository_id: &repo.repository_id,
        directory_branch_id: branch,
        expected_tree_id: tree,
        expected_head: head,
        principal: "admin",
        operation_id: operation,
    })
}
#[test]
fn browsing_frozen_directory_is_read_only_and_default_publishes_only_explicitly() {
    let (temp, mut engine, repo, branch) = fixture();
    let tree = engine.state.directory_branches[&branch]
        .head_tree_id
        .clone();
    let frozen = engine.state.directory_branches[&branch].entries[0]
        .version_id
        .clone();
    save(&mut engine, &repo, "Café/file.txt", b"later");
    let head = engine.state.branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let state = storage_contract::canonical_json(engine.state()).unwrap();
    let preview = engine
        .preview_directory_branch(&repo.repository_id, &branch, &tree)
        .unwrap();
    assert_eq!(preview.path(), "Café");
    assert_eq!(preview.source_path("CAFÉ").unwrap(), "Café");
    assert_eq!(preview.source_path("CAFÉ/sub").unwrap(), "Café/sub");
    assert_eq!(preview.entry(0).unwrap().version_id, frozen);
    assert!(preview.source_path("outside").is_err());
    assert_eq!(
        storage_contract::canonical_json(engine.state()).unwrap(),
        state
    );
    assert_ne!(
        engine
            .list_tree(&repo.main_branch_id)
            .unwrap()
            .iter()
            .find(|entry| entry.path() == "Café/file.txt")
            .unwrap()
            .version_id,
        frozen
    );
    let operation = OperationId::generate().unwrap();
    let published = set(&mut engine, &repo, &branch, &tree, &head, operation).unwrap();
    assert_eq!(
        set(&mut engine, &repo, &branch, &tree, &head, operation).unwrap(),
        published
    );
    assert_eq!(
        engine.state.directory_selections[&engine.state.directory_branches[&branch].directory_id],
        branch
    );
    assert!(
        engine
            .list_tree(&repo.main_branch_id)
            .unwrap()
            .iter()
            .any(|entry| entry.path() == "outside.txt")
    );
    drop(preview);
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(
        engine
            .list_tree(&repo.main_branch_id)
            .unwrap()
            .iter()
            .find(|entry| entry.path() == "Café/file.txt")
            .unwrap()
            .version_id,
        frozen
    );
    assert_eq!(
        set(&mut engine, &repo, &branch, &tree, &head, operation).unwrap(),
        published
    );
}
#[test]
fn stale_tree_and_namespace_are_rejected_without_default_publication() {
    let (_temp, mut engine, repo, branch) = fixture();
    let tree = engine.state.directory_branches[&branch]
        .head_tree_id
        .clone();
    let head = engine.state.branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let selected = engine.state.directory_selections.clone();
    assert_eq!(
        set(
            &mut engine,
            &repo,
            &branch,
            "stale-tree",
            &head,
            OperationId::generate().unwrap()
        )
        .unwrap_err()
        .code(),
        ErrorCode::ListingChanged
    );
    assert_eq!(
        set(
            &mut engine,
            &repo,
            &branch,
            &tree,
            "stale-head",
            OperationId::generate().unwrap()
        )
        .unwrap_err()
        .code(),
        ErrorCode::ListingChanged
    );
    assert_eq!(engine.state.directory_selections, selected);
    assert_eq!(
        engine.state.branches[&repo.main_branch_id].head_commit_id,
        head
    );
    assert!(
        engine
            .preview_directory_branch("other", &branch, &tree)
            .is_err()
    );
    assert!(
        engine
            .preview_directory_branch(&repo.repository_id, &branch, "stale-tree")
            .is_err()
    );
}
#[test]
fn failed_default_journal_prepare_keeps_old_selection_after_reopen() {
    let (temp, mut engine, repo, branch) = fixture();
    let tree = engine.state.directory_branches[&branch]
        .head_tree_id
        .clone();
    let head = engine.state.branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let selected = engine.state.directory_selections.clone();
    let member = engine.members()[0].disk_id.clone();
    engine.journals.get_mut(&member).unwrap().fault = Some(crate::journal::JournalFault::Sync);
    let operation = OperationId::generate().unwrap();
    assert!(set(&mut engine, &repo, &branch, &tree, &head, operation).is_err());
    assert_eq!(engine.state.directory_selections, selected);
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(engine.state.directory_selections, selected);
    assert_eq!(
        engine.state.branches[&repo.main_branch_id].head_commit_id,
        head
    );
    set(&mut engine, &repo, &branch, &tree, &head, operation).unwrap();
}
#[test]
fn file_default_is_pinned_to_viewed_node_and_namespace_and_replays_after_reopen() {
    let (temp, mut engine, repo, _) = fixture();
    let entry = engine
        .list_tree(&repo.main_branch_id)
        .unwrap()
        .into_iter()
        .find(|entry| entry.path() == "Café/file.txt")
        .unwrap();
    let branch = engine
        .create_file_branch_from_node(
            &entry.file_id,
            &entry.version_id,
            &entry.version_id,
            &BranchName::parse("file draft").unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap()
        .branch_id
        .unwrap();
    let node = engine.state.branches[&branch].file_node_id.clone().unwrap();
    let head = engine.state.branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let operation = OperationId::generate().unwrap();
    let request = |expected_node, expected_head, operation_id| FileDefaultSelection {
        file_id: &entry.file_id,
        branch_id: &branch,
        repository_id: &repo.repository_id,
        path: &entry.path,
        expected_node_id: expected_node,
        expected_head,
        principal: "admin",
        operation_id,
    };
    assert_eq!(
        engine
            .set_file_default(request("stale", &head, OperationId::generate().unwrap()))
            .unwrap_err()
            .code(),
        ErrorCode::ListingChanged
    );
    assert_eq!(
        engine
            .set_file_default(request(&node, "stale", OperationId::generate().unwrap()))
            .unwrap_err()
            .code(),
        ErrorCode::ListingChanged
    );
    let versions = engine.state.versions.len();
    let result = engine
        .set_file_default(request(&node, &head, operation))
        .unwrap();
    assert_eq!(engine.state.versions.len(), versions);
    assert_eq!(
        engine
            .file_context(&repo.repository_id, entry.path())
            .unwrap()
            .branch_id,
        branch
    );
    assert_eq!(
        engine
            .set_file_default(request(&node, &head, operation))
            .unwrap(),
        result
    );
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(
        engine
            .file_context(&repo.repository_id, entry.path())
            .unwrap()
            .branch_id,
        branch
    );
    assert_eq!(
        engine
            .set_file_default(request(&node, &head, operation))
            .unwrap(),
        result
    );
}
