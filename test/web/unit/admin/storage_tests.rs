use super::*;
use storage_contract::storage_directories::{DirectoryIdentity, MountIdentity};

fn candidate(root: &str) -> CandidateSelection {
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
fn activation_requires_inspected_selection_and_explicit_current_confirmation() {
    let mut state = ready();
    let model = &mut state.admin_console.storage;
    assert!(model.editable());
    assert!(!model.can_activate());
    model.selections.push(candidate("/mnt/disk/subdir"));
    assert!(!model.can_activate());
    model.confirmed = true;
    assert!(model.can_activate());
    model.unknown = true;
    assert!(!model.can_activate() && !model.editable());
    model.unknown = false;
    model.snapshot.as_mut().unwrap().state = StorageState::Active;
    assert!(!model.can_activate());
}

#[test]
fn a_changed_mount_or_storage_directory_invalidates_inspection_not_selected_entries() {
    Owner::new().with(|| {
        let mut initial = ready();
        initial.admin_console.storage.inspected = Some(candidate("/mnt/disk/a"));
        initial
            .admin_console
            .storage
            .selections
            .push(candidate("/mnt/disk/b"));
        initial.admin_console.storage.confirmed = true;
        let state = RwSignal::new(initial);
        change(state, "root", "/mnt/disk/another".into());
        state.with_untracked(|s| {
            assert!(s.admin_console.storage.inspected.is_none());
            assert!(!s.admin_console.storage.confirmed);
            assert_eq!(s.admin_console.storage.selections.len(), 1);
            assert_eq!(s.admin_console.storage.root_path, "/mnt/disk/another");
        });
    });
}

#[test]
fn duplicate_selection_and_pending_changes_do_not_repeat_or_rewrite_activation() {
    Owner::new().with(|| {
        let mut initial = ready();
        initial.admin_console.storage.inspected = Some(candidate("/mnt/disk/subdir"));
        let state = RwSignal::new(initial);
        add_selection(state);
        state.update(|s| s.admin_console.storage.inspected = Some(candidate("/mnt/disk/subdir")));
        add_selection(state);
        assert_eq!(
            state.with_untracked(|s| s.admin_console.storage.selections.len()),
            1
        );
        state.update(|s| {
            s.admin_console.storage.confirmed = true;
            assert_eq!(begin(s, true), Some((7, 1)));
        });
        change(state, "root", "must-not-change".into());
        remove_selection(state, "selection-/mnt/disk/subdir");
        state.with_untracked(|s| {
            assert!(s.admin_console.storage.root_path.is_empty());
            assert_eq!(s.admin_console.storage.selections.len(), 1);
            assert!(s.admin_console.reload_blocked());
        });
        assert_eq!(state.try_update(|s| begin(s, true)).flatten(), None);
    });
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
    assert!(!model.can_activate() && !model.can_complete());
    model.snapshot.as_mut().unwrap().completion_available = true;
    assert!(model.can_complete());
    finish_activation(&mut model, Err(failure(409)));
    assert!(
        model.unknown,
        "a later rejected completion cannot disprove the original activation"
    );
    finish_activation(&mut model, Ok(Success::Recovering));
    assert!(!model.unknown && !model.reload_blocked());
    assert!(!model.can_activate() && !model.can_complete());
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
fn discovery_setting_is_available_only_with_current_known_state() {
    let mut model = ready().admin_console.storage;
    assert!(!model.can_set_root());
    model.discovery_root = "/zxs/data/disks".into();
    assert!(model.can_set_root());
    model.discovery_unknown = true;
    assert!(!model.can_set_root() && !model.editable() && model.reload_blocked());
    model.discovery_unknown = false;
    model.snapshot.as_mut().unwrap().state = StorageState::Active;
    assert!(model.can_set_root());
    assert!(!model.can_activate());
    model.snapshot.as_mut().unwrap().state = StorageState::RecoveryRequired;
    assert!(!model.can_set_root());
}

#[test]
fn storage_diagnostics_do_not_hide_existing_disk_cards() {
    for status in [
        StorageState::Active,
        StorageState::RecoveryRequired,
        StorageState::Unavailable,
    ] {
        for settings_succeed in [false, true] {
            let mut model = ready().admin_console.storage;
            let mut snapshot = model.snapshot.clone().unwrap();
            snapshot.state = status;
            let settings = if settings_succeed {
                Ok(DiscoveryRootSettings {
                    discovery_root: "/zxs/data/disks".into(),
                })
            } else {
                Err(failure(503))
            };
            assert!(finish_load(&mut model, Ok(snapshot), settings));
            assert_eq!(model.snapshot.as_ref().unwrap().state, status);
            assert_eq!(model.error.is_some(), !settings_succeed);
        }
    }
    let mut model = ready().admin_console.storage;
    assert!(finish_load(
        &mut model,
        Err(failure(503)),
        Err(failure(503))
    ));
    assert!(model.error.is_some());
}

#[test]
fn management_only_status_keeps_an_empty_disk_view_even_if_discovery_settings_fail() {
    let mut model = ready().admin_console.storage;
    let snapshot = model.snapshot.clone().unwrap();
    assert_eq!(snapshot.state, StorageState::Pending);
    assert!(!finish_load(&mut model, Ok(snapshot), Err(failure(503))));
    assert_eq!(
        model.snapshot.as_ref().unwrap().state,
        StorageState::Pending
    );
    assert!(model.error.is_some());
}
