use super::*;
use json_codec::json;

fn ready() -> Model {
    Model {
        username: "admin".into(),
        password: "password".into(),
        status: Status::Empty,
        screen: Screen::CreateAccount,
        ..Default::default()
    }
}

#[test]
fn first_setup_needs_no_disks_and_never_sends_storage_settings() {
    let model = ready();
    assert!(can_initialize(&model));
    let (method, path, body) = submission(&model, "secret".into()).into_request();
    assert_eq!(method, "POST");
    assert_eq!(path, "/api/setup/initialize");
    assert_eq!(body, Some(json!({"username":"admin","password":"secret"})));
}

#[test]
fn initialization_requires_known_empty_state_and_credentials() {
    assert!(!can_initialize(&Model::default()));
    let mut model = ready();
    assert!(can_initialize(&model));
    for state in [
        Status::Unknown,
        Status::Existing,
        Status::ManagementReady,
        Status::RecoveryRequired,
        Status::Unavailable,
    ] {
        model.status = state;
        assert!(!can_initialize(&model));
    }
    model.status = Status::Empty;
    for flag in ["pending", "unknown", "complete"] {
        let mut blocked = model.clone();
        match flag {
            "pending" => blocked.pending = true,
            "unknown" => blocked.unknown = true,
            _ => blocked.complete = true,
        }
        assert!(!can_initialize(&blocked));
    }
    model.username = " ".into();
    assert!(!can_initialize(&model));
    model.username = "admin".into();
    model.password.clear();
    assert!(!can_initialize(&model));
}

#[test]
fn setup_success_confirms_management_without_claiming_storage() {
    let mut model = ready();
    model.unknown = true;
    finish_initialization(
        &mut model,
        Ok(json!({"initialized":true,"storage_pending":true})),
    );
    assert!(model.complete && !model.unknown);
    assert_eq!(model.status, Status::ManagementReady);
    assert!(!model.reload_blocked());
    assert!(!can_initialize(&model));
}

#[test]
fn malformed_or_old_pool_initialization_responses_do_not_confirm_account_setup() {
    for value in [
        json!({}),
        json!({"initialized":true}),
        json!({"initialized":true,"storage_pending":false}),
        json!({"initialized":false,"storage_pending":true}),
        json!({"initialized":"true","storage_pending":true}),
        json!({"initialized":true,"storage_pending":null}),
        json!({"initialized":true,"storage_pending":true,"extra":true}),
        json!({"initialized":true,"recovering":true,"restart_required":false}),
    ] {
        let mut model = ready();
        model.pending = true;
        finish_initialization(&mut model, Ok(value.clone()));
        assert!(model.unknown && !model.complete, "{value}");
        assert!(!model.pending && model.reload_blocked());
        assert!(!can_initialize(&model));
    }
}

#[test]
fn uncertain_setup_is_resolved_only_by_independently_confirmed_management_authority() {
    for (status, complete) in [
        ("empty", false),
        ("existing", false),
        ("recovery_required", false),
        ("management_ready", true),
    ] {
        let mut model = ready();
        model.unknown = true;
        model.pending = true;
        model.generation = 7;
        finish_check(&mut model, 7, Ok(json!({"state":status})));
        assert_eq!(model.complete, complete);
        assert_eq!(model.unknown, !complete);
        assert!(!can_initialize(&model));
        if complete {
            assert!(model.password.is_empty());
        }
    }
}

#[test]
fn stale_reads_cannot_finish_newer_work() {
    let mut model = ready();
    model.generation = 8;
    model.pending = true;
    finish_check(&mut model, 7, Ok(json!({"state":"empty"})));
    assert!(model.pending);
    assert_eq!(model.generation, 8);
}

#[test]
fn cleanup_preserves_an_issued_mutation_and_its_uncertainty() {
    for (value, complete) in [
        (json!({"initialized":true,"storage_pending":true}), true),
        (json!({}), false),
    ] {
        let mut model = ready();
        model.pending = true;
        model.generation = 7;
        clear_secrets(&mut model);
        assert!(model.pending);
        assert_eq!(model.generation, 7);
        finish_initialization(&mut model, Ok(value));
        assert_eq!(model.complete, complete);
        assert_eq!(model.reload_blocked(), !complete);
        assert!(model.password.is_empty());
    }
}

#[test]
fn clear_or_failed_reads_cannot_clear_the_unknown_mutation_fence() {
    let mut model = ready();
    model.pending = true;
    model.unknown = true;
    finish_initialization(
        &mut model,
        Err(api::Error {
            code: storage_contract::ErrorCode::InvalidRequest,
            message: "rejected".into(),
            retryable: false,
            status: 400,
            request_id: None,
            source: None,
        }),
    );
    assert!(model.unknown && model.reload_blocked());
    clear_secrets(&mut model);
    assert!(model.unknown && !can_initialize(&model));
}

#[test]
fn setup_and_sign_in_use_one_screen_without_losing_unknown_outcomes() {
    let mut model = Model::default();
    finish_check(&mut model, 0, Ok(json!({"state":"empty"})));
    assert!(model.screen == Screen::CreateAccount);
    model.username = "admin".into();
    model.password = "secret".into();
    model.unknown = true;
    finish_check(&mut model, 0, Ok(json!({"state":"existing"})));
    assert!(model.screen == Screen::CreateAccount);
    assert!(model.unknown);
    assert!(model.retry_available());
    finish_check(&mut model, 0, Ok(json!({"state":"management_ready"})));
    assert!(model.screen == Screen::SignIn);
    assert!(!model.unknown);
    assert!(model.password.is_empty());
}

#[test]
fn clear_inputs_preserves_the_account_screen_and_unknown_fence() {
    let mut model = ready();
    model.unknown = true;
    clear_inputs(&mut model);
    assert!(model.username.is_empty() && model.password.is_empty());
    assert!(model.screen == Screen::CreateAccount);
    assert!(model.unknown && model.reload_blocked());
    assert_eq!(model.status, Status::Empty);
    assert!(!can_initialize(&model));
    assert!(model.feedback().is_some());
    let generation = model.generation;
    model.pending = true;
    model.username = "held".into();
    clear_inputs(&mut model);
    assert_eq!(model.username, "held");
    assert_eq!(model.generation, generation);
}

#[test]
fn unknown_feedback_keeps_the_warning_error_and_request_identity() {
    let mut model = ready();
    model.unknown = true;
    model.failure = Some(Failure::Request(api::Error {
        code: storage_contract::ErrorCode::InvalidRequest,
        message: "transport failure".into(),
        retryable: false,
        status: 400,
        request_id: Some("setup-request".into()),
        source: None,
    }));
    let feedback = model.feedback().unwrap();
    assert!(feedback.message.contains("transport failure"));
    assert!(
        feedback.message.contains("Outcome unconfirmed") || feedback.message.contains("结果未确认")
    );
    assert_eq!(feedback.request_id.as_deref(), Some("setup-request"));
    model.failure = None;
    assert!(
        model.feedback().is_some(),
        "clearing the error cannot erase the unknown warning"
    );
}

#[test]
fn successful_authentication_resolves_account_uncertainty_but_stale_reads_cannot() {
    let mut model = ready();
    model.unknown = true;
    model.authenticated();
    assert!(model.complete && !model.reload_blocked());
    assert!(model.screen == Screen::SignIn);
    assert!(model.password.is_empty());
    finish_check(&mut model, 0, Ok(json!({"state":"empty"})));
    assert!(model.screen == Screen::SignIn);
    assert!(!can_initialize(&model));
}

#[test]
fn blocked_setup_states_keep_actionable_feedback_in_the_information_row() {
    for status in [Status::RecoveryRequired, Status::Unavailable] {
        let model = Model {
            status,
            ..Default::default()
        };
        assert!(model.feedback().is_some());
        assert!(model.retry_available());
        assert!(!can_initialize(&model));
    }
    for status in [Status::Empty, Status::Existing, Status::ManagementReady] {
        let model = Model {
            status,
            ..Default::default()
        };
        assert!(model.feedback().is_none());
    }
}
