use super::*;
use std::io::Read;

fn fixture() -> (tempfile::TempDir, Engine, Repository) {
    let root = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &root.path().join("system"),
        Config::development([
            root.path().join("p"),
            root.path().join("b"),
            root.path().join("c"),
        ]),
    )
    .unwrap();
    let repo = engine
        .create_repository("branch regression", "P01", 3, "admin", &new_id("op"))
        .unwrap();
    (root, engine, repo)
}
fn save(engine: &mut Engine, branch: &str, bytes: &[u8]) -> OperationResult {
    let workspace = engine.create_workspace(branch, "admin").unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "report.txt", bytes)
        .unwrap();
    commit(engine, workspace)
}
fn commit(engine: &mut Engine, workspace: Workspace) -> OperationResult {
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: new_id("op"),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "Regression fixture".into(),
            request_digest: None,
        })
        .unwrap()
}
fn file(engine: &Engine, branch: &str) -> Entry {
    engine
        .list_tree(branch)
        .unwrap()
        .into_iter()
        .find(|entry| entry.kind == NamespaceEntryKind::File)
        .unwrap()
}

#[test]
fn inactive_selected_owner_cannot_receive_main_workspace_writes() {
    for state in [BranchState::Archived, BranchState::Merged] {
        let (_root, mut engine, repo) = fixture();
        save(&mut engine, &repo.main_branch_id, b"original");
        let original = file(&engine, &repo.main_branch_id);
        let branch = engine
            .create_file_branch(
                &original.file_id,
                &original.version_id,
                "selected draft",
                "admin",
                &new_id("op"),
            )
            .unwrap()
            .branch_id
            .unwrap();
        engine
            .select_file_branch(&original.file_id, &branch, "admin", &new_id("op"))
            .unwrap();
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        std::sync::Arc::make_mut(&mut engine.state)
            .branches
            .get_mut(&branch)
            .unwrap()
            .state = state;
        let before = engine.state.branches[&branch].head_commit_id.clone();
        assert_eq!(
            engine
                .stage_bytes(&workspace.workspace_id, "report.txt", b"must not publish")
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::BranchNotActive
        );
        let current = engine.state.commits[&workspace.base_head].clone();
        let mut changed = current.clone();
        changed.commit_id = new_id("commit");
        changed
            .entries
            .iter_mut()
            .find(|entry| entry.kind == NamespaceEntryKind::File)
            .unwrap()
            .version_id = new_id("ver");
        std::sync::Arc::make_mut(&mut engine.state)
            .commits
            .insert(changed.commit_id.clone(), changed.clone());
        assert_eq!(
            synchronize_directory_edits(
                std::sync::Arc::make_mut(&mut engine.state),
                &changed.commit_id,
                &["report.txt".into()],
                "admin",
                &new_id("op"),
            )
            .unwrap_err()
            .code(),
            storage_contract::ErrorCode::BranchNotActive
        );
        assert_eq!(engine.state.branches[&branch].head_commit_id, before);
    }
}

#[test]
fn inherited_range_fences_selection_and_preserves_parent_bytes() {
    let (_root, mut engine, repo) = fixture();
    save(&mut engine, &repo.main_branch_id, b"SOURCE_BYTES");
    let original = file(&engine, &repo.main_branch_id);
    let pending = engine
        .create_file_branch(
            &original.file_id,
            &original.version_id,
            "pending",
            "admin",
            &new_id("op"),
        )
        .unwrap();
    let pending = pending.result["branch_id"].as_str().unwrap().to_owned();
    let materialized_main = engine.state().file_selections[&original.file_id].clone();
    engine
        .select_file_branch(&original.file_id, &pending, "admin", &new_id("op"))
        .unwrap();
    let old_workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .select_file_branch(
            &original.file_id,
            &materialized_main,
            "admin",
            &new_id("op"),
        )
        .unwrap();
    let error = engine
        .stage_range(
            &old_workspace.workspace_id,
            &new_id("entry"),
            "report.txt",
            0,
            b"X",
            None,
        )
        .unwrap_err();
    assert_eq!(
        error.code(),
        storage_contract::ErrorCode::FileBranchConflict
    );
    assert!(
        engine
            .workspace(&old_workspace.workspace_id)
            .unwrap()
            .files
            .is_empty()
    );
    assert_eq!(engine.file_history(&original.file_id).unwrap().len(), 1);

    engine
        .select_file_branch(&original.file_id, &pending, "admin", &new_id("op"))
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    let staged = engine
        .stage_range(
            &workspace.workspace_id,
            &new_id("entry"),
            "report.txt",
            0,
            b"X",
            None,
        )
        .unwrap();
    let mut content = engine
        .open_workspace_content(&workspace.workspace_id, &staged.item_id, "admin")
        .unwrap();
    let mut bytes = Vec::new();
    content.file.read_to_end(&mut bytes).unwrap();
    assert_eq!(
        bytes, b"XOURCE_BYTES",
        "an inherited range edits the visible parent without changing that immutable version"
    );
    drop(content);
    commit(&mut engine, workspace);
    let written = file(&engine, &repo.main_branch_id);
    assert_eq!(
        engine.state().versions[&written.version_id].source_version_ids,
        vec![original.version_id]
    );
    assert_eq!(written.size, 12);
}

#[test]
fn selected_file_materialization_replaces_a_split_main_placeholder() {
    let (root, mut engine, repo) = fixture();
    let original = save(&mut engine, &repo.main_branch_id, b"original");
    let pending = engine
        .create_branch(
            original.commit_id.as_ref().unwrap(),
            "pending full",
            "admin",
            &new_id("op"),
        )
        .unwrap();
    let plan = engine
        .create_split_plan(&repo.history_tree_id, &pending.head_commit_id, "admin")
        .unwrap();
    let split = engine
        .commit_plan(&plan.plan_id, "admin", &new_id("op"))
        .unwrap();
    let repository = engine
        .state()
        .repositories
        .values()
        .find(|repo| Some(&repo.history_tree_id) == split.history_tree_id.as_ref())
        .unwrap()
        .clone();
    let placeholder = file(&engine, &repository.main_branch_id);
    assert!(!placeholder.version_id.is_empty());
    assert_eq!(placeholder.size, 8);
    let source = engine
        .pending_file_source(&repository.main_branch_id, &placeholder.file_id)
        .unwrap()
        .to_owned();
    let created = engine
        .create_file_branch(
            &placeholder.file_id,
            &source,
            "actual",
            "admin",
            &new_id("op"),
        )
        .unwrap();
    let branch = created.result["branch_id"].as_str().unwrap().to_owned();
    engine
        .select_file_branch(&placeholder.file_id, &branch, "admin", &new_id("op"))
        .unwrap();
    save(&mut engine, &branch, b"written branch bytes");
    let written = file(&engine, &branch);
    let selected = file(&engine, &repository.main_branch_id);
    assert_eq!(selected.version_id, written.version_id);
    assert_eq!(selected.size, 20);
    assert!(
        engine
            .pending_file_source(&repository.main_branch_id, &placeholder.file_id)
            .is_none()
    );
    assert!(
        !engine.state().branches[&repository.main_branch_id]
            .pending_file_versions
            .contains_key(&placeholder.file_id)
    );
    drop(engine);
    let engine = Engine::open(&root.path().join("system")).unwrap();
    assert_eq!(
        file(&engine, &repository.main_branch_id).version_id,
        written.version_id
    );
    assert!(
        engine
            .pending_file_source(&repository.main_branch_id, &placeholder.file_id)
            .is_none()
    );
}

#[test]
fn historical_range_edits_keep_the_explicit_old_snapshot_as_their_base() {
    let (_root, mut engine, repo) = fixture();
    let original = save(&mut engine, &repo.main_branch_id, b"old body");
    let source = original.commit_id.unwrap();
    save(&mut engine, &repo.main_branch_id, b"current body");
    let workspace = engine
        .create_historical_workspace(&repo.main_branch_id, &source, "admin")
        .unwrap();
    engine
        .stage_range(
            &workspace.workspace_id,
            &new_id("entry"),
            "report.txt",
            0,
            b"X",
            None,
        )
        .unwrap();
    let plan = engine
        .create_historical_save_plan(
            &repo.history_tree_id,
            HistoricalSaveOptions {
                source_commit_id: source,
                name: "edited old snapshot".into(),
                workspace_id: Some(workspace.workspace_id),
                primary_slot_id: None,
                target_copies: None,
            },
            "admin",
        )
        .unwrap();
    let saved = engine
        .commit_historical_save_plan(&plan.plan_id, "admin", &new_id("op"))
        .unwrap();
    let written = file(&engine, saved.branch_id.as_ref().unwrap());
    let mut content = engine.open_content(&written.version_id).unwrap();
    let mut bytes = Vec::new();
    content.file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"Xld body");
    let unchanged = file(&engine, &repo.main_branch_id);
    assert_ne!(written.version_id, unchanged.version_id);
    assert_eq!(unchanged.size, 12);
}
