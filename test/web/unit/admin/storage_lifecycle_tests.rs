use super::*;

#[test]
fn shared_exit_ownership_retains_the_published_receipt() {
    let pending = status(ExitAction::Deregister);
    let mut complete = pending.clone();
    complete.state = ExitState::Completed;
    complete.phase = ExitPhase::Completed;
    complete.result = Some(storage_contract::OperationResult {
        operation_id: complete.operation_id,
        kind: "disk-deregister".into(),
        commit_id: None,
        history_tree_id: None,
        branch_id: None,
        commit_state: storage_contract::CommitState::Published,
        protection_at_publish: storage_contract::ProtectionAtPublish::NotApplicable,
        current_health: storage_contract::CurrentHealth::Healthy,
        request_digest: "d".repeat(64),
        sequence: 4,
        result: json!({
            "target_io": "closed",
            "disk_id": complete.preview.target.disk_id,
            "slot_id": complete.preview.target.slot_id,
            "binding_generation": "1",
            "registration": "detached",
        }),
    });
    registration_operation::tests::ownership(pending, complete);
}
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
        authority: ExitAuthority::ActiveMembership,
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
    assert!(!model.accept(record.clone()));
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
        result: json!({
            "target_io": "closed",
            "disk_id": record.preview.target.disk_id,
            "slot_id": record.preview.target.slot_id,
            "binding_generation": "1",
            "registration": "detached",
        }),
    });
    assert!(validate_status(&record, None).is_err());
    record.result.as_mut().unwrap().kind = "disk-deregister".into();
    assert!(validate_status(&record, None).is_ok());
    let mut model = Model {
        submitted: true,
        ..Default::default()
    };
    assert!(model.accept(record.clone()));
    assert!(!model.blocks_edit());
    assert!(!model.accept(record.clone()));
    record.result.as_mut().unwrap().commit_state = storage_contract::CommitState::InDoubt;
    assert!(validate_status(&record, None).is_err());
}

#[test]
fn final_voter_requires_explicit_terminal_authority_and_exact_terminal_receipt() {
    let mut record = status(ExitAction::Deregister);
    record.preview.original_metadata_members = 1;
    record.preview.target_metadata_members = 0;
    assert!(validate_preview(&record.preview).is_err());
    record.preview.authority = ExitAuthority::TerminalManagement;
    assert!(validate_preview(&record.preview).is_ok());
    record.state = ExitState::Completed;
    record.phase = ExitPhase::Completed;
    record.result = Some(storage_contract::OperationResult {
        operation_id: record.operation_id,
        kind: "disk-deregister".into(),
        commit_id: None,
        history_tree_id: None,
        branch_id: None,
        commit_state: storage_contract::CommitState::Published,
        protection_at_publish: storage_contract::ProtectionAtPublish::NotApplicable,
        current_health: storage_contract::CurrentHealth::Unknown,
        request_digest: "d".repeat(64),
        sequence: 4,
        result: json!({
            "target_io": "closed",
            "disk_id": record.preview.target.disk_id,
            "slot_id": record.preview.target.slot_id,
            "binding_generation": "1",
            "registration": "detached",
        }),
    });
    assert!(validate_status(&record, None).is_err());
    record.result.as_mut().unwrap().result["authority_kind"] = json!("terminal-management");
    assert!(validate_status(&record, None).is_ok());
    record.preview.target_metadata_members = 1;
    assert!(validate_status(&record, None).is_err());
}

fn registration() -> RegistrationEntry {
    let preview = preview(ExitAction::PlannedOffline);
    RegistrationEntry {
        slot_id: preview.target.slot_id,
        disk_id: preview.target.disk_id,
        binding_generation: preview.target.binding_generation,
        mode: RegistrationMode::Enabled,
        metadata_member: true,
        readmission: None,
    }
}

#[test]
fn exit_preview_matches_the_current_binding_role_and_available_action() {
    let mut entry = registration();
    let mut preview = preview(ExitAction::PlannedOffline);
    assert!(preview_matches_registration(&preview, &entry));
    for change in [0, 1, 2, 3] {
        let mut wrong = entry.clone();
        match change {
            0 => wrong.slot_id = "P02".into(),
            1 => wrong.disk_id = "disk_00000000000000000000000000000002".into(),
            2 => wrong.binding_generation += 1,
            3 => wrong.metadata_member = false,
            _ => unreachable!(),
        }
        assert!(!preview_matches_registration(&preview, &wrong));
    }
    entry.mode = RegistrationMode::Disabled;
    entry.metadata_member = false;
    preview.target_metadata_members = preview.original_metadata_members;
    assert!(!preview_matches_registration(&preview, &entry));
    preview.request.action = ExitAction::Deregister;
    assert!(preview_matches_registration(&preview, &entry));
}

#[test]
fn leaving_exit_preview_fences_late_reads_but_preserves_submitted_recovery() {
    let mut model = Model {
        preview: Some(preview(ExitAction::PlannedOffline)),
        sending: true,
        reading: Some(0),
        acknowledged: true,
        ..Default::default()
    };
    model.operation = Some(model.preview.as_ref().unwrap().request.operation_id);
    let generation = model.generation;
    model.leave_preview();
    assert_ne!(model.generation, generation);
    assert!(model.preview.is_none());
    assert!(model.operation.is_none());
    assert!(!model.sending);
    assert!(model.reading.is_none());
    assert!(!model.acknowledged);

    let original = status(ExitAction::Deregister);
    model.accept(original.clone());
    model.reading = Some(model.generation);
    let generation = model.generation;
    model.leave_preview();
    assert_eq!(model.generation, generation);
    assert_eq!(model.operation, Some(original.operation_id));
    assert_eq!(model.status, Some(original));
    assert!(model.submitted);
    assert!(model.reading.is_some());
    assert!(model.blocks_edit());
}

#[test]
fn completion_requires_the_exact_target_registration_and_closed_io() {
    for action in [ExitAction::PlannedOffline, ExitAction::Deregister] {
        let mut record = status(action);
        record.state = ExitState::Completed;
        record.phase = ExitPhase::Completed;
        record.result = Some(storage_contract::OperationResult {
            operation_id: record.operation_id,
            kind: action.result_kind().into(),
            commit_id: None,
            history_tree_id: None,
            branch_id: None,
            commit_state: storage_contract::CommitState::Published,
            protection_at_publish: storage_contract::ProtectionAtPublish::NotApplicable,
            current_health: storage_contract::CurrentHealth::Unknown,
            request_digest: "d".repeat(64),
            sequence: 4,
            result: json!({
                "disk_id": record.preview.target.disk_id,
                "slot_id": record.preview.target.slot_id,
                "binding_generation": "1",
                "registration": match action {
                    ExitAction::PlannedOffline => "disabled",
                    ExitAction::Deregister => "detached",
                },
                "target_io": "closed",
            }),
        });
        assert!(validate_status(&record, None).is_ok());
        for field in [
            "disk_id",
            "slot_id",
            "binding_generation",
            "registration",
            "target_io",
        ] {
            let mut wrong = record.clone();
            wrong.result.as_mut().unwrap().result[field] = json!("other");
            assert!(
                validate_status(&wrong, None).is_err(),
                "accepted wrong {field}"
            );
            wrong
                .result
                .as_mut()
                .unwrap()
                .result
                .as_object_mut()
                .unwrap()
                .remove(field);
            assert!(
                validate_status(&wrong, None).is_err(),
                "accepted missing {field}"
            );
        }
    }
}

#[test]
fn superseded_exit_status_releases_its_own_read_without_releasing_a_new_owner() {
    let mut model = Model {
        reading: Some(0),
        generation: 1,
        ..Default::default()
    };
    assert!(!model.finish_read(0));
    assert!(model.reading.is_none());
    model.reading = Some(1);
    assert!(!model.finish_read(0));
    assert_eq!(model.reading, Some(1));
    assert!(model.finish_read(1));
    assert!(model.reading.is_none());
}
