use super::*;

#[test]
fn receipts_settle_only_the_matching_publication_and_preserve_unknown_outcomes() {
    let mut model = Model {
        checked_nodes: crate::files::tags::tests::selection(&["selected-node"]),
        ..Default::default()
    };
    model.publications.insert(
        Kind::Tags,
        Publication {
            operation_id: Some("original-tags-operation".into()),
            error: "network interrupted".into(),
        },
    );
    model.publications.insert(
        Kind::Merge,
        Publication {
            operation_id: Some("different-merge-operation".into()),
            error: String::new(),
        },
    );
    settle(&mut model, "unrelated-operation", SaveStatus::Published);
    assert_eq!(model.publications.len(), 2);
    settle(&mut model, "original-tags-operation", SaveStatus::Unknown);
    assert_eq!(
        model.publications[&Kind::Tags].operation_id.as_deref(),
        Some("original-tags-operation")
    );
    assert!(model.checked_nodes.contains("selected-node"));
    settle(&mut model, "original-tags-operation", SaveStatus::Rejected);
    assert!(model.publications[&Kind::Tags].operation_id.is_none());
    assert!(!model.publications[&Kind::Tags].error.is_empty());
    assert!(model.checked_nodes.contains("selected-node"));
    model
        .publications
        .get_mut(&Kind::Tags)
        .unwrap()
        .operation_id = Some("retry-tags-operation".into());
    settle(&mut model, "original-tags-operation", SaveStatus::Published);
    assert!(model.checked_nodes.contains("selected-node"));
    settle(&mut model, "retry-tags-operation", SaveStatus::Degraded);
    assert!(model.checked_nodes.is_empty());
    assert!(!model.publications.contains_key(&Kind::Tags));
    assert_eq!(
        model.publications[&Kind::Merge].operation_id.as_deref(),
        Some("different-merge-operation")
    );
}

fn initial() -> (State, Value) {
    let mut state = State {
        authenticated: true,
        ..Default::default()
    };
    state.management.storage = crate::management::Storage::Ready;
    state.files.writable = true;
    state.files.selected = Some(crate::files::test_entries::entry(json_codec::json!({
        "file_id":"file", "repository_id":"repo", "repository_path":"a.txt", "path":"a.txt"
    })));
    let (draft, selected, body) = crate::files::tags::tests::submission_fixture();
    state.files.tag_edit = Some(draft);
    state.files.checked_nodes = selected;
    (state, body)
}
fn start(state: &mut State, body: Value) -> Attempt {
    assert!(can_start(state, Kind::Tags));
    begin_submission(
        state,
        Kind::Tags,
        "original-tags".into(),
        "POST",
        "/api/file-manager/files/file/tags".into(),
        body,
    )
}
fn failure(status: u16) -> api::Error {
    api::Error {
        code: if status == 409 {
            storage_contract::ErrorCode::FileBranchConflict
        } else {
            storage_contract::ErrorCode::RecoveryRequired
        },
        message: "test request failed".into(),
        retryable: true,
        status,
        request_id: Some(format!("request-{status}")),
        source: None,
    }
}
pub(crate) fn outcome(id: &str) -> Value {
    let mut value: Value =
        json_codec::from_str(include_str!("../fixtures/tag-operation.json")).unwrap();
    value["operation_id"] = id.into();
    value
}

pub(crate) fn uncertain_tags() -> RwSignal<State> {
    let (mut initial, body) = initial();
    let attempt = start(&mut initial, body);
    let state = RwSignal::new(initial);
    complete(state, Kind::Tags, &attempt, Err(failure(0)));
    state
}
pub(crate) fn in_flight_tags() -> RwSignal<State> {
    let state = uncertain_tags();
    assert!(state.try_update(begin_retry).flatten().is_some());
    state
}
pub(crate) fn unauthorized_retry() -> RwSignal<State> {
    let state = uncertain_tags();
    let retry = state.try_update(begin_retry).flatten().unwrap();
    complete(state, Kind::Tags, &retry, Err(failure(401)));
    state
}

#[test]
fn uncertain_retry_freezes_all_targets_and_is_available_after_navigation() {
    Owner::new().with(|| {
        let (mut initial, body) = initial();
        let attempt = start(&mut initial, body);
        let original = attempt.request.body.clone();
        let state = RwSignal::new(initial);
        assert!(!complete(state, Kind::Tags, &attempt, Err(failure(0))));
        assert!(state.with_untracked(can_retry_tags));
        state.update(|s| {
            s.files.navigate(crate::files::model::Navigation::Root);
            s.files.tag_edit = Some(crate::files::tags::tests::draft_for_instance(
                88,
                "other input",
            ));
        });
        assert!(!state.with_untracked(actions::write_allowed));
        assert!(
            state.with_untracked(can_retry_tags),
            "original scope has independent retry admission"
        );
        let retry = state.try_update(begin_retry).flatten().unwrap();
        assert_eq!(retry.request.id, "original-tags");
        assert_eq!(retry.request.method, "POST");
        assert_eq!(retry.request.endpoint, "/api/file-manager/files/file/tags");
        assert_eq!(retry.request.body, original);
        assert_eq!(retry.request.body["targets"].as_array().unwrap().len(), 2);
        assert!(
            state.try_update(begin_retry).flatten().is_none(),
            "double click cannot start another attempt"
        );
        assert!(!complete(
            state,
            Kind::Tags,
            &retry,
            Ok(outcome("original-tags"))
        ));
        state.with_untracked(|s| {
            assert!(s.files.tag_submission.is_none());
            assert!(!actions::pending(s));
            assert_eq!(
                s.files.tag_edit.as_ref().unwrap().instance(),
                88,
                "old success must not clear a successor draft"
            );
        });
    });
}

#[test]
fn retry_errors_and_malformed_success_never_reclassify_an_uncertain_original() {
    for status in [0, 401, 403, 404, 409, 429, 503] {
        Owner::new().with(|| {
            let (mut initial, body) = initial();
            let attempt = start(&mut initial, body);
            let original = attempt.request.body.clone();
            let state = RwSignal::new(initial);
            complete(state, Kind::Tags, &attempt, Err(failure(0)));
            let retry = state.try_update(begin_retry).flatten().unwrap();
            assert!(!complete(state, Kind::Tags, &retry, Err(failure(status))));
            state.with_untracked(|s| {
                assert!(actions::pending(s));
                assert!(has_tag_replay(s));
                assert_eq!(
                    s.files.tag_submission.as_ref().unwrap().request.body,
                    original
                );
                assert!(
                    !s.files.tag_edit.as_ref().unwrap().requires_review(),
                    "an uncertain original is not a fresh rejected request"
                );
                assert_eq!(s.authenticated, status != 401);
            });
            if status == 401 {
                assert!(!state.with_untracked(can_retry_tags));
                // Actual poll admission after revocation is covered in operation tests.
                return;
            }
            // A late authoritative outcome can still settle the original ID.
            actions::record_receipt(state, "original-tags", outcome("original-tags"));
            assert!(!state.with_untracked(actions::pending));
            assert!(!state.with_untracked(has_tag_replay));
        });
    }
    Owner::new().with(|| {
        let (mut initial, body) = initial();
        let attempt = start(&mut initial, body);
        let state = RwSignal::new(initial);
        complete(state, Kind::Tags, &attempt, Ok(Value::Null));
        let retry = state.try_update(begin_retry).flatten().unwrap();
        complete(state, Kind::Tags, &retry, Ok(Value::Null));
        assert!(state.with_untracked(actions::pending));
        assert!(state.with_untracked(can_retry_tags));
    });
}

#[test]
fn retry_admission_is_session_storage_and_poll_scoped_and_old_attempts_do_not_clear_busy() {
    Owner::new().with(|| {
        let (mut initial, body) = initial();
        let first = start(&mut initial, body);
        let state = RwSignal::new(initial);
        complete(state, Kind::Tags, &first, Err(failure(0)));
        state.update(|s| s.files.polling_operations.push("original-tags".into()));
        assert!(state.try_update(begin_retry).flatten().is_none());
        state.update(|s| s.files.polling_operations.clear());
        for storage in [
            crate::management::Storage::ReadOnly,
            crate::management::Storage::Unavailable,
            crate::management::Storage::Unknown,
            crate::management::Storage::RecoveryRequired,
            crate::management::Storage::Unconfigured,
            crate::management::Storage::TerminalManagement,
        ] {
            state.update(|s| s.management.storage = storage);
            assert!(!state.with_untracked(can_retry_tags));
            assert!(state.try_update(begin_retry).flatten().is_none());
        }
        state.update(|s| s.management.storage = crate::management::Storage::Ready);
        let retry = state.try_update(begin_retry).flatten().unwrap();
        complete(state, Kind::Tags, &first, Err(failure(409)));
        state.with_untracked(|s| {
            assert!(s.files.mutation_pending);
            assert!(tag_attempt_in_flight(s, "original-tags"));
        });
        complete(state, Kind::Tags, &retry, Err(failure(503)));
        state.update(|s| s.authenticated = false);
        assert!(!state.with_untracked(can_retry_tags));
        state.update(|s| {
            s.authenticated = true;
            s.session_epoch += 1;
        });
        assert!(!state.with_untracked(can_retry_tags));
        complete(state, Kind::Tags, &retry, Err(failure(401)));
        assert!(
            state.with_untracked(|s| s.authenticated),
            "old epoch must not revoke the replacement session"
        );
    });
}

#[test]
fn fresh_conflict_marks_only_the_original_draft_and_cancels_its_old_review() {
    Owner::new().with(|| {
        let (mut initial, body) = initial();
        let owner = super::super::history::reads::ReviewOwner {
            kind: Kind::Tags,
            instance: 1,
        };
        let generation = initial.files.history_aux.begin_review(owner);
        let attempt = start(&mut initial, body);
        let state = RwSignal::new(initial);
        assert!(complete(state, Kind::Tags, &attempt, Err(failure(409))));
        state.with_untracked(|s| {
            assert!(!actions::pending(s));
            assert!(!can_start(s, Kind::Tags));
            assert!(s.files.tag_edit.as_ref().unwrap().requires_review());
            assert!(!s.files.history_aux.current_review(owner, generation));
            assert_eq!(s.files.checked_nodes.len(), 2);
            assert!(s.files.tag_submission.is_none());
        });
    });
    Owner::new().with(|| {
        let (mut initial, body) = initial();
        let attempt = start(&mut initial, body);
        let state = RwSignal::new(initial);
        state.update(|s| {
            s.files.tag_edit = Some(crate::files::tags::tests::draft_for_instance(
                2,
                "successor",
            ))
        });
        assert!(!complete(state, Kind::Tags, &attempt, Err(failure(409))));
        assert!(!state.with_untracked(|s| s.files.tag_edit.as_ref().unwrap().requires_review()));
    });
}

#[test]
fn fresh_inactive_branch_conflict_also_requires_explicit_review() {
    Owner::new().with(|| {
        let (mut initial, body) = initial();
        let attempt = start(&mut initial, body);
        let state = RwSignal::new(initial);
        let mut error = failure(409);
        error.code = storage_contract::ErrorCode::BranchNotActive;
        assert!(complete(state, Kind::Tags, &attempt, Err(error)));
        assert!(state.with_untracked(|s| s.files.tag_edit.as_ref().unwrap().requires_review()));
    });
}
#[test]
fn a_receipt_for_another_operation_kind_cannot_settle_the_frozen_tag_request() {
    Owner::new().with(|| {
        let state = uncertain_tags();
        let retry = state.try_update(begin_retry).flatten().unwrap();
        let mut wrong = outcome("original-tags");
        wrong["operation_kind"] = "directory.namespace.batch".into();
        complete(state, Kind::Tags, &retry, Ok(wrong));
        assert!(state.with_untracked(actions::pending));
        assert!(state.with_untracked(has_tag_replay));
        let retry = state.try_update(begin_retry).flatten().unwrap();
        complete(state, Kind::Tags, &retry, Ok(outcome("original-tags")));
        assert!(!state.with_untracked(actions::pending));
    });
}

#[test]
fn mixed_backup_submission_freezes_endpoint_body_and_operation_before_unknown_outcome() {
    Owner::new().with(|| {
        let mut initial = State {
            authenticated: true,
            ..Default::default()
        };
        initial.management.storage = crate::management::Storage::Ready;
        initial.files.writable = true;
        let endpoint = "/api/repositories/repo/recursive-backup-tree-configuration";
        let body = json_codec::json!({"path":"docs", "capture":{"source_sequence":"7"}});
        assert!(can_start(&initial, Kind::Backup));
        let attempt = begin_submission(
            &mut initial,
            Kind::Backup,
            "original-tree".into(),
            "PUT",
            endpoint.into(),
            body.clone(),
        );
        let state = RwSignal::new(initial);
        complete(state, Kind::Backup, &attempt, Err(failure(0)));
        assert_eq!(attempt.request.endpoint, endpoint);
        assert_eq!(attempt.request.body["capture"], body["capture"]);
        assert_eq!(attempt.request.body["operation_id"], "original-tree");
        assert!(!state.with_untracked(|s| can_start(s, Kind::Backup)));
        state.with_untracked(|s| {
            assert_eq!(
                s.files.publications[&Kind::Backup].operation_id.as_deref(),
                Some("original-tree")
            );
            assert_eq!(s.files.operations[0].id(), "original-tree");
            assert_eq!(s.files.operations[0].status(), SaveStatus::Unknown);
        });
    });
}
