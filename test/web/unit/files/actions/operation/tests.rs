use super::*;
use crate::files::history::state::HistoryState;
use json_codec::json;

fn published() -> Value {
    let mut value: Value = json_codec::from_str(include_str!(
        "../../fixtures/namespace-batch-operation.json"
    ))
    .unwrap();
    value["operation_id"] = "original".into();
    value
}

#[test]
fn publication_expires_the_observed_history_even_when_selection_identity_is_unchanged() {
    Owner::new().with(|| {
        let mut initial = State {
            authenticated: true,
            ..Default::default()
        };
        initial.files.selected = Some(crate::files::test_entries::entry(
            json!({"file_id":"file","version_id":"n1","node_id":"n1"}),
        ));
        initial
            .files
            .history
            .install(crate::files::history::tests::chain(1));
        let selected = initial.files.selected.clone();
        let state = RwSignal::new(initial);
        record_receipt(state, "original", published());
        state.with_untracked(|s| {
            assert_eq!(s.files.selected, selected);
            assert!(matches!(s.files.history.state, HistoryState::Stale { .. }));
            assert!(s.files.history.root().is_none());
            assert!(
                s.files.history.window().is_some(),
                "old data remains read-only while fresh reads run"
            );
        });
    });
}

#[test]
fn navigation_clears_the_local_review_but_preserves_original_operation_recovery() {
    use crate::files::{model::Navigation, review};
    Owner::new().with(|| {
        let state = RwSignal::new(State {
            authenticated: true,
            ..Default::default()
        });
        state.update(|s| {
            s.management.storage = crate::management::Storage::Ready;
            s.files.writable = true;
            s.files.selected = Some(crate::files::test_entries::entry(json!({})));
            s.files
                .history
                .install(crate::files::history::tests::chain(1));
            assert_eq!(
                crate::files::tags::check(&mut s.files, "n1", true),
                crate::files::tags::Check::Changed
            );
        });
        crate::files::tags::start(state);
        assert!(state.with_untracked(|s| s.files.tag_edit.is_some()));
        state.update(|s| {
            s.files.publications.insert(
                review::Kind::Tags,
                review::Publication {
                    operation_id: Some("original".into()),
                    error: "unconfirmed".into(),
                },
            );
            s.files.polling_operations.push("original".into());
        });
        assert_eq!(
            record_receipt(state, "original", Value::Null),
            SaveStatus::Unknown
        );
        for destination in [
            Navigation::Disk("P01".into()),
            Navigation::Directory("docs".into()),
            Navigation::Root,
        ] {
            state.update(|s| s.files.navigate(destination));
            state.with_untracked(|s| {
                assert!(s.files.tag_edit.is_none() && s.files.checked_nodes.is_empty());
                assert!(super::super::pending(s));
                assert!(!super::super::write_allowed(s));
                assert_eq!(s.files.operations, [FileOperation::unknown("original")]);
                assert_eq!(s.files.polling_operations, ["original".to_owned()]);
                assert_eq!(
                    s.files.publications[&review::Kind::Tags]
                        .operation_id
                        .as_deref(),
                    Some("original")
                );
            });
        }
        assert_eq!(state.with_untracked(poll_work), Some((0, vec![])));
        // Navigation did not authorize another write or settle the receipt.
        // Its original poll still settles normally, without resurrecting a panel.
        complete_poll(state, 0, "original", Ok(published()));
        state.with_untracked(|s| {
            assert!(!super::super::pending(s));
            assert!(s.files.publications.is_empty() && s.files.polling_operations.is_empty());
            assert!(s.files.tag_edit.is_none());
            assert_eq!(s.files.operations[0].status(), SaveStatus::Published);
            assert!(!super::super::write_allowed(s), "root is never writable");
        });
    });
}

#[test]
fn missing_malformed_or_unrelated_receipts_keep_the_original_mutation_pending() {
    Owner::new().with(|| {
        let mut missing = published();
        missing.as_object_mut().unwrap().remove("request_digest");
        let mut wrong = published();
        wrong["operation_id"] = "other".into();
        let mut missing_id = published();
        missing_id.as_object_mut().unwrap().remove("operation_id");
        let mut empty_id = published();
        empty_id["operation_id"] = "".into();
        let mut contradictory = published();
        contradictory["state"] = "in_doubt".into();
        let mut extra = published();
        extra["legacy"] = true.into();
        for response in [
            Value::Null,
            json!({}),
            json!({"state":"SUCCEEDED"}),
            json!({"operation_id":"original","state":"SUCCEEDED"}),
            json!({"operation_id":"original","state":"completed"}),
            json!({"operation_id":"original","state":"REJECTED"}),
            json!({"operation_id":"original","commit_state":"PUBLISHED"}),
            json!({"operation":published()}),
            missing,
            wrong,
            missing_id,
            empty_id,
            contradictory,
            extra,
        ] {
            let state = RwSignal::new(State {
                authenticated: true,
                ..Default::default()
            });
            state.update(|s| {
                s.management.storage = crate::management::Storage::Ready;
                s.files.writable = true;
                s.files.repository_id = "repository".into();
                s.files.selected = Some(crate::files::test_entries::entry(
                    json!({"file_id":"file", "name":"file.txt"}),
                ));
                s.files
                    .history
                    .install(crate::files::history::tests::chain(1));
                assert_eq!(
                    crate::files::tags::check(&mut s.files, "n1", true),
                    crate::files::tags::Check::Changed
                );
            });
            crate::files::tags::start(state);
            assert!(state.with_untracked(|s| s.files.tag_edit.is_some()));
            state.update(|s| {
                s.files.publications.insert(
                    crate::files::review::Kind::Tags,
                    crate::files::review::Publication {
                        operation_id: Some("original".into()),
                        error: String::new(),
                    },
                );
            });
            assert_eq!(
                record_receipt(state, "original", response),
                SaveStatus::Unknown
            );
            state.with_untracked(|s| {
                assert!(super::super::pending(s));
                assert!(!super::super::write_allowed(s));
                assert!(s.files.tag_edit.is_some());
                assert!(s.files.checked_nodes.contains_key("n1"));
                assert_eq!(
                    s.files.publications[&crate::files::review::Kind::Tags]
                        .operation_id
                        .as_deref(),
                    Some("original")
                );
                assert_eq!(s.files.operations.len(), 1);
                assert_eq!(s.files.operations[0], FileOperation::unknown("original"));
            });
            // A later valid poll settles this ID and draft, without a new write.
            assert_eq!(
                record_receipt(state, "original", published()),
                SaveStatus::Published
            );
            state.with_untracked(|s| {
                assert!(!super::super::pending(s));
                assert!(super::super::write_allowed(s));
                assert!(s.files.tag_edit.is_none());
                assert!(s.files.checked_nodes.is_empty());
                assert!(s.files.publications.is_empty());
                assert_eq!(s.files.operations.len(), 1);
            });
        }
    });
}

#[test]
fn receipt_observations_keep_terminal_and_unknown_statuses() {
    let mut aborted = published();
    aborted["commit_state"] = "ABORTED".into();
    aborted["state"] = "in_doubt".into();
    let mut pending = published();
    pending["commit_state"] = "IN_DOUBT".into();
    pending["state"] = "in_doubt".into();
    pending["protection_at_publish"] = "UNCONFIRMED".into();
    let mut degraded: Value =
        json_codec::from_str(include_str!("../../fixtures/degraded-save-operation.json")).unwrap();
    degraded["operation_id"] = "original".into();
    for (mut value, status) in [
        (published(), SaveStatus::Published),
        (aborted, SaveStatus::Rejected),
        (pending, SaveStatus::Unknown),
        (degraded, SaveStatus::Degraded),
    ] {
        let operation = FileOperation::from_receipt("original", &value);
        assert_eq!(operation.id(), "original");
        assert_eq!(operation.status(), status);
        assert_eq!(
            storage_contract::client_protocol::operation_status(&value),
            Some(status)
        );
        value["operation_id"] = "unrelated".into();
        value["state"] = "invalid".into();
        assert_eq!(operation.id(), "original");
        assert_eq!(operation.status(), status);
    }
}

#[test]
fn metadata_polling_requires_an_active_session_before_scheduling_network_work() {
    Owner::new().with(|| {
        for (authenticated, session_transition) in [
            (false, crate::ui::SessionTransition::Idle),
            (true, crate::ui::SessionTransition::LoggingOut),
            (true, crate::ui::SessionTransition::Restoring),
        ] {
            let mut initial = State {
                authenticated,
                session_transition,
                ..Default::default()
            };
            initial
                .files
                .operations
                .push(FileOperation::unknown("pending-rename"));
            let state = RwSignal::new(initial);
            // Native tests cannot schedule browser requests; returning here also
            // proves that inactive sessions do not admit a metadata poll.
            poll(state);
            state.with_untracked(|s| {
                assert!(s.files.polling_operations.is_empty());
                assert!(super::super::pending(s));
                assert!(!super::super::write_allowed(s));
            });
        }
    });
}

#[test]
fn poll_projection_copies_only_unpolled_operation_ids_and_keeps_session_epoch() {
    let mut state = State {
        authenticated: true,
        session_epoch: 17,
        ..Default::default()
    };
    state
        .files
        .history
        .install(crate::files::history::tests::chain(200));
    state.monitor.history = json!({"samples":[{"extra":"y".repeat(100_000)}]}).into();
    state.files.operations = vec![
        FileOperation::unknown("pending"),
        FileOperation::unknown("in-flight"),
        {
            let mut value = published();
            value["operation_id"] = "done".into();
            FileOperation::from_receipt("done", &value)
        },
    ];
    state.files.polling_operations.push("in-flight".into());
    crate::snapshot::CLONES.with(|c| c.set(0));
    assert_eq!(poll_work(&state), Some((17, vec!["pending".into()])));
    assert_eq!(
        crate::snapshot::CLONES.with(|c| c.get()),
        0,
        "polling must not clone State or its history snapshots"
    );
    state.session_transition = crate::ui::SessionTransition::LoggingOut;
    assert_eq!(poll_work(&state), None);
}

#[test]
fn local_outcomes_are_typed_but_network_markers_cannot_claim_rejection() {
    assert_eq!(
        FileOperation::unknown("original").status(),
        SaveStatus::Unknown
    );
    assert_eq!(
        FileOperation::rejected("original").status(),
        SaveStatus::Rejected
    );
    for status in ["REJECTED", "in_doubt", "completed"] {
        let operation = FileOperation::from_receipt(
            "original",
            &json!({"operation_id":"original", "state":status}),
        );
        assert_eq!(operation.id(), "original");
        assert_eq!(operation.status(), SaveStatus::Unknown);
    }
}

#[test]
fn observations_replace_only_the_original_id_and_keep_the_ui_bound() {
    Owner::new().with(|| {
        let state = RwSignal::new(State::default());
        for index in 0..101 {
            record_operation(state, FileOperation::unknown(&format!("operation-{index}")));
        }
        record_operation(state, FileOperation::rejected("operation-50"));
        state.with_untracked(|s| {
            assert_eq!(s.files.operations.len(), 100);
            assert_eq!(s.files.operations[0].id(), "operation-50");
            assert_eq!(s.files.operations[0].status(), SaveStatus::Rejected);
            assert_eq!(s.files.operations[1].id(), "operation-100");
            assert!(
                !s.files
                    .operations
                    .iter()
                    .any(|operation| operation.id() == "operation-0")
            );
            assert_eq!(
                s.files
                    .operations
                    .iter()
                    .filter(|operation| operation.id() == "operation-50")
                    .count(),
                1
            );
            assert!(super::super::pending(s));
        });
    });
}

fn poll_failure(request: &str, status: u16) -> api::Error {
    api::Error {
        code: storage_contract::ErrorCode::RecoveryRequired,
        message: "query unavailable".into(),
        status,
        retryable: true,
        request_id: Some(request.into()),
        source: None,
    }
}

#[test]
fn repeated_poll_failures_keep_one_notice_and_the_original_recovery_fence() {
    Owner::new().with(|| {
        let state = RwSignal::new(State {
            authenticated: true,
            session_epoch: 7,
            ..Default::default()
        });
        record_operation(state, FileOperation::unknown("original"));
        for attempt in 0..10 {
            state.update(|s| s.files.polling_operations.push("original".into()));
            complete_poll(
                state,
                7,
                "original",
                Err(poll_failure(&format!("query-{attempt}"), 503)),
            );
            state.with_untracked(|s| {
                assert_eq!(s.errors.len(), 1);
                assert_eq!(
                    s.errors[0].feedback.request_id,
                    Some(format!("query-{attempt}"))
                );
                assert_eq!(s.files.operations, vec![FileOperation::unknown("original")]);
                assert!(s.files.polling_operations.is_empty());
                assert!(super::super::pending(s));
                assert_eq!(poll_work(s), Some((7, vec!["original".into()])));
            });
        }
        state.update(|s| s.files.polling_operations.push("original".into()));
        complete_poll(state, 7, "original", Ok(published()));
        state.with_untracked(|s| {
            assert_eq!(s.files.operations[0].id(), "original");
            assert_eq!(s.files.operations[0].status(), SaveStatus::Published);
            assert!(s.files.polling_operations.is_empty());
            assert!(!super::super::pending(s));
            assert_eq!(poll_work(s), Some((7, vec![])));
            assert_eq!(
                s.errors.len(),
                1,
                "recovery does not silently dismiss diagnostic evidence"
            );
        });
    });
}

#[test]
fn poll_failures_cannot_cross_session_but_current_unauthorized_still_closes_admission() {
    Owner::new().with(|| {
        let state = RwSignal::new(State {
            authenticated: true,
            session_epoch: 8,
            ..Default::default()
        });
        state.update(|s| {
            s.files.operations.push(FileOperation::unknown("original"));
            s.files.polling_operations.push("original".into());
        });
        complete_poll(state, 7, "original", Err(poll_failure("old", 401)));
        state.with_untracked(|s| {
            assert!(s.authenticated);
            assert!(s.errors.is_empty());
            assert_eq!(s.files.polling_operations, vec!["original"]);
        });
        complete_poll(state, 8, "original", Err(poll_failure("current", 401)));
        state.with_untracked(|s| {
            assert!(!s.authenticated);
            assert_eq!(s.errors.len(), 1);
            assert_eq!(s.errors[0].feedback.request_id.as_deref(), Some("current"));
            assert!(s.files.polling_operations.is_empty());
            assert_eq!(s.files.operations, vec![FileOperation::unknown("original")]);
            assert!(super::super::pending(s));
        });
    });
}
