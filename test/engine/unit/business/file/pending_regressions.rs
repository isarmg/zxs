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
        ])
        .unwrap(),
    )
    .unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("branch regression").unwrap(),
            "P01",
            3,
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
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
            operation_id: storage_contract::OperationId::generate().unwrap(),
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

fn select_pending(engine: &mut Engine, source: &Entry) -> String {
    let branch = engine
        .create_file_branch(
            &source.file_id,
            &source.version_id,
            &storage_contract::BranchName::parse("unchanged draft").unwrap(),
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap()
        .branch_id
        .unwrap();
    engine
        .select_file_branch(
            &source.file_id,
            &branch,
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    branch
}

fn unchanged_request(engine: &mut Engine, branch: &str, bytes: &[u8]) -> CommitRequest {
    let workspace = engine.create_workspace(branch, "admin").unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "report.txt", bytes)
        .unwrap();
    CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: storage_contract::OperationId::generate().unwrap(),
        principal: "admin".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "unchanged content".into(),
        request_digest: None,
    }
}

#[test]
fn selected_pending_unchanged_save_replays_without_materializing_content() {
    for bytes in [b"same bytes".as_slice(), b"".as_slice()] {
        let (root, mut engine, repo) = fixture();
        save(&mut engine, &repo.main_branch_id, bytes);
        let source = file(&engine, &repo.main_branch_id);
        let selected = select_pending(&mut engine, &source);
        let request = unchanged_request(&mut engine, &repo.main_branch_id, bytes);
        let branches = engine.state().branches.clone();
        let commits = engine.state().commits.clone();
        let versions = engine.state().versions.clone();
        let contexts = engine.state().file_contexts.clone();
        let receipt = engine.commit_workspace(request.clone()).unwrap();
        assert_eq!(receipt.result["no_change"], true);
        assert_eq!(engine.state().branches, branches);
        assert_eq!(engine.state().commits, commits);
        assert_eq!(engine.state().versions, versions);
        assert_eq!(engine.state().file_contexts, contexts);
        assert_eq!(
            engine.pending_file_source(&selected, &source.file_id),
            Some(source.version_id.as_str())
        );
        let sequence = engine.state().sequence;
        assert_eq!(engine.commit_workspace(request.clone()).unwrap(), receipt);
        assert_eq!(engine.state().sequence, sequence);
        drop(engine);

        let mut engine = Engine::open(&root.path().join("system")).unwrap();
        assert_eq!(engine.commit_workspace(request.clone()).unwrap(), receipt);
        assert_eq!(engine.operation(request.operation_id).unwrap(), receipt);
        assert_eq!(engine.state().sequence, sequence);
        assert_eq!(engine.state().branches, branches);
        assert_eq!(engine.state().versions, versions);
        assert_eq!(file(&engine, &selected).version_id, source.version_id);
        assert_eq!(
            engine.workspace(&request.workspace_id).unwrap().state,
            WorkspaceState::Committed
        );
    }
}

#[test]
fn pending_unchanged_save_fault_recovery_keeps_original_node() {
    for (fault, accepted) in [("after_prepare_1", false), ("after_accept_1", true)] {
        let (root, mut engine, repo) = fixture();
        save(&mut engine, &repo.main_branch_id, b"same bytes");
        let source = file(&engine, &repo.main_branch_id);
        let selected = select_pending(&mut engine, &source);
        let request = unchanged_request(&mut engine, &selected, b"same bytes");
        let branches = engine.state().branches.clone();
        let versions = engine.state().versions.clone();
        engine.inject_fault_once(fault);
        assert_eq!(
            engine.commit_workspace(request.clone()).unwrap_err().code(),
            if accepted {
                storage_contract::ErrorCode::InDoubt
            } else {
                storage_contract::ErrorCode::InjectedFailure
            }
        );
        drop(engine);

        let mut engine = Engine::open(&root.path().join("system")).unwrap();
        assert_eq!(
            engine.state().operations.contains_key(&request.operation_id),
            accepted
        );
        let receipt = engine.commit_workspace(request.clone()).unwrap();
        assert_eq!(receipt.result["no_change"], true);
        assert_eq!(engine.state().branches, branches);
        assert_eq!(engine.state().versions, versions);
        let sequence = engine.state().sequence;
        drop(engine);

        let mut engine = Engine::open(&root.path().join("system")).unwrap();
        assert_eq!(engine.commit_workspace(request).unwrap(), receipt);
        assert_eq!(engine.state().sequence, sequence);
        assert_eq!(engine.state().branches, branches);
        assert_eq!(engine.state().versions, versions);
    }
}

#[test]
fn pending_unchanged_file_keeps_its_head_when_sibling_content_is_published() {
    let (root, mut engine, repo) = fixture();
    save(&mut engine, &repo.main_branch_id, b"same bytes");
    let source = file(&engine, &repo.main_branch_id);
    let selected = select_pending(&mut engine, &source);
    let branch = engine.state().branches[&selected].clone();
    let versions = engine.state().versions.len();
    let request = unchanged_request(&mut engine, &repo.main_branch_id, b"same bytes");
    engine
        .stage_bytes(&request.workspace_id, "sibling.txt", b"new sibling")
        .unwrap();
    engine.commit_workspace(request).unwrap();
    assert_eq!(engine.state().branches[&selected], branch);
    assert_eq!(engine.state().versions.len(), versions + 1);
    assert_eq!(file(&engine, &selected).version_id, source.version_id);
    drop(engine);

    let engine = Engine::open(&root.path().join("system")).unwrap();
    assert_eq!(engine.state().branches[&selected], branch);
    assert_eq!(engine.state().versions.len(), versions + 1);
}

#[test]
fn pending_unchanged_save_still_rejects_a_changed_selection() {
    let (_root, mut engine, repo) = fixture();
    save(&mut engine, &repo.main_branch_id, b"same bytes");
    let source = file(&engine, &repo.main_branch_id);
    select_pending(&mut engine, &source);
    let other = engine
        .create_file_branch(
            &source.file_id,
            &source.version_id,
            &storage_contract::BranchName::parse("other selection").unwrap(),
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap()
        .branch_id
        .unwrap();
    assert_eq!(file(&engine, &other).version_id, source.version_id);
    let request = unchanged_request(&mut engine, &repo.main_branch_id, b"same bytes");
    engine
        .select_file_branch(
            &source.file_id,
            &other,
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    let before = canonical_json(engine.state()).unwrap();
    assert_eq!(
        engine.commit_workspace(request.clone()).unwrap_err().code(),
        storage_contract::ErrorCode::FileBranchConflict
    );
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    assert!(!engine.state().operations.contains_key(&request.operation_id));
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
                &storage_contract::BranchName::parse("selected draft").unwrap(),
                "admin",
                storage_contract::OperationId::generate().unwrap(),
            )
            .unwrap()
            .branch_id
            .unwrap();
        engine
            .select_file_branch(
                &original.file_id,
                &branch,
                "admin",
                storage_contract::OperationId::generate().unwrap(),
            )
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
        changed.commit_id = new_id("commit").unwrap();
        changed
            .entries
            .iter_mut()
            .find(|entry| entry.kind == NamespaceEntryKind::File)
            .unwrap()
            .version_id = new_id("ver").unwrap();
        std::sync::Arc::make_mut(&mut engine.state)
            .commits
            .insert(changed.commit_id.clone(), changed.clone());
        assert_eq!(
            synchronize_directory_edits(
                std::sync::Arc::make_mut(&mut engine.state),
                &changed.commit_id,
                &["report.txt".into()],
                "admin",
                storage_contract::OperationId::generate().unwrap(),
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
            &storage_contract::BranchName::parse("pending").unwrap(),
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    let pending = pending.result["branch_id"].as_str().unwrap().to_owned();
    let materialized_main = engine.state().file_selections[&original.file_id].clone();
    engine
        .select_file_branch(
            &original.file_id,
            &pending,
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    let old_workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .select_file_branch(
            &original.file_id,
            &materialized_main,
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    let error = engine
        .stage_range(
            &old_workspace.workspace_id,
            &new_id("entry").unwrap(),
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
        .select_file_branch(
            &original.file_id,
            &pending,
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    let staged = engine
        .stage_range(
            &workspace.workspace_id,
            &new_id("entry").unwrap(),
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
            &storage_contract::BranchName::parse("pending full").unwrap(),
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    let plan = engine
        .create_split_plan(&repo.history_tree_id, &pending.head_commit_id, "admin")
        .unwrap();
    let split = engine
        .commit_plan(
            &plan.plan_id,
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
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
            &storage_contract::BranchName::parse("actual").unwrap(),
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    let branch = created.result["branch_id"].as_str().unwrap().to_owned();
    engine
        .select_file_branch(
            &placeholder.file_id,
            &branch,
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
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
            &new_id("entry").unwrap(),
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
        .commit_historical_save_plan(
            &plan.plan_id,
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
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
