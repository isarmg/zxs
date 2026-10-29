use super::*;
use crate::files::FileOperation;
#[test]
fn session_transitions_reject_duplicate_and_overlapping_submissions() {
    let mut state = State::default();
    assert_eq!(begin_restore(&mut state), Some(1));
    assert_eq!(begin_restore(&mut state), None);
    assert_eq!(begin_login(&mut state), None);
    assert_eq!(state.session_epoch, 1);
    state.session_transition = crate::ui::SessionTransition::Idle;
    assert_eq!(begin_login(&mut state), Some(2));
    assert_eq!(begin_login(&mut state), None);
    assert_eq!(begin_restore(&mut state), None);
    assert!(state.login_in_flight);
    state.login_in_flight = false;
    state.session_transition = crate::ui::SessionTransition::LoggingOut;
    assert_eq!(begin_login(&mut state), None);
    assert_eq!(begin_restore(&mut state), None);
    assert_eq!(state.session_epoch, 2);
}
#[test]
fn content_and_account_completion_require_the_original_active_session() {
    Owner::new().with(|| {
        let state = RwSignal::new(State {
            authenticated: true,
            session_epoch: 4,
            ..Default::default()
        });
        assert!(current_session(state, 4));
        state.update(|s| s.session_transition = crate::ui::SessionTransition::LoggingOut);
        assert!(!current_session(state, 4));
        state.update(|s| {
            s.session_transition = crate::ui::SessionTransition::Idle;
            s.session_epoch = 5;
        });
        assert!(!current_session(state, 4));
        assert!(current_session(state, 5));
        end_session(state, "Password changed".into());
        assert!(!current_session(state, 5));
        state.with_untracked(|s| {
            assert_eq!(s.session_epoch, 6);
            assert_eq!(s.notice, "Password changed");
            assert!(!s.authenticated);
        });
    });
}
#[test]
fn detached_completion_rejects_a_disposed_root() {
    let owner = Owner::new();
    let state = owner.with(|| {
        RwSignal::new(State {
            authenticated: true,
            session_epoch: 4,
            ..Default::default()
        })
    });
    assert!(current_session(state, 4));
    owner.cleanup();
    assert!(!current_session(state, 4));
    assert_eq!(state.try_with_untracked(|s| s.logout_pending()), None);
}
#[test]
fn shell_identity_does_not_change_with_file_progress() {
    let mut state = State {
        authenticated: true,
        principal: "writer".into(),
        ..Default::default()
    };
    let session = session_view(&state);
    state.login_in_flight = true;
    state.password_change = crate::ui::PasswordChangeState::Pending;
    state.notice = "saving".into();
    assert_eq!(session_view(&state), session);
    assert_eq!(session.phase, SessionPhase::Authenticated);
}
#[test]
fn wrong_password_does_not_discard_pending_file_saves() {
    let mut state = State {
        authenticated: true,
        password_change: crate::ui::PasswordChangeState::Pending,
        ..Default::default()
    };
    state.files.slot = "P01".into();
    reject_password_change(
        &mut state,
        &api::Error {
            code: storage_contract::ErrorCode::CurrentPasswordInvalid,
            message: "Incorrect password".into(),
            status: 403,
            retryable: false,
            request_id: Some("req_test".into()),
            source: None,
        },
    );
    assert!(state.authenticated);
    assert_eq!(state.files.slot, "P01");
    assert!(!state.password_pending());
    assert_eq!(
        state.password_failure.unwrap().request_id.as_deref(),
        Some("req_test")
    );
}

#[test]
fn failed_session_restore_keeps_login_available_during_storage_recovery() {
    let mut state = State {
        session_failure: Some("Authentication service unavailable".into()),
        ..Default::default()
    };
    state.management.storage = crate::management::Storage::RecoveryRequired;
    assert_eq!(session_view(&state).phase, SessionPhase::Anonymous);
    assert_eq!(begin_login(&mut state), Some(1));
    assert!(!state.management.storage.writable());
}

#[test]
fn logout_clears_management_and_administration_state() {
    Owner::new().with(|| {
        let state = RwSignal::new(State {
            authenticated: true,
            session_epoch: 9,
            ..Default::default()
        });
        state.update(|s| s.management.storage = crate::management::Storage::Ready);
        end_session(state, String::new());
        state.with_untracked(|s| {
            assert_eq!(s.session_epoch, 10);
            assert!(!s.authenticated);
            assert_eq!(s.management.storage, crate::management::Storage::Unknown);
        });
    });
}

#[test]
fn language_reload_waits_for_pending_or_uncertain_work() {
    let mut state = State::default();
    assert!(!state.reload_blocked());
    state.files.mutation_pending = true;
    assert!(state.reload_blocked());
    state.files.mutation_pending = false;
    state
        .files
        .operations
        .push(FileOperation::unknown("original"));
    assert!(state.reload_blocked());
    let mut receipt: json_codec::Value = json_codec::from_str(include_str!(
        "../files/fixtures/damaged-published-save-operation.json"
    ))
    .unwrap();
    receipt["operation_id"] = "original".into();
    state.files.operations[0] = FileOperation::from_receipt("original", &receipt);
    assert!(!state.reload_blocked());
    state.password_change = crate::ui::PasswordChangeState::Pending;
    assert!(state.reload_blocked());
    state.password_change = crate::ui::PasswordChangeState::Idle;
    state.session_transition = crate::ui::SessionTransition::LogoutUnconfirmed;
    assert!(state.reload_blocked());
}

#[test]
fn language_reload_preserves_storage_retry_admission_until_resolved() {
    let mut state = State::default();
    state.management.retrying = true;
    assert!(state.reload_blocked());
    state.management.retrying = false;
    state.management.retry_unknown = true;
    assert!(state.reload_blocked());
    state.management.retry_unknown = false;
    assert!(!state.reload_blocked());
    // A known asynchronous recovery is visible in server status, not a lost
    // request outcome, and therefore does not need to prevent locale changes.
    state.management.recovering = true;
    assert!(!state.reload_blocked());
}

#[test]
fn unconfirmed_logout_cannot_be_replaced_by_login_or_restore() {
    let mut state = State {
        principal: "original-admin".into(),
        session_epoch: 7,
        session_transition: crate::ui::SessionTransition::LogoutUnconfirmed,
        ..Default::default()
    };
    for _ in 0..3 {
        assert_eq!(begin_login(&mut state), None);
        assert_eq!(begin_restore(&mut state), None);
        assert_eq!(state.session_epoch, 7);
        assert_eq!(state.principal, "original-admin");
        assert_eq!(session_view(&state).phase, SessionPhase::LogoutUnconfirmed);
        assert!(!state.session_active());
    }
    // Even stale authentication state cannot admit business work or hide retry.
    state.authenticated = true;
    assert!(!state.session_active());
    assert_eq!(session_view(&state).phase, SessionPhase::LogoutUnconfirmed);
    Owner::new().with(|| {
        let state = RwSignal::new(state);
        end_session(state, String::new());
        state.update(|s| assert!(begin_login(s).is_some()));
    });
}

#[test]
fn retrying_unconfirmed_logout_keeps_recovery_shell_until_settled() {
    Owner::new().with(|| {
        let state = RwSignal::new(State {
            session_epoch: 7,
            session_transition: SessionTransition::LogoutUnconfirmed,
            ..Default::default()
        });
        state.update(|s| {
            assert_eq!(begin_logout(s), Some(8));
            assert!(s.logout_pending());
            assert!(!s.authenticated);
            assert!(!s.session_active());
            assert_eq!(session_view(s).phase, SessionPhase::LogoutUnconfirmed);
            assert!(s.reload_blocked());
            assert_eq!(begin_login(s), None);
            assert_eq!(begin_restore(s), None);
            assert_eq!(begin_logout(s), None);
        });
        end_session(state, String::new());
        state.with_untracked(|s| {
            assert_eq!(s.session_epoch, 9);
            assert_eq!(session_view(s).phase, SessionPhase::Anonymous);
            assert!(!s.logout_pending());
            assert!(!s.reload_blocked());
        });
    });
}

#[test]
fn business_reads_do_not_start_outside_a_settled_session() {
    Owner::new().with(|| {
        for (authenticated, session_transition) in [
            (false, crate::ui::SessionTransition::Idle),
            (true, crate::ui::SessionTransition::Restoring),
            (true, crate::ui::SessionTransition::LoggingOut),
            (true, crate::ui::SessionTransition::LogoutUnconfirmed),
        ] {
            let state = RwSignal::new(State {
                authenticated,
                session_transition,
                ..Default::default()
            });
            state.update(|s| {
                s.files.slot = "P01".into();
                s.files.selected = Some(crate::files::test_entries::entry(
                    json!({"kind":"file","file_id":"file-1"}),
                ));
            });
            files::load_disks(state);
            files::load_entries(state);
            history::load(state);
            monitor::load(state);
            state.with_untracked(|s| {
                assert!(!s.files.loading_disks);
                assert!(!s.files.loading);
                assert!(!s.files.history.loading());
                assert_eq!(s.files.generation, 0);
                assert_eq!(s.files.history.generation, 0);
                assert!(!s.activity.loading);
                assert!(!s.monitor.loading);
                assert!(!s.monitor.history_loading);
            });
        }
    });
}

#[test]
fn uncertain_password_change_blocks_resubmission_and_reload_without_discarding_work() {
    for status in [0, 200, 500, 503] {
        let mut state = State {
            authenticated: true,
            password_change: crate::ui::PasswordChangeState::Pending,
            ..Default::default()
        };
        state.files.slot = "P01".into();
        reject_password_change(
            &mut state,
            &api::Error {
                status,
                code: storage_contract::ErrorCode::InjectedFailure,
                message: "Outcome unknown".into(),
                retryable: true,
                request_id: Some("req_password".into()),
                source: None,
            },
        );
        assert!(state.password_unknown());
        assert!(!state.password_pending());
        assert!(state.password_change_blocked());
        assert!(state.reload_blocked());
        assert!(state.authenticated);
        assert_eq!(state.files.slot, "P01");
        assert_eq!(
            state.password_failure.unwrap().request_id.as_deref(),
            Some("req_password")
        );
    }
}

#[test]
fn malformed_password_success_requires_signout_before_another_change() {
    Owner::new().with(|| {
        for value in [
            Value::Null,
            json!([]),
            json!({}),
            json!({"changed":true}),
            json!({"reauthentication_required":true}),
            json!({"changed":false,"reauthentication_required":true}),
            json!({"changed":true,"reauthentication_required":false}),
            json!({"changed":"true","reauthentication_required":true}),
            json!({"changed":true,"reauthentication_required":true,"admin":true}),
        ] {
            let state = RwSignal::new(State {
                authenticated: true,
                password_change: crate::ui::PasswordChangeState::Pending,
                ..Default::default()
            });
            complete_password_change(state, Ok(value));
            state.with_untracked(|s| {
                assert!(s.authenticated);
                assert!(s.password_unknown());
                assert!(s.password_change_blocked());
                assert!(s.reload_blocked());
            });
            end_session(state, String::new());
            state.with_untracked(|s| {
                assert!(!s.password_unknown());
                assert!(!s.reload_blocked());
            });
        }
    });
}

#[test]
fn exact_password_confirmation_and_unauthorized_logout_settle_the_session() {
    Owner::new().with(|| {
        let state = RwSignal::new(State {
            authenticated: true,
            password_change: PasswordChangeState::Pending,
            ..Default::default()
        });
        complete_password_change(
            state,
            Ok(json!({"changed":true,"reauthentication_required":true})),
        );
        state.with_untracked(|s| {
            assert!(!s.authenticated);
            assert_eq!(s.password_change, PasswordChangeState::Idle);
            assert!(!s.notice.is_empty());
        });
        state.update(|s| {
            s.authenticated = true;
            assert!(begin_logout(s).is_some());
        });
        complete_logout(
            state,
            Err(api::Error {
                code: storage_contract::ErrorCode::Unauthorized,
                message: "Session expired".into(),
                status: 401,
                retryable: false,
                request_id: None,
                source: None,
            }),
        );
        state.with_untracked(|s| {
            assert_eq!(session_view(s).phase, SessionPhase::Anonymous);
            assert!(!s.reload_blocked());
        });
    });
}

#[test]
fn known_password_rejection_allows_correction_but_never_erases_an_existing_fence() {
    let mut state = State {
        authenticated: true,
        password_change: crate::ui::PasswordChangeState::Pending,
        ..Default::default()
    };
    let error = api::Error {
        status: 403,
        code: storage_contract::ErrorCode::CurrentPasswordInvalid,
        message: "Incorrect password".into(),
        retryable: false,
        request_id: None,
        source: None,
    };
    reject_password_change(&mut state, &error);
    assert!(!state.password_unknown());
    assert!(!state.password_change_blocked());
    state.password_change = crate::ui::PasswordChangeState::Unknown;
    reject_password_change(&mut state, &error);
    assert!(state.password_change_blocked());
}

#[test]
fn logout_supersedes_restore_and_unknown_retry_without_overlapping_phases() {
    for transition in [
        SessionTransition::Restoring,
        SessionTransition::LogoutUnconfirmed,
    ] {
        let mut state = State {
            authenticated: true,
            session_transition: transition,
            password_change: PasswordChangeState::Unknown,
            ..Default::default()
        };
        assert_eq!(begin_logout(&mut state), Some(1));
        assert_eq!(state.session_transition, SessionTransition::LoggingOut);
        assert!(!state.restoring());
        assert!(!state.logout_unconfirmed());
        assert!(!state.session_active());
        assert!(state.reload_blocked());
        assert_eq!(state.password_change, PasswordChangeState::Unknown);
        assert_eq!(begin_logout(&mut state), None);
    }
}

#[test]
fn typed_navigation_preserves_unknown_password_and_reload_fences() {
    Owner::new().with(|| {
        let state = RwSignal::new(State {
            authenticated: true,
            password_change: PasswordChangeState::Unknown,
            ..Default::default()
        });
        for page in [Page::Account, Page::Details, Page::Files] {
            navigate(state, page);
            state.with_untracked(|s| {
                assert_eq!(s.page, page);
                assert_eq!(s.password_change, PasswordChangeState::Unknown);
                assert!(s.password_change_blocked());
                assert!(s.reload_blocked());
            });
        }
    });
}

#[test]
fn expired_password_change_session_closes_business_admission_without_losing_recovery() {
    let mut state = State {
        authenticated: true,
        password_change: PasswordChangeState::Pending,
        ..Default::default()
    };
    state.files.slot = "P01".into();
    state
        .files
        .operations
        .push(FileOperation::unknown("original"));
    reject_password_change(
        &mut state,
        &api::Error {
            status: 401,
            code: storage_contract::ErrorCode::Unauthorized,
            message: "Session expired".into(),
            retryable: false,
            request_id: Some("req_expired_password".into()),
            source: None,
        },
    );
    assert!(!state.authenticated);
    assert!(!state.session_active());
    assert!(state.password_change_blocked());
    assert_eq!(session_view(&state).phase, SessionPhase::Anonymous);
    assert_eq!(state.password_change, PasswordChangeState::Idle);
    assert_eq!(state.files.slot, "P01");
    assert_eq!(state.files.operations[0].id(), "original");
    assert!(
        state.reload_blocked(),
        "unresolved file receipts remain fenced"
    );
    assert_eq!(
        state.password_failure.unwrap().request_id.as_deref(),
        Some("req_expired_password")
    );
}

#[test]
fn logout_supersedes_pending_login_and_settlement_reopens_admission() {
    Owner::new().with(|| {
        let state = RwSignal::new(State::default());
        let login_epoch = state.try_update(begin_login).flatten().unwrap();
        state.with_untracked(|s| assert!(s.login_in_flight));
        let logout_epoch = state.try_update(begin_logout).flatten().unwrap();
        assert!(logout_epoch > login_epoch);
        state.with_untracked(|s| {
            assert_ne!(s.session_epoch, login_epoch);
            assert!(s.logout_pending());
        });
        state.update(|s| assert_eq!(begin_login(s), None));
        end_session(state, String::new());
        state.with_untracked(|s| assert!(!s.login_in_flight));
        state.update(|s| assert!(begin_login(s).unwrap() > logout_epoch));
    });
}
