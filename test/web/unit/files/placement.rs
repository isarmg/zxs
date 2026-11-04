use super::*;

fn draft() -> Draft {
    Draft {
        id: "draft".into(),
        repository: "repo".into(),
        path: "docs".into(),
        expected_scope: Some("directory".into()),
        configuration: None,
        selected: String::new(),
        loading: true,
    }
}
fn response() -> Value {
    json!({"repository_id":"repo","repository_name":"Documents",
        "scope":{"id":"directory","path":"docs"},"rule_id":null,"generation":"0",
        "primary_slot_id":"P01","inherited":true,"required_copies":3,
        "targets":[{"slot_id":"P01","root":"/storage/documents","unavailable_reason":null},
            {"slot_id":"P02","root":"/storage/short-chain","unavailable_reason":"insufficient_copies"}]})
}
fn ready() -> Draft {
    let mut draft = draft();
    draft.configuration = Some(read_configuration(response(), &draft).unwrap());
    draft.selected = "P01".into();
    draft.loading = false;
    draft
}
#[test]
fn placement_configuration_carries_scope_identity_and_exact_generation_without_user_id_fields() {
    let draft = ready();
    let body = request(&draft).unwrap();
    assert_eq!(body["path"], "docs");
    assert_eq!(body["expected_scope_id"], "directory");
    assert_eq!(body["expected_rule_id"], Value::Null);
    assert_eq!(body["expected_generation"], "0");
    let configuration = draft.configuration.as_ref().unwrap();
    let options = options(configuration);
    assert!(options[0].label.contains("/storage/documents"));
    assert!(!options[0].disabled);
    assert!(options[1].disabled);
    let mut existing = response();
    existing["rule_id"] = "rule_00000000000000000000000000000001".into();
    existing["generation"] = u64::MAX.to_string().into();
    existing["inherited"] = false.into();
    let mut draft = draft;
    draft.configuration = Some(read_configuration(existing, &draft).unwrap());
    assert_eq!(
        request(&draft).unwrap()["expected_generation"],
        u64::MAX.to_string()
    );
}
#[test]
fn placement_configuration_rejects_unloaded_incompatible_and_forged_choices() {
    assert!(request(&draft()).is_err());
    let mut draft = ready();
    draft.configuration.as_mut().unwrap().targets[1].unavailable_reason =
        Some(storage_contract::PrimaryPlacementBlock::MissingBackupPrefixes);
    for selection in ["P02", "B01", "unknown"] {
        draft.selected = selection.into();
        assert!(request(&draft).is_err());
    }
}
#[test]
fn placement_configuration_rejects_wrong_scope_missing_fields_and_inconsistent_baselines() {
    for field in [
        "repository_id",
        "repository_name",
        "scope",
        "rule_id",
        "generation",
        "primary_slot_id",
        "inherited",
        "required_copies",
        "targets",
    ] {
        let mut value = response();
        value.as_object_mut().unwrap().remove(field);
        assert!(
            read_configuration(value, &draft()).is_err(),
            "accepted missing {field}"
        );
    }
    for (field, value) in [
        ("repository_id", json!("other")),
        ("scope", json!({"id":"replaced","path":"docs"})),
        ("scope", json!({"id":"directory","path":"different"})),
        ("scope", json!({"id":"directory","path":"../docs"})),
        ("generation", json!(0)),
        ("generation", json!("1")),
        ("rule_id", json!("")),
        ("inherited", json!(false)),
        ("required_copies", json!(0)),
        ("primary_slot_id", json!("B01")),
        ("targets", json!([])),
        (
            "targets",
            json!([{"slot_id":"P01","root":"/p","unavailable_reason":null},
            {"slot_id":"P01","root":"/duplicate","unavailable_reason":null}]),
        ),
    ] {
        let mut invalid = response();
        invalid[field] = value;
        assert!(
            read_configuration(invalid, &draft()).is_err(),
            "accepted invalid {field}"
        );
    }
}
#[test]
fn placement_configuration_navigation_and_replacement_discard_stale_loads() {
    Owner::new().with(|| {
        let state = RwSignal::new(State {
            authenticated: true,
            session_epoch: 7,
            ..Default::default()
        });
        state.update(|s| s.files.placement = Some(draft()));
        complete_load(
            state,
            7,
            "old-draft",
            read_configuration(response(), &draft()),
        );
        assert!(state.with_untracked(|s| s.files.placement.as_ref().unwrap().loading));
        state.update(|s| {
            s.files.navigate(super::super::model::Navigation::Directory(
                "elsewhere".into(),
            ))
        });
        complete_load(state, 7, "draft", read_configuration(response(), &draft()));
        assert!(state.with_untracked(|s| s.files.placement.is_none()));
    });
}
#[test]
fn placement_configuration_current_session_expiry_survives_close_but_not_new_login() {
    Owner::new().with(|| {
        for epoch in [7, 8] {
            let state = RwSignal::new(State {
                authenticated: true,
                session_epoch: epoch,
                ..Default::default()
            });
            let error = api::Error {
                code: storage_contract::ErrorCode::Unauthorized,
                message: "expired".into(),
                retryable: false,
                status: 401,
                request_id: None,
                source: None,
            };
            complete_load(state, 7, "closed-draft", Err(error));
            assert_eq!(state.with_untracked(|s| s.session_active()), epoch == 8);
            assert_eq!(
                state.with_untracked(|s| s.errors.len()),
                usize::from(epoch == 7)
            );
        }
    });
}
#[test]
fn placement_configuration_unknown_outcomes_keep_draft_until_matching_receipt() {
    let mut model = super::super::model::Model {
        placement: Some(ready()),
        ..Default::default()
    };
    model.publications.insert(
        review::Kind::Placement,
        review::Publication {
            operation_id: Some("operation".into()),
            error: "network interrupted".into(),
        },
    );
    review::settle(
        &mut model,
        "unrelated",
        super::super::model::SaveStatus::Published,
    );
    review::settle(
        &mut model,
        "operation",
        super::super::model::SaveStatus::Unknown,
    );
    assert!(model.placement.is_some());
    assert_eq!(
        model.publications[&review::Kind::Placement]
            .operation_id
            .as_deref(),
        Some("operation")
    );
    review::settle(
        &mut model,
        "operation",
        super::super::model::SaveStatus::Rejected,
    );
    assert!(model.placement.is_some());
    assert!(
        model.publications[&review::Kind::Placement]
            .operation_id
            .is_none()
    );
    model
        .publications
        .get_mut(&review::Kind::Placement)
        .unwrap()
        .operation_id = Some("retry".into());
    review::settle(
        &mut model,
        "retry",
        super::super::model::SaveStatus::Published,
    );
    assert!(model.placement.is_none());
    assert!(!model.publications.contains_key(&review::Kind::Placement));
}
