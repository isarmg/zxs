use super::*;
use storage_contract::{CommitState, CurrentHealth, OperationResult, ProtectionAtPublish};

fn operation() -> OperationId {
    "op_11111111111111111111111111111111".parse().unwrap()
}
fn pending() -> wire::Snapshot {
    wire::Snapshot {
        request: wire::OriginalRequest {
            operation_id: operation(),
            source_slot_id: "P01".into(),
            expected_sequence: 17,
            targets: vec![wire::OriginalTarget {
                root_path: "/fresh/a".into(),
                disk_id: "disk_new".into(),
                slot_id: "B01".into(),
                upstream_slot_id: "P01".into(),
            }],
        },
        progress: wire::Progress {
            observation: wire::Observation::Preparing,
            stage: "GenerationOrdinaryServingInstalled".into(),
            completed_files: 8,
            total_files: 8,
            completed_bytes: 1024,
            total_bytes: 1024,
            error_code: None,
            detail: "still stopped".into(),
        },
        completion: None,
    }
}
fn receipt() -> OperationResult {
    OperationResult {
        operation_id: operation(),
        kind: "backup-target-replacement".into(),
        commit_id: None,
        history_tree_id: None,
        branch_id: None,
        commit_state: CommitState::Published,
        protection_at_publish: ProtectionAtPublish::NotApplicable,
        current_health: CurrentHealth::Unknown,
        request_digest: "a".repeat(64),
        sequence: 18,
        result: json_codec::json!({"native":"original"}),
    }
}

#[test]
fn all_bytes_copied_and_first_publication_are_not_success() {
    let mut snapshot = pending();
    assert!(validate(&snapshot).is_ok());
    snapshot.completion = Some(receipt());
    assert!(validate(&snapshot).is_err());
    snapshot.progress.stage = "GenerationFirstCandidatePublished".into();
    assert!(validate(&snapshot).is_err());
}

#[test]
fn active_state_requires_matching_published_receipt() {
    let mut snapshot = pending();
    snapshot.progress.observation = wire::Observation::ServingActive;
    assert!(validate(&snapshot).is_err());
    snapshot.completion = Some(receipt());
    assert!(validate(&snapshot).is_ok());
    snapshot.completion.as_mut().unwrap().kind = "backup.configure".into();
    assert!(validate(&snapshot).is_err());
    snapshot.completion.as_mut().unwrap().kind = "backup-target-replacement".into();
    snapshot.completion.as_mut().unwrap().operation_id =
        "op_22222222222222222222222222222222".parse().unwrap();
    assert!(validate(&snapshot).is_err());
}

#[test]
fn active_error_or_nonpublished_record_is_not_success() {
    let mut snapshot = pending();
    snapshot.progress.observation = wire::Observation::ServingActive;
    snapshot.completion = Some(receipt());
    snapshot.progress.error_code = Some(storage_contract::ErrorCode::RecoveryRequired);
    assert!(validate(&snapshot).is_err());
    snapshot.progress.error_code = None;
    snapshot.completion.as_mut().unwrap().commit_state = CommitState::InDoubt;
    assert!(validate(&snapshot).is_err());
}

#[test]
fn unknown_attempt_keeps_original_id_and_disables_new_creation() {
    let model = Model {
        retained_operation: Some(operation()),
        unknown: true,
        ..Default::default()
    };
    assert!(model.reload_blocked());
    assert!(!model.editable());
    assert!(!model.can_start());
    assert_eq!(model.operation(), Some(operation()));
}

#[test]
fn response_cannot_replace_original_targets_or_operation() {
    let mut model = Model::default();
    model.accept(pending()).unwrap();
    let mut changed = pending();
    changed.request.targets[0].root_path = "/replacement".into();
    assert!(model.accept(changed).is_err());
    let mut changed = pending();
    changed.request.operation_id = "op_22222222222222222222222222222222".parse().unwrap();
    assert!(model.accept(changed).is_err());
    assert_eq!(model.snapshot.unwrap(), pending());
}

#[test]
fn readback_resolves_unknown_without_advancing_or_erasing_confirmation() {
    let mut model = Model {
        unknown: true,
        retained_operation: Some(operation()),
        approval: Some(Approval::Fence),
        ..Default::default()
    };
    model.accept(pending()).unwrap();
    assert!(!model.unknown);
    assert!(!model.completed());
    assert_eq!(model.approval, Some(Approval::Fence));
    assert_eq!(
        model.snapshot.as_ref().unwrap().progress.stage,
        "GenerationOrdinaryServingInstalled"
    );
}

#[test]
fn queued_cold_recovery_stays_incomplete_and_uses_original_id() {
    let model = Model {
        retained_operation: Some(operation()),
        ..Default::default()
    };
    assert_eq!(input_operation(&model).unwrap(), operation());
    assert!(!model.completed());
    assert!(!model.can_start());
}

#[test]
fn malformed_original_id_keeps_input_error_source() {
    let model = Model {
        operation_text: "not-an-operation".into(),
        ..Default::default()
    };
    let error = input_operation(&model).unwrap_err();
    assert!(matches!(
        error.source.as_deref(),
        Some(api::ErrorSource::BackupInput(_))
    ));
}

#[test]
fn revoked_and_busy_sessions_do_not_admit_new_browser_requests() {
    let mut state = State {
        authenticated: true,
        ..Default::default()
    };
    assert!(available(&state));
    state.admin_console.backup.busy = true;
    assert!(!available(&state));
    state.admin_console.backup.busy = false;
    state.session_transition = crate::ui::SessionTransition::LoggingOut;
    assert!(!available(&state));
}

#[test]
fn retained_system_input_without_a_worker_is_not_an_empty_task() {
    let mut model = Model::default();
    assert!(
        !model
            .accept_current(wire::Current {
                request: Some(pending().request),
                transaction: None
            })
            .unwrap()
    );
    assert_eq!(model.operation(), Some(operation()));
    assert!(model.unknown);
    assert!(!model.can_start());
    assert!(!model.completed());
    assert!(
        model
            .accept_current(wire::Current {
                request: None,
                transaction: None
            })
            .is_err()
    );
    let mut wrong = pending();
    wrong.request.targets[0].root_path = "/replacement".into();
    assert!(model.accept(wrong).is_err());
    model.accept(pending()).unwrap();
    assert!(!model.unknown);
}

#[test]
fn current_read_requires_the_same_original_input_as_its_snapshot() {
    let mut model = Model::default();
    assert!(
        model
            .accept_current(wire::Current {
                request: None,
                transaction: Some(pending())
            })
            .is_err()
    );
    let mut other = pending().request;
    other.targets[0].root_path = "/replacement".into();
    assert!(
        model
            .accept_current(wire::Current {
                request: Some(other),
                transaction: Some(pending())
            })
            .is_err()
    );
    assert!(
        !model
            .accept_current(wire::Current {
                request: Some(pending().request),
                transaction: Some(pending())
            })
            .unwrap()
    );
}

#[test]
fn an_empty_current_is_only_admitted_without_an_existing_operation() {
    let mut model = Model::default();
    assert!(
        model
            .accept_current(wire::Current {
                request: None,
                transaction: None
            })
            .unwrap()
    );
    model.retained_operation = Some(operation());
    assert!(
        model
            .accept_current(wire::Current {
                request: None,
                transaction: None
            })
            .is_err()
    );
}
