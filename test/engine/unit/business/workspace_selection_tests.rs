//! An ordinary workspace is fenced by its real namespace, FILE selection and
//! source occurrence. Folder path equality is not parent identity.
use super::*;

fn fixture() -> (tempfile::TempDir, Engine, Repository) {
    let temporary = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temporary.path().join("system"),
        Config::development(crate::test_disks([temporary.path().join("primary")])).unwrap(),
    )
    .unwrap();
    let repo = engine.create_disk_root("P01", "admin", OperationId::generate().unwrap()).unwrap();
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
            operation_id: storage_contract::OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "selection fixture".into(),
            request_digest: None,
        })
        .unwrap();
    (temporary, engine, repo)
}

#[test]
fn missing_ordinary_workspace_references_are_never_treated_as_an_empty_folder() {
    let (_temporary, mut engine, repo) = fixture();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    for missing in ["branch", "base", "current"] {
        let mut state = engine.state().clone();
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
                    .head_commit_id = new_id("commit").unwrap();
                "current namespace missing"
            }
        };
        let error = validate_ordinary_parent_key(&state, &workspace, "new.txt").unwrap_err();
        assert!(error.to_string().contains(expected), "{missing}: {error}");
    }
}


fn request(workspace: &Workspace, message: &str) -> CommitRequest {
    CommitRequest {
        workspace_id: workspace.workspace_id.clone(),
        operation_id: OperationId::generate().unwrap(),
        principal: workspace.principal.clone(),
        expected_head: workspace.base_head.clone(),
        fencing_token: workspace.fencing_token,
        message: message.to_owned(),
        request_digest: None,
    }
}

#[test]
fn new_ordinary_parent_after_open_is_not_adopted_by_a_stale_writer() {
    let (temporary, mut engine, repo) = fixture();
    let original = engine.create_workspace(&repo.main_branch_id, "admin").unwrap();
    let concurrent = engine.create_workspace(&repo.main_branch_id, "admin").unwrap();
    engine.stage_bytes(&concurrent.workspace_id, "new/marker.txt", b"other").unwrap();
    let decision = request(&concurrent, "create the actual different parent");
    let result = engine.commit_workspace(decision.clone()).unwrap();
    let parent = engine.namespace_tree(&repo.main_branch_id).unwrap().get("new").unwrap().clone();
    assert_eq!(parent.kind, NamespaceEntryKind::Directory);
    let stale = name_policy::PortablePath::parse("NEW/child.txt").unwrap();
    let untouched = name_policy::PortablePath::parse("Folderish/file.txt").unwrap();
    assert_eq!(
        engine.validate_workspace_file_selection(&original, &stale).unwrap_err().code(),
        storage_contract::ErrorCode::FileBranchConflict,
    );
    engine.validate_workspace_file_selection(&original, &untouched).unwrap();
    assert!(engine.stage_bytes(&original.workspace_id, stale.as_str(), b"refused").is_err());
    assert!(engine.workspace_view(&original.workspace_id).unwrap().files.is_empty());
    drop(engine);
    let mut opened = Engine::open(&temporary.path().join("system")).unwrap();
    assert_eq!(opened.commit_workspace(decision).unwrap(), result);
    let saved = opened.workspace_view(&original.workspace_id).unwrap();
    assert!(opened.validate_workspace_file_selection(&saved, &stale).is_err());
    opened.validate_workspace_file_selection(&saved, &untouched).unwrap();
    assert_eq!(opened.namespace_tree(&repo.main_branch_id).unwrap().get("new"), Some(&parent));
}

#[test]
fn same_spelling_replacement_does_not_match_a_frozen_parent_or_its_descendants() {
    let (_temporary, engine, repo) = fixture();
    let original = engine.namespace_tree(&repo.main_branch_id).unwrap();
    let mut replaced = original.entries().to_vec();
    let row = replaced.iter_mut().find(|entry| entry.lookup_key() == "folder/child").unwrap();
    assert_eq!(row.kind, NamespaceEntryKind::Directory);
    row.entry_id = new_id("entry").unwrap();
    let bad = namespace::NamespaceTree::from_published_entries(&replaced);
    for path in ["folder/child", "folder/child/file.txt", "folder/child/new/item"] {
        assert!(file_location::require_parents(original, bad, path).is_err());
    }
    file_location::require_parents(original, bad, "folderish/file.txt").unwrap();
    // Unrelated new descendants of the same unchanged parent may still be created.
    file_location::require_parents(original, bad, "folder/not-yet-created/new.txt").unwrap();
}

#[test]
fn genuine_file_branch_round_trip_stales_only_its_selected_occurrence() {
    let (_temporary, mut engine, repo) = fixture();
    let path = name_policy::PortablePath::parse("Folder/Child/file.txt").unwrap();
    let other_path = name_policy::PortablePath::parse("Folderish/file.txt").unwrap();
    let held = engine.namespace_tree(&repo.main_branch_id).unwrap().get(path.as_str()).unwrap().clone();
    let main = engine.file_context(&repo.repository_id, path.as_str()).unwrap();
    let fork = engine.create_file_branch(
        &held.file_id,
        &held.version_id,
        &storage_contract::BranchName::parse("explicit-alternate").unwrap(),
        "admin",
        OperationId::generate().unwrap(),
    ).unwrap().branch_id.unwrap();
    let stale = engine.create_workspace(&repo.main_branch_id, "admin").unwrap();
    for branch in [&fork, &main.branch_id] {
        engine.select_file_branch_at(
            &repo.repository_id, &path, &held.file_id, branch, "admin",
            OperationId::generate().unwrap(),
        ).unwrap();
    }
    assert_eq!(
        engine.validate_workspace_file_selection(&stale, &path).unwrap_err().code(),
        storage_contract::ErrorCode::FileBranchConflict,
    );
    engine.validate_workspace_file_selection(&stale, &other_path).unwrap();
    let current = engine.namespace_tree(&repo.main_branch_id).unwrap().get(path.as_str()).unwrap();
    assert_eq!(current.entry_id, held.entry_id);
    assert_eq!(current.file_id, held.file_id);
    assert_eq!(current.version_id, held.version_id);
}
