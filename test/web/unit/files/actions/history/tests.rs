use super::*;
use crate::files::history::tests::{ROOT, chain, window_wire};
use crate::files::test_entries::entry;
use json_codec::json;

fn selected_state() -> State {
    let mut state = State {
        authenticated: true,
        session_epoch: 3,
        ..Default::default()
    };
    state.files.selected = Some(entry(json!({
        "entry_id":"entry", "file_id":"file", "kind":"file", "name":"a.txt", "path":"docs/a.txt",
        "repository_id":"repo", "repository_path":"docs/a.txt", "version_id":"n1", "node_id":"n1"
    })));
    state
}

fn injected(code: storage_contract::ErrorCode, status: u16) -> api::Error {
    api::Error {
        code,
        message: "injected".into(),
        retryable: false,
        status,
        request_id: None,
        source: None,
    }
}

#[test]
fn main_endpoints_carry_context_limit_and_exactly_one_position_parameter() {
    let context = ("repo".to_owned(), "docs/a b.txt".to_owned());
    assert_eq!(
        main_endpoint("file", Some(&context), &MainRequest::First, None),
        "/api/file-manager/files/file/history?repository_id=repo&path=docs%2Fa%20b.txt&limit=50"
    );
    assert_eq!(
        main_endpoint("file", None, &MainRequest::Cursor("ab".into()), Some(ROOT)),
        format!("/api/file-manager/files/file/history?limit=50&cursor=ab&history_root={ROOT}")
    );
    assert_eq!(
        main_endpoint("file", None, &MainRequest::Anchor("n/1".into()), Some(ROOT)),
        format!(
            "/api/file-manager/files/file/history?limit=50&anchor_node_id=n%2F1&history_root={ROOT}"
        )
    );
}

#[test]
fn history_reads_require_an_active_session_and_a_selected_file() {
    Owner::new().with(|| {
        for (authenticated, transition) in [
            (false, crate::ui::SessionTransition::Idle),
            (true, crate::ui::SessionTransition::Restoring),
            (true, crate::ui::SessionTransition::LoggingOut),
        ] {
            let mut initial = selected_state();
            initial.authenticated = authenticated;
            initial.session_transition = transition;
            let state = RwSignal::new(initial);
            assert!(begin_main(state, &MainRequest::First, None).is_none());
            state.with_untracked(|s| {
                assert!(!s.files.history.loading());
                assert_eq!(s.files.history.generation, 0);
            });
        }
        let state = RwSignal::new(State {
            authenticated: true,
            ..Default::default()
        });
        assert!(begin_main(state, &MainRequest::First, None).is_none());
        // Continuations and anchors need a current root; the first page does not.
        let state = RwSignal::new(selected_state());
        assert!(begin_main(state, &MainRequest::Cursor("ab".into()), None).is_none());
        assert!(begin_main(state, &MainRequest::Anchor("n1".into()), None).is_none());
        assert!(begin_main(state, &MainRequest::First, None).is_some());
    });
}

#[test]
fn a_main_response_replaces_the_window_and_late_responses_are_rejected_after_any_fence_change() {
    Owner::new().with(|| {
        let state = RwSignal::new(selected_state());
        let (fence, endpoint) = begin_main(state, &MainRequest::First, None).unwrap();
        assert!(state.with_untracked(|s| s.files.history.loading()));
        complete_main(state, &fence, &endpoint, Ok(chain(2)));
        state.with_untracked(|s| {
            assert_eq!(s.files.history.loaded().unwrap().nodes.len(), 2);
            assert!(!s.files.history.loading());
        });
        let first = state.with_untracked(|s| s.files.history.loaded().unwrap().clone());

        // A newer read supersedes the previous fence; the old page stays
        // readable until the new response is installed. While a replacement
        // is loading there is no current root, so only a first page can start.
        let (old, _) = begin_main(state, &MainRequest::Cursor("ab".into()), None).unwrap();
        assert!(begin_main(state, &MainRequest::Anchor("n1".into()), None).is_none());
        let (new, new_endpoint) = begin_main(state, &MainRequest::First, None).unwrap();
        assert!(state.with_untracked(|s| s.files.history.window().is_some()));
        complete_main(state, &old, &endpoint, Ok(chain(3)));
        state.with_untracked(|s| {
            assert!(
                s.files.history.loading(),
                "a superseded response cannot install"
            );
            assert_eq!(s.files.history.window().unwrap(), &first);
        });
        complete_main(state, &new, &new_endpoint, Ok(chain(1)));
        state.with_untracked(|s| {
            assert_eq!(s.files.history.loaded().unwrap().nodes.len(), 1);
        });

        for change in ["file", "context", "epoch", "cleared"] {
            let state = RwSignal::new(selected_state());
            let (fence, endpoint) = begin_main(state, &MainRequest::First, None).unwrap();
            state.update(|s| match change {
                "file" => crate::files::test_entries::update(
                    s.files.selected.as_mut().unwrap(),
                    json!({"file_id":"other"}),
                ),
                "context" => crate::files::test_entries::update(
                    s.files.selected.as_mut().unwrap(),
                    json!({"repository_path":"docs/b.txt","path":"docs/b.txt","name":"b.txt"}),
                ),
                "epoch" => s.session_epoch += 1,
                "cleared" => s.files.clear_selection(),
                _ => unreachable!(),
            });
            complete_main(state, &fence, &endpoint, Ok(chain(2)));
            state.with_untracked(|s| {
                assert!(s.files.history.loaded().is_none(), "{change}");
            });
        }
    });
}

#[test]
fn a_cancelled_read_never_settles_even_if_its_response_races_the_abort() {
    Owner::new().with(|| {
        let state = RwSignal::new(selected_state());
        let (fence, endpoint) = begin_main(state, &MainRequest::First, None).unwrap();
        state.update(|s| s.files.history.reads.abort(ReadSlot::Main));
        complete_main(state, &fence, &endpoint, Ok(chain(2)));
        state.with_untracked(|s| assert!(s.files.history.loaded().is_none()));
        complete_main(
            state,
            &fence,
            &endpoint,
            Err(injected(storage_contract::ErrorCode::InjectedFailure, 503)),
        );
        state.with_untracked(|s| assert!(s.errors.is_empty(), "a cancelled read reports nothing"));
    });
}

#[test]
fn history_changed_marks_the_resident_page_stale_and_closes_write_entries_without_losing_drafts() {
    Owner::new().with(|| {
        let state = RwSignal::new(selected_state());
        state.update(|s| {
            s.management.storage = crate::management::Storage::Ready;
            s.files.writable = true;
        });
        let (fence, endpoint) = begin_main(state, &MainRequest::First, None).unwrap();
        complete_main(state, &fence, &endpoint, Ok(chain(2)));
        // Branch page and a tag selection depend on this root.
        state.update(|s| {
            s.files.history.branches = Some(crate::files::history::tests::branch_page(
                vec![crate::files::history::tests::branch_wire(
                    json!({"current":true}),
                )],
                Some("branch"),
            ));
            s.files.open_editors = 1;
        });
        assert_eq!(
            state.try_update(|s| crate::files::tags::check(&mut s.files, "n2", true)),
            Some(crate::files::tags::Check::Changed)
        );
        crate::files::tags::start(state);
        assert!(state.with_untracked(|s| s.files.tag_edit.is_some()));
        assert!(state.with_untracked(|s| crate::files::model::history_root(&s.files).is_some()));

        // An auxiliary read reports 409.
        let aux = begin_aux(state, ReadSlot::Merges, None, |binding| {
            format!("/api/file-manager/files/{}/history/merges", binding.file_id)
        })
        .unwrap();
        assert!(aux.endpoint.ends_with(&format!("?history_root={ROOT}")));
        complete_aux::<MergePage>(
            state,
            &aux,
            Err(injected(storage_contract::ErrorCode::HistoryChanged, 409)),
            |_, _| panic!("a stale response installs nothing"),
        );
        state.with_untracked(|s| {
            assert!(s.files.history.stale());
            assert!(crate::files::model::history_root(&s.files).is_none());
            assert!(s.files.history.branch_page().is_none());
            assert_eq!(s.files.history.window().unwrap().nodes.len(), 2);
            assert_eq!(s.files.open_editors, 0);
            assert!(s.notice.contains("changed"));
            assert!(
                s.errors.is_empty(),
                "a stale root is a state, not an error notice"
            );
            // Drafts and selections are retained as input awaiting review.
            assert!(s.files.tag_edit.is_some());
            assert!(s.files.checked_nodes.contains_key("n2"));
            assert!(!crate::files::merge::available(&s.files));
        });
        // Write entries that depend on the window refuse to start.
        assert!(
            crate::files::actions::update_node_note(state, "n2".into(), "note".into()).is_none()
        );
        assert!(crate::files::actions::update_note(state, None, "note".into()).is_none());
        assert!(begin_aux(state, ReadSlot::Branches, None, |_| String::new()).is_none());
        let (_, endpoint) = begin_main(state, &MainRequest::Latest, None).unwrap();
        assert!(!endpoint.contains("history_root="));
    });
}

#[test]
fn a_main_continuation_that_lost_its_root_keeps_the_previous_page_as_stale() {
    Owner::new().with(|| {
        let state = RwSignal::new(selected_state());
        let (fence, endpoint) = begin_main(state, &MainRequest::First, None).unwrap();
        complete_main(state, &fence, &endpoint, Ok(chain(2)));
        let (fence, endpoint) = begin_main(state, &MainRequest::Cursor("ab".into()), None).unwrap();
        complete_main(
            state,
            &fence,
            &endpoint,
            Err(injected(storage_contract::ErrorCode::HistoryChanged, 409)),
        );
        state.with_untracked(|s| {
            assert!(s.files.history.stale());
            assert_eq!(s.files.history.window().unwrap().nodes.len(), 2);
        });
    });
}

#[test]
fn latest_refresh_never_sends_the_resident_root_even_when_the_selection_did_not_change() {
    Owner::new().with(|| {
        let mut state = selected_state();
        let mut wire = window_wire(
            vec![crate::files::history::tests::node("n1", 1, &[], &["n1"])],
            json!({}),
        );
        wire["context"]["current_node_id"] = "n1".into();
        wire["nodes"][0]["current"] = true.into();
        state
            .files
            .history
            .install(crate::files::history::HistoryWindow::parse(wire, "file").unwrap());
        let state = RwSignal::new(state);
        let (_, endpoint) = begin_main(state, &MainRequest::Latest, None).unwrap();
        assert!(!endpoint.contains("history_root="));
        assert!(!endpoint.contains("anchor_node_id="));
        assert!(state.with_untracked(|s| s.files.history.root().is_none()));
    });
}

#[test]
fn failed_main_reads_report_once_and_leave_no_write_basis() {
    Owner::new().with(|| {
        let state = RwSignal::new(selected_state());
        let (fence, endpoint) = begin_main(state, &MainRequest::First, None).unwrap();
        complete_main(state, &fence, &endpoint, Ok(chain(1)));
        let (fence, endpoint) = begin_main(state, &MainRequest::First, None).unwrap();
        complete_main(
            state,
            &fence,
            &endpoint,
            Err(injected(storage_contract::ErrorCode::InjectedFailure, 503)),
        );
        state.with_untracked(|s| {
            assert!(matches!(
                s.files.history.state,
                crate::files::history::state::HistoryState::Failed(_)
            ));
            assert!(s.files.history.window().is_none());
            assert!(crate::files::model::history_root(&s.files).is_none());
            assert_eq!(s.errors.len(), 1);
        });
        // A current-session 401 closes the session before any history state.
        let state = RwSignal::new(selected_state());
        let (fence, endpoint) = begin_main(state, &MainRequest::First, None).unwrap();
        complete_main(
            state,
            &fence,
            &endpoint,
            Err(injected(storage_contract::ErrorCode::Unauthorized, 401)),
        );
        state.with_untracked(|s| {
            assert!(!s.session_active());
            assert!(
                s.files.history.loading(),
                "the fence never ran for a revoked session"
            );
        });
    });
}

#[test]
fn open_editors_block_first_previous_next_anchor_and_explicit_refresh() {
    Owner::new().with(|| {
        let state = RwSignal::new(selected_state());
        let (fence, endpoint) = begin_main(state, &MainRequest::First, None).unwrap();
        complete_main(state, &fence, &endpoint, Ok(chain(2)));
        state.update(|s| s.files.open_editors = 1);
        assert!(begin_main(state, &MainRequest::First, None).is_none());
        assert!(begin_main(state, &MainRequest::Latest, None).is_none());
        assert!(begin_main(state, &MainRequest::Cursor("ab".into()), None).is_none());
        assert!(begin_main(state, &MainRequest::Anchor("n1".into()), None).is_none());
        state.with_untracked(|s| {
            assert!(s.notice.contains("Finish or cancel"));
            assert!(
                s.files.history.loaded().is_some(),
                "the page was not replaced"
            );
        });
        state.update(|s| s.files.open_editors = 0);
        assert!(begin_main(state, &MainRequest::Cursor("ab".into()), None).is_some());
    });
}

#[test]
fn auxiliary_reads_bind_the_resident_root_and_are_fenced_per_slot() {
    Owner::new().with(|| {
        let state = RwSignal::new(selected_state());
        assert!(
            begin_aux(state, ReadSlot::Branches, None, |_| String::new()).is_none(),
            "no window, no request"
        );
        let (fence, endpoint) = begin_main(state, &MainRequest::First, None).unwrap();
        complete_main(state, &fence, &endpoint, Ok(chain(2)));
        let branches = begin_aux(state, ReadSlot::Branches, None, |binding| {
            format!(
                "/api/file-manager/files/{}/history/branches?limit=100",
                binding.file_id
            )
        })
        .unwrap();
        assert_eq!(
            branches.endpoint,
            format!("/api/file-manager/files/file/history/branches?limit=100&history_root={ROOT}")
        );
        let merges = begin_aux(state, ReadSlot::Merges, None, |binding| {
            format!("/api/file-manager/files/{}/history/merges", binding.file_id)
        })
        .unwrap();
        // A second read in the same slot supersedes the first; other slots stay valid.
        let branches_again =
            begin_aux(state, ReadSlot::Branches, None, |_| "/api/x".into()).unwrap();
        complete_aux(
            state,
            &branches,
            Ok(crate::files::history::tests::branch_page(
                vec![],
                Some("old"),
            )),
            |s, page| s.files.history.branches = Some(page),
        );
        state.with_untracked(|s| assert!(s.files.history.branches.is_none()));
        complete_aux(
            state,
            &branches_again,
            Ok(crate::files::history::tests::branch_page(
                vec![],
                Some("new"),
            )),
            |s, page| s.files.history.branches = Some(page),
        );
        complete_aux(
            state,
            &merges,
            Ok(MergePage::parse(
                crate::files::history::tests::merge_page_wire(vec![]),
                &crate::files::history::tests::binding(),
            )
            .unwrap()),
            |s, page| s.files.history.merges = Some(page),
        );
        state.with_untracked(|s| {
            assert!(s.files.history.branch_page().is_some());
            assert!(s.files.history.merge_page().is_some());
        });
        // Replacing the main window drops auxiliary pages and rejects their late responses.
        let pending = begin_aux(state, ReadSlot::Panel, None, |_| "/api/p".into()).unwrap();
        let (fence, endpoint) = begin_main(state, &MainRequest::First, None).unwrap();
        complete_main(state, &fence, &endpoint, Ok(chain(1)));
        complete_aux(state, &pending, Ok(()), |_, ()| {
            panic!("a response for the previous window must not install")
        });
        state.with_untracked(|s| {
            assert!(s.files.history.branches.is_none() && s.files.history.merges.is_none());
        });
    });
}

#[test]
fn branch_failures_consume_automatic_demand_until_retry_or_a_new_window() {
    Owner::new().with(|| {
        for status in [429, 500, 200] {
            let state = RwSignal::new(selected_state());
            state.update(|s| s.files.history.install(chain(1)));
            assert!(state.with_untracked(|s| s.files.history.needs_branches()));
            let aux = begin_aux(state, ReadSlot::Branches, None, |_| "/api/branches".into()).unwrap();
            assert!(!state.with_untracked(|s| s.files.history.needs_branches()));
            // status 200 models a rejected JSON/schema decode, not success.
            complete_aux::<BranchPage>(state, &aux, Err(injected(storage_contract::ErrorCode::InvalidRequest, status)), |_, _| panic!("failed branch page cannot install"));
            state.with_untracked(|s| {
                assert!(matches!(&s.files.history.branch_read, BranchReadState::Failed(error) if error.status == status));
                assert!(!s.files.history.needs_branches());
                assert_eq!(s.errors.len(), 1);
            });
            let retry = begin_aux(state, ReadSlot::Branches, None, |_| "/api/branches".into()).unwrap();
            complete_aux(state, &retry, Ok(crate::files::history::tests::branch_page(vec![], None)), |s, page| s.files.history.branches = Some(page));
            assert!(state.with_untracked(|s| matches!(s.files.history.branch_read, BranchReadState::Loaded)));
            state.update(|s| s.files.history.install(chain(2)));
            assert!(state.with_untracked(|s| s.files.history.needs_branches()));
            complete_aux::<BranchPage>(state, &aux, Err(injected(storage_contract::ErrorCode::InvalidRequest, status)), |_, _| panic!("old failure cannot install"));
            assert!(state.with_untracked(|s| s.files.history.needs_branches()));
            state.update(|s| s.files.clear_selection());
            assert!(!state.with_untracked(|s| s.files.history.needs_branches()));
        }
    });
}

#[test]
fn first_page_navigation_keeps_the_observed_root_but_latest_refresh_discovers_a_new_one() {
    Owner::new().with(|| {
        let state = RwSignal::new(selected_state());
        state.update(|s| s.files.history.install(chain(1)));
        let (_, first) = begin_main(state, &MainRequest::First, None).unwrap();
        assert!(first.contains(&format!("history_root={ROOT}")));
        state.update(|s| s.files.history.install(chain(1)));
        let (_, latest) = begin_main(state, &MainRequest::Latest, None).unwrap();
        assert!(!latest.contains("history_root="));
    });
}
