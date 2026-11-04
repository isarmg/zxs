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
