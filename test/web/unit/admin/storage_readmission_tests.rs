use super::*;

fn preview() -> ReadmissionPreview {
    ReadmissionPreview {
        request: ReadmissionRequest {
            operation_id: "op_00000000000000000000000000000001".parse().unwrap(),
            slot_id: "P01".into(),
            expected_disk_id: "disk_00000000000000000000000000000001".into(),
        },
        target: ReadmissionTarget {
            pool_id: "pool_00000000000000000000000000000001".into(),
            disk_id: "disk_00000000000000000000000000000001".into(),
            slot_id: "P01".into(),
            binding_generation: 1,
            root_path: "/storage/registered".into(),
        },
        principal: "admin".into(),
        metadata_member: true,
        configuration_digest: "a".repeat(64),
        preview_digest: "b".repeat(64),
        authority: ReadmissionAuthority::ActiveMembership {
            membership_authority_hash: "c".repeat(64),
        },
        registry_root_hash: "d".repeat(64),
        registry_generation: 2,
        source_sequence: 4,
        topology_generation: 2,
        snapshot_hash: "e".repeat(64),
    }
}
fn pending() -> ReadmissionStatus {
    let preview = preview();
    ReadmissionStatus {
        operation_id: preview.request.operation_id,
        state: ReadmissionState::RecoveryRequired,
        phase: ReadmissionPhase::Activating,
        preview,
        failure: None,
        result: None,
    }
}
fn completion(status: &ReadmissionStatus) -> storage_contract::OperationResult {
    storage_contract::OperationResult {
        operation_id: status.operation_id,
        kind: "disk-readmit".into(),
        commit_id: None,
        history_tree_id: None,
        branch_id: None,
        commit_state: storage_contract::CommitState::Published,
        protection_at_publish: storage_contract::ProtectionAtPublish::NotApplicable,
        current_health: storage_contract::CurrentHealth::Healthy,
        request_digest: "f".repeat(64),
        sequence: 5,
        result: json!({
            "disk_id": status.preview.target.disk_id,
            "slot_id": status.preview.target.slot_id,
            "binding_generation": "1",
            "registration": "enabled",
            "io_admission_epoch": "f".repeat(64),
        }),
    }
}

#[test]
fn completion_requires_exact_identity_and_published_new_io_epoch() {
    let mut status = pending();
    assert!(validate_status(&status, Some(status.operation_id)).is_ok());
    let mut model = Model::default();
    assert!(!accept(&mut model, status.clone()));
    assert!(model.blocks_edit());
    assert_eq!(model.operation, Some(status.operation_id));
    status.state = ReadmissionState::Completed;
    status.phase = ReadmissionPhase::Completed;
    assert!(validate_status(&status, None).is_err());
    status.result = Some(completion(&status));
    assert!(validate_status(&status, None).is_ok());
    for field in [
        "disk_id",
        "slot_id",
        "binding_generation",
        "registration",
        "io_admission_epoch",
    ] {
        let mut wrong = status.clone();
        wrong.result.as_mut().unwrap().result[field] = json!("other");
        assert!(
            validate_status(&wrong, None).is_err(),
            "accepted wrong {field}"
        );
    }
    let mut wrong = status.clone();
    wrong.result.as_mut().unwrap().kind = "membership-add".into();
    assert!(validate_status(&wrong, None).is_err());
    assert!(accept(&mut model, status.clone()));
    assert!(!model.blocks_edit());
    assert!(!accept(&mut model, status));
}

#[test]
fn unknown_registration_never_grants_readmission_and_terminal_roles_are_explicit() {
    let entry = RegistrationEntry {
        slot_id: "P01".into(),
        disk_id: "disk_00000000000000000000000000000001".into(),
        binding_generation: 1,
        mode: RegistrationMode::Disabled,
        metadata_member: false,
        readmission: Some(ReadmissionRole::MetadataMember),
    };
    let (authority, entries) = registration_projection(RegistrationSnapshot::Unadmitted).unwrap();
    assert_eq!(authority, RegistrationAuthority::Unadmitted);
    assert!(entries.is_empty());
    let (authority, entries) = registration_projection(RegistrationSnapshot::TerminalManagement {
        registrations: vec![entry.clone()],
    })
    .unwrap();
    assert_eq!(authority, RegistrationAuthority::Terminal);
    assert_eq!(entries["P01"].mode, RegistrationMode::Disabled);
    assert!(
        registration_projection(RegistrationSnapshot::ActiveMembership {
            registrations: Vec::new()
        })
        .is_err()
    );
    assert!(
        registration_projection(RegistrationSnapshot::ActiveMembership {
            registrations: vec![entry.clone(), entry.clone()]
        })
        .is_err()
    );
    let mut invalid = entry.clone();
    invalid.metadata_member = true;
    assert!(
        registration_projection(RegistrationSnapshot::ActiveMembership {
            registrations: vec![invalid]
        })
        .is_err()
    );
    let (authority, entries) = registration_projection(RegistrationSnapshot::ActiveMembership {
        registrations: vec![entry],
    })
    .unwrap();
    assert_eq!(authority, RegistrationAuthority::Active);
    assert_eq!(entries["P01"].mode, RegistrationMode::Disabled);
}

#[test]
fn current_registry_identity_cannot_be_replaced_by_same_slot_or_device_health() {
    let mut state = State::default();
    let preview = preview();
    let model = &mut state.admin_console.storage.readmission;
    model.authority = RegistrationAuthority::Active;
    model.registrations.insert(
        "P01".into(),
        RegistrationEntry {
            slot_id: "P01".into(),
            disk_id: preview.target.disk_id.clone(),
            binding_generation: 1,
            mode: RegistrationMode::Disabled,
            metadata_member: false,
            readmission: Some(ReadmissionRole::MetadataMember),
        },
    );
    assert_eq!(
        registration_mode(&state, "P01", &preview.target.disk_id),
        Some(RegistrationMode::Disabled)
    );
    assert_eq!(
        registration_mode(&state, "P01", "disk_00000000000000000000000000000002"),
        None
    );
    state
        .admin_console
        .storage
        .readmission
        .registrations
        .get_mut("P01")
        .unwrap()
        .mode = RegistrationMode::Enabled;
    assert_eq!(
        registration_mode(&state, "P01", &preview.target.disk_id),
        Some(RegistrationMode::Enabled)
    );
    state.admin_console.storage.readmission.authority = RegistrationAuthority::Unadmitted;
    assert_eq!(
        registration_mode(&state, "P01", &preview.target.disk_id),
        None
    );
}

fn terminal_preview() -> ReadmissionPreview {
    let mut preview = preview();
    preview.authority = ReadmissionAuthority::TerminalManagement {
        terminal_authority_hash: "c".repeat(64),
        terminal_operation_id: "op_00000000000000000000000000000002".parse().unwrap(),
    };
    preview
}

fn terminal_state() -> State {
    let preview = terminal_preview();
    let mut state = State {
        authenticated: true,
        page: crate::ui::Page::Admin,
        ..Default::default()
    };
    state.admin_console.storage.snapshot = Some(Snapshot {
        state: StorageState::TerminalManagement,
        _disk_ids: vec![preview.target.disk_id.clone()],
        completion_available: false,
        allow_development: false,
        terminal: Some(storage_contract::storage_lifecycle::TerminalStorageStatus {
            pool_id: preview.target.pool_id.clone(),
            operation_id: "op_00000000000000000000000000000002".parse().unwrap(),
            authority_hash: preview.authority.hash().into(),
            registrations:
                storage_contract::storage_lifecycle::TerminalRegistrations::RetainedRegistrations,
            registered_directories: 1,
            original_metadata_members: 1,
            original_metadata_quorum: 1,
        }),
    });
    state.admin_console.storage.readmission.authority = RegistrationAuthority::Terminal;
    state
        .admin_console
        .storage
        .readmission
        .registrations
        .insert(
            preview.target.slot_id.clone(),
            RegistrationEntry {
                slot_id: preview.target.slot_id,
                disk_id: preview.target.disk_id,
                binding_generation: preview.target.binding_generation,
                mode: RegistrationMode::Disabled,
                metadata_member: false,
                readmission: Some(ReadmissionRole::MetadataMember),
            },
        );
    state
}

#[test]
fn terminal_button_requires_an_authenticated_restorable_role_and_exact_registration() {
    let mut state = terminal_state();
    let preview = terminal_preview();
    let slot = &preview.target.slot_id;
    let disk = &preview.target.disk_id;
    assert!(can_preview(&state, slot, disk));
    assert!(!can_preview(
        &state,
        slot,
        "disk_00000000000000000000000000000003"
    ));
    state
        .admin_console
        .storage
        .readmission
        .registrations
        .get_mut(slot)
        .unwrap()
        .readmission = None;
    assert!(!can_preview(&state, slot, disk));
    state
        .admin_console
        .storage
        .readmission
        .registrations
        .get_mut(slot)
        .unwrap()
        .readmission = Some(ReadmissionRole::DataOnly);
    assert!(!can_preview(&state, slot, disk));
    state
        .admin_console
        .storage
        .readmission
        .registrations
        .get_mut(slot)
        .unwrap()
        .readmission = Some(ReadmissionRole::MetadataMember);
    state.admin_console.storage.readmission.sending = true;
    assert!(!can_preview(&state, slot, disk));
    state.admin_console.storage.readmission.sending = false;
    state.session_transition = crate::ui::SessionTransition::LoggingOut;
    assert!(!can_preview(&state, slot, disk));
    state.session_transition = crate::ui::SessionTransition::Idle;
    state.admin_console.storage.readmission.authority = RegistrationAuthority::Unadmitted;
    assert!(!can_preview(&state, slot, disk));
}

#[test]
fn late_preview_must_match_its_authority_role_and_binding() {
    let state = terminal_state();
    let preview = terminal_preview();
    let entry = &state.admin_console.storage.readmission.registrations["P01"];
    assert!(preview_matches_registration(
        &preview,
        RegistrationAuthority::Terminal,
        entry
    ));
    assert!(!preview_matches_registration(
        &preview,
        RegistrationAuthority::Active,
        entry
    ));
    let mut changed = entry.clone();
    changed.binding_generation += 1;
    assert!(!preview_matches_registration(
        &preview,
        RegistrationAuthority::Terminal,
        &changed
    ));
    changed = entry.clone();
    changed.readmission = None;
    assert!(!preview_matches_registration(
        &preview,
        RegistrationAuthority::Terminal,
        &changed
    ));
    let mut wrong_role = preview;
    wrong_role.metadata_member = false;
    assert!(validate_preview(&wrong_role).is_err());
}

#[test]
fn leaving_the_page_invalidates_only_unsubmitted_preview_responses() {
    let preview = terminal_preview();
    let mut model = Model {
        operation: Some(preview.request.operation_id),
        preview: Some(preview),
        sending: true,
        acknowledged: true,
        ..Default::default()
    };
    let generation = model.generation;
    model.leave_preview();
    assert_ne!(model.generation, generation);
    assert!(model.preview.is_none());
    assert!(model.operation.is_none());
    assert!(!model.sending);
    assert!(!model.acknowledged);
    let status = pending();
    accept(&mut model, status.clone());
    let generation = model.generation;
    model.leave_preview();
    assert_eq!(model.generation, generation);
    assert_eq!(model.operation, Some(status.operation_id));
    assert!(model.submitted);
    assert!(model.blocks_edit());
}

#[test]
fn terminal_completion_requires_terminal_authority_as_well_as_identity_and_new_epoch() {
    let mut status = pending();
    status.preview = terminal_preview();
    status.state = ReadmissionState::Completed;
    status.phase = ReadmissionPhase::Completed;
    let mut result = completion(&status);
    status.result = Some(result.clone());
    assert!(validate_status(&status, None).is_err());
    result.result["authority_kind"] = json!("terminal-metadata-resume");
    result.result["terminal_authority"] = json!(status.preview.authority.hash());
    status.result = Some(result);
    assert!(validate_status(&status, None).is_ok());
    for field in ["authority_kind", "terminal_authority"] {
        let mut wrong = status.clone();
        wrong.result.as_mut().unwrap().result[field] = json!("wrong");
        assert!(validate_status(&wrong, None).is_err());
    }
    let mut active = status;
    active.preview.authority = preview().authority;
    assert!(validate_status(&active, None).is_err());
}

#[test]
fn exit_buttons_require_active_exact_registration_and_do_not_repeat_offline() {
    use storage_contract::storage_lifecycle::ExitAction;
    let mut state = terminal_state();
    let preview = terminal_preview();
    let slot = &preview.target.slot_id;
    let disk = &preview.target.disk_id;
    let can_exit =
        |state: &State, action| super::super::lifecycle::can_preview(state, slot, disk, action);
    assert!(!can_exit(&state, ExitAction::Deregister));
    state.admin_console.storage.snapshot.as_mut().unwrap().state = StorageState::Active;
    assert!(!can_exit(&state, ExitAction::Deregister));
    state.admin_console.storage.readmission.authority = RegistrationAuthority::Active;
    assert!(can_exit(&state, ExitAction::Deregister));
    assert!(!can_exit(&state, ExitAction::PlannedOffline));
    assert!(!super::super::lifecycle::can_preview(
        &state,
        slot,
        "disk_00000000000000000000000000000003",
        ExitAction::Deregister,
    ));
    let entry = state
        .admin_console
        .storage
        .readmission
        .registrations
        .get_mut(slot)
        .unwrap();
    entry.mode = RegistrationMode::Enabled;
    entry.metadata_member = true;
    entry.readmission = None;
    assert!(can_exit(&state, ExitAction::PlannedOffline));
    state.admin_console.storage.busy = true;
    assert!(!can_exit(&state, ExitAction::Deregister));
    state.admin_console.storage.busy = false;
    state.session_transition = crate::ui::SessionTransition::LoggingOut;
    assert!(!can_exit(&state, ExitAction::Deregister));
    state.session_transition = crate::ui::SessionTransition::Idle;
    state.admin_console.storage.readmission.authority = RegistrationAuthority::Unadmitted;
    assert!(!can_exit(&state, ExitAction::Deregister));
}

#[test]
fn resuming_readmission_releases_superseded_status_without_accepting_its_result() {
    let original = pending();
    let mut model = Model::default();
    accept(&mut model, original.clone());
    let reading_generation = model.generation;
    model.reading = Some(reading_generation);

    // The explicit original-operation retry starts while its GET is in flight.
    model.generation += 1;
    model.sending = true;
    assert!(!model.finish_read(reading_generation));
    assert!(model.reading.is_none());
    assert_eq!(model.operation, Some(original.operation_id));
    assert_eq!(model.status, Some(original));
    assert!(model.submitted);
    assert!(model.sending);

    // A new GET can own the current generation; an old completion cannot free it.
    let current_generation = model.generation;
    model.reading = Some(current_generation);
    assert!(!model.finish_read(reading_generation));
    assert_eq!(model.reading, Some(current_generation));
    assert!(model.finish_read(current_generation));
    assert!(model.reading.is_none());
}

#[test]
fn dismissing_unsubmitted_readmission_does_not_release_a_later_status_read() {
    let mut model = Model {
        preview: Some(preview()),
        acknowledged: true,
        reading: Some(0),
        ..Default::default()
    };
    model.leave_preview();
    assert!(model.preview.is_none());
    assert!(!model.acknowledged);
    assert!(model.reading.is_none());
    assert_eq!(model.generation, 1);
    model.reading = Some(model.generation);
    assert!(!model.finish_read(0));
    assert_eq!(model.reading, Some(1));
    assert!(model.finish_read(1));
    assert!(model.reading.is_none());
}
