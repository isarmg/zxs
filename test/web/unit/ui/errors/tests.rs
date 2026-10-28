use super::*;

fn failure(request: &str) -> api::Error {
    api::Error {
        code: storage_contract::ErrorCode::RecoveryRequired,
        status: 503,
        message: format!("failure {request}"),
        request_id: Some(request.into()),
        retryable: true,
        source: None,
    }
}

fn operation(id: &str) -> Option<(u64, ErrorContext)> {
    Some((7, ErrorContext::Operation(id.into())))
}

#[test]
fn repeat_operation_errors_keep_one_notice_and_latest_request_details() {
    let mut state = State {
        session_epoch: 7,
        ..Default::default()
    };
    state.report_error(failure("first"), operation("original"));
    let notice_id = state.errors[0].id;
    for attempt in 0..20 {
        state.report_error(
            failure(&format!("attempt-{attempt}")),
            operation("original"),
        );
    }
    assert_eq!(state.errors.len(), 1);
    assert_eq!(state.errors[0].id, notice_id);
    assert_eq!(
        state.errors[0].feedback.request_id.as_deref(),
        Some("attempt-19")
    );
    assert!(
        state.errors[0]
            .feedback
            .message
            .contains("failure attempt-19")
    );
}

#[test]
fn operation_resource_session_and_error_identity_are_not_conflated() {
    let mut state = State {
        session_epoch: 7,
        ..Default::default()
    };
    state.report_error(failure("one"), operation("original"));
    state.report_error(failure("two"), operation("another"));
    state.report_error(
        failure("read"),
        Some((7, ErrorContext::Read("original".into()))),
    );
    let mut changed = failure("changed-code");
    changed.code = storage_contract::ErrorCode::TransportError;
    state.report_error(changed, operation("original"));
    let mut changed = failure("changed-status");
    changed.status = 500;
    state.report_error(changed, operation("original"));
    assert_eq!(state.errors.len(), 5);
    state.session_epoch = 8;
    state.report_error(
        failure("replacement"),
        Some((8, ErrorContext::Operation("original".into()))),
    );
    assert_eq!(state.errors.len(), 5);
    assert_eq!(state.errors[0].key.as_ref().unwrap().session_epoch, 8);
    // A late old-session failure cannot evict notices or revoke its replacement.
    state.authenticated = true;
    let mut expired = failure("old-expired");
    expired.status = 401;
    state.report_error(expired, operation("original"));
    assert!(state.authenticated);
    assert_eq!(
        state.errors[0].feedback.request_id.as_deref(),
        Some("replacement")
    );
}

#[test]
fn reads_deduplicate_only_the_exact_resource_and_unscoped_actions_remain_distinct() {
    let mut state = State::default();
    for request in ["old", "new"] {
        state.report_error(
            failure(request),
            Some((0, ErrorContext::Read("/api/disks?slot=P01".into()))),
        );
    }
    state.report_error(
        failure("other-disk"),
        Some((0, ErrorContext::Read("/api/disks?slot=B01".into()))),
    );
    state.report_error(failure("same-request-id"), None);
    state.report_error(failure("same-request-id"), None);
    assert_eq!(state.errors.len(), 4);
    assert_ne!(state.errors[0].id, state.errors[1].id);
    assert_eq!(state.errors[3].feedback.request_id.as_deref(), Some("new"));
}

#[test]
fn stable_dismissal_survives_reordering_and_allows_reappearance() {
    let mut state = State {
        session_epoch: 7,
        ..Default::default()
    };
    state.report_error(failure("one"), operation("one"));
    let first = state.errors[0].id;
    state.report_error(failure("two"), operation("two"));
    let second = state.errors[0].id;
    state.report_error(failure("one-again"), operation("one"));
    state.dismiss_error(second);
    assert_eq!(state.errors.len(), 1);
    assert_eq!(state.errors[0].id, first);
    state.dismiss_error(first);
    assert!(state.errors.is_empty());
    state.report_error(failure("reappeared"), operation("one"));
    assert_eq!(state.errors.len(), 1);
    assert_ne!(state.errors[0].id, first);
    state.dismiss_error(first);
    assert_eq!(
        state.errors.len(),
        1,
        "an old dismissal must not remove a new notice"
    );
}

#[test]
fn old_dismissal_cannot_remove_a_replacement_sessions_notice() {
    let mut state = State {
        session_epoch: 7,
        ..Default::default()
    };
    state.report_error(failure("old-session"), None);
    let old = state.errors[0].id;
    state = State {
        session_epoch: 8,
        ..Default::default()
    };
    state.report_error(failure("new-session"), None);
    state.dismiss_error(old);
    assert_eq!(state.errors.len(), 1);
    assert_eq!(
        state.errors[0].feedback.request_id.as_deref(),
        Some("new-session")
    );
}

#[test]
fn same_operation_in_another_session_does_not_replace_the_old_sessions_notice() {
    let mut state = State {
        session_epoch: 7,
        ..Default::default()
    };
    state.report_error(failure("old-session"), operation("original"));
    let old = state.errors[0].id;
    state.session_epoch = 8;
    state.report_error(
        failure("new-session"),
        Some((8, ErrorContext::Operation("original".into()))),
    );
    assert_eq!(state.errors.len(), 2);
    assert_ne!(state.errors[0].id, old);
    assert_eq!(state.errors[1].id, old);
}

#[test]
fn notices_are_bounded_and_current_unauthorized_failure_still_revokes_session() {
    let mut state = State {
        session_epoch: 7,
        authenticated: true,
        ..Default::default()
    };
    for index in 0..10 {
        state.report_error(failure(&index.to_string()), operation(&index.to_string()));
    }
    assert_eq!(state.errors.len(), 5);
    assert_eq!(state.errors[4].feedback.request_id.as_deref(), Some("5"));
    let mut expired = failure("expired");
    expired.status = 401;
    state.report_error(expired.clone(), operation("expired"));
    let id = state.errors[0].id;
    state.authenticated = true;
    state.report_error(expired, operation("expired"));
    assert!(!state.authenticated);
    assert_eq!(state.errors.len(), 5);
    assert_eq!(state.errors[0].id, id);
}
