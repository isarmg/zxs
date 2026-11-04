use super::*;

fn ready() -> Model {
    Model {
        username: "admin".into(),
        password: "password".into(),
        status: Status::Empty,
        confirmed: true,
        ..Default::default()
    }
}

#[test]
fn first_setup_needs_no_disks_and_never_sends_storage_settings() {
    let model = ready();
    assert!(can_initialize(&model));
    assert_eq!(
        submission(&model, "secret".into()),
        json!({"username":"admin","password":"secret"})
    );
    let source = include_str!("../../../../src/web/src/setup.rs");
    for obsolete in [
        "data-setup-key",
        "data-setup-disk",
        "initialize-preset",
        "preset-preview",
        "complete-initialization",
        "data-setup-development",
    ] {
        assert!(
            !source.contains(obsolete),
            "retained obsolete bootstrap control {obsolete}"
        );
    }
}

#[test]
fn initialization_requires_confirmation_and_known_empty_state() {
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
    model.confirmed = false;
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
