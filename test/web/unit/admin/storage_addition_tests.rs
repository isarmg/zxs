use super::*;

fn preview() -> AdditionPreview {
    let selection = super::super::tests::candidate("/mnt/disk/new");
    AdditionPreview {
        authority: AdditionAuthority::ActiveMembership,
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

fn terminal_preview() -> AdditionPreview {
    let mut preview = preview();
    preview.authority = AdditionAuthority::TerminalManagement {
        terminal_authority_hash: "d".repeat(64),
        terminal_operation_id: "op_00000000000000000000000000000002".parse().unwrap(),
    };
    preview.request.additions[0].metadata_member = true;
    preview
}

#[test]
fn metadata_resume_keeps_old_history_count_and_requires_an_actual_fresh_group() {
    let mut preview = terminal_preview();
    assert!(validate_preview(&preview).is_ok());
    preview.target_metadata_members = 0;
    assert!(validate_preview(&preview).is_err());
    preview.target_metadata_members = 1;
    preview.request.additions[0].metadata_member = false;
    assert!(validate_preview(&preview).is_err());
    let value = json!(terminal_preview());
    let mut missing = value;
    missing.as_object_mut().unwrap().remove("authority");
    assert!(decode::<AdditionPreview>(missing).is_err());
}

#[test]
fn metadata_resume_mixed_preview_counts_only_explicit_voters() {
    let mut preview = terminal_preview();
    let mut data = preview.request.additions[0].clone();
    data.metadata_member = false;
    data.slot_id = "N02".into();
    data.selection = super::super::tests::candidate("/mnt/disk/data");
    preview.request.additions.push(data);
    preview.added_directories = 2;
    assert!(validate_preview(&preview).is_ok());
    preview.target_metadata_members = 2;
    assert!(validate_preview(&preview).is_err());
    preview.target_metadata_members = 1;
    let mut model = super::super::Model {
        snapshot: Some(super::super::Snapshot {
            state: StorageState::TerminalManagement,
            _disk_ids: Vec::new(),
            completion_available: false,
            allow_development: false,
            terminal: None,
        }),
        selections: preview
            .request
            .additions
            .iter()
            .map(|addition| addition.selection.clone())
            .collect(),
        ..Default::default()
    };
    selected(&mut model);
    model
        .addition
        .fields
        .get_mut(&model.selections[1].selection_id)
        .unwrap()
        .metadata = false;
    let request = make_request(&model, preview.request.operation_id).unwrap();
    assert!(request.additions[0].metadata_member);
    assert!(!request.additions[1].metadata_member);
    model
        .addition
        .fields
        .get_mut(&model.selections[0].selection_id)
        .unwrap()
        .metadata = false;
    assert!(make_request(&model, preview.request.operation_id).is_err());
}

#[test]
fn terminal_resume_allows_explicit_cancellation_but_requires_its_exact_receipt() {
    let mut pending = status(
        AdditionState::RecoveryRequired,
        AdditionPhase::ResumeCancellable,
    );
    pending.preview = terminal_preview();
    assert!(validate_status(&pending, None).is_ok());
    let mut model = Model::default();
    accept(&mut model, pending.clone());
    assert!(can_cancel(&model));
    assert!(model.blocks_edit());
    pending.continuation = AdditionContinuation::Cancel;
    assert!(validate_status(&pending, None).is_ok());
    pending.phase = AdditionPhase::CertifyingAbort;
    pending.state = AdditionState::Cancelling;
    assert!(validate_status(&pending, None).is_ok());
    assert!(model.blocks_edit());
    let mut complete = status(AdditionState::Completed, AdditionPhase::Completed);
    complete.preview = terminal_preview();
    assert!(validate_status(&complete, None).is_err());
    let result = complete.result.as_mut().unwrap();
    result.kind = "terminal-metadata-resume".into();
    result.result = json!({"terminal_authority":"d".repeat(64),"metadata_members":1});
    assert!(validate_status(&complete, None).is_ok());
    complete.result.as_mut().unwrap().result["terminal_authority"] = "e".repeat(64).into();
    assert!(validate_status(&complete, None).is_err());
}

#[test]
fn terminal_cancelled_status_requires_permanent_cancelled_kind_and_authority() {
    let mut cancelled = status(AdditionState::Cancelled, AdditionPhase::Cancelled);
    cancelled.preview = terminal_preview();
    assert!(validate_status(&cancelled, None).is_err());
    let result = cancelled.result.as_mut().unwrap();
    result.kind = "terminal-metadata-resume-cancelled".into();
    result.result = json!({"terminal_authority":"d".repeat(64),"metadata_members":1});
    assert!(validate_status(&cancelled, None).is_ok());
    let mut model = Model {
        submitted: true,
        ..Default::default()
    };
    assert!(accept(&mut model, cancelled.clone()));
    assert!(!model.blocks_edit());
    assert!(!can_cancel(&model));
    cancelled.continuation = AdditionContinuation::Add;
    assert!(validate_status(&cancelled, None).is_err());
    cancelled.continuation = AdditionContinuation::Cancel;
    cancelled.result.as_mut().unwrap().kind = "terminal-metadata-resume".into();
    assert!(validate_status(&cancelled, None).is_err());
}

#[test]
fn terminal_cancel_requires_a_current_observation_and_never_guesses_from_progress() {
    for phase in [
        AdditionPhase::JointDecision,
        AdditionPhase::ResumeCancellationUnknown,
        AdditionPhase::ResumeQuorumAccepted,
        AdditionPhase::Activating,
    ] {
        let mut pending = status(AdditionState::RecoveryRequired, phase);
        pending.preview = terminal_preview();
        pending.continuation = AdditionContinuation::Cancel;
        assert!(validate_status(&pending, None).is_ok());
        let operation = pending.operation_id;
        let mut model = Model::default();
        accept(&mut model, pending);
        assert!(!can_cancel(&model));
        assert!(model.blocks_edit());
        assert_eq!(model.operation, Some(operation));
        assert_eq!(
            model.status.unwrap().continuation,
            AdditionContinuation::Cancel
        );
    }
    for phase in [
        AdditionPhase::ResumeCancellable,
        AdditionPhase::ResumeQuorumAccepted,
        AdditionPhase::ResumeCancellationUnknown,
    ] {
        assert!(validate_status(&status(AdditionState::RecoveryRequired, phase), None).is_err());
    }
}
