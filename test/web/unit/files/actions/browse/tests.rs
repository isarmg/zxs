use super::*;
use crate::files::actions::history::complete_history_load;
use crate::files::history::FileHistory;
use json_codec::Value;

#[test]
fn navigation_actions_keep_session_authority_and_only_change_the_file_view() {
    use crate::ui::{Page, PasswordChangeState, SessionTransition};
    Owner::new().with(|| {
        let state = RwSignal::new(State {
            authenticated: true,
            session_epoch: 9,
            session_transition: SessionTransition::LoggingOut,
            password_change: PasswordChangeState::Unknown,
            principal: "administrator".into(),
            page: Page::Details,
            notice: "unconfirmed save".into(),
            ..Default::default()
        });
        // All three production actions execute their synchronous transition,
        // but the existing inactive-session gates admit no browser request.
        for (navigate, value, slot, directory) in [
            (open_disk as fn(RwSignal<State>, String), "P01", "P01", ""),
            (go_directory, "资料/子目录", "P01", "资料/子目录"),
        ] {
            navigate(state, value.into());
            state.with_untracked(|s| {
                assert_eq!(s.page, Page::Files);
                assert_eq!((&*s.files.slot, &*s.files.directory), (slot, directory));
                assert!(!s.files.loading && !s.files.loaded && !s.files.writable);
            });
        }
        search(state, "new query".into());
        go_root(state);
        state.with_untracked(|s| {
            assert_eq!(s.page, Page::Files);
            assert!(s.files.slot.is_empty() && s.files.directory.is_empty());
            assert_eq!(s.files.query, "new query");
            assert_eq!(s.files.generation, 4);
            assert!(!s.files.loading_disks && !s.files.loading);
            assert!(s.authenticated);
            assert_eq!(s.session_epoch, 9);
            assert_eq!(s.session_transition, SessionTransition::LoggingOut);
            assert_eq!(s.password_change, PasswordChangeState::Unknown);
            assert_eq!(s.principal, "administrator");
            assert_eq!(s.notice, "unconfirmed save");
            assert!(!super::super::write_allowed(s));
        });
    });
}

#[test]
fn navigation_away_and_back_rejects_old_listing_work_without_hiding_session_expiry() {
    Owner::new().with(|| {
        let state = RwSignal::new(State {
            authenticated: true,
            session_epoch: 3,
            ..Default::default()
        });
        state.update(|s| s.files.navigate(Navigation::Disk("P01".into())));
        let old_generation = state.with_untracked(|s| s.files.generation);
        for destination in [
            Navigation::Directory("docs".into()),
            Navigation::Root,
            Navigation::Disk("P01".into()),
        ] {
            state.update(|s| s.files.navigate(destination));
            // Also rejects a queued debounce tick after returning to the same path.
            assert!(!current_listing(state, 3, "P01", "", old_generation));
        }
        let generation = state.with_untracked(|s| s.files.generation);
        assert!(current_listing(state, 3, "P01", "", generation));
        assert!(!accept_read_response::<()>(
            state,
            2,
            &Err(disk_read_error(401))
        ));
        assert!(state.with_untracked(State::session_active));
        // Production checks session authority before the obsolete listing fence.
        assert!(!accept_read_response::<()>(
            state,
            3,
            &Err(disk_read_error(401))
        ));
        assert!(!state.with_untracked(State::session_active));
    });
}

#[test]
fn listing_responses_require_the_original_location_generation_and_active_session() {
    Owner::new().with(|| {
        let state = RwSignal::new(State {
            authenticated: true,
            session_epoch: 1,
            ..Default::default()
        });
        state.update(|s| {
            s.files.slot = "P01".into();
            s.files.directory = "docs".into();
            s.files.generation = 3;
        });
        assert!(current_listing(state, 1, "P01", "docs", 3));
        assert!(!current_listing(state, 1, "P01", "docs", 2));
        assert!(!current_listing(state, 1, "P02", "docs", 3));
        assert!(!current_listing(state, 1, "P01", "other", 3));
        assert!(!current_listing(state, 0, "P01", "docs", 3));
        state.update(|s| s.session_transition = crate::ui::SessionTransition::LoggingOut);
        assert!(!current_listing(state, 1, "P01", "docs", 3));
    });
}

#[test]
fn directory_navigation_drops_the_previous_write_permission_before_loading() {
    Owner::new().with(|| {
        let state = RwSignal::new(State::default());
        state.update(|s| {
            s.files.writable = true;
            s.files.loaded = true;
        });
        // No disk is selected, so this exercises the synchronous transition
        // without starting a browser request on the native test runner.
        go_directory(state, "another".into());
        state.with_untracked(|s| {
            assert!(!s.files.writable);
            assert!(!s.files.loaded);
            assert_eq!(s.files.directory, "another");
        });
    });
}

#[test]
fn selected_file_history_read_requires_an_active_session() {
    Owner::new().with(|| {
        for (authenticated, session_transition) in [
            (false, crate::ui::SessionTransition::Idle),
            (true, crate::ui::SessionTransition::Restoring),
            (true, crate::ui::SessionTransition::LoggingOut),
            (true, crate::ui::SessionTransition::LogoutUnconfirmed),
        ] {
            let state = RwSignal::new(State {
                authenticated,
                session_transition,
                ..Default::default()
            });
            state.update(|s| {
                s.files.selected = Some(crate::files::test_entries::entry(
                    json_codec::json!({"file_id":"file-1"}),
                ));
            });
            load_file_history(state);
            state.with_untracked(|s| {
                assert!(!s.files.history_loading);
                assert_eq!(s.files.history_generation, 0);
            });
        }
    });
}

fn history_response(note: &str) -> FileHistory {
    (*crate::files::history::tests::history(json_codec::json!({"note":note}))).clone()
}

fn history_load_state() -> State {
    State {
        authenticated: true,
        session_epoch: 3,
        files: crate::files::Model {
            selected: Some(crate::files::test_entries::entry(
                json_codec::json!({"file_id":"file"}),
            )),
            file_history: history_response("installed").into(),
            history_loading: true,
            history_generation: 8,
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn stale_history_success_and_failure_cannot_settle_the_current_read() {
    for result in [Ok(history_response("late")), Err(disk_read_error(503))] {
        let mut state = history_load_state();
        let installed = state.files.file_history.clone();
        let selected = state.files.selected.clone();
        complete_history_load(&mut state, 3, 7, "file", result).unwrap();
        assert_eq!(state.files.file_history, installed);
        assert_eq!(state.files.selected, selected);
        assert!(state.files.history_loading);
        assert_eq!(state.files.history_generation, 8);
    }
}

#[test]
fn history_completion_requires_its_session_and_selected_file() {
    for (epoch, file) in [(2, "file"), (3, "another-file")] {
        let mut state = history_load_state();
        let installed = state.files.file_history.clone();
        complete_history_load(&mut state, epoch, 8, file, Ok(history_response("late"))).unwrap();
        assert_eq!(state.files.file_history, installed);
        assert!(state.files.history_loading);
        assert!(state.session_active());
    }
    let mut state = history_load_state();
    state.session_transition = crate::ui::SessionTransition::LoggingOut;
    let installed = state.files.file_history.clone();
    complete_history_load(&mut state, 3, 8, "file", Ok(history_response("late"))).unwrap();
    assert_eq!(state.files.file_history, installed);
    assert!(state.files.history_loading);
}

#[test]
fn current_history_completion_settles_loading_and_failure_discards_stale_heads() {
    let mut state = history_load_state();
    complete_history_load(&mut state, 3, 8, "file", Ok(history_response("current"))).unwrap();
    assert_eq!(state.files.file_history.file_note(), Some("current"));
    assert!(!state.files.history_loading);
    assert!(state.files.history_cancellation.is_none());
    state.files.history_loading = true;
    state.files.history_generation = 9;
    let error =
        complete_history_load(&mut state, 3, 9, "file", Err(disk_read_error(503))).unwrap_err();
    assert_eq!(error.status, 503);
    assert!(state.files.file_history.loaded() && !state.files.file_history.ready());
    assert!(!state.files.history_loading);
    assert!(state.files.history_cancellation.is_none());
}

#[test]
fn history_response_session_authority_precedes_the_request_generation_fence() {
    Owner::new().with(|| {
        let state = RwSignal::new(history_load_state());
        let selected = state.with_untracked(|s| s.files.selected.clone());
        let installed = state.with_untracked(|s| s.files.file_history.clone());
        let unauthorized: Result<FileHistory, api::Error> = Err(disk_read_error(401));
        assert!(!accept_read_response(state, 2, &unauthorized));
        assert!(state.with_untracked(State::session_active));
        assert!(state.with_untracked(|s| s.files.history_loading));
        // Even an obsolete history generation cannot hide a current-session 401.
        assert!(!accept_read_response(state, 3, &unauthorized));
        assert!(!state.with_untracked(State::session_active));
        assert!(!current_session(state, 3));
        assert!(!state.with_untracked(super::super::write_allowed));
        // A read 401 revokes authority while retaining private recovery state.
        // That retained state cannot admit the obsolete completion or a write.
        state.update(|s| {
            complete_history_load(s, 3, 7, "file", Ok(history_response("late"))).unwrap();
        });
        assert_eq!(state.with_untracked(|s| s.files.selected.clone()), selected);
        assert_eq!(
            state.with_untracked(|s| s.files.file_history.clone()),
            installed
        );
    });
}

#[test]
fn a_failed_disk_refresh_invalidates_readiness_without_discarding_cached_rows() {
    Owner::new().with(|| {
        for disks in [
            vec![],
            vec![crate::files::model::Disk {
                slot_id: "P01".into(),
                online: true,
                writable: true,
                available_bytes: 4096,
            }],
        ] {
            let state = RwSignal::new(State {
                authenticated: true,
                ..Default::default()
            });
            state.update(|s| {
                s.files.disks = disks.clone();
                s.files.disks_loaded = true;
            });
            let (epoch, generation) = state.try_update(begin_disk_load).flatten().unwrap();
            let error = state
                .try_update(|s| complete_disk_load(s, epoch, generation, Err(disk_read_error(503))))
                .unwrap()
                .unwrap_err();
            fail(state, error);
            state.with_untracked(|s| {
                assert!(
                    !s.files.disks_loaded,
                    "a later health poll must retry this read"
                );
                assert_eq!(s.files.disks, disks);
                assert!(s.session_active());
                assert_eq!(s.errors.len(), 1);
            });
        }
    });
}

fn disk_read_error(status: u16) -> api::Error {
    api::Error {
        status,
        code: storage_contract::ErrorCode::InjectedFailure,
        message: "Delayed disk listing failure".into(),
        retryable: true,
        request_id: None,
        source: None,
    }
}

fn disk_snapshot(online: bool, available_bytes: u64) -> Vec<Disk> {
    vec![Disk {
        slot_id: "B01".into(),
        online,
        writable: false,
        available_bytes,
    }]
}

#[test]
fn recovery_replaces_an_inflight_disk_read_after_either_old_success_or_failure() {
    for old_result in [Ok(disk_snapshot(false, 512)), Err(disk_read_error(503))] {
        let mut state = State {
            authenticated: true,
            session_epoch: 3,
            ..Default::default()
        };
        let cached = disk_snapshot(false, 1024);
        state.management.storage = crate::management::Storage::Ready;
        state.files.disks = cached.clone();
        state.files.disks_loaded = true;
        let (epoch_a, generation_a) = begin_disk_load(&mut state).unwrap();

        // H1 reports unavailable; H2 reports restored storage before A's old
        // payload (already generated before the outage) finishes arriving.
        state.files.invalidate_disks();
        assert!(begin_disk_load(&mut state).is_none());
        assert!(!state.files.disks_loaded);
        assert!(complete_disk_load(&mut state, epoch_a, generation_a, old_result).unwrap());
        assert_eq!(state.files.disks, cached, "A must not overwrite the cache");
        assert!(
            !state.files.disks_loaded,
            "A cannot consume H2's refresh demand"
        );
        assert!(!state.files.loading_disks);

        let (epoch_b, generation_b) = begin_disk_load(&mut state).unwrap();
        assert_eq!(epoch_b, epoch_a);
        assert_eq!(generation_b, generation_a + 1);
        assert!(
            begin_disk_load(&mut state).is_none(),
            "only one replacement B"
        );
        let fresh = disk_snapshot(true, 4096);
        assert!(!complete_disk_load(&mut state, epoch_b, generation_b, Ok(fresh.clone())).unwrap());
        assert_eq!(state.files.disks, fresh);
        assert!(state.files.disks_loaded);
        assert!(!state.files.loading_disks);
    }
}

#[test]
fn repeated_recovery_coalesces_and_a_failed_replacement_still_needs_a_fresh_read() {
    let mut state = State {
        authenticated: true,
        session_epoch: 4,
        ..Default::default()
    };
    let cached = disk_snapshot(false, 1024);
    state.management.storage = crate::management::Storage::Ready;
    state.files.disks = cached.clone();
    state.files.disks_loaded = true;
    let (epoch_a, generation_a) = begin_disk_load(&mut state).unwrap();
    for _ in 0..2 {
        state.files.invalidate_disks();
        assert!(begin_disk_load(&mut state).is_none());
    }
    assert!(
        complete_disk_load(&mut state, epoch_a, generation_a, Err(disk_read_error(503))).unwrap()
    );
    let (epoch_b, generation_b) = begin_disk_load(&mut state).unwrap();
    assert_eq!(generation_b, generation_a + 2);
    assert!(begin_disk_load(&mut state).is_none());
    assert!(
        complete_disk_load(&mut state, epoch_b, generation_b, Err(disk_read_error(503))).is_err()
    );
    assert_eq!(state.files.disks, cached);
    assert!(
        !state.files.disks_loaded,
        "healthy polling must still retry"
    );
    assert!(!state.files.loading_disks);

    let (epoch_c, generation_c) = begin_disk_load(&mut state).unwrap();
    assert_eq!(generation_c, generation_b);
    assert!(!complete_disk_load(&mut state, epoch_c, generation_c, Ok(vec![])).unwrap());
    assert!(
        state.files.disks_loaded,
        "an authorized empty list is a settled read"
    );
    assert!(state.files.disks.is_empty());
}

#[test]
fn obsolete_disk_completions_cannot_write_or_release_another_session_or_reload() {
    use crate::ui::SessionTransition;
    for (authenticated, transition, current_epoch) in [
        (true, SessionTransition::LoggingOut, 5),
        (false, SessionTransition::LogoutUnconfirmed, 5),
        (true, SessionTransition::Restoring, 5),
        (false, SessionTransition::Idle, 5),
        (true, SessionTransition::Idle, 6),
    ] {
        for old_result in [Ok(disk_snapshot(false, 512)), Err(disk_read_error(401))] {
            let mut state = State {
                authenticated: true,
                session_epoch: 5,
                ..Default::default()
            };
            let (epoch_a, generation_a) = begin_disk_load(&mut state).unwrap();
            state.files.invalidate_disks();
            state.authenticated = authenticated;
            state.session_transition = transition;
            state.session_epoch = current_epoch;
            let current = disk_snapshot(true, 4096);
            state.files.disks = current.clone();
            state.files.disks_loaded = true;
            // This may now belong to the new session's in-flight request.
            state.files.loading_disks = true;
            assert!(!complete_disk_load(&mut state, epoch_a, generation_a, old_result).unwrap());
            assert_eq!(state.files.disks, current);
            assert!(state.files.disks_loaded);
            assert!(state.files.loading_disks);
            assert_eq!(state.files.disks_generation, generation_a + 1);
            assert_eq!(state.authenticated, authenticated);
            assert_eq!(state.session_epoch, current_epoch);
        }
    }
}

#[test]
fn a_current_session_unauthorized_disk_read_is_not_hidden_by_recovery() {
    Owner::new().with(|| {
        for invalidate in [false, true] {
            let state = RwSignal::new(State {
                authenticated: true,
                session_epoch: 7,
                ..Default::default()
            });
            let cached = disk_snapshot(true, 1024);
            state.update(|s| {
                s.management.storage = crate::management::Storage::Ready;
                s.files.disks = cached.clone();
                s.files.disks_loaded = true;
            });
            let (epoch, generation) = state.try_update(begin_disk_load).flatten().unwrap();
            if invalidate {
                state.update(|s| s.files.invalidate_disks());
            }
            // The production continuation routes Err to fail, never to a replacement read.
            let error = state
                .try_update(|s| complete_disk_load(s, epoch, generation, Err(disk_read_error(401))))
                .unwrap()
                .unwrap_err();
            assert_eq!(error.status, 401);
            fail(state, error);
            state.with_untracked(|s| {
                assert!(!s.session_active());
                assert!(!s.files.loading_disks);
                assert!(!s.files.disks_loaded);
                assert_eq!(s.files.disks, cached);
                assert_eq!(s.errors.len(), 1);
            });
            assert!(state.try_update(begin_disk_load).flatten().is_none());
        }
    });
}

#[test]
fn an_obsolete_disk_read_waits_for_readable_storage_before_reloading() {
    use crate::management::Storage;
    for storage in [
        Storage::Unknown,
        Storage::Unavailable,
        Storage::Unconfigured,
        Storage::RecoveryRequired,
    ] {
        for old_result in [Ok(disk_snapshot(false, 512)), Err(disk_read_error(503))] {
            let mut state = State {
                authenticated: true,
                session_epoch: 8,
                ..Default::default()
            };
            let cached = disk_snapshot(false, 1024);
            state.management.storage = Storage::Ready;
            state.files.disks = cached.clone();
            state.files.disks_loaded = true;
            let (epoch_a, generation_a) = begin_disk_load(&mut state).unwrap();
            state.files.invalidate_disks();
            // Recovery was observed, but a newer health response is no longer readable.
            state.management.storage = storage;
            assert!(!complete_disk_load(&mut state, epoch_a, generation_a, old_result).unwrap());
            assert_eq!(state.files.disks, cached);
            assert!(!state.files.loading_disks);
            assert!(
                !state.files.disks_loaded,
                "the next readable health poll must still request a fresh snapshot"
            );

            state.management.storage = Storage::Ready;
            state.files.invalidate_disks();
            let (epoch_b, generation_b) = begin_disk_load(&mut state).unwrap();
            let fresh = disk_snapshot(true, 4096);
            assert!(
                !complete_disk_load(&mut state, epoch_b, generation_b, Ok(fresh.clone())).unwrap()
            );
            assert_eq!(state.files.disks, fresh);
            assert!(state.files.disks_loaded);
            assert!(!state.files.loading_disks);
        }
    }
}

#[test]
fn page_endpoint_encodes_search_and_opaque_cursor_and_caps_page_size() {
    let endpoint = page_endpoint("disk", "a/b", "x&y", Some("opaque+/="));
    assert_eq!(
        endpoint,
        "/api/file-manager/disks/disk/entries?path=a%2Fb&query=x%26y&limit=200&cursor=opaque%2B%2F%3D"
    );
}

#[test]
fn query_change_resets_page_local_state_before_any_read() {
    Owner::new().with(|| {
        let state = RwSignal::new(State::default());
        state.update(|s| {
            s.files.cursor = Some("stale".into());
            s.files.next_cursor = Some("next".into());
            s.files.previous_cursor = Some("previous".into());
            s.files.checked_entries.insert("selected".into());
            s.files.entries = vec![crate::files::test_entries::entry(
                json_codec::json!({"entry_id":"old"}),
            )]
            .into();
            s.files.loaded = true;
        });
        search(state, "new query".into());
        state.with_untracked(|s| {
            assert_eq!(s.files.query, "new query");
            assert!(s.files.cursor.is_none());
            assert!(s.files.next_cursor.is_none());
            assert!(s.files.previous_cursor.is_none());
            assert!(s.files.checked_entries.is_empty());
            assert!(s.files.entries.is_empty());
            assert!(!s.files.loaded);
            assert_eq!(s.files.generation, 1);
        });
    });
}

fn directory_response(entries: Vec<Value>, selected: Value) -> Value {
    json_codec::json!({"slot_id":"P01","path":"","writable":true,
        "entries":entries,"selected_entry":selected,"context":{"repository_id":"repo",
        "repository_path":"","branch_id":"main","base_head":"head"},
        "directory_file_count":"10","directory_branches":[],"directory_branches_supported":true,
        "default_branch_scope":true,"total_entries":"10","snapshot":"snapshot",
        "next_cursor":"next","previous_cursor":null})
}

#[test]
fn admitted_page_applies_all_metadata_and_keeps_independent_selected_details() {
    use crate::files::{model::entry_key, test_entries};
    let selected =
        test_entries::entry(json_codec::json!({"entry_id":"selected","path":"outside.txt"}));
    let row = test_entries::entry(json_codec::json!({"entry_id":"row","path":"row.txt"}));
    let mut state = State {
        page: crate::ui::Page::Details,
        files: crate::files::Model {
            selected: Some(selected.clone()),
            checked_entries: [entry_key(&row), "removed".into()].into_iter().collect(),
            file_history: crate::files::history::tests::history(
                json_codec::json!({"note":"loaded history"}),
            ),
            ..Default::default()
        },
        ..State::default()
    };
    let mut refreshed = test_entries::wire(&selected);
    refreshed["note"] = "fresh entry note".into();
    let page = Page::parse(
        directory_response(vec![test_entries::wire(&row)], refreshed),
        "P01",
        &DirectoryKey::navigation(""),
        200,
        Some(&selected),
    )
    .unwrap();
    apply_page(&mut state, page, Some(&selected));
    assert_eq!(state.files.entries.as_ref(), std::slice::from_ref(&row));
    assert_eq!(
        state.files.selected.as_ref().unwrap().note(),
        Some("fresh entry note")
    );
    assert_eq!(state.files.file_history.file_note(), Some("loaded history"));
    assert_eq!(
        state.files.checked_entries,
        [entry_key(&row)].into_iter().collect()
    );
    assert_eq!(state.files.next_cursor.as_deref(), Some("next"));
    assert_eq!(state.files.total_entries, 10);
    assert_eq!(state.files.directory_file_count, 10);
    assert!(state.files.writable && state.files.loaded && state.files.default_branch_scope);
}

#[test]
fn page_completion_does_not_replace_a_newer_selection_and_still_prunes_checks() {
    use crate::files::{model::entry_key, test_entries};
    let requested = test_entries::entry(json_codec::json!({"entry_id":"old","path":"old.txt"}));
    let current = test_entries::entry(json_codec::json!({"entry_id":"new","path":"new.txt"}));
    let mut state = State::default();
    state.files.selected = Some(current.clone());
    state.files.checked_entries.insert(entry_key(&requested));
    let page = Page::parse(
        directory_response(vec![test_entries::wire(&current)], Value::Null),
        "P01",
        &DirectoryKey::navigation(""),
        200,
        Some(&requested),
    )
    .unwrap();
    apply_page(&mut state, page, Some(&requested));
    assert_eq!(state.files.selected, Some(current));
    assert!(state.files.checked_entries.is_empty());
}

#[test]
fn malformed_page_never_installs_partial_entries_context_or_writeability() {
    use crate::files::test_entries;
    let old = test_entries::entry(json_codec::json!({"entry_id":"old","path":"old.txt"}));
    let mut state = State::default();
    state.files.entries = vec![old.clone()].into();
    state.files.repository_id = "retained-context".into();
    state.files.writable = false;
    let mut value = directory_response(
        vec![test_entries::entry_wire(json_codec::json!({}))],
        Value::Null,
    );
    value["directory_file_count"] = "invalid".into();
    let result = Page::parse(value, "P01", &DirectoryKey::navigation(""), 200, None)
        .map(|page| apply_page(&mut state, page, None));
    assert!(result.is_err());
    assert_eq!(state.files.entries.as_ref(), &[old]);
    assert_eq!(state.files.repository_id, "retained-context");
    assert!(!state.files.writable);
}
