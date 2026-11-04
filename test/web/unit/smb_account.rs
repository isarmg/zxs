use super::*;

fn available() -> Model {
    Model {
        status: Some(SmbAccountStatus::Configured {
            username: "owner".into(),
            generation: 1,
            candidates: vec!["owner".into(), "other".into()],
            password_change: PasswordChangeAvailability::Available,
        }),
        selected: "owner".into(),
        ..Default::default()
    }
}

#[test]
fn only_available_settled_model_accepts_password_submission() {
    assert!(!Model::default().can_change());
    let mut model = available();
    assert!(model.can_change());
    model.pending = true;
    assert!(!model.can_change());
    assert!(model.reload_blocked());
    model.pending = false;
    model.unknown = true;
    assert!(!model.can_change());
    assert!(model.reload_blocked());
}

#[test]
fn exact_success_confirms_smb_change_without_claiming_web_revocation() {
    let mut model = available();
    model.pending = true;
    complete(&mut model, &Ok(json!({"changed":true})));
    assert!(model.changed);
    assert!(!model.pending);
    assert!(!model.unknown);
    assert!(model.can_change());
    for invalid in [
        json!({"changed":false}),
        json!({"changed":true,"reauthentication_required":true}),
        json!({}),
    ] {
        let mut model = available();
        model.pending = true;
        complete(&mut model, &Ok(invalid));
        assert!(model.unknown);
        assert!(!model.changed);
        assert!(!model.can_change());
    }
}

#[test]
fn unknown_outcomes_keep_fence_but_known_rejections_allow_correction() {
    for (status, unknown) in [
        (0, true),
        (502, true),
        (409, false),
        (400, false),
        (429, false),
    ] {
        let mut error = api::error(storage_contract::ErrorCode::SmbUnavailable, "test");
        error.status = status;
        let mut model = available();
        model.pending = true;
        complete(&mut model, &Err(error));
        assert_eq!(model.unknown, unknown);
        assert!(!model.pending);
        assert_eq!(model.can_change(), !unknown);
    }
}

#[test]
fn account_contract_rejects_unknown_fields_and_states() {
    for value in [
        json!({"state":"enabled"}),
        json!({"state":"disabled","password":"leak"}),
        json!({"state":"configured","username":"owner","password_change":"ready"}),
    ] {
        assert!(SmbAccountStatus::deserialize(value).is_err());
    }
}

#[test]
fn refresh_and_unsaved_selection_fence_both_mutations() {
    let mut model = available();
    model.selected = "other".into();
    assert!(model.can_bind());
    assert!(!model.can_change());
    model.loading = true;
    assert!(!model.can_bind());
    assert!(!model.can_change());
    model.selected = "owner".into();
    assert!(!model.can_change());
    model.loading = false;
    assert!(model.can_change());
}

#[test]
fn binding_success_requires_requested_account_and_next_generation() {
    let status = json!({"state":"configured","username":"other","generation":"2","candidates":["owner","other"],"password_change":"available"});
    let confirmed = confirmed_binding(status.clone(), "other", 1).unwrap();
    for (account, generation) in [("owner", 1), ("other", 2), ("other", u64::MAX)] {
        let error = confirmed_binding(status.clone(), account, generation).unwrap_err();
        assert!(error.mutation_outcome_unknown());
    }
    assert!(confirmed_binding(json!({"state":"disabled"}), "other", 1).is_err());
    let mut model = available();
    model.pending = true;
    complete_binding(&mut model, &Ok(confirmed));
    assert!(!model.pending);
    assert!(model.binding_changed);
    assert_eq!(model.selected, "other");
    assert_eq!(model.generation(), Some(2));
    assert!(model.can_change());
    assert!(!model.can_bind());
}

#[test]
fn binding_unknown_requires_readback_and_cannot_clear_password_unknown() {
    let mut model = available();
    model.pending = true;
    let mut error = api::error(storage_contract::ErrorCode::SmbUnavailable, "test");
    error.status = 502;
    complete_binding(&mut model, &Err(error));
    assert!(model.binding_unknown);
    assert!(!model.can_bind());
    assert!(!model.can_change());
    model.unknown = true;
    model.loaded(available().status.unwrap());
    assert!(!model.binding_unknown);
    assert!(model.unknown);
    assert!(!model.can_change());
}

#[test]
fn malformed_binding_response_retains_decoder_source_without_confirming_save() {
    let error = confirmed_binding(json!({"state":"invalid"}), "other", 1).unwrap_err();
    assert!(error.mutation_outcome_unknown());
    assert!(matches!(
        error.source.as_deref(),
        Some(api::ErrorSource::SmbAccount(_))
    ));
}
