use super::*;
use crate::files::SaveStatus;

#[test]
fn business_mutations_require_an_active_session_before_browser_work() {
    for (authenticated, session_transition) in [
        (true, crate::ui::SessionTransition::LoggingOut),
        (false, crate::ui::SessionTransition::Idle),
    ] {
        Owner::new().with(|| {
            let state = RwSignal::new(State {
                authenticated,
                session_transition,
                ..Default::default()
            });
            state.update(|s| {
                s.files.writable = true;
                s.files.slot = "P01".into();
                s.files.selected =
                    Some(crate::files::test_entries::entry(json!({"file_id":"file"})));
            });
            let _ = namespace(state, NamespaceAction::CreateDirectory, "new".into());
            let _ = directory_branch(state, branch_source(), "draft".into());
            let _ = update_note(state, None, "note".into());
            select_branch(state, "other".into());
            let _ = record_mutation(
                state,
                MutationAccess::Current,
                "PATCH",
                "/api/test".into(),
                json!({}),
            );
            crate::files::review::submit(
                state,
                crate::files::review::Kind::Backup,
                "PUT",
                "/api/test".into(),
                json!({}),
            );
            state.update(|s| s.files.repository_id = "repository".into());
            crate::files::batch::start(state, crate::files::batch::Kind::Delete, None);
            assert!(!state.with_untracked(|s| s.files.mutation_pending));
            assert!(state.with_untracked(|s| s.files.batch.is_none()));
            assert!(state.with_untracked(|s| s.files.publications.is_empty()));
        });
    }
}

#[test]
fn refreshed_or_navigating_directories_block_writes_at_the_action_boundary() {
    Owner::new().with(|| {
        let state = RwSignal::new(State::default());
        state.update(|s| {
            s.files.writable = true;
            s.files.loading = true;
            s.files.selected = Some(crate::files::test_entries::entry(json!({"file_id":"file"})));
        });
        let _ = namespace(state, NamespaceAction::CreateDirectory, "new".into());
        let _ = directory_branch(state, branch_source(), "draft".into());
        let _ = update_note(state, None, "note".into());
        select_branch(state, "other".into());
        assert!(!state.with_untracked(|s| s.files.mutation_pending));
    });
}

#[test]
fn shared_dispatch_keeps_history_admission_before_ids_or_browser_work() {
    let mut initial = State {
        authenticated: true,
        ..Default::default()
    };
    initial.management.storage = crate::management::Storage::Ready;
    initial.files.writable = true;
    initial.files.loaded = true;
    initial.files.selected = Some(crate::files::test_entries::entry(json!({"file_id":"file"})));
    initial.files.file_history = crate::files::history::tests::history(json!({}));
    assert!(history_write_allowed(&initial));
    for change in 0..10 {
        Owner::new().with(|| {
            let mut blocked = initial.clone();
            match change {
                0 => blocked.authenticated = false,
                1 => blocked.session_transition = crate::ui::SessionTransition::LoggingOut,
                2 => blocked.management.storage = crate::management::Storage::RecoveryRequired,
                3 => blocked.files.writable = false,
                4 => blocked.files.loading = true,
                5 => blocked.files.mutation_pending = true,
                6 => blocked
                    .files
                    .operations
                    .push(FileOperation::unknown("original")),
                7 => blocked.files.history_loading = true,
                8 => blocked.files.file_history.invalidate(),
                9 => {
                    blocked.files.selected = Some(crate::files::test_entries::entry(json!({
                        "file_id":"file", "read_only":true,
                    })));
                }
                _ => unreachable!(),
            }
            let pending = blocked.files.mutation_pending;
            let operations = blocked.files.operations.len();
            let state = RwSignal::new(blocked);
            assert!(
                update_note(state, None, "file note".into()).is_none(),
                "{change}"
            );
            assert!(
                update_note(state, Some("version".into()), "version note".into()).is_none(),
                "{change}"
            );
            assert!(
                create_branch(state, "version".into(), Some("node".into()), "draft".into())
                    .is_none(),
                "{change}"
            );
            state.with_untracked(|state| {
                assert_eq!(state.files.mutation_pending, pending);
                assert_eq!(state.files.operations.len(), operations);
                assert!(state.files.default_operation.is_none());
                assert!(state.errors.is_empty());
            });
        });
    }
}

#[test]
fn failed_editor_requests_record_only_their_original_outcome() {
    Owner::new().with(|| {
        for (status, expected) in [
            (403, SaveStatus::Rejected),
            (409, SaveStatus::Rejected),
            (0, SaveStatus::Unknown),
            (200, SaveStatus::Unknown),
            (500, SaveStatus::Unknown),
        ] {
            let state = RwSignal::new(State::default());
            state.update(|s| s.notices.begin_operation("original".into()));
            record_failure(
                state,
                0,
                "original",
                api::Error {
                    status,
                    code: storage_contract::ErrorCode::InjectedFailure,
                    message: "request failed".into(),
                    retryable: false,
                    request_id: Some("failure-request".into()),
                    source: None,
                },
            );
            state.with_untracked(|s| {
                assert_eq!(s.files.operations[0].id(), "original");
                assert_eq!(s.files.operations[0].status(), expected);
                assert_eq!(super::super::pending(s), expected == SaveStatus::Unknown);
                assert_eq!(s.errors.len(), 1);
                assert_eq!(
                    s.errors[0].feedback.request_id.as_deref(),
                    Some("failure-request")
                );
                assert!(s.errors[0].feedback.message.contains("request failed"));
                if expected == SaveStatus::Unknown {
                    assert_eq!(
                        s.notices.message(),
                        Some(crate::files::model::publication_notice(expected).as_str())
                    );
                } else {
                    assert!(s.notices.message().is_none());
                }
            });
        }
    });
}

#[test]
fn published_reconciliation_failure_keeps_the_original_operation_pending() {
    Owner::new().with(|| {
        let state = RwSignal::new(State::default());
        state.update(|s| s.notices.begin_operation("published-original".into()));
        record_failure(
            state,
            0,
            "published-original",
            api::Error {
                status: 503,
                code: storage_contract::ErrorCode::RecoveryRequired,
                message: "operation published-original is already published; retain the original operation".into(),
                retryable: false,
                request_id: Some("published-request".into()),
                source: None,
            },
        );
        state.with_untracked(|s| {
            assert_eq!(s.files.operations.len(), 1);
            assert_eq!(s.files.operations[0].id(), "published-original");
            assert_eq!(s.files.operations[0].status(), SaveStatus::Unknown);
            assert!(super::super::pending(s));
            assert_eq!(s.errors.len(), 1);
            assert_eq!(s.errors[0].feedback.request_id.as_deref(), Some("published-request"));
            assert!(s.errors[0].feedback.message.contains("already published"));
            assert_eq!(s.notices.message(), Some(crate::files::model::publication_notice(SaveStatus::Unknown).as_str()));
        });
    });
}

#[test]
fn blocked_actions_do_not_clone_unrelated_state_snapshots() {
    Owner::new().with(|| {
        let state = RwSignal::new(State::default());
        crate::snapshot::CLONES.with(|count| count.set(0));
        assert!(namespace(state, NamespaceAction::CreateDirectory, "new".into()).is_none());
        crate::files::merge::start(state);
        crate::files::tags::start(state);
        crate::files::batch::start(state, crate::files::batch::Kind::Delete, None);
        crate::snapshot::CLONES.with(|count| assert_eq!(count.get(), 0));
    });
}

#[test]
fn preview_is_not_general_mutation_authority() {
    use crate::files::model::DirectoryPreview;
    let mut state = State {
        authenticated: true,
        ..Default::default()
    };
    state.files.writable = true;
    state.files.loaded = true;
    state.files.directory_preview = Some(DirectoryPreview {
        source: crate::files::model::DirectoryPreviewSource::Branch(
            crate::files::test_entries::branch(json!({"directory_branch_id":"draft"})),
        ),
        path: String::new(),
    });
    assert!(!super::super::write_allowed(&state));
    assert!(!super::super::history_write_allowed(&state));
    state.files.directory_preview = None;
    state.files.file_preview = Some(crate::files::history::tests::branch(
        json!({"branch_id":"draft"}),
    ));
    assert!(!super::super::write_allowed(&state));
    assert!(!super::super::history_write_allowed(&state));
}

#[test]
fn selecting_an_empty_file_branch_is_browsing_without_a_publication() {
    Owner::new().with(|| {
        let state = RwSignal::new(State {
            authenticated: true,
            ..Default::default()
        });
        state.update(|s| {
            s.files.file_history = crate::files::history::tests::history(json!({
                "selected_branch_id":"default",
                "branches":[crate::files::history::tests::branch_wire(json!({
                    "branch_id":"empty", "node_id":null, "version_id":null,
                    "materialized":false, "own_version_id":null,
                }))],
            }));
        });
        select_branch(state, "empty".into());
        state.with_untracked(|s| {
            assert_eq!(s.files.file_preview.as_ref().unwrap().id(), "empty");
            assert_eq!(s.files.file_history.selected(), Some("default"));
            assert!(!s.files.mutation_pending);
            assert!(s.files.operations.is_empty() && s.files.default_operation.is_none());
        });
    });
}

fn branch_source() -> storage_contract::directory_git::DirectoryBranchCreateSource {
    storage_contract::directory_git::DirectoryBranchCreateSource {
        repository_id: format!("repo_{:032x}", 1),
        directory_id: format!("entry_{:032x}", 1),
        directory_branch_id: format!("dbr_{:032x}", 1),
        working_tree_id: format!("dtree_{:032x}", 1),
        head_tree_id: format!("dtree_{:032x}", 1),
        index_generation: 1,
        namespace_head: format!("commit_{:032x}", 1),
    }
}
