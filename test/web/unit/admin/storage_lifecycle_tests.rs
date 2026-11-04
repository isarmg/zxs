use super::*;
fn preview(action: ExitAction) -> ExitPreview {
    ExitPreview {
        request: ExitRequest {
            operation_id: "op_00000000000000000000000000000001".parse().unwrap(),
            slot_id: "P01".into(),
            expected_disk_id: "disk_00000000000000000000000000000001".into(),
            action,
        },
        target: ExitTarget {
            pool_id: "pool_00000000000000000000000000000001".into(),
            disk_id: "disk_00000000000000000000000000000001".into(),
            slot_id: "P01".into(),
            binding_generation: 1,
            root_path: "/storage/registered".into(),
        },
        configuration_digest: "a".repeat(64),
        preview_digest: "b".repeat(64),
        snapshot_hash: "c".repeat(64),
        source_sequence: 2,
        topology_generation: 1,
        original_metadata_members: 3,
        target_metadata_members: 2,
    }
}
fn status(action: ExitAction) -> ExitStatus {
    let preview = preview(action);
    ExitStatus {
        operation_id: preview.request.operation_id,
        state: ExitState::RecoveryRequired,
        phase: ExitPhase::Activating,
        preview,
        failure: None,
        result: None,
    }
}
#[test]
fn exit_target_and_permanent_operation_are_exact_and_interruption_stays_pending() {
    let mut record = status(ExitAction::Deregister);
    assert!(validate_status(&record, Some(record.operation_id)).is_ok());
    let mut model = Model::default();
    assert!(!accept(&mut model, record.clone()));
    assert!(model.blocks_edit());
    record.preview.target.disk_id = "disk_00000000000000000000000000000002".into();
    assert!(validate_status(&record, None).is_err());
}
#[test]
fn exit_never_accepts_preparation_or_another_action_as_completion() {
    let mut record = status(ExitAction::Deregister);
    record.state = ExitState::Completed;
    record.phase = ExitPhase::Completed;
    assert!(validate_status(&record, None).is_err());
    record.result = Some(storage_contract::OperationResult {
        operation_id: record.operation_id,
        kind: "disk-planned-offline".into(),
        commit_id: None,
        history_tree_id: None,
        branch_id: None,
        commit_state: storage_contract::CommitState::Published,
        protection_at_publish: storage_contract::ProtectionAtPublish::NotApplicable,
        current_health: storage_contract::CurrentHealth::Healthy,
        request_digest: "d".repeat(64),
        sequence: 4,
        result: json!({"target_io":"closed"}),
    });
    assert!(validate_status(&record, None).is_err());
    record.result.as_mut().unwrap().kind = "disk-deregister".into();
    assert!(validate_status(&record, None).is_ok());
    let mut model = Model {
        submitted: true,
        ..Default::default()
    };
    assert!(accept(&mut model, record.clone()));
    assert!(!model.blocks_edit());
    assert!(!accept(&mut model, record.clone()));
    record.result.as_mut().unwrap().commit_state = storage_contract::CommitState::InDoubt;
    assert!(validate_status(&record, None).is_err());
}
