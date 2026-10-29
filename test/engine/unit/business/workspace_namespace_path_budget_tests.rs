//! Non-main directory workspaces must reject invalid composed paths during
//! planning, before test-only full-history admission could mask the gap.
use super::*;

#[test]
fn non_main_directory_workspace_rejects_stored_parent_spelling_overflow() {
    for upload in [false, true] {
        let temporary = tempfile::tempdir().unwrap();
        let mut engine = Engine::initialize(
            &temporary.path().join("system"),
            Config::development([temporary.path().join("primary")]),
        )
        .unwrap();
        let repository = engine
            .create_repository(
                &storage_contract::RepositoryName::parse("Composed path budget").unwrap(),
                "P01",
                1,
                "admin",
                OperationId::generate(),
            )
            .unwrap();
        engine
            .file_namespace(
                &repository.repository_id,
                NamespaceOperation::parse(NamespaceOperationKind::Mkdir, "STRASSE", None).unwrap(),
                "admin",
                OperationId::generate(),
            )
            .unwrap();
        let main_head = engine.state().branches[&repository.main_branch_id]
            .head_commit_id
            .clone();
        let branch = engine
            .create_branch(
                &main_head,
                &storage_contract::BranchName::parse("directory fork").unwrap(),
                "admin",
                OperationId::generate(),
            )
            .unwrap();
        assert!(repository::main_for_branch(engine.state(), &branch.branch_id).is_none());
        assert!(branch.file_id.is_none());
        assert_eq!(branch.branch_kind, BranchKind::Full);
        let workspace = engine.create_workspace(&branch.branch_id, "admin").unwrap();
        assert!(
            engine.state().commits[&workspace.base_head]
                .pending_file_versions
                .is_empty()
        );
        let requested = format!("Straße/{}", "x".repeat(193));
        let staged = if upload {
            Some(
                engine
                    .stage_bytes(
                        &workspace.workspace_id,
                        &requested,
                        b"retained staged input",
                    )
                    .unwrap(),
            )
        } else {
            engine
                .namespace_operation(
                    &workspace.workspace_id,
                    NamespaceOperation::parse(NamespaceOperationKind::Mkdir, &requested, None)
                        .unwrap(),
                )
                .unwrap();
            None
        };
        let before = canonical_json(engine.state()).unwrap();
        let request = CommitRequest {
            workspace_id: workspace.workspace_id.clone(),
            operation_id: OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head.clone(),
            fencing_token: workspace.fencing_token,
            message: "reject composed path overflow".into(),
            request_digest: None,
        };
        let error = match engine.plan_workspace_commit_inner(request, None) {
            Ok(_) => panic!("invalid composed path reached a prepared publication"),
            Err(error) => error,
        };
        assert_eq!(error.code(), ErrorCode::PathInvalid);
        assert_eq!(canonical_json(engine.state()).unwrap(), before);
        assert_eq!(
            engine.state().branches[&branch.branch_id].head_commit_id,
            workspace.base_head
        );
        // Planning did not consume the workspace or its staged generation.
        let retained = engine.workspace(&workspace.workspace_id).unwrap();
        assert_eq!(retained.fencing_token, workspace.fencing_token);
        if let Some(staged) = staged {
            let retained = &retained.files[&staged.item_id];
            assert_eq!(
                canonical_json(retained).unwrap(),
                canonical_json(&staged).unwrap()
            );
            assert_eq!(
                std::fs::read(&retained.source_path).unwrap(),
                b"retained staged input"
            );
        }
    }
}
