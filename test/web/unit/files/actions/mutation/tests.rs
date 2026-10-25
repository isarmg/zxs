use super::*;

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
                s.files.selected = Some(json!({"file_id":"file"}));
            });
            // No browser ID generation or server request may start after
            // sign-out admission closes, even while credentials are kept
            // for a failed revocation retry.
            let _ = namespace(state, NamespaceAction::CreateDirectory, "new".into());
            let _ = directory_branch(state, "folder".into(), "draft".into(), false);
            let _ = update_note(state, None, "note".into());
            select_branch(state, "other".into());
            let _ = record_mutation(state, "PATCH", "/api/test".into(), json!({}));
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
            s.files.selected = Some(json!({"file_id":"file"}));
        });
        // These must return before generating IDs or spawning browser work.
        let _ = namespace(state, NamespaceAction::CreateDirectory, "new".into());
        let _ = directory_branch(state, "folder".into(), "draft".into(), false);
        let _ = update_note(state, None, "note".into());
        select_branch(state, "other".into());
        assert!(!state.with_untracked(|s| s.files.mutation_pending));
    });
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
            record_failure(
                state,
                "original",
                api::Error {
                    status,
                    code: storage_contract::ErrorCode::InjectedFailure,
                    message: "request failed".into(),
                    retryable: false,
                    request_id: None,
                },
            );
            state.with_untracked(|s| {
                assert_eq!(s.files.operations[0]["operation_id"], "original");
                assert_eq!(
                    crate::files::model::operation_status(&s.files.operations[0]),
                    expected
                );
                assert_eq!(super::super::pending(s), expected == SaveStatus::Unknown);
            });
        }
    });
}
