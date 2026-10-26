use super::*;
fn authorized() -> Model {
    Model {
        key: "a".repeat(64),
        csrf: "csrf".into(),
        username: "admin".into(),
        password: "password".into(),
        disks: vec!["A".into(), "B".into(), "C".into(), "D".into()],
        selected: vec!["A".into(), "B".into(), "C".into()],
        empty: true,
        confirmed: true,
        ..Default::default()
    }
}
#[test]
fn initialization_requires_explicit_empty_confirmation_and_settled_outcome() {
    assert!(!can_initialize(&Model::default()));
    let mut m = authorized();
    assert!(can_initialize(&m));
    m.confirmed = false;
    assert!(!can_initialize(&m));
    m.confirmed = true;
    m.empty = false;
    assert!(!can_initialize(&m));
    m.empty = true;
    m.pending = true;
    assert!(!can_initialize(&m));
    m.pending = false;
    m.unknown = true;
    assert!(!can_initialize(&m));
    m.unknown = false;
    m.complete = true;
    assert!(!can_initialize(&m));
    m.complete = false;
    m.csrf.clear();
    assert!(!can_initialize(&m));
}
#[test]
fn one_or_more_unique_allowlisted_disk_ids_are_required() {
    let mut m = authorized();
    assert!(valid_selection(&m));
    m.selected.push("D".into());
    assert!(valid_selection(&m));
    m.selected = vec!["A".into()];
    assert!(valid_selection(&m));
    m.selected.clear();
    assert!(!valid_selection(&m));
    m.selected = vec!["A".into(), "B".into(), "/tmp/unsafe".into()];
    assert!(!valid_selection(&m));
    m.selected = vec!["A".into(), "A".into(), "B".into()];
    assert!(!valid_selection(&m));
    m.disks = (0..33).map(|n| format!("disk-{n}")).collect();
    m.selected = m.disks.clone();
    assert!(!valid_selection(&m));
    m.selected.pop();
    assert!(valid_selection(&m));
}

#[test]
fn completion_requires_authorized_proof_and_never_infers_account_reset() {
    let mut m = authorized();
    m.action = Action::Complete;
    m.empty = false;
    m.status = Status::RecoveryRequired;
    m.unknown = true;
    assert!(!can_initialize(&m));
    accept_status(
        &mut m,
        &json!({"state":"recovery_required","csrf_token":"csrf"}),
    );
    assert!(!m.completion_available);
    assert!(!can_initialize(&m));
    accept_status(
        &mut m,
        &json!({"state":"existing","csrf_token":"csrf","completion_available":true}),
    );
    assert!(
        can_initialize(&m),
        "new explicit proof admits only bounded completion"
    );
    m.action = Action::Wizard;
    assert!(!can_initialize(&m), "proof does not authorize fresh init");
    m.action = Action::Complete;
    m.confirmed = false;
    assert!(!can_initialize(&m));
    accept_status(
        &mut m,
        &json!({"state":"recovery_required","completion_available":true}),
    );
    assert!(
        !m.completion_available,
        "proof without CSRF authorization is incomplete"
    );
}

#[test]
fn preset_requires_exact_review_and_does_not_rewrite_configuration() {
    let mut m = authorized();
    m.action = Action::Preset;
    m.presets = vec!["local-four-disk".into()];
    m.preset_id = "local-four-disk".into();
    assert!(!can_initialize(&m));
    m.preset_preview = json!({"disks":[{"candidate_id":"A"},{"candidate_id":"B"},{"candidate_id":"C"},{"candidate_id":"D"}],"resource_limits":{"workers":7}});
    assert!(!can_initialize(&m));
    m.preset_digest = "review-bound-digest".into();
    assert!(can_initialize(&m));
    m.selected.clear();
    assert!(
        can_initialize(&m),
        "preset disk count does not depend on wizard selection"
    );
    let (path, body) = submission(&m, "secret".into());
    assert_eq!(path, "/api/setup/initialize-preset");
    assert_eq!(
        body,
        json!({"preset_id":"local-four-disk","config_digest":"review-bound-digest","username":"admin","password":"secret"})
    );
    assert!(body.get("configuration").is_none());
    assert!(body.get("mode").is_none());
    assert!(body.get("disk_ids").is_none());
    m.preset_id = "unapproved".into();
    assert!(!can_initialize(&m));
}

#[test]
fn changing_setup_action_invalidates_confirmation_and_completion_is_bounded() {
    let mut m = authorized();
    choose(&mut m, Action::Complete);
    assert!(!m.confirmed);
    let (path, body) = submission(&m, "secret".into());
    assert_eq!(path, "/api/setup/complete-initialization");
    assert_eq!(body, json!({"username":"admin","password":"secret"}));
    choose(&mut m, Action::Preset);
    assert!(!m.confirmed);
}

#[test]
fn single_disk_submission_contains_only_selected_authorized_id() {
    let mut m = authorized();
    m.disks = vec!["A".into()];
    m.selected = vec!["A".into()];
    assert!(can_initialize(&m));
    let (path, body) = submission(&m, "secret".into());
    assert_eq!(path, "/api/setup/initialize");
    assert_eq!(
        body,
        json!({"disk_ids":["A"],"mode":"production","username":"admin","password":"secret"})
    );
    m.development = true;
    assert_eq!(submission(&m, "secret".into()).1["mode"], "production");
    m.allow_development = true;
    assert_eq!(submission(&m, "secret".into()).1["mode"], "development");
}

#[test]
fn setup_outcome_blocks_app_owned_reload_until_confirmed() {
    let mut model = Model::default();
    assert!(!model.reload_blocked());
    model.pending = true;
    assert!(model.reload_blocked());
    model.pending = false;
    model.unknown = true;
    assert!(model.reload_blocked());
    model.unknown = false;
    model.complete = true;
    assert!(!model.reload_blocked());
}

#[test]
fn verified_completion_releases_reload_guard_without_readmitting_initialization() {
    for success in [
        json!({"accepted":true}),
        json!({"initialized":true,"restart_required":true}),
    ] {
        let mut model = authorized();
        finish_initialization(&mut model, Ok(json!({})));
        assert!(model.unknown);
        assert!(model.reload_blocked());
        assert!(!can_initialize(&model));
        accept_status(
            &mut model,
            &json!({"state":"existing","csrf_token":"csrf","completion_available":true}),
        );
        choose(&mut model, Action::Complete);
        model.confirmed = true;
        assert!(can_initialize(&model));
        assert!(
            model.reload_blocked(),
            "proof alone cannot settle the outcome"
        );
        model.pending = true;
        finish_initialization(&mut model, Ok(success));
        assert!(model.complete);
        assert!(!model.unknown);
        assert!(!model.reload_blocked());
        assert!(!can_initialize(&model));
    }
}

#[test]
fn failed_completion_preserves_the_original_unknown_reload_fence() {
    let mut model = authorized();
    model.unknown = true;
    model.pending = true;
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
    assert!(!model.pending);
    assert!(model.unknown);
    assert!(!model.complete);
    assert!(model.reload_blocked());
}

#[test]
fn authorization_read_settles_but_cannot_restore_secrets_after_form_cleanup() {
    let key = "a".repeat(64);
    let response = json!({
        "state":"empty", "csrf_token":"late-csrf", "disk_ids":["A"],
        "completion_available":true, "config_presets":["local"]
    });
    let pending = Model {
        key: key.clone(),
        password: "unsent-password".into(),
        pending: true,
        generation: 7,
        ..Default::default()
    };
    let mut current = pending.clone();
    finish_check(&mut current, 7, &key, Ok(response.clone()));
    assert!(!current.pending);
    assert_eq!(current.csrf, "late-csrf");
    assert!(current.empty && current.completion_available);
    assert_eq!(current.presets, ["local"]);

    let mut detached = pending.clone();
    clear_authorization(&mut detached);
    finish_check(&mut detached, 7, &key, Ok(response.clone()));
    assert!(
        !detached.pending,
        "a discarded read must not strand loading"
    );
    assert!(detached.key.is_empty() && detached.password.is_empty() && detached.csrf.is_empty());
    assert!(!detached.empty && !detached.completion_available);
    assert!(detached.presets.is_empty() && detached.disks.is_empty());
    assert!(!can_initialize(&detached));

    let mut replaced = pending;
    replaced.key = "b".repeat(64);
    finish_check(&mut replaced, 7, &key, Ok(response));
    assert!(!replaced.pending);
    assert!(replaced.csrf.is_empty());
    assert!(!replaced.empty && !replaced.completion_available);
}

#[test]
fn preset_read_requires_the_current_key_and_selection_without_stranding_loading() {
    let key = "a".repeat(64);
    let response = json!({
        "preset_id":"local", "configuration":{"disks":[]}, "config_digest":"reviewed"
    });
    let pending = Model {
        key: key.clone(),
        pending: true,
        generation: 7,
        preset_id: "local".into(),
        presets: vec!["local".into()],
        ..Default::default()
    };
    let mut current = pending.clone();
    finish_preset_preview(&mut current, 7, &key, "local", Ok(response.clone()));
    assert!(!current.pending);
    assert!(preset_reviewed(&current));

    for change in ["cleanup", "key", "selection"] {
        let mut stale = pending.clone();
        match change {
            "cleanup" => clear_authorization(&mut stale),
            "key" => stale.key = "b".repeat(64),
            "selection" => stale.preset_id = "other".into(),
            _ => unreachable!(),
        }
        finish_preset_preview(&mut stale, 7, &key, "local", Ok(response.clone()));
        assert!(!stale.pending);
        assert!(stale.preset_preview.is_null() && stale.preset_digest.is_empty());
        assert!(!preset_reviewed(&stale));
    }
}

#[test]
fn stale_authorization_reads_do_not_settle_a_newer_request() {
    let key = "a".repeat(64);
    let mut model = Model {
        key: key.clone(),
        pending: true,
        generation: 8,
        preset_id: "local".into(),
        ..Default::default()
    };
    finish_check(&mut model, 7, &key, Ok(json!({"csrf_token":"old"})));
    assert!(model.pending);
    assert!(model.csrf.is_empty());
    finish_preset_preview(
        &mut model,
        7,
        &key,
        "local",
        Ok(json!({"preset_id":"local", "configuration":{}, "config_digest":"old"})),
    );
    assert!(model.pending);
    assert!(model.preset_preview.is_null() && model.preset_digest.is_empty());
}

#[test]
fn an_empty_key_cannot_accept_authorization_but_still_reports_input_errors() {
    let mut model = Model {
        pending: true,
        generation: 7,
        ..Default::default()
    };
    finish_check(
        &mut model,
        7,
        "",
        Ok(json!({"state":"empty", "csrf_token":"must-not-accept"})),
    );
    assert!(!model.pending);
    assert!(model.csrf.is_empty() && !model.empty);
    model.pending = true;
    finish_check(
        &mut model,
        7,
        "",
        Err(api::Error {
            code: storage_contract::ErrorCode::InvalidSetupRequest,
            message: "Enter the authorization key".into(),
            retryable: false,
            status: 0,
            request_id: None,
            source: None,
        }),
    );
    assert!(!model.pending);
    assert!(
        model
            .failure
            .as_ref()
            .unwrap()
            .contains("INVALID_SETUP_REQUEST")
    );
}

#[test]
fn form_cleanup_does_not_discard_initialization_outcome_recovery() {
    for (result, complete) in [(json!({"initialized":true}), true), (json!({}), false)] {
        let mut model = authorized();
        model.pending = true;
        model.generation = 7;
        clear_authorization(&mut model);
        assert_eq!(model.generation, 7);
        assert!(model.pending, "cleanup cannot settle an issued write");
        finish_initialization(&mut model, Ok(result));
        assert!(!model.pending);
        assert_eq!(model.complete, complete);
        assert_eq!(model.unknown, !complete);
        assert_eq!(model.reload_blocked(), !complete);
        assert!(model.key.is_empty() && model.password.is_empty() && model.csrf.is_empty());
        assert!(!can_initialize(&model));
    }
}

#[test]
fn setup_status_accepts_only_the_wire_vocabulary() {
    assert!(matches!(Status::from_wire(Some("empty")), Status::Empty));
    assert!(matches!(
        Status::from_wire(Some("existing")),
        Status::Existing
    ));
    assert!(matches!(
        Status::from_wire(Some("recovery_required")),
        Status::RecoveryRequired
    ));
    assert!(matches!(Status::from_wire(None), Status::Unavailable));
    assert!(matches!(
        Status::from_wire(Some("unknown")),
        Status::Unknown
    ));
}
