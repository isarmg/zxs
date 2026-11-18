use super::*;
use json_codec::Value;

fn status_wire(authentication: &str, storage: &str, retryable: bool, recovering: bool) -> Value {
    json!({
        "management": "alive",
        "authentication": authentication,
        "storage": {"state": storage, "retryable": retryable, "recovering": recovering},
        "auxiliary": {"http_uploads": "ready", "web_assets": "ready"}
    })
}

fn status_fixture(
    authentication: &str,
    storage: &str,
    retryable: bool,
    recovering: bool,
) -> response::Status {
    response::status(status_wire(authentication, storage, retryable, recovering)).unwrap()
}

#[test]
fn unconfigured_storage_is_setup_not_a_global_recovery_notice() {
    let mut model = Model::default();
    accept(
        &mut model,
        status_fixture("ready", "unconfigured", true, false),
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
        ("terminal_management", Storage::TerminalManagement),
    ] {
        let mut model = Model::default();
        accept(&mut model, status_fixture("ready", wire, true, false));
        assert_eq!(model.storage, expected);
        assert_eq!(model.storage.writable(), wire == "ready");
    }
    for value in [
        Value::Null,
        json!({"state":"future_state", "retryable":true,"recovering":false}),
    ] {
        assert!(response::storage(value).is_err());
    }
    assert!(!Storage::default().writable());
}

#[test]
fn authentication_recovery_never_becomes_new_installation() {
    let mut model = Model::default();
    accept(
        &mut model,
        status_fixture("recovery_required", "unavailable", true, false),
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
        principal: Some("admin".parse().unwrap()),
        ..Default::default()
    };
    state.management.storage = Storage::RecoveryRequired;
    assert!(state.session_active());
    assert_eq!(state.principal.as_ref().map(|p| p.as_str()), Some("admin"));
    state.management.storage = Storage::ReadOnly;
    assert!(state.session_active());
    assert!(state.management.storage.readable());
    assert!(!state.management.storage.writable());
}

#[test]
fn authentication_lock_contention_is_not_recovery_or_session_revocation() {
    let mut state = State {
        authenticated: true,
        principal: Some("admin".parse().unwrap()),
        ..Default::default()
    };
    accept(
        &mut state.management,
        status_fixture("busy", "ready", false, false),
    );
    assert!(!state.management.auth_recovery);
    assert!(state.session_active());
    assert_eq!(state.principal.as_ref().map(|p| p.as_str()), Some("admin"));
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
            status_fixture("ready", "ready", false, false),
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
    accept(
        &mut state.management,
        status_fixture("ready", "ready", false, false),
    );
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
            assert!(!accept_status(
                &mut state,
                status_fixture("ready", storage, true, false)
            ));
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
            accept_status(&mut state, status_fixture("ready", storage, true, false));
            assert_eq!(state.page, page);
        }
    }
}

fn active_files_status_fixture() -> State {
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
fn status_response(storage: &str) -> Result<response::Status, api::Error> {
    response::status(status_wire("ready", storage, true, false))
}

#[test]
fn invalidated_health_read_keeps_admission_and_cannot_restore_cached_ready() {
    let mut state = active_files_status_fixture();
    let read = begin_load(&mut state).unwrap();
    state.management.invalidate_storage();
    assert_eq!(state.management.storage, Storage::Unknown);
    assert!(state.management.loading);
    assert!(begin_load(&mut state).is_none());
    assert_eq!(
        complete_load(&mut state, &read, status_response("ready")),
        StatusCompletion::Superseded
    );
    assert!(!state.management.loading);
    assert_eq!(state.management.storage, Storage::Unknown);
    let fresh = begin_load(&mut state).unwrap();
    assert_eq!(
        complete_load(&mut state, &fresh, status_response("recovery_required")),
        StatusCompletion::Accepted { disks: false }
    );
    assert_eq!(state.management.storage, Storage::RecoveryRequired);
    assert!(state.management.retryable);
}

#[test]
fn health_read_failure_preserves_diagnostic_and_does_not_request_disks() {
    let mut state = active_files_status_fixture();
    let read = begin_load(&mut state).unwrap();
    let mut error = api::error(
        storage_contract::ErrorCode::RecoveryRequired,
        "disk requires recovery",
    );
    error.status = 503;
    error.request_id = Some("health-failure".into());
    assert_eq!(
        complete_load(&mut state, &read, Err(error)),
        StatusCompletion::Accepted { disks: false }
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
fn stale_session_status_cannot_release_the_new_health_read() {
    let mut state = active_files_status_fixture();
    let read = begin_load(&mut state).unwrap();
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
fn normal_health_poll_preserves_a_loaded_ready_view() {
    let mut state = active_files_status_fixture();
    let read = begin_load(&mut state).unwrap();
    assert_eq!(state.management.storage, Storage::Ready);
    assert_eq!(
        complete_load(&mut state, &read, status_response("ready")),
        StatusCompletion::Accepted { disks: false }
    );
}

#[test]
fn malformed_status_keeps_recovery_fences_and_does_not_refresh_files() {
    for field in ["management", "authentication", "storage", "auxiliary"] {
        let mut value = status_wire("ready", "ready", false, false);
        value.as_object_mut().unwrap().remove(field);
        let mut state = active_files_status_fixture();
        state.management.auth_recovery = true;
        state.management.recovering = true;
        state.management.retry_unknown = true;
        let read = begin_load(&mut state).unwrap();
        assert_eq!(
            complete_load(&mut state, &read, response::status(value)),
            StatusCompletion::Accepted { disks: false },
            "{field}"
        );
        assert_eq!(state.management.storage, Storage::Unknown);
        assert!(state.management.auth_recovery);
        assert!(state.management.recovering);
        assert!(state.management.retry_unknown);
        assert!(state.management.failure.is_some());
        assert!(!state.management.loading);
        assert!(state.session_active());
        assert_eq!(state.files.disks_generation, 0);
    }
}

#[test]
fn storage_response_accepts_current_embedded_and_managed_forms_only() {
    let mut model = Model {
        recovering: true,
        ..Default::default()
    };
    response::storage(json!({"state":"ready", "retryable":false}))
        .unwrap()
        .apply(&mut model);
    assert_eq!(model.storage, Storage::Ready);
    assert!(!model.recovering);
    assert!(!model.retryable);
    for recovering in [true, false] {
        response::storage(
            json!({"state":"unavailable", "retryable":true, "recovering":recovering}),
        )
        .unwrap()
        .apply(&mut model);
        assert_eq!(model.storage, Storage::Unavailable);
        assert_eq!(model.recovering, recovering);
        assert!(model.retryable);
    }
    for value in [
        json!({"state":"ready"}),
        json!({"state":"unavailable", "retryable":true}),
        json!({"state":"ready", "retryable":false, "recovering":null}),
        json!({"state":"ready", "retryable":"false", "recovering":false}),
        json!({"state":"ready", "retryable":false, "recovering":"false"}),
        json!({"state":"unknown", "retryable":false, "recovering":false}),
        json!({"state":"ready", "retryable":false, "recovering":false, "extra":true}),
    ] {
        let error = response::storage(value).unwrap_err();
        assert_eq!(error.code, storage_contract::ErrorCode::InvalidResponse);
        assert_eq!(error.status, 200);
        assert!(std::error::Error::source(&error).is_some());
        assert!(error.mutation_outcome_unknown());
    }
    for (field, invalid) in [
        ("management", json!("starting")),
        ("authentication", json!("unknown")),
        ("authentication", Value::Null),
    ] {
        let mut value = status_wire("ready", "ready", false, false);
        value[field] = invalid;
        assert!(response::status(value).is_err());
    }
}

#[test]
fn malformed_retry_stays_unknown_until_a_valid_status_observation() {
    let mut state = active_files_status_fixture();
    state.management.retrying = true;
    complete_retry(&mut state, response::storage(json!({"state":"ready"})));
    assert!(!state.management.retrying);
    assert!(state.management.retry_unknown);
    assert_eq!(state.management.storage, Storage::Unknown);
    assert!(state.management.failure.is_some());
    assert_eq!(state.files.disks_generation, 0);
    let read = begin_load(&mut state).unwrap();
    complete_load(&mut state, &read, response::status(Value::Null));
    assert!(state.management.retry_unknown);
    let read = begin_load(&mut state).unwrap();
    assert_eq!(
        complete_load(&mut state, &read, status_response("ready")),
        StatusCompletion::Accepted { disks: true }
    );
    assert!(!state.management.retry_unknown);
    assert!(state.management.failure.is_none());
    assert!(state.management.storage.writable());
}

#[test]
fn typed_retry_retains_restored_disk_demand_and_original_error_details() {
    let mut state = active_files_status_fixture();
    state.management.storage = Storage::Unavailable;
    state.management.retrying = true;
    complete_retry(
        &mut state,
        response::storage(json!({"state":"ready", "retryable":false})),
    );
    assert_eq!(state.management.storage, Storage::Ready);
    assert!(!state.files.disks_loaded);
    assert_eq!(state.files.disks_generation, 1);
    let mut error = api::error(
        storage_contract::ErrorCode::RecoveryRequired,
        "disk unavailable",
    );
    error.status = 503;
    error.request_id = Some("original-retry".into());
    complete_retry(&mut state, Err(error));
    assert!(state.management.retry_unknown);
    assert_eq!(
        state.management.failure.unwrap().request_id.as_deref(),
        Some("original-retry")
    );
}
