use super::*;

#[test]
fn unconfigured_storage_is_setup_not_a_global_recovery_notice() {
    let mut model = Model::default();
    accept(
        &mut model,
        &json!({"authentication":"ready", "storage":{
            "state":"unconfigured", "retryable":true, "recovering":false
        }}),
    );
    assert!(!model.needs_attention());
    assert!(!model.retry_visible());
    assert!(!model.storage.readable());
    assert!(!model.storage.writable());
}

#[test]
fn storage_notices_and_retry_controls_follow_observed_states() {
    for (storage, retryable, notice, retry) in [
        (Storage::Ready, false, false, false),
        (Storage::Unconfigured, true, false, false),
        (Storage::Unknown, false, true, false),
        (Storage::ReadOnly, true, true, true),
        (Storage::Unavailable, true, true, true),
        (Storage::RecoveryRequired, true, true, true),
        (Storage::TerminalManagement, false, true, false),
    ] {
        let model = Model {
            storage,
            retryable,
            ..Default::default()
        };
        assert_eq!(model.needs_attention(), notice, "{storage:?}");
        assert_eq!(model.retry_visible(), retry, "{storage:?}");
    }
}

#[test]
fn quiet_storage_never_hides_failures_or_unsettled_recovery() {
    for storage in [Storage::Ready, Storage::Unconfigured] {
        for model in [
            Model {
                storage,
                auth_recovery: true,
                ..Default::default()
            },
            Model {
                storage,
                failure: Some(Feedback {
                    message: "status request failed".into(),
                    request_id: Some("failed-status".into()),
                }),
                ..Default::default()
            },
        ] {
            assert!(model.needs_attention());
            assert!(!model.retry_visible());
        }
        for model in [
            Model {
                storage,
                retrying: true,
                ..Default::default()
            },
            Model {
                storage,
                recovering: true,
                ..Default::default()
            },
            Model {
                storage,
                retry_unknown: true,
                ..Default::default()
            },
        ] {
            assert!(model.needs_attention());
            assert!(model.retry_visible());
        }
    }
}

#[test]
fn only_explicit_ready_status_admits_writes() {
    for (wire, expected) in [
        ("ready", Storage::Ready),
        ("read_only", Storage::ReadOnly),
        ("unavailable", Storage::Unavailable),
        ("unconfigured", Storage::Unconfigured),
        ("recovery_required", Storage::RecoveryRequired),
        ("future_state", Storage::Unknown),
    ] {
        let state = Storage::parse(&json!({"state":wire}));
        assert_eq!(state, expected);
        assert_eq!(state.writable(), wire == "ready");
    }
    assert!(!Storage::parse(&Value::Null).writable());
    assert!(!Storage::default().writable());
}

#[test]
fn missing_authentication_never_becomes_new_installation() {
    let mut model = Model::default();
    accept(
        &mut model,
        &json!({"authentication":"recovery_required", "storage":{"state":"unavailable", "retryable":true}}),
    );
    assert!(model.auth_recovery);
    assert_eq!(model.storage, Storage::Unavailable);
    assert_ne!(model.storage, Storage::Unconfigured);
    assert!(model.retryable);
    assert!(!model.storage.writable());
}

#[test]
fn health_failure_does_not_revoke_session_or_overwrite_data_outcomes() {
    let mut state = State {
        authenticated: true,
        principal: "admin".into(),
        ..Default::default()
    };
    state.management.storage = Storage::RecoveryRequired;
    assert!(state.session_active());
    assert_eq!(state.principal, "admin");
    state.management.storage = Storage::ReadOnly;
    assert!(state.session_active());
    assert!(state.management.storage.readable());
    assert!(!state.management.storage.writable());
}

#[test]
fn authentication_lock_contention_is_not_recovery_or_session_revocation() {
    let mut state = State {
        authenticated: true,
        principal: "admin".into(),
        ..Default::default()
    };
    accept(
        &mut state.management,
        &json!({"authentication":"busy", "storage":{"state":"ready"}}),
    );
    assert!(!state.management.auth_recovery);
    assert!(state.session_active());
    assert_eq!(state.principal, "admin");
}

#[test]
fn status_reads_wait_until_storage_retry_settles() {
    Owner::new().with(|| {
        let state = RwSignal::new(State {
            authenticated: true,
            ..Default::default()
        });
        state.update(|s| {
            s.management.retrying = true;
            s.management.storage = Storage::Unavailable;
            s.management.generation = 7;
        });
        // The timer and the manual action share this admission boundary.
        // An overlapping read could otherwise return pre-retry state and
        // incorrectly clear retry_unknown after the POST fails ambiguously.
        load(state);
        state.with_untracked(|s| {
            assert!(!s.management.loading);
            assert_eq!(s.management.generation, 7);
            assert_eq!(s.management.storage, Storage::Unavailable);
            assert!(s.management.retrying);
        });
        state.update(|s| {
            s.management.retrying = false;
            s.management.retry_unknown = true;
        });
        load(state);
        state.with_untracked(|s| {
            assert!(s.management.loading);
            assert_eq!(s.management.generation, 8);
            // Admission alone cannot resolve an uncertain write.
            assert!(s.management.retry_unknown);
        });
    });
}

#[test]
fn an_empty_authorized_disk_list_does_not_poll_again_after_each_health_check() {
    let mut state = State {
        authenticated: true,
        ..Default::default()
    };
    state.management.storage = Storage::Ready;
    assert!(
        needs_disk_refresh(&state, true),
        "initial listing is still required"
    );
    state.files.disks_loaded = true;
    for _ in 0..12 {
        accept(
            &mut state.management,
            &json!({"authentication":"ready", "storage":{"state":"ready"}}),
        );
        assert!(
            !needs_disk_refresh(&state, true),
            "a successful empty listing is not an unfetched listing"
        );
    }
}

#[test]
fn failed_disk_reads_retry_and_storage_recovery_refreshes_loaded_disks() {
    let mut state = State {
        authenticated: true,
        ..Default::default()
    };
    state.management.storage = Storage::Ready;
    // A failed first request does not mark the listing loaded.
    assert!(needs_disk_refresh(&state, true));
    state.files.disks_loaded = true;
    for storage in [Storage::Ready, Storage::ReadOnly] {
        state.management.storage = storage;
        assert!(
            needs_disk_refresh(&state, false),
            "recovery refreshes even an empty list"
        );
    }
    for storage in [
        Storage::Unknown,
        Storage::Unavailable,
        Storage::RecoveryRequired,
    ] {
        state.management.storage = storage;
        assert!(!needs_disk_refresh(&state, false));
    }
    state.management.storage = Storage::Ready;
    state.authenticated = false;
    assert!(!needs_disk_refresh(&state, false));
}

#[test]
fn recovery_invalidates_inflight_disk_snapshots_but_repeated_ready_polls_do_not() {
    for storage in [Storage::Ready, Storage::ReadOnly] {
        let mut state = State {
            authenticated: true,
            ..Default::default()
        };
        state.management.storage = Storage::Unavailable;
        state.files.disks_loaded = true;
        state.files.loading_disks = true;
        state.files.disks_generation = 7;
        let was_readable = state.management.storage.readable();
        state.management.storage = storage;
        invalidate_restored_disks(&mut state, was_readable);
        assert_eq!(state.files.disks_generation, 8);
        assert!(!state.files.disks_loaded);
        assert!(
            state.files.loading_disks,
            "the old read keeps its admission until completion"
        );
        assert!(needs_disk_refresh(&state, was_readable));
        for _ in 0..3 {
            invalidate_restored_disks(&mut state, true);
            assert_eq!(state.files.disks_generation, 8);
            assert!(needs_disk_refresh(&state, true));
        }
        state.files.loading_disks = false;
        state.files.disks_loaded = true;
        assert!(!needs_disk_refresh(&state, true));
    }
}

#[test]
fn an_already_ready_retry_response_retains_refresh_demand_for_the_following_status_read() {
    let mut state = State {
        authenticated: true,
        ..Default::default()
    };
    state.management.storage = Storage::Unavailable;
    state.files.disks_loaded = true;
    // The retry POST may itself report Ready before the subsequent status GET.
    let was_readable = state.management.storage.readable();
    state.management.storage = Storage::Ready;
    invalidate_restored_disks(&mut state, was_readable);
    assert!(!state.files.disks_loaded);
    assert_eq!(state.files.disks_generation, 1);

    let was_readable = state.management.storage.readable();
    accept(&mut state.management, &json!({"storage":{"state":"ready"}}));
    invalidate_restored_disks(&mut state, was_readable);
    assert_eq!(state.files.disks_generation, 1);
    assert!(needs_disk_refresh(&state, was_readable));
}

#[test]
fn global_status_changes_preserve_the_current_page_editors_and_unknown_operations() {
    for page in [crate::ui::Page::Files, crate::ui::Page::Details] {
        for storage in ["unavailable", "recovery_required"] {
            let mut state = State {
                authenticated: true,
                page,
                ..Default::default()
            };
            state.management.storage = Storage::Ready;
            state.files.history_editors = 1;
            state
                .files
                .operations
                .push(crate::files::FileOperation::unknown("original"));
            let response = json!({"authentication":"ready","storage":{
                "state":storage,"retryable":true,"recovering":false
            }});
            assert!(!accept_status(&mut state, &response));
            assert_eq!(
                state.page, page,
                "automatic navigation must not dispose the draft owner"
            );
            assert_eq!(state.files.history_editors, 1);
            assert!(state.management.retryable);
            assert!(!state.management.storage.writable());
            assert!(crate::files::pending(&state));
            assert_eq!(state.files.operations[0].id(), "original");
            // Closing the editor does not make a later status poll take over navigation.
            state.files.history_editors = 0;
            accept_status(&mut state, &response);
            assert_eq!(state.page, page);
        }
    }
}

fn folder_refresh_fixture() -> State {
    let mut state = State {
        authenticated: true,
        session_epoch: 3,
        page: crate::ui::Page::Files,
        ..Default::default()
    };
    state.management.storage = Storage::Ready;
    state.files.slot = "slot_00000000000000000000000000000001".into();
    state.files.generation = 7;
    state.files.loaded = true;
    state.files.writable = true;
    state.files.disks_loaded = true;
    state
}
fn status_response(storage: &str) -> Result<Value, api::Error> {
    Ok(json!({"authentication":"ready","storage":{
        "state":storage,"retryable":true,"recovering":false
    }}))
}

#[test]
fn invalidated_health_read_keeps_admission_and_cannot_restore_cached_ready() {
    let mut state = folder_refresh_fixture();
    let read = begin_load(&mut state, Refresh::Status).unwrap();
    state.management.invalidate_storage();
    assert_eq!(state.management.storage, Storage::Unknown);
    assert!(state.management.loading);
    assert!(begin_load(&mut state, Refresh::Status).is_none());
    assert!(begin_load(&mut state, Refresh::CurrentFolder).is_none());
    assert_eq!(
        complete_load(&mut state, &read, status_response("ready")),
        StatusCompletion::Superseded
    );
    assert!(!state.management.loading);
    assert_eq!(state.management.storage, Storage::Unknown);
    let fresh = begin_load(&mut state, Refresh::Status).unwrap();
    assert_eq!(
        complete_load(&mut state, &fresh, status_response("recovery_required")),
        StatusCompletion::Accepted {
            disks: false,
            folder: false
        }
    );
    assert_eq!(state.management.storage, Storage::RecoveryRequired);
    assert!(state.management.retryable);
}

#[test]
fn explicit_folder_recovery_waits_for_readable_health_without_settling_mutations() {
    for (storage, readable) in [
        ("ready", true),
        ("read_only", true),
        ("recovery_required", false),
        ("unavailable", false),
        ("unconfigured", false),
        ("terminal_management", false),
        ("unknown", false),
    ] {
        let mut state = folder_refresh_fixture();
        let read = begin_load(&mut state, Refresh::CurrentFolder).unwrap();
        assert!(state.management.loading);
        assert_eq!(state.management.storage, Storage::Unknown);
        assert!(!state.management.storage.writable());
        assert!(
            !state.files.loading,
            "health must precede the folder request"
        );
        assert_eq!(
            complete_load(&mut state, &read, status_response(storage)),
            StatusCompletion::Accepted {
                disks: readable,
                folder: readable
            },
            "{storage}"
        );
        assert_eq!(state.management.storage.writable(), storage == "ready");
        assert!(
            state.files.operations.is_empty(),
            "health is not a mutation receipt"
        );
        assert!(
            !state.management.retrying,
            "checking health never retries storage"
        );
    }
}

#[test]
fn folder_refresh_failure_preserves_diagnostic_and_does_not_request_entries() {
    let mut state = folder_refresh_fixture();
    let read = begin_load(&mut state, Refresh::CurrentFolder).unwrap();
    let mut error = api::error(
        storage_contract::ErrorCode::RecoveryRequired,
        "disk requires recovery",
    );
    error.status = 503;
    error.request_id = Some("health-failure".into());
    assert_eq!(
        complete_load(&mut state, &read, Err(error)),
        StatusCompletion::Accepted {
            disks: false,
            folder: false
        }
    );
    assert_eq!(state.management.storage, Storage::Unknown);
    assert_eq!(
        state
            .management
            .failure
            .as_ref()
            .unwrap()
            .request_id
            .as_deref(),
        Some("health-failure")
    );
    assert!(!state.management.loading);
}

#[test]
fn navigation_or_new_mutation_cancels_only_the_requested_folder_refresh() {
    for change in 0..4 {
        let mut state = folder_refresh_fixture();
        let read = begin_load(&mut state, Refresh::CurrentFolder).unwrap();
        match change {
            0 => state.files.generation += 1,
            1 => state.change_page(crate::ui::Page::Admin),
            2 => state.files.mutation_pending = true,
            3 => state.files.loading = true,
            _ => unreachable!(),
        }
        assert_eq!(
            complete_load(&mut state, &read, status_response("ready")),
            StatusCompletion::Accepted {
                disks: true,
                folder: false
            }
        );
        assert_eq!(state.management.storage, Storage::Ready);
    }
}

#[test]
fn stale_session_status_cannot_release_new_read_or_refresh_a_folder() {
    let mut state = folder_refresh_fixture();
    let read = begin_load(&mut state, Refresh::CurrentFolder).unwrap();
    state.session_epoch += 1;
    state.management.storage = Storage::ReadOnly;
    assert_eq!(
        complete_load(&mut state, &read, status_response("ready")),
        StatusCompletion::Obsolete
    );
    assert!(state.management.loading);
    assert_eq!(state.management.storage, Storage::ReadOnly);
}

#[test]
fn explicit_folder_health_refresh_has_single_admission_and_respects_pending_writes() {
    let mut state = folder_refresh_fixture();
    state.files.mutation_pending = true;
    assert!(begin_load(&mut state, Refresh::CurrentFolder).is_none());
    assert_eq!(state.management.storage, Storage::Ready);
    state.files.mutation_pending = false;
    let read = begin_load(&mut state, Refresh::CurrentFolder).unwrap();
    let generation = state.management.generation;
    assert!(begin_load(&mut state, Refresh::CurrentFolder).is_none());
    assert_eq!(state.management.generation, generation);
    complete_load(&mut state, &read, status_response("ready"));
    assert!(begin_load(&mut state, Refresh::CurrentFolder).is_some());
}

#[test]
fn normal_health_poll_does_not_request_folder_reload_or_disable_known_ready() {
    let mut state = folder_refresh_fixture();
    let read = begin_load(&mut state, Refresh::Status).unwrap();
    assert_eq!(state.management.storage, Storage::Ready);
    assert_eq!(
        complete_load(&mut state, &read, status_response("ready")),
        StatusCompletion::Accepted {
            disks: false,
            folder: false
        }
    );
}
