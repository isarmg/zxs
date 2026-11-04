use super::*;

fn preview() -> AdditionPreview {
    let selection = super::super::tests::candidate("/mnt/disk/new");
    AdditionPreview {
        request: AdditionRequest {
            operation_id: "op_00000000000000000000000000000001".parse().unwrap(),
            additions: vec![DirectoryAddition {
                selection,
                slot_id: "N01".into(),
                level: 1,
                upstream_slot_id: None,
                metadata_member: false,
            }],
        },
        pool_id: "pool_00000000000000000000000000000001".into(),
        configuration_digest: "a".repeat(64),
        preview_digest: "b".repeat(64),
        original_metadata_members: 1,
        target_metadata_members: 1,
        added_directories: 1,
    }
}
fn status(state: AdditionState, phase: AdditionPhase) -> AdditionStatus {
    let preview = preview();
    AdditionStatus {
        operation_id: preview.request.operation_id,
        preview,
        state,
        phase,
        continuation: if state == AdditionState::Cancelled {
            AdditionContinuation::Cancel
        } else {
            AdditionContinuation::Add
        },
        result: state
            .is_terminal()
            .then(|| storage_contract::OperationResult {
                operation_id: "op_00000000000000000000000000000001".parse().unwrap(),
                kind: if state == AdditionState::Cancelled {
                    "membership-add-cancelled"
                } else {
                    "membership-add"
                }
                .into(),
                commit_id: None,
                history_tree_id: None,
                branch_id: None,
                commit_state: storage_contract::CommitState::Published,
                protection_at_publish: storage_contract::ProtectionAtPublish::NotApplicable,
                current_health: storage_contract::CurrentHealth::Healthy,
                request_digest: "c".repeat(64),
                sequence: 2,
                result: json!({"generation": "d".repeat(64)}),
            }),
        failure: None,
    }
}
#[test]
fn addition_never_accepts_first_setup_or_partial_completion_as_success() {
    for value in [
        json!({"activated":true,"recovering":true,"restart_required":false}),
        json!({"initialized":true,"storage_pending":true}),
    ] {
        assert!(decode::<AdditionStatus>(value).is_err());
    }
    assert!(
        validate_status(
            &status(AdditionState::Completed, AdditionPhase::JointDecision),
            None
        )
        .is_err()
    );
    assert!(
        validate_status(
            &status(AdditionState::Adding, AdditionPhase::Completed),
            None
        )
        .is_err()
    );
    assert!(
        validate_status(
            &status(AdditionState::Completed, AdditionPhase::Completed),
            None
        )
        .is_ok()
    );
}
#[test]
fn uncertain_original_operation_blocks_editing_until_confirmed_complete() {
    let mut model = Model {
        submitted: true,
        operation: Some(preview().request.operation_id),
        ..Default::default()
    };
    assert!(model.blocks_edit());
    assert!(!accept(
        &mut model,
        status(AdditionState::RecoveryRequired, AdditionPhase::Enrolling)
    ));
    assert!(model.blocks_edit());
    assert!(accept(
        &mut model,
        status(AdditionState::Completed, AdditionPhase::Completed)
    ));
    assert!(!model.blocks_edit());
    assert!(!accept(
        &mut model,
        status(AdditionState::Completed, AdditionPhase::Completed)
    ));
}
#[test]
fn metadata_role_counts_and_permanent_operation_match_the_preview() {
    let mut record = status(AdditionState::Adding, AdditionPhase::Fencing);
    let other = "op_00000000000000000000000000000002".parse().unwrap();
    assert!(validate_status(&record, Some(other)).is_err());
    record.preview.target_metadata_members = 2;
    assert!(validate_status(&record, None).is_err());
    record.preview.request.additions[0].metadata_member = true;
    assert!(validate_status(&record, Some(record.operation_id)).is_ok());
}

#[test]
fn only_published_original_cancellation_unlocks_editing() {
    let mut model = Model {
        submitted: true,
        ..Default::default()
    };
    let mut pending = status(
        AdditionState::RecoveryRequired,
        AdditionPhase::AbortCertified,
    );
    pending.continuation = AdditionContinuation::Cancel;
    assert!(!accept(&mut model, pending));
    assert!(model.blocks_edit());
    let cancelled = status(AdditionState::Cancelled, AdditionPhase::Cancelled);
    assert!(validate_status(&cancelled, None).is_ok());
    assert!(accept(&mut model, cancelled.clone()));
    assert!(!model.blocks_edit());
    assert!(!accept(&mut model, cancelled));
    let mut false_cancel = status(AdditionState::Cancelled, AdditionPhase::Cancelled);
    false_cancel.result.as_mut().unwrap().kind = "membership-add".into();
    assert!(validate_status(&false_cancel, None).is_err());
    false_cancel.result.as_mut().unwrap().kind = "membership-add-cancelled".into();
    false_cancel.result.as_mut().unwrap().commit_state = storage_contract::CommitState::InDoubt;
    assert!(validate_status(&false_cancel, None).is_err());
}

#[test]
fn cancelling_and_uncertain_cancel_keep_the_original_operation_locked() {
    let mut model = Model::default();
    let mut record = status(AdditionState::Cancelling, AdditionPhase::CertifyingAbort);
    record.continuation = AdditionContinuation::Cancel;
    accept(&mut model, record.clone());
    model.cancel_acknowledged = true;
    record.state = AdditionState::RecoveryRequired;
    accept(&mut model, record);
    assert!(model.blocks_edit());
    assert!(!model.cancel_acknowledged);
    assert!(!model.resume_add_acknowledged);
    assert_eq!(model.operation, Some(preview().request.operation_id));
    assert_eq!(
        model.status.unwrap().continuation,
        AdditionContinuation::Cancel
    );
}
