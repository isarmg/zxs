use super::*;
use json_codec::json;

#[test]
fn historical_completion_rejects_invalid_persisted_fence_instead_of_leaving_workspace_open() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([
        temp.path().join("primary"),
        temp.path().join("backup"),
        temp.path().join("tertiary"),
    ]);
    let mut engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
    let repo = engine
        .create_repository("history", "P01", 1, "admin", OperationId::generate())
        .unwrap();
    let head = engine.state.branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let workspace = engine
        .create_historical_workspace(&repo.main_branch_id, &head, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "file", b"historical bytes")
        .unwrap();
    let plan = engine
        .create_historical_save_plan(
            &repo.history_tree_id,
            crate::HistoricalSaveOptions {
                source_commit_id: head,
                name: "saved branch".into(),
                workspace_id: Some(workspace.workspace_id.clone()),
                primary_slot_id: None,
                target_copies: None,
            },
            "admin",
        )
        .unwrap();
    let sealed = engine.workspace(&workspace.workspace_id).unwrap();
    engine
        .commit_historical_save_plan(&plan.plan_id, "admin", OperationId::generate())
        .unwrap();
    let mut recovered = sealed.clone();
    assert!(engine.reconcile_workspace_receipt(&mut recovered).unwrap());
    assert_eq!(recovered.state, WorkspaceState::Committed);
    for invalid in [
        json!(null),
        json!("+1"),
        json!("01"),
        json!("garbage"),
        json!("0"),
        json!("18446744073709551615"),
        json!("18446744073709551616"),
    ] {
        Arc::make_mut(&mut engine.state)
            .plans
            .get_mut(&plan.plan_id)
            .unwrap()
            .input["workspace_fencing_token"] = invalid.clone();
        let mut recovered = sealed.clone();
        let error = engine
            .reconcile_workspace_receipt(&mut recovered)
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::RecoveryRequired, "{invalid}");
        assert_eq!(recovered.state, sealed.state);
        assert_eq!(recovered.fencing_token, sealed.fencing_token);
    }
}

#[test]
fn workspace_evidence_distinguishes_absent_leaf_and_parent_from_json_damage() {
    let root = tempfile::tempdir().unwrap();
    let mut damage = None;
    for path in [
        root.path().join("absent.json"),
        root.path().join("absent/record.json"),
    ] {
        assert!(
            read_workspace_evidence::<json_codec::Value>(&path, &mut damage)
                .unwrap()
                .is_none()
        );
        assert!(damage.is_none());
    }
    let path = root.path().join("record.json");
    fs::write(&path, b"{").unwrap();
    assert!(
        read_workspace_evidence::<json_codec::Value>(&path, &mut damage)
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        damage,
        Some(Error::Contract(storage_contract::Error::Json { .. }))
    ));
    fs::write(&path, b"{\"healthy\":true}").unwrap();
    assert_eq!(
        read_workspace_evidence::<json_codec::Value>(&path, &mut damage).unwrap(),
        Some(json!({"healthy":true}))
    );
}
