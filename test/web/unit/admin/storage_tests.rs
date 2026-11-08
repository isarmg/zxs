use super::*;
use storage_contract::storage_directories::{CandidateSelection, DirectoryIdentity, MountIdentity};

pub(super) fn candidate(root: &str) -> CandidateSelection {
    CandidateSelection {
        selection_id: format!("selection-{root}"),
        discovery_root: "/mnt".into(),
        discovery_identity: DirectoryIdentity {
            device: 2049,
            inode: 2,
            ctime_seconds: "100".into(),
            ctime_nanoseconds: 0,
        },
        mount_path: "/mnt/disk".into(),
        root_path: root.into(),
        mount_identity: MountIdentity {
            mount_id: 7,
            device_major: 8,
            device_minor: 1,
            root_inode: 2,
            filesystem: "ext4".into(),
            filesystem_root: "/".into(),
        },
        root_identity: DirectoryIdentity {
            device: 2049,
            inode: 8,
            ctime_seconds: "100".into(),
            ctime_nanoseconds: 0,
        },
    }
}
fn ready() -> State {
    let mut state = State {
        authenticated: true,
        session_epoch: 7,
        ..Default::default()
    };
    state.admin_console.storage.snapshot = Some(Snapshot {
        state: StorageState::Pending,
        _disk_ids: vec![],
        completion_available: false,
        allow_development: false,
        terminal: None,
    });
    state
}
fn failure(status: u16) -> api::Error {
    api::Error {
        code: storage_contract::ErrorCode::InvalidRequest,
        message: "specific error".into(),
        retryable: false,
        status,
        request_id: Some("activation-request".into()),
        source: None,
    }
}

#[test]
fn failed_and_unknown_activation_preserve_evidence_and_require_proven_completion() {
    let mut model = ready().admin_console.storage;
    model.mutation = true;
    model.busy = true;
    finish_activation(&mut model, Err(failure(503)));
    assert!(model.unknown && model.reload_blocked());
    assert!(!model.busy && !model.mutation);
    assert_eq!(
        model
            .activation_error
            .as_ref()
            .unwrap()
            .request_id
            .as_deref(),
        Some("activation-request")
    );
    assert!(!model.can_complete());
    model.snapshot.as_mut().unwrap().completion_available = true;
    assert!(model.can_complete());
    finish_activation(&mut model, Err(failure(409)));
    assert!(
        model.unknown,
        "a later rejected completion cannot disprove the original activation"
    );
    finish_activation(&mut model, Ok(Success::Recovering));
    assert!(!model.unknown && !model.reload_blocked());
    assert!(!model.can_complete());
}

#[test]
fn activation_wire_success_is_strict_and_distinct_from_account_creation() {
    for (value, expected) in [
        (
            json!({"activated":true,"restart_required":false,"recovering":true}),
            Success::Recovering,
        ),
        (
            json!({"activated":true,"restart_required":true,"recovering":false}),
            Success::RestartRequired,
        ),
    ] {
        assert_eq!(response::activation(value).unwrap(), expected);
    }
    for value in [
        json!({}),
        json!({"initialized":true,"storage_pending":true}),
        json!({"activated":false,"restart_required":false,"recovering":true}),
        json!({"activated":true,"restart_required":false,"recovering":false}),
        json!({"activated":true,"restart_required":true,"recovering":true}),
        json!({"activated":true,"restart_required":false,"recovering":true,"extra":true}),
    ] {
        assert!(response::activation(value).is_err());
    }
}

#[test]
fn obsolete_generations_and_sessions_cannot_own_storage_results() {
    let mut state = ready();
    assert_eq!(begin(&mut state, false), Some((7, 1)));
    assert!(current(&state, 7, 1));
    assert!(!current(&state, 7, 0) && !current(&state, 6, 1));
    state.authenticated = false;
    assert!(!current(&state, 7, 1));
}

#[test]
fn storage_diagnostics_preserve_existing_disk_cards_without_directory_settings() {
    for status in [
        StorageState::Active,
        StorageState::RecoveryRequired,
        StorageState::Unavailable,
    ] {
        let mut model = ready().admin_console.storage;
        let mut snapshot = model.snapshot.clone().unwrap();
        snapshot.state = status;
        assert!(finish_load(&mut model, Ok(snapshot)));
        assert_eq!(model.snapshot.as_ref().unwrap().state, status);
    }
    let mut model = ready().admin_console.storage;
    assert!(finish_load(&mut model, Err(failure(503))));
    assert!(model.error.is_some());
    let snapshot = ready().admin_console.storage.snapshot.unwrap();
    assert!(!finish_load(&mut model, Ok(snapshot)));
}

#[test]
fn administration_has_no_free_discovery_mount_or_path_controls() {
    let source = concat!(
        include_str!("../../../../src/web/src/admin/storage.rs"),
        include_str!("../../../../src/web/src/admin/storage/wizard.rs")
    );
    for removed in [
        "data-storage-field",
        "data-storage-discovery-root",
        "data-storage-save-discovery-root",
        "/api/admin/storage/directories?path=",
    ] {
        assert!(!source.contains(removed));
    }
}

#[test]
fn terminal_management_allows_fresh_resume_but_never_initial_activation() {
    let value = json!({
        "state":"terminal_management", "disk_ids":["disk_retained"],
        "completion_available":false, "allow_development":false,
        "terminal": {
            "pool_id":"pool_00000000000000000000000000000001", "operation_id":"op_00000000000000000000000000000001",
            "authority_hash":"a".repeat(64), "registrations":"retained_registrations",
            "registered_directories":1, "original_metadata_members":1, "original_metadata_quorum":1, "pending_retirement":null,"completed_retirement":null
        }
    });
    let snapshot = storage_snapshot(value.clone()).unwrap();
    let mut state = ready();
    state.admin_console.storage.snapshot = Some(snapshot);
    assert!(!state.admin_console.storage.can_complete());
    assert!(!lifecycle::can_preview(
        &state,
        "P01",
        "disk_retained",
        storage_contract::storage_lifecycle::ExitAction::Deregister,
    ));
    let mut mismatched = value;
    mismatched["terminal"]["registrations"] = json!("empty_registry");
    assert!(storage_snapshot(mismatched).is_err());
}

#[test]
fn terminal_pending_retirement_is_recovery_not_initialization_or_completed_removal() {
    let value = json!({"state":"recovery_required","disk_ids":["disk_retained"],
        "completion_available":false,"allow_development":false,"terminal":{
        "pool_id":"pool_00000000000000000000000000000001","operation_id":"op_00000000000000000000000000000001",
        "authority_hash":"a".repeat(64),"registrations":"retained_registrations","registered_directories":1,
        "original_metadata_members":1,"original_metadata_quorum":1,"completed_retirement":null,"pending_retirement":{
        "operation_id":"op_00000000000000000000000000000002","principal":"admin",
        "pool_id":"pool_00000000000000000000000000000001","disk_id":"disk_retained","slot_id":"P02",
        "binding_generation":"1","intent_digest":"b".repeat(64),"phase":"decision_stored"}}});
    let snapshot = storage_snapshot(value.clone()).unwrap();
    assert!(snapshot.terminal.unwrap().pending_retirement.is_some());
    let mut wrong = value;
    wrong["state"] = "terminal_management".into();
    assert!(storage_snapshot(wrong).is_err());
}
