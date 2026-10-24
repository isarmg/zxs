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
    model.uncertain_operation = "original".into();
    assert!(model.reload_blocked());
    model.uncertain_operation = "unknown".into();
    assert!(model.reload_blocked());
}

#[test]
fn edited_plan_inputs_require_a_new_preview_before_commit_or_cancel() {
    for (kind, key, original, changed) in [
        ("inspect", "id", "plan-a", "plan-b"),
        ("purge", "versions", "version-a", "version-b"),
        ("split", "commit", "commit-a", "commit-b"),
        ("save", "name", "saved-a", "saved-b"),
    ] {
        let mut model = Model {
            kind: kind.into(),
            fields: [(key.into(), original.into())].into_iter().collect(),
            plan: json_codec::json!({"plan_id":"plan-a", "digest":"reviewed", "state":"ready"}),
            result: "previous plan preview".into(),
            acknowledged: true,
            ..Default::default()
        };
        model.edit_field(key, changed.into());
        assert_eq!(model.fields[key], changed);
        assert!(model.plan.is_null(), "{kind} retained its previous plan");
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
    for (busy, uncertain) in [(true, ""), (false, "original"), (false, "unknown")] {
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
            uncertain_operation: uncertain.into(),
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
            uncertain_operation: "original".into(),
            ..Default::default()
        };
        let contract =
            response::Contract::new("POST", "/api/plans/p/commit", None, "original", "admin");
        match contract.validate(response) {
            Ok(value) => accept(&mut model, "POST", "/api/plans/p/commit", false, value),
            Err(error) => reject(&mut model, "POST", false, "original", &error),
        }
        assert_eq!(model.plan, reviewed);
        assert!(model.acknowledged);
        assert_eq!(model.uncertain_operation, "original");
    }
    // The operation may not be indexed yet. A lookup error is not evidence of
    // rejection and never enters the success reducer.
    let error = api::Error {
        code: "NOT_FOUND".into(),
        message: "not indexed".into(),
        status: 404,
        retryable: false,
        request_id: None,
    };
    let mut model = Model {
        plan: reviewed.clone(),
        uncertain_operation: "original".into(),
        ..Default::default()
    };
    reject(&mut model, "GET", false, "", &error);
    assert_eq!(model.plan, reviewed);
    assert!(model.reload_blocked());
}

#[test]
fn inspection_snapshots_never_settle_an_interrupted_storage_mutation() {
    let mut model = Model {
        uncertain_operation: "original".into(),
        ..Default::default()
    };
    for value in [
        json_codec::json!([]),
        json_codec::json!([{"repository_id":"repo"}]),
    ] {
        accept(&mut model, "GET", "/api/repositories", false, value);
        assert_eq!(model.uncertain_operation, "original");
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
                    "original".into()
                } else {
                    String::new()
                };
                s.session_transition = match guard {
                    "restoring" => crate::ui::SessionTransition::Restoring,
                    "logout" => crate::ui::SessionTransition::LoggingOut,
                    "unconfirmed" => crate::ui::SessionTransition::LogoutUnconfirmed,
                    _ => crate::ui::SessionTransition::Idle,
                };
            });
            select(state, "repository");
            assert_eq!(
                state.with_untracked(|s| s.admin_console.kind.clone()),
                "inspect"
            );
        }
        state.update(|s| {
            *s = State {
                authenticated: true,
                ..Default::default()
            };
        });
        select(state, "repository");
        assert_eq!(
            state.with_untracked(|s| s.admin_console.kind.clone()),
            "repository"
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
            state.with(|s| !permitted(s, true) || !s.admin_console.uncertain_operation.is_empty())
        });
        let commit = Memo::new(move |_| {
            state.with(|s| {
                !permitted(s, true)
                    || !s.admin_console.acknowledged
                    || !s.admin_console.uncertain_operation.is_empty()
            })
        });
        let poll = Memo::new(move |_| {
            state.with(|s| !permitted(s, false) || s.admin_console.uncertain_operation == "unknown")
        });
        let controls = [write, commit, poll];
        for busy in [false, true, false, true, false] {
            state.update(|s| s.admin_console.busy = busy);
            assert_eq!(allowed(state, true), !busy);
            for disabled in controls {
                assert_eq!(
                    disabled.get_untracked(),
                    busy,
                    "the control must keep observing every busy transition"
                );
            }
        }
        state.update(|s| s.admin_console.uncertain_operation = "original".into());
        assert!(write.get_untracked());
        assert!(commit.get_untracked());
        assert!(!poll.get_untracked());
        state.update(|s| s.admin_console.uncertain_operation = "unknown".into());
        assert!(poll.get_untracked());
        state.update(|s| {
            s.admin_console.uncertain_operation.clear();
            s.admin_console.acknowledged = false;
        });
        assert!(!write.get_untracked());
        assert!(commit.get_untracked());
        assert!(!poll.get_untracked());
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
            state.with(|s| !permitted(s, true) || !s.admin_console.uncertain_operation.is_empty())
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
