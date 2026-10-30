use super::*;
use crate::files::model::FileOperation;

fn unobserved_draft(state: RwSignal<State>) -> Draft {
    Draft::unobserved_in(state, EditScope::Location)
}

#[test]
fn metadata_limits_are_bytes_without_splitting_utf8() {
    let ascii = "a".repeat(128);
    let chinese = format!("{}ab", "文".repeat(42));
    assert_eq!(ascii.len(), 128);
    assert_eq!(chinese.len(), 128);
    for value in [&ascii, &chinese] {
        assert_eq!(
            edit_value(value, InputKind::Note),
            Edit::Save(value.clone())
        );
        assert_eq!(
            edit_value(value, InputKind::BranchName),
            Edit::Save(value.clone())
        );
        assert_eq!(
            edit_value(&format!("{value}a"), InputKind::Note),
            Edit::Invalid
        );
        assert_eq!(
            edit_value(&format!("{value}a"), InputKind::BranchName),
            Edit::Invalid
        );
    }
    assert_eq!(edit_value(&"文".repeat(43), InputKind::Note), Edit::Invalid);
}

#[test]
fn directory_names_use_current_portable_names_not_the_metadata_budget() {
    let target = Target::CreateDirectory;
    for name in ["文".repeat(43), "文".repeat(80)] {
        assert!(name_policy::validate_path(&format!("资料/{name}")).is_ok());
        assert_eq!(target.edit_value(&name, "资料"), Edit::Save(name.clone()));
        assert_eq!(edit_value(&name, InputKind::Note), Edit::Invalid);
        assert_eq!(edit_value(&name, InputKind::BranchName), Edit::Invalid);
    }
    assert_eq!(target.edit_value("  ", ""), Edit::Cancel);
    assert_eq!(target.edit_value(" 草稿 ", ""), Edit::Save("草稿".into()));
    for name in [
        ".",
        "..",
        "a/b",
        "a\\b",
        "CON",
        "bad.",
        "e\u{301}",
        "a\n",
        &"文".repeat(81),
    ] {
        assert_eq!(target.edit_value(name, ""), Edit::Invalid, "{name}");
    }
}

#[test]
fn directory_creation_validates_the_complete_path_budget() {
    let target = Target::CreateDirectory;
    assert_eq!(
        target.edit_value("文", &"a".repeat(198)),
        Edit::Save("文".into())
    );
    assert_eq!(target.edit_value("文", &"a".repeat(199)), Edit::Invalid);
    assert!(name_policy::validate_name(&"a".repeat(201)).is_ok());
    assert_eq!(target.edit_value(&"a".repeat(201), ""), Edit::Invalid);
    assert_eq!(
        target.edit_value("文", &vec!["a"; 31].join("/")),
        Edit::Save("文".into())
    );
    assert_eq!(
        target.edit_value("文", &vec!["a"; 32].join("/")),
        Edit::Invalid
    );
}

#[test]
fn both_branch_rename_targets_keep_their_own_metadata_limit() {
    for target in [
        Target::RenameDirectoryBranch {
            repository: "repo".into(),
            branch_id: "branch".into(),
            name: "old".into(),
        },
        Target::RenameFileBranch {
            file: "file".into(),
            branch_id: "branch".into(),
            name: "old".into(),
        },
    ] {
        let boundary = format!("{}ab", "文".repeat(42));
        assert_eq!(target.edit_value(&boundary, ""), Edit::Save(boundary));
        assert_eq!(target.edit_value(&"文".repeat(43), ""), Edit::Invalid);
    }
}

#[test]
fn clearing_a_note_is_distinct_from_cancelling_a_name() {
    assert_eq!(edit_value("", InputKind::Note), Edit::Save(String::new()));
    assert_eq!(edit_value("  ", InputKind::BranchName), Edit::Cancel);
    assert_eq!(
        edit_value(" 草稿 ", InputKind::BranchName),
        Edit::Save("草稿".into())
    );
    for value in ["a\nb", "a\rb", "a\tb", "a\0b", "a\u{7f}b", "a\n"] {
        assert_eq!(edit_value(value, InputKind::Note), Edit::Invalid);
        assert_eq!(edit_value(value, InputKind::BranchName), Edit::Invalid);
    }
}

#[test]
fn a_path_nodes_generated_caption_is_display_only_and_note_input_starts_from_metadata() {
    let mut state = State::default();
    state.files.file_history = crate::files::history::tests::history(
        json_codec::json!({"nodes":[{"node_id":"fnode-rename","version_id":"ver-one","kind":"rename","previous_path":"old/report.txt","path":"new/report.txt","note":""}]}),
    );
    let target = Target::NodeNote("fnode-rename".into());
    assert_eq!(target.initial(&state), Some(String::new()));
    assert_eq!(
        target.display(&super::super::history_index::HistoryIndex::new(
            state.files.file_history.clone()
        )),
        "Path changed old/report.txt → new/report.txt"
    );
    assert!(target.input_kind() == InputKind::Note);
    crate::files::history::tests::update_metadata(&mut state.files.file_history, |value| {
        value["nodes"][0]["note"] = "Reviewed rename".into()
    });
    assert_eq!(
        target.display(&super::super::history_index::HistoryIndex::new(
            state.files.file_history.clone()
        )),
        "Reviewed rename"
    );
    assert_eq!(
        edit_value("", target.input_kind()),
        Edit::Save(String::new())
    );
}
#[test]
fn body_note_editors_use_body_versions_and_never_the_graph_node_identity() {
    let mut state = State::default();
    state.files.file_history = crate::files::history::tests::history(json_codec::json!({
        "note":"file note",
        "nodes":[
            {"node_id":"content-node","version_id":"body-version","kind":"content",
                "note":"body note","body_note":"body note"},
            {"node_id":"rename-node","version_id":"body-version","kind":"rename",
                "note":"path note","previous_path":"old.txt","path":"new.txt"}
        ],
    }));
    assert_eq!(Target::FileNote.initial(&state), Some("file note".into()));
    assert_eq!(
        Target::NodeNote("rename-node".into()).initial(&state),
        Some("path note".into())
    );
    assert_eq!(
        Target::VersionNote("body-version".into()).initial(&state),
        Some("body note".into())
    );
    assert_eq!(
        Target::VersionNote("rename-node".into()).initial(&state),
        None
    );
    assert_eq!(
        Target::NodeNote("body-version".into()).initial(&state),
        None
    );
}

#[test]
fn unloaded_note_targets_do_not_become_known_empty_notes() {
    let state = State::default();
    assert_eq!(Target::FileNote.initial(&state), None);
    assert_eq!(
        Target::NodeNote("unloaded-node".into()).initial(&state),
        None
    );
    assert_eq!(
        Target::VersionNote("unloaded-body".into()).initial(&state),
        None
    );
}

#[test]
fn directory_branch_rename_keeps_the_record_identity_and_shares_cancel_and_byte_rules() {
    let target = Target::RenameDirectoryBranch {
        repository: "repository-a".into(),
        branch_id: "directory-a".into(),
        name: "auto-generated".into(),
    };
    let state = State::default();
    assert_eq!(target.initial(&state), Some("auto-generated".into()));
    assert!(target.input_kind() == InputKind::BranchName);
    assert_eq!(edit_value("", target.input_kind()), Edit::Cancel);
    assert_eq!(
        edit_value(&"a".repeat(129), target.input_kind()),
        Edit::Invalid
    );
    assert_eq!(
        edit_value(" renamed ", target.input_kind()),
        Edit::Save("renamed".into())
    );
}

fn active_state() -> State {
    State {
        authenticated: true,
        session_epoch: 7,
        page: crate::ui::Page::Details,
        ..Default::default()
    }
}

#[test]
fn history_navigation_requires_explicit_editor_completion_or_cancellation() {
    Owner::new().with(|| {
        let state = RwSignal::new(active_state());
        let draft = unobserved_draft(state);
        assert!(history_navigation_allowed(state));
        draft.start("keep this exact input".into());
        draft.start("do not replace the draft".into());
        assert_eq!(state.with_untracked(|s| s.files.history_editors), 1);
        assert!(!history_navigation_allowed(state));
        assert_eq!(draft.value.get_untracked(), "keep this exact input");
        draft.submit(|| Some("pending".into()));
        draft.cancel_idle();
        assert!(!history_navigation_allowed(state));
        draft.settle("pending", super::super::model::SaveStatus::Rejected);
        assert!(!history_navigation_allowed(state));
        draft.cancel_idle();
        assert!(history_navigation_allowed(state));
        assert_eq!(state.with_untracked(|s| s.files.history_editors), 0);
    });
}

#[test]
fn editor_owner_cleanup_releases_only_its_own_history_navigation_registration() {
    let root = Owner::new();
    let state = root.with(|| RwSignal::new(active_state()));
    let first_owner = root.with(Owner::new);
    let first = first_owner.with(|| unobserved_draft(state));
    let second_owner = root.with(Owner::new);
    let second = second_owner.with(|| unobserved_draft(state));
    first.start("first".into());
    second.start("second".into());
    assert_eq!(state.with_untracked(|s| s.files.history_editors), 2);
    first_owner.cleanup();
    assert_eq!(state.with_untracked(|s| s.files.history_editors), 1);
    assert_eq!(second.value.get_untracked(), "second");
    second.cancel_idle();
    second_owner.cleanup();
    assert_eq!(state.with_untracked(|s| s.files.history_editors), 0);
    root.cleanup();
}

#[test]
fn old_editor_cleanup_cannot_release_a_new_sessions_history_navigation_gate() {
    let root = Owner::new();
    let state = root.with(|| RwSignal::new(active_state()));
    let old_owner = root.with(Owner::new);
    let old = old_owner.with(|| unobserved_draft(state));
    old.start("old session".into());
    state.set(State {
        session_epoch: 8,
        ..active_state()
    });
    let current_owner = root.with(Owner::new);
    let current = current_owner.with(|| unobserved_draft(state));
    current.start("current session".into());
    assert_eq!(state.with_untracked(|s| s.files.history_editors), 1);
    old_owner.cleanup();
    assert_eq!(state.with_untracked(|s| s.files.history_editors), 1);
    assert_eq!(current.value.get_untracked(), "current session");
    current_owner.cleanup();
    assert_eq!(state.with_untracked(|s| s.files.history_editors), 0);
    root.cleanup();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn history_invalidation_retains_editor_text_without_refreshing_its_write_basis() {
    use task_dispatch::Executor;
    Executor::init_futures_executor().expect("test initializes the native effect executor once");
    let root = Owner::new();
    root.with(|| {
        let mut initial = active_state();
        initial.management.storage = crate::management::Storage::Ready;
        initial.files.writable = true;
        initial.files.selected = Some(crate::files::test_entries::entry(
            json_codec::json!({"file_id":"file"}),
        ));
        initial.files.file_history = crate::files::history::tests::history(
            json_codec::json!({"history_root":"original-root","note":"stored note"}),
        );
        let state = RwSignal::new(initial);
        provide_context(state);
        let draft = Draft::new(state, EditScope::History);
        Executor::poll_local();
        draft.start("exact unsaved text".into());
        assert!(draft.editing.get_untracked());
        state.update(|s| s.files.file_history.invalidate());
        Executor::poll_local();
        assert_eq!(draft.value.get_untracked(), "exact unsaved text");
        assert!(draft.editing.get_untracked());
        draft.submit(|| panic!("an invalidated history basis cannot admit this write"));
        assert!(!draft.pending());
        assert!(!history_navigation_allowed(state));

        state.update(|s| {
            s.files.file_history = crate::files::history::tests::history(
                json_codec::json!({"history_root":"replacement-root","note":"new stored note"}),
            );
        });
        Executor::poll_local();
        draft.submit(|| panic!("a new root cannot silently replace the captured basis"));
        assert_eq!(draft.value.get_untracked(), "exact unsaved text");
        assert!(draft.editing.get_untracked());
        assert!(!draft.pending());
        draft.cancel_idle();
        assert!(history_navigation_allowed(state));
    });
    root.cleanup();
}

#[test]
fn an_editor_keeps_text_until_its_own_success_and_locks_unknown_outcomes() {
    Owner::new().with(|| {
        let state = RwSignal::new(active_state());
        let draft = unobserved_draft(state);
        draft.start("user's exact draft".into());
        draft.submit(|| Some("first".into()));
        draft.cancel_idle(); // A disabled input's blur is not cancellation.
        assert_eq!(draft.value.get_untracked(), "user's exact draft");
        assert!(draft.editing.get_untracked());
        draft.settle("unrelated", super::super::model::SaveStatus::Published);
        draft.settle("first", super::super::model::SaveStatus::Unknown);
        assert!(draft.pending());
        draft.submit(|| panic!("unknown outcomes cannot submit another operation"));
        draft.settle("first", super::super::model::SaveStatus::Rejected);
        assert!(!draft.pending());
        assert_eq!(draft.value.get_untracked(), "user's exact draft");
        assert!(draft.editing.get_untracked());
        draft.submit(|| Some("retry".into()));
        draft.settle("first", super::super::model::SaveStatus::Published);
        assert_eq!(draft.value.get_untracked(), "user's exact draft");
        draft.settle("retry", super::super::model::SaveStatus::Degraded);
        assert!(!draft.editing.get_untracked());
        assert_eq!(draft.value.get_untracked(), "");
    });
}

#[test]
fn rejected_admission_keeps_the_draft_and_unsubmitted_edits_can_cancel() {
    Owner::new().with(|| {
        let state = RwSignal::new(active_state());
        let draft = unobserved_draft(state);
        draft.start("keep me".into());
        draft.submit(|| None);
        assert_eq!(draft.value.get_untracked(), "keep me");
        assert!(!draft.pending());
        draft.cancel_idle();
        assert!(!draft.editing.get_untracked());
        assert_eq!(draft.value.get_untracked(), "");
    });
}

#[test]
fn owned_navigation_invalidates_a_queued_editor_without_admitting_its_write() {
    use crate::files::model::Navigation;
    Owner::new().with(|| {
        for destination in [
            Navigation::Root,
            Navigation::Disk("B01".into()),
            Navigation::Directory("other".into()),
        ] {
            let state = RwSignal::new(active_state());
            state.update(|s| {
                s.files.slot = "P01".into();
                s.files.directory = "docs".into();
            });
            let draft = unobserved_draft(state);
            draft.start("old location draft".into());
            state.update(|s| s.files.navigate(destination));
            draft.submit(|| panic!("navigation must invalidate the old editor context"));
            assert!(!draft.editing.get_untracked());
            assert!(!draft.pending());
        }
    });
}

#[test]
fn queued_edits_cannot_submit_after_session_navigation_alias_or_branch_changes() {
    Owner::new().with(|| {
        for change in [
            "epoch",
            "logout",
            "page",
            "slot",
            "directory",
            "repository",
            "directory_branch",
            "path",
            "branch",
            "version",
            "selected_repository",
            "selected_path",
            "backup",
        ] {
            let mut initial = active_state();
            initial.files.selected = Some(crate::files::test_entries::entry(json_codec::json!({"entry_id":"same","file_id":"same","path":"docs/a","selected_branch_id":"main","version_id":"v1"})));
            let state = RwSignal::new(initial);
            let draft = unobserved_draft(state);
            draft.start("old context".into());
            state.update(|s| match change {
                "epoch" => s.session_epoch += 1,
                "logout" => s.session_transition = crate::ui::SessionTransition::LoggingOut,
                "page" => s.page = crate::ui::Page::Files,
                "slot" => s.files.slot = "other".into(),
                "directory" => s.files.directory = "other".into(),
                "repository" => s.files.repository_id = "other".into(),
                "directory_branch" => s.files.directory_branches = vec![crate::files::test_entries::branch(json_codec::json!({"current":true,"directory_branch_id":"other"}))],
                "path" => crate::files::test_entries::update(s.files.selected.as_mut().unwrap(), json_codec::json!({"path":"docs/b"})),
                "branch" => crate::files::test_entries::update(s.files.selected.as_mut().unwrap(), json_codec::json!({"selected_branch_id":"other"})),
                "version" => crate::files::test_entries::update(s.files.selected.as_mut().unwrap(), json_codec::json!({"version_id":"v2"})),
                "selected_repository" => crate::files::test_entries::update(s.files.selected.as_mut().unwrap(), json_codec::json!({"repository_id":"other"})),
                "selected_path" => crate::files::test_entries::update(s.files.selected.as_mut().unwrap(), json_codec::json!({"repository_path":"alias/a"})),
                "backup" => crate::files::test_entries::update(s.files.selected.as_mut().unwrap(), json_codec::json!({"backup":true})),
                _ => unreachable!(),
            });
            draft.submit(|| panic!("old editor must not submit into a new context"));
            assert!(!draft.editing.get_untracked());
            assert!(!draft.pending());
        }
    });
}

#[test]
fn page_context_is_shared_and_only_invalidates_on_editor_ownership_changes() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let owner = Owner::new();
    owner.with(|| {
        let state = RwSignal::new(active_state());
        provide_context(state);
        let first = use_context::<SharedContext>().unwrap().0;
        let second = use_context::<SharedContext>().unwrap().0;
        assert!(first == second);
        let runs = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&runs);
        let consumer = Memo::new(move |_| {
            observed.fetch_add(1, Ordering::Relaxed);
            first.with(|context| context.epoch)
        });
        assert_eq!(consumer.get(), 7);
        state.update(|s| s.files.history_loading = true);
        assert_eq!(consumer.get(), 7);
        assert_eq!(runs.load(Ordering::Relaxed), 1);
        state.update(|s| s.session_epoch += 1);
        assert_eq!(consumer.get(), 8);
        assert_eq!(runs.load(Ordering::Relaxed), 2);
        assert_eq!(second.with(|context| context.epoch), 8);
    });
    owner.cleanup();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn scheduled_shared_context_effects_preserve_pending_drafts_and_follow_page_ownership() {
    use task_dispatch::Executor;
    Executor::init_futures_executor().expect("test initializes the native effect executor once");
    let root = Owner::new();
    let state = root.with(|| RwSignal::new(active_state()));
    let page = root.with(Owner::new);
    let (first, second, old_context) = page.with(|| {
        provide_context(state);
        (
            Draft::new(state, EditScope::Location),
            Draft::new(state, EditScope::Location),
            use_context::<SharedContext>().unwrap().0,
        )
    });
    Executor::poll_local(); // Run the real initial effects before user input.
    for (draft, id, text) in [
        (first, "first", "first pending text"),
        (second, "second", "second pending text"),
    ] {
        draft.start(text.into());
        draft.submit(|| Some(id.into()));
    }
    state.update(|s| {
        s.files.operations = vec![
            FileOperation::unknown("first"),
            FileOperation::unknown("second"),
        ];
        s.management.storage = crate::management::Storage::ReadOnly;
    });
    Executor::poll_local();
    assert_eq!(first.value.get_untracked(), "first pending text");
    assert_eq!(second.value.get_untracked(), "second pending text");
    assert!(first.pending() && second.pending());
    state.update(|s| s.session_epoch += 1);
    Executor::poll_local();
    for draft in [first, second] {
        assert!(!draft.editing.get_untracked());
        assert!(!draft.pending());
        assert!(draft.value.get_untracked().is_empty());
        draft.start("next session draft".into());
    }
    state.update(|s| s.session_transition = crate::ui::SessionTransition::LoggingOut);
    Executor::poll_local();
    assert!(!first.editing.get_untracked());
    assert!(!second.editing.get_untracked());

    // Queue notifications, then unmount before the effects consume them.
    state.update(|s| s.session_epoch += 1);
    page.cleanup();
    assert!(old_context.try_get_untracked().is_none());
    Executor::poll_local();
    state.set(active_state());
    let remounted = root.with(Owner::new);
    let (fresh, fresh_context) = remounted.with(|| {
        provide_context(state);
        (
            Draft::new(state, EditScope::Location),
            use_context::<SharedContext>().unwrap().0,
        )
    });
    assert!(old_context != fresh_context);
    Executor::poll_local();
    fresh.start("new mounted draft".into());
    state.update(|s| s.files.history_loading = true);
    Executor::poll_local();
    assert_eq!(fresh.value.get_untracked(), "new mounted draft");
    assert_eq!(fresh_context.with(|context| context.epoch), 7);
    remounted.cleanup();
    Executor::poll_local();
    root.cleanup();
}
