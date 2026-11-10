use super::*;

impl Notices {
    pub(crate) fn message(&self) -> Option<&str> {
        self.visible.as_ref().map(|notice| notice.message.as_str())
    }
}

fn admitted() -> State {
    let mut state = State {
        authenticated: true,
        session_epoch: 3,
        page: Page::Details,
        ..Default::default()
    };
    state.notices.begin_operation("restore-folder".into());
    state
        .notices
        .operation_success("Folder restored; other folders unchanged".into());
    state
}

fn publish(state: &mut State) {
    state
        .notices
        .record_operation(state.session_epoch, "restore-folder", SaveStatus::Published);
}

#[test]
fn folder_restore_feedback_does_not_follow_any_page_or_reappear_on_return() {
    for page in [
        Page::Files,
        Page::Status,
        Page::History,
        Page::Admin,
        Page::Account,
    ] {
        let mut state = admitted();
        publish(&mut state);
        assert_eq!(
            state.notices.message(),
            Some("Folder restored; other folders unchanged")
        );
        state.change_page(page);
        assert!(state.notices.message().is_none());
        state.change_page(Page::Details);
        publish(&mut state);
        assert!(
            state.notices.message().is_none(),
            "late receipts cannot reopen the old visit"
        );
    }
}

#[test]
fn navigating_before_completion_suppresses_the_late_success() {
    let mut state = admitted();
    state.change_page(Page::Status);
    publish(&mut state);
    assert!(state.notices.message().is_none());
}

#[test]
fn confirmed_success_expires_but_old_timers_cannot_dismiss_a_new_notice() {
    let mut state = admitted();
    publish(&mut state);
    let old = state.notices.expiry().unwrap();
    state.notices.begin_operation("second".into());
    state
        .notices
        .record_operation(3, "second", SaveStatus::Published);
    let new = state.notices.expiry().unwrap();
    assert_ne!(old, new);
    state.notices.dismiss(old);
    assert_eq!(state.notices.expiry(), Some(new));
    state.notices.dismiss(new);
    assert!(state.notices.message().is_none());
    state
        .notices
        .record_operation(3, "second", SaveStatus::Published);
    assert!(
        state.notices.message().is_none(),
        "repeated receipts do not restart success timers"
    );
}

#[test]
fn repeated_clicks_begin_a_fresh_notice_and_ignore_the_older_operation() {
    let mut state = admitted();
    publish(&mut state);
    state.notices.begin_operation("next-restore".into());
    assert!(state.notices.message().is_none());
    publish(&mut state);
    assert!(state.notices.message().is_none());
    state
        .notices
        .record_operation(3, "next-restore", SaveStatus::Published);
    assert_eq!(
        state.notices.message(),
        Some(publication_notice(SaveStatus::Published).as_str())
    );
}

#[test]
fn degraded_unknown_and_rejected_outcomes_never_claim_folder_restore_success_or_expire() {
    for status in [
        SaveStatus::Degraded,
        SaveStatus::Unknown,
        SaveStatus::Rejected,
    ] {
        let mut state = admitted();
        state.notices.record_operation(3, "restore-folder", status);
        assert_eq!(
            state.notices.message(),
            Some(publication_notice(status).as_str())
        );
        assert!(state.notices.expiry().is_none());
        let id = state.notices.visible.as_ref().unwrap().id;
        state.notices.record_operation(3, "restore-folder", status);
        assert_eq!(state.notices.visible.as_ref().unwrap().id, id);
        state.notices.dismiss(id);
        state.notices.record_operation(3, "restore-folder", status);
        assert!(
            state.notices.message().is_none(),
            "unchanged polling respects dismissal"
        );
    }
}

#[test]
fn delayed_confirmation_uses_original_success_copy_without_a_second_write() {
    let mut state = admitted();
    state
        .notices
        .record_operation(3, "restore-folder", SaveStatus::Unknown);
    assert!(state.notices.expiry().is_none());
    publish(&mut state);
    assert_eq!(
        state.notices.message(),
        Some("Folder restored; other folders unchanged")
    );
    assert!(state.notices.expiry().is_some());
}

#[test]
fn file_validation_feedback_is_not_displayed_on_unrelated_pages() {
    let mut state = admitted();
    state.change_page(Page::Account);
    state.set_file_notice("Invalid branch name".into());
    assert!(state.notices.message().is_none());
    state.change_page(Page::Details);
    state.set_file_notice("Invalid branch name".into());
    assert_eq!(state.notices.visible.as_ref().unwrap().kind, Kind::Warning);
    assert!(state.notices.expiry().is_none());
}

#[test]
fn a_replacement_sessions_notice_has_a_different_expiry_identity() {
    let mut old = admitted();
    publish(&mut old);
    let old_id = old.notices.expiry().unwrap();
    let mut replacement = admitted();
    replacement.session_epoch += 1;
    publish(&mut replacement);
    replacement.notices.dismiss(old_id);
    assert!(replacement.notices.message().is_some());
}

#[test]
fn a_request_error_clears_success_without_dropping_diagnostics_or_recovery() {
    Owner::new().with(|| {
        let mut initial = admitted();
        publish(&mut initial);
        initial
            .files
            .operations
            .push(crate::files::FileOperation::unknown("pending"));
        let state = RwSignal::new(initial);
        super::super::fail(
            state,
            crate::api::Error {
                code: storage_contract::ErrorCode::InjectedFailure,
                status: 503,
                message: "Still unavailable".into(),
                retryable: true,
                request_id: Some("latest-request".into()),
                source: None,
            },
        );
        state.update(|s| s.change_page(Page::Status));
        state.with_untracked(|s| {
            assert!(s.notices.message().is_none());
            assert_eq!(s.errors.len(), 1);
            assert_eq!(
                s.errors[0].feedback.request_id.as_deref(),
                Some("latest-request")
            );
            assert_eq!(s.files.operations[0].status(), SaveStatus::Unknown);
        });
    });
}

#[test]
fn same_page_refresh_keeps_the_current_notice_without_restarting_its_timer() {
    let mut state = admitted();
    publish(&mut state);
    let id = state.notices.expiry();
    state.change_page(Page::Details);
    assert_eq!(state.notices.expiry(), id);
}

#[test]
fn unresolved_outcome_survives_navigation_and_settles_without_late_success() {
    let mut state = admitted();
    state
        .notices
        .record_operation(3, "restore-folder", SaveStatus::Unknown);
    state.change_page(Page::Status);
    assert_eq!(
        state.notices.message(),
        Some(publication_notice(SaveStatus::Unknown).as_str())
    );
    publish(&mut state);
    assert!(state.notices.message().is_none());
    state.change_page(Page::Details);
    publish(&mut state);
    assert!(state.notices.message().is_none());
}

#[test]
fn reduced_protection_remains_visible_on_navigation_until_explicit_dismissal() {
    let mut state = admitted();
    state
        .notices
        .record_operation(3, "restore-folder", SaveStatus::Degraded);
    state.change_page(Page::Status);
    assert_eq!(
        state.notices.message(),
        Some(publication_notice(SaveStatus::Degraded).as_str())
    );
    let id = state.notices.visible.as_ref().unwrap().id;
    state.notices.dismiss(id);
    state.change_page(Page::Details);
    state
        .notices
        .record_operation(3, "restore-folder", SaveStatus::Degraded);
    assert!(state.notices.message().is_none());
}
