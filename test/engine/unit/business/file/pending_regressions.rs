use super::*;
use std::io::Read;

fn fixture() -> (tempfile::TempDir, Engine, Repository) {
    let root = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &root.path().join("system"),
        Config::development(crate::test_disks(
            [
                root.path().join("p"),
                root.path().join("b"),
                root.path().join("c"),
            ],
        ))
        .unwrap(),
    )
    .unwrap();
    let repo = engine
        .create_disk_root("P01", "admin", OperationId::generate().unwrap())
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
        let mut changed_state = engine.state.as_ref().clone();
        changed_state.branches.get_mut(&branch).unwrap().state = state;
        // Simulate an installed owner-state change, not a stale name-cache source.
        engine.named_namespaces.install_state(
            &mut engine.state, std::sync::Arc::new(changed_state),
        );
        let before = engine.state.branches[&branch].head_commit_id.clone();
        let source_before = canonical_json(engine.state()).unwrap();
        assert_eq!(
            engine
                .stage_bytes(&workspace.workspace_id, "report.txt", b"must not publish")
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::BranchNotActive
        );
        assert_eq!(canonical_json(engine.state()).unwrap(), source_before);
        // Exercise the real second (publication) fence on the exact retained
        // Entry/FILE/Version in this ordinary HEAD. Do not fabricate a body
        // or a directory snapshot to select the owner of a FILE.
        let held = engine.namespace_tree(&repo.main_branch_id).unwrap()
            .get("report.txt").unwrap().clone();
        let written = [(held.entry_id, (held.file_id, held.version_id))].into();
        let mut candidate = engine.state().clone();
        assert_eq!(publish_workspace_file_heads(
            &mut candidate, &repo, &workspace.base_head, &written,
            "admin", OperationId::generate().unwrap(),
        ).unwrap_err().code(), storage_contract::ErrorCode::BranchNotActive);
        assert_eq!(canonical_json(&candidate).unwrap(), source_before);
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
fn only_the_written_independent_file_branch_materializes_after_reopen() {
    let (root, mut engine, repo) = fixture();
    save(&mut engine, &repo.main_branch_id, b"original");
    let first = file(&engine, &repo.main_branch_id);
    let sibling_save = engine.create_workspace(&repo.main_branch_id, "admin").unwrap();
    engine.stage_bytes(&sibling_save.workspace_id, "sibling.txt", b"original").unwrap();
    commit(&mut engine, sibling_save);
    let sibling = engine.namespace_tree(&repo.main_branch_id).unwrap().get("sibling.txt").unwrap().clone();
    assert_ne!(first.file_id, sibling.file_id);
    let selected = select_pending(&mut engine, &first);
    let other = select_pending(&mut engine, &sibling);
    let other_head = engine.state().branches[&other].head_commit_id.clone();
    save(&mut engine, &repo.main_branch_id, b"written branch bytes");
    let selected_version = engine.namespace_tree(&repo.main_branch_id).unwrap()
        .get("report.txt").unwrap().version_id.clone();
    assert_ne!(selected_version, first.version_id);
    for reopen in [false, true] {
        if reopen { drop(engine); engine = Engine::open(&root.path().join("system")).unwrap(); }
        let changed = engine.namespace_tree(&selected).unwrap().get("report.txt").unwrap();
        assert_eq!(changed.version_id, selected_version);
        assert_eq!(changed.file_id, first.file_id);
        assert_eq!(engine.state().versions[&selected_version].source_version_ids, [first.version_id.clone()]);
        assert!(engine.pending_file_source(&selected, &first.file_id).is_none());
        assert_eq!(engine.state().branches[&other].head_commit_id, other_head);
        assert_eq!(engine.pending_file_source(&other, &sibling.file_id), Some(sibling.version_id.as_str()));
        assert_eq!(engine.namespace_tree(&repo.main_branch_id).unwrap().get("sibling.txt").unwrap(), &sibling);
    }
}

#[test]
fn historical_range_edits_use_the_selected_files_explicit_old_version_not_newer_main() {
    let (_root, mut engine, repo) = fixture();
    save(&mut engine, &repo.main_branch_id, b"old body");
    let original = file(&engine, &repo.main_branch_id);
    save(&mut engine, &repo.main_branch_id, b"current body");
    let current = file(&engine, &repo.main_branch_id);
    let main = engine.file_context(&repo.repository_id, "report.txt").unwrap().branch_id;
    let fork = select_pending(&mut engine, &original);
    let workspace = engine.create_workspace(&fork, "admin").unwrap();
    engine.stage_range(&workspace.workspace_id, &new_id("entry").unwrap(), "report.txt", 0,
        b"X", None).unwrap();
    commit(&mut engine, workspace);
    let written = file(&engine, &fork);
    assert_eq!(engine.state().versions[&written.version_id].source_version_ids, [original.version_id.clone()]);
    let read = |engine: &Engine, version: &str| {
        let mut content = engine.open_content(version).unwrap();
        let mut bytes = Vec::new();
        content.file.read_to_end(&mut bytes).unwrap();
        bytes
    };
    assert_eq!(read(&engine, &written.version_id), b"Xld body");
    assert_eq!(read(&engine, &original.version_id), b"old body");
    assert_eq!(file(&engine, &main).version_id, current.version_id);
    assert_eq!(read(&engine, &current.version_id), b"current body");
}
