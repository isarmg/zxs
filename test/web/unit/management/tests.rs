use super::*;

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
