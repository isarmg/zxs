use super::*;
use crate::management::Storage;
#[test]
fn inspection_survives_data_failure_but_storage_writes_require_an_active_session() {
    let mut state = State {
        authenticated: true,
        ..Default::default()
    };
    for storage in [
        Storage::Unknown,
        Storage::ReadOnly,
        Storage::Unavailable,
        Storage::Unconfigured,
        Storage::RecoveryRequired,
    ] {
        state.management.storage = storage;
        assert!(permitted(&state, false));
        assert!(!permitted(&state, true));
    }
    state.management.storage = Storage::Ready;
    assert!(permitted(&state, true));
    state.admin_console.busy = true;
    assert!(!permitted(&state, false));
    state.admin_console.busy = false;
    state.session_transition = crate::ui::SessionTransition::LoggingOut;
    assert!(!permitted(&state, false));
    state.session_transition = crate::ui::SessionTransition::Idle;
    state.authenticated = false;
    assert!(!permitted(&state, false));
}

#[test]
fn language_reload_preserves_admin_operation_fences() {
    let mut model = Model::default();
    assert!(!model.reload_blocked());
    model.busy = true;
    assert!(model.reload_blocked());
    model.busy = false;
    model.uncertain_operation = Some(UncertainOperation::Known("original".into()));
    assert!(model.reload_blocked());
    model.uncertain_operation = Some(UncertainOperation::MissingId);
    assert!(model.reload_blocked());
}

#[test]
fn edited_plan_inputs_require_a_new_preview_before_commit_or_cancel() {
    for (kind, key, original, changed) in [
        (Kind::Inspect, "id", "plan-a", "plan-b"),
        (Kind::Purge, "versions", "version-a", "version-b"),
        (Kind::Split, "commit", "commit-a", "commit-b"),
        (Kind::Save, "name", "saved-a", "saved-b"),
    ] {
        let mut model = Model {
            kind,
            fields: [(key.into(), original.into())].into_iter().collect(),
            plan: json_codec::json!({"plan_id":"plan-a", "digest":"reviewed", "state":"ready"}),
            result: "previous plan preview".into(),
            acknowledged: true,
            ..Default::default()
        };
        model.edit_field(key, changed.into());
        assert_eq!(model.fields[key], changed);
        assert!(model.plan.is_null(), "{kind:?} retained its previous plan");
        assert!(!model.acknowledged);
        assert!(model.result.is_empty());
        // Even a queued acknowledgment cannot make the previous target usable.
        model.acknowledged = true;
        for commit in [true, false] {
            assert!(request::plan_action(&model.plan, commit, model.acknowledged, "new").is_err());
        }
    }
}

#[test]
fn field_events_cannot_change_pending_or_unconfirmed_admin_intent() {
    let plan = json_codec::json!({"plan_id":"plan-a", "digest":"reviewed", "state":"ready"});
    for (busy, uncertain) in [
        (true, None),
        (false, Some(UncertainOperation::Known("original".into()))),
        (false, Some(UncertainOperation::MissingId)),
    ] {
        let mut model = Model {
            fields: [
                ("id".into(), "plan-a".into()),
                ("enabled".into(), "false".into()),
            ]
            .into_iter()
            .collect(),
            plan: plan.clone(),
            acknowledged: true,
            result: "original operation receipt".into(),
            busy,
            uncertain_operation: uncertain.clone(),
            ..Default::default()
        };
        model.edit_field("id", "plan-b".into());
        model.edit_field("enabled", "true".into());
        assert_eq!(model.fields["id"], "plan-a");
        assert_eq!(model.fields["enabled"], "false");
        assert_eq!(model.plan, plan);
        assert!(model.acknowledged);
        assert_eq!(model.result, "original operation receipt");
        assert_eq!(model.uncertain_operation, uncertain);
        assert_eq!(model.busy, busy);
    }
}

#[test]
fn repeated_field_values_preserve_the_exact_review() {
    let mut model = Model {
        fields: [("id".into(), "plan-a".into())].into_iter().collect(),
        plan: json_codec::json!({"plan_id":"plan-a", "digest":"reviewed", "state":"ready"}),
        acknowledged: true,
        ..Default::default()
    };
    let plan = model.plan.clone();
    model.edit_field("id", "plan-a".into());
    model.edit_field("workspace", String::new());
    assert_eq!(model.plan, plan);
    assert!(model.acknowledged);
}

#[test]
fn invalid_success_and_early_not_found_preserve_review_and_uncertainty() {
    let reviewed = json_codec::json!({"plan_id":"p", "digest":"reviewed", "state":"ready"});
    for response in [
        Value::Null,
        json_codec::json!({}),
        json_codec::json!({"operation_id":"wrong","commit_state":"PUBLISHED"}),
    ] {
        let mut model = Model {
            plan: reviewed.clone(),
            acknowledged: true,
            uncertain_operation: Some(UncertainOperation::Known("original".into())),
            ..Default::default()
        };
        let request = request::plan_action(&reviewed, true, true, "original").unwrap();
        respond(&mut model, &request, response);
        assert_eq!(model.plan, reviewed);
        assert!(model.acknowledged);
        assert_eq!(
            model.uncertain_operation,
            Some(UncertainOperation::Known("original".into()))
        );
    }
    // The operation may not be indexed yet. A lookup error is not evidence of
    // rejection and never enters the success reducer.
    let error = api::Error {
        code: storage_contract::ErrorCode::NotFound,
        message: "not indexed".into(),
        status: 404,
        retryable: false,
        request_id: None,
        source: None,
    };
    let mut model = Model {
        plan: reviewed.clone(),
        uncertain_operation: Some(UncertainOperation::Known("original".into())),
        ..Default::default()
    };
    reject(&mut model, &request::operation("original").contract, &error);
    assert_eq!(model.plan, reviewed);
    assert!(model.reload_blocked());
}

#[test]
fn inspection_snapshots_never_settle_an_interrupted_storage_mutation() {
    let mut model = Model {
        uncertain_operation: Some(UncertainOperation::Known("original".into())),
        ..Default::default()
    };
    for value in [
        json_codec::json!([]),
        json_codec::json!([{"repository_id":"repo"}]),
    ] {
        let request = request::query(
            QueryKind::Inspect {
                prefix: "/api/repositories",
                key: None,
                suffix: "",
            },
            &request::Fields::new(),
        )
        .unwrap();
        respond(&mut model, &request, value);
        assert_eq!(
            model.uncertain_operation,
            Some(UncertainOperation::Known("original".into()))
        );
        assert!(model.reload_blocked());
    }
}

#[test]
fn management_selection_does_not_bypass_session_busy_or_uncertainty_guards() {
    Owner::new().with(|| {
        let state = RwSignal::new(State {
            authenticated: true,
            ..Default::default()
        });
        for guard in [
            "anonymous",
            "restoring",
            "logout",
            "unconfirmed",
            "busy",
            "unknown",
        ] {
            state.update(|s| {
                s.authenticated = guard != "anonymous";
                s.admin_console.busy = guard == "busy";
                s.admin_console.uncertain_operation = if guard == "unknown" {
                    Some(UncertainOperation::Known("original".into()))
                } else {
                    None
                };
                s.session_transition = match guard {
                    "restoring" => crate::ui::SessionTransition::Restoring,
                    "logout" => crate::ui::SessionTransition::LoggingOut,
                    "unconfirmed" => crate::ui::SessionTransition::LogoutUnconfirmed,
                    _ => crate::ui::SessionTransition::Idle,
                };
            });
            select(state, Kind::Repository);
            assert_eq!(
                state.with_untracked(|s| s.admin_console.kind),
                Kind::Inspect
            );
        }
        state.update(|s| {
            *s = State {
                authenticated: true,
                ..Default::default()
            };
        });
        select(state, Kind::Repository);
        assert_eq!(
            state.with_untracked(|s| s.admin_console.kind),
            Kind::Repository
        );
    });
}

#[test]
fn storage_action_controls_reenable_after_busy_request_completes() {
    Owner::new().with(|| {
        let state = RwSignal::new(State {
            authenticated: true,
            ..Default::default()
        });
        state.update(|s| {
            s.management.storage = Storage::Ready;
            s.admin_console.acknowledged = true;
        });
        // Save/Create preview and Cancel share this disabled binding.
        let write = Memo::new(move |_| {
            state.with(|s| !permitted(s, true) || s.admin_console.uncertain_operation.is_some())
        });
        let commit = Memo::new(move |_| {
            state.with(|s| {
                !permitted(s, true)
                    || !s.admin_console.acknowledged
                    || s.admin_console.uncertain_operation.is_some()
            })
        });
        let poll = Memo::new(move |_| {
            state.with(|s| {
                !permitted(s, false)
                    || s.admin_console
                        .uncertain_operation
                        .as_ref()
                        .and_then(UncertainOperation::id)
                        .is_none()
            })
        });
        let controls = [write, commit];
        for busy in [false, true, false, true, false] {
            state.update(|s| s.admin_console.busy = busy);
            assert_eq!(allowed(state, true), !busy);
            assert!(
                poll.get_untracked(),
                "there is no original operation to query"
            );
            for disabled in controls {
                assert_eq!(
                    disabled.get_untracked(),
                    busy,
                    "the control must keep observing every busy transition"
                );
            }
        }
        state.update(|s| {
            s.admin_console.uncertain_operation = Some(UncertainOperation::Known("original".into()))
        });
        assert!(write.get_untracked());
        assert!(commit.get_untracked());
        assert!(!poll.get_untracked());
        state.update(|s| s.admin_console.uncertain_operation = Some(UncertainOperation::MissingId));
        assert!(poll.get_untracked());
        state.update(|s| {
            s.admin_console.uncertain_operation = None;
            s.admin_console.acknowledged = false;
        });
        assert!(!write.get_untracked());
        assert!(commit.get_untracked());
        assert!(poll.get_untracked());
    });
}

#[test]
fn storage_action_controls_observe_initial_readiness_changes() {
    Owner::new().with(|| {
        let state = RwSignal::new(State {
            authenticated: true,
            ..Default::default()
        });
        let disabled = Memo::new(move |_| {
            state.with(|s| !permitted(s, true) || s.admin_console.uncertain_operation.is_some())
        });
        assert!(disabled.get_untracked());
        state.update(|s| s.management.storage = Storage::Ready);
        assert!(allowed(state, true), "event admission should be ready");
        assert!(
            !disabled.get_untracked(),
            "the control must observe storage becoming writable"
        );
    });
}

#[test]
fn missing_operation_identity_cannot_match_a_receipt_named_unknown() {
    let mut model = Model {
        uncertain_operation: Some(UncertainOperation::MissingId),
        ..Default::default()
    };
    let mut receipt: Value = json_codec::from_str(include_str!(
        "../files/fixtures/namespace-batch-operation.json"
    ))
    .unwrap();
    receipt["operation_id"] = "unknown".into();
    assert!(response::terminal(&receipt, "unknown"));
    respond(&mut model, &request::operation("unknown"), receipt);
    assert_eq!(
        model.uncertain_operation,
        Some(UncertainOperation::MissingId)
    );
    assert!(model.reload_blocked());
}

fn respond(model: &mut Model, request: &request::Request, value: Value) {
    match request.contract.validate(value).map_err(api::Error::from) {
        Ok(value) => accept(model, &request.contract, value),
        Err(error) => reject(model, &request.contract, &error),
    }
}
fn reviewed_plan() -> Value {
    json_codec::json!({"plan_id":"p","kind":"purge","principal":"admin","expected_sequence":"4","digest":"reviewed","input":{"history_tree_id":"h"},"state":"ready"})
}
fn operation_receipt(commit: &str, state: &str, protection: &str) -> Value {
    let mut value: Value = json_codec::from_str(include_str!(
        "../files/fixtures/namespace-batch-operation.json"
    ))
    .unwrap();
    value["operation_id"] = "original".into();
    value["commit_state"] = commit.into();
    value["state"] = state.into();
    value["protection_at_publish"] = protection.into();
    value
}

#[test]
fn only_the_original_terminal_operation_query_settles_the_fence() {
    for (commit, state, protection, terminal) in [
        ("PUBLISHED", "completed", "NOT_APPLICABLE", true),
        ("PUBLISHED_DEGRADED", "in_doubt", "BELOW_TARGET", true),
        ("ABORTED", "in_doubt", "UNCONFIRMED", true),
        ("IN_DOUBT", "in_doubt", "UNCONFIRMED", false),
    ] {
        for queried in ["original", "other"] {
            let plan = reviewed_plan();
            let mut model = Model {
                plan: plan.clone(),
                acknowledged: true,
                fields: [("name".into(), "draft".into())].into_iter().collect(),
                uncertain_operation: Some(UncertainOperation::Known("original".into())),
                ..Default::default()
            };
            let request = request::operation(queried);
            let mut value = operation_receipt(commit, state, protection);
            // A valid receipt for another lookup still cannot settle original.
            value["operation_id"] = queried.into();
            respond(&mut model, &request, value);
            let settled = terminal && queried == "original";
            assert_eq!(
                model.uncertain_operation.is_none(),
                settled,
                "{commit}/{queried}"
            );
            assert_eq!(model.plan.is_null(), settled);
            assert_eq!(model.acknowledged, !settled);
            assert_eq!(model.fields["name"], "draft");
        }
    }
}

#[test]
fn wrong_operation_response_preserves_review_and_original_identity() {
    let plan = reviewed_plan();
    let mut model = Model {
        plan: plan.clone(),
        acknowledged: true,
        uncertain_operation: Some(UncertainOperation::Known("original".into())),
        ..Default::default()
    };
    let mut value = operation_receipt("PUBLISHED", "completed", "NOT_APPLICABLE");
    value["operation_id"] = "other".into();
    respond(&mut model, &request::operation("original"), value);
    assert_eq!(model.plan, plan);
    assert!(model.acknowledged);
    assert_eq!(
        model.uncertain_operation,
        Some(UncertainOperation::Known("original".into()))
    );
    assert!(!model.error.is_empty());
}

#[test]
fn plan_queries_install_the_plan_but_cannot_settle_a_mutation() {
    let plan = reviewed_plan();
    let mut model = Model {
        acknowledged: true,
        uncertain_operation: Some(UncertainOperation::Known("original".into())),
        ..Default::default()
    };
    let request = request::query(
        QueryKind::Plan,
        &[("id".into(), "p".into())].into_iter().collect(),
    )
    .unwrap();
    respond(&mut model, &request, plan.clone());
    assert_eq!(model.plan, plan);
    assert!(!model.acknowledged);
    assert_eq!(
        model.uncertain_operation,
        Some(UncertainOperation::Known("original".into()))
    );
}

#[test]
fn preview_success_installs_review_but_preview_failures_never_create_a_mutation_fence() {
    let fields = [
        ("history".into(), "h".into()),
        ("versions".into(), "v".into()),
    ]
    .into_iter()
    .collect();
    let request = request::build(Kind::Purge, &fields, "original", "admin").unwrap();
    let mut model = Model {
        acknowledged: true,
        ..Default::default()
    };
    let plan = reviewed_plan();
    respond(&mut model, &request, plan.clone());
    assert_eq!(model.plan, plan);
    assert!(!model.acknowledged);
    respond(&mut model, &request, Value::Null);
    assert!(model.uncertain_operation.is_none());
    assert_eq!(model.plan, plan);
    let error = api::Error {
        code: storage_contract::ErrorCode::InternalError,
        message: "preview failed".into(),
        status: 503,
        retryable: false,
        request_id: None,
        source: None,
    };
    reject(&mut model, &request.contract, &error);
    assert!(model.uncertain_operation.is_none());
}

#[test]
fn every_mutation_failure_keeps_its_captured_operation_not_the_current_form() {
    let fields = [
        ("name", "Name"),
        ("slot", "P01"),
        ("copies", "1"),
        ("repository", "r"),
        ("branch", "b"),
        ("generation", "1"),
        ("id", "rule"),
        ("commit", "c"),
        ("history", "h"),
        ("source_branch", "s"),
        ("source_commit", "sc"),
        ("target_branch", "t"),
        ("target_commit", "tc"),
    ]
    .into_iter()
    .map(|(key, value)| (key.into(), value.into()))
    .collect();
    let mut requests = [
        Kind::Repository,
        Kind::Smb,
        Kind::Policy,
        Kind::Rule,
        Kind::Branch,
        Kind::Link,
    ]
    .into_iter()
    .map(|kind| request::build(kind, &fields, "original", "admin").unwrap())
    .collect::<Vec<_>>();
    for commit in [true, false] {
        requests.push(request::plan_action(&reviewed_plan(), commit, true, "original").unwrap());
    }
    for request in requests {
        for status in [0, 200, 409, 503] {
            let mut model = Model {
                fields: [("id".into(), "changed".into())].into_iter().collect(),
                ..Default::default()
            };
            let error = api::Error {
                code: storage_contract::ErrorCode::InvalidAdminResponse,
                message: "failed".into(),
                status,
                retryable: false,
                request_id: None,
                source: None,
            };
            reject(&mut model, &request.contract, &error);
            assert_eq!(
                model.uncertain_operation,
                if status == 409 {
                    None
                } else {
                    Some(UncertainOperation::Known("original".into()))
                }
            );
            assert_eq!(model.fields["id"], "changed");
        }
    }
}

#[test]
fn successful_commit_and_cancel_clear_the_review_only_after_raw_validation() {
    let operation = "op_00000000000000000000000000000001";
    let receipt = json_codec::json!({"operation_id":operation,"kind":"plan.cancel","commit_id":null,"history_tree_id":null,"branch_id":null,"commit_state":"PUBLISHED","protection_at_publish":"NOT_APPLICABLE","current_health":"UNKNOWN","request_digest":"digest","sequence":"4","result":{}});
    for commit in [true, false] {
        let plan = reviewed_plan();
        let request = request::plan_action(&plan, commit, true, operation).unwrap();
        let mut model = Model {
            plan,
            acknowledged: true,
            ..Default::default()
        };
        respond(&mut model, &request, receipt.clone());
        assert!(model.plan.is_null());
        assert!(!model.acknowledged);
        assert!(model.error.is_empty());
        assert!(model.uncertain_operation.is_none());
    }
}

#[test]
fn late_success_and_unauthorized_completion_cannot_change_a_new_session() {
    Owner::new().with(|| {
        let state = RwSignal::new(State {
            authenticated: true,
            session_epoch: 5,
            principal: "new admin".into(),
            admin_console: Model {
                busy: true,
                plan: reviewed_plan(),
                acknowledged: true,
                result: "new result".into(),
                error: "new error".into(),
                uncertain_operation: Some(UncertainOperation::Known("new operation".into())),
                ..Default::default()
            },
            ..Default::default()
        });
        let request = request::operation("original");
        complete(
            state,
            4,
            &request.contract,
            request
                .contract
                .validate(operation_receipt(
                    "PUBLISHED",
                    "completed",
                    "NOT_APPLICABLE",
                ))
                .map_err(api::Error::from),
        );
        let error = api::Error {
            code: storage_contract::ErrorCode::InvalidAdminResponse,
            message: "old unauthorized".into(),
            status: 401,
            retryable: false,
            request_id: None,
            source: None,
        };
        complete(state, 4, &request.contract, Err(error));
        state.with_untracked(|s| {
            assert!(s.authenticated);
            assert_eq!(s.session_epoch, 5);
            assert_eq!(s.principal, "new admin");
            assert!(s.admin_console.busy);
            assert_eq!(s.admin_console.plan, reviewed_plan());
            assert!(s.admin_console.acknowledged);
            assert_eq!(s.admin_console.result, "new result");
            assert_eq!(s.admin_console.error, "new error");
            assert_eq!(
                s.admin_console.uncertain_operation,
                Some(UncertainOperation::Known("new operation".into()))
            );
        });
        state.update(|s| s.session_transition = crate::ui::SessionTransition::LoggingOut);
        complete(
            state,
            5,
            &request.contract,
            request
                .contract
                .validate(operation_receipt(
                    "PUBLISHED",
                    "completed",
                    "NOT_APPLICABLE",
                ))
                .map_err(api::Error::from),
        );
        assert!(state.with_untracked(|s| s.admin_console.busy));
    });
}

#[test]
fn current_session_completion_releases_busy_and_unauthorized_session() {
    Owner::new().with(|| {
        let state = RwSignal::new(State {
            authenticated: true,
            session_epoch: 5,
            admin_console: Model {
                busy: true,
                ..Default::default()
            },
            ..Default::default()
        });
        let request = request::operation("original");
        complete(
            state,
            5,
            &request.contract,
            request
                .contract
                .validate(operation_receipt(
                    "PUBLISHED",
                    "completed",
                    "NOT_APPLICABLE",
                ))
                .map_err(api::Error::from),
        );
        assert!(!state.with_untracked(|s| s.admin_console.busy));
        assert!(!state.with_untracked(|s| s.admin_console.result.is_empty()));
        let error = api::Error {
            code: storage_contract::ErrorCode::InvalidAdminResponse,
            message: "expired".into(),
            status: 401,
            retryable: false,
            request_id: None,
            source: None,
        };
        complete(state, 5, &request.contract, Err(error));
        assert!(!state.with_untracked(State::session_active));
    });
}

#[test]
fn typed_execution_checks_session_busy_and_storage_before_starting_work() {
    Owner::new().with(|| {
        let state = RwSignal::new(State::default());
        let fields = [
            ("history".into(), "h".into()),
            ("versions".into(), "v".into()),
        ]
        .into_iter()
        .collect();
        for (authenticated, transition, busy, storage) in [
            (
                false,
                crate::ui::SessionTransition::Idle,
                false,
                Storage::Ready,
            ),
            (
                true,
                crate::ui::SessionTransition::Restoring,
                false,
                Storage::Ready,
            ),
            (
                true,
                crate::ui::SessionTransition::LoggingOut,
                false,
                Storage::Ready,
            ),
            (
                true,
                crate::ui::SessionTransition::LogoutUnconfirmed,
                false,
                Storage::Ready,
            ),
            (
                true,
                crate::ui::SessionTransition::Idle,
                true,
                Storage::Ready,
            ),
            (
                true,
                crate::ui::SessionTransition::Idle,
                false,
                Storage::ReadOnly,
            ),
        ] {
            state.update(|s| {
                s.authenticated = authenticated;
                s.session_transition = transition;
                s.management.storage = storage;
                s.admin_console.busy = busy;
                s.admin_console.error = "existing error".into();
            });
            execute(
                state,
                request::build(Kind::Purge, &fields, "original", "admin").unwrap(),
            );
            if storage == Storage::Ready {
                execute(state, request::operation("original"));
            }
            state.with_untracked(|s| {
                assert_eq!(s.admin_console.busy, busy);
                assert_eq!(s.admin_console.error, "existing error");
            });
        }
    });
}
