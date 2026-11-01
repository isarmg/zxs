use super::*;
use crate::files::model::FileOperation;

fn unobserved_draft(state: RwSignal<State>) -> Draft {
    Draft::unobserved_in(state, EditScope::Location)
}

#[cfg(not(target_arch = "wasm32"))]
fn init_native_effect_executor() {
    static INITIALIZE: std::sync::Once = std::sync::Once::new();
    INITIALIZE.call_once(|| {
        task_dispatch::Executor::init_futures_executor()
            .expect("inline tests initialize the process-wide effect executor once");
    });
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
    assert!(matches!(
        target.edit_value(" 草稿 ", ""),
        Edit::NameError(_)
    ));
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
        assert!(
            matches!(target.edit_value(name, ""), Edit::NameError(_)),
            "{name}"
        );
    }
}

#[test]
fn directory_creation_validates_the_complete_path_budget() {
    let target = Target::CreateDirectory;
    assert_eq!(
        target.edit_value("文", &"a".repeat(198)),
        Edit::Save("文".into())
    );
    assert!(matches!(
        target.edit_value("文", &"a".repeat(199)),
        Edit::NameError(_)
    ));
    assert!(name_policy::validate_name(&"a".repeat(201)).is_ok());
    assert!(matches!(
        target.edit_value(&"a".repeat(201), ""),
        Edit::NameError(_)
    ));
    assert_eq!(
        target.edit_value("文", &vec!["a"; 31].join("/")),
        Edit::Save("文".into())
    );
    assert!(matches!(
        target.edit_value("文", &vec!["a"; 32].join("/")),
        Edit::NameError(_)
    ));
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
    assert!(matches!(
        edit_value(" 草稿 ", InputKind::BranchName),
        Edit::NameError(_)
    ));
    for value in ["a\nb", "a\rb", "a\tb", "a\0b", "a\u{7f}b", "a\n"] {
        assert_eq!(edit_value(value, InputKind::Note), Edit::Invalid);
        assert!(matches!(
            edit_value(value, InputKind::BranchName),
            Edit::NameError(_)
        ));
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
    assert!(matches!(
        edit_value(" renamed ", target.input_kind()),
        Edit::NameError(_)
    ));
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
    init_native_effect_executor();
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
    init_native_effect_executor();
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

#[test]
fn nonempty_names_preserve_original_bytes_in_policy_errors() {
    for original in [
        " draft ",
        "\u{a0}draft",
        "draft\u{2003}",
        " draft",
        "draft ",
    ] {
        for kind in [InputKind::NamespaceName, InputKind::BranchName] {
            let Edit::NameError(error) = edit_value(original, kind) else {
                panic!("nonempty input must reach the policy unchanged: {original:?}");
            };
            assert_eq!(error.name, original);
            assert!(std::error::Error::source(&error).is_some());
        }
    }
    for blank in ["", " \t\n", "\u{a0}\u{2003}"] {
        assert_eq!(edit_value(blank, InputKind::NamespaceName), Edit::Cancel);
    }
    assert_eq!(
        edit_value("draft 2", InputKind::NamespaceName),
        Edit::Save("draft 2".into())
    );
}

#[test]
fn nfc_candidate_is_not_a_save_and_must_fit_the_current_destination() {
    for (original, candidate) in [("e\u{301}", "é"), ("\u{1100}\u{1161}", "가")] {
        let Edit::NameError(error) = Target::CreateDirectory.edit_value(original, "") else {
            panic!("NFD input must wait for explicit confirmation");
        };
        assert_eq!(error.name, original);
        assert_eq!(error.candidate(), Some(candidate));
        assert_eq!(
            Target::CreateDirectory.edit_value(candidate, ""),
            Edit::Save(candidate.into())
        );
        assert!(matches!(
            Target::CreateDirectory.edit_value(candidate, &"a".repeat(199)),
            Edit::NameError(_)
        ));
    }
}

fn accept_displayed_name(draft: Draft) -> bool {
    let Some(expected) = draft.name_error.get_untracked() else {
        return false;
    };
    draft.accept_name(&expected)
}

fn propose(draft: Draft, original: &str) {
    draft.start(original.into());
    draft.name_error.set(Some(std::sync::Arc::new(
        super::super::naming::validate(original).unwrap_err(),
    )));
}

#[test]
fn confirmation_cancel_and_changed_context_never_admit_a_request() {
    Owner::new().with(|| {
        let state = RwSignal::new(active_state());
        let draft = unobserved_draft(state);
        propose(draft, "e\u{301}");
        assert_eq!(draft.value.get_untracked(), "e\u{301}");
        assert!(!draft.pending());
        draft.cancel_idle();
        assert!(!accept_displayed_name(draft));
        assert!(!draft.editing.get_untracked());
        propose(draft, "e\u{301}");
        state.update(|s| s.files.directory = "other".into());
        assert!(accept_displayed_name(draft));
        draft.submit(|| panic!("confirmation must not write to a changed destination"));
        assert!(!draft.pending());
        assert!(!draft.editing.get_untracked());
    });
}

#[test]
fn confirmation_is_exact_and_does_not_survive_a_changed_input() {
    Owner::new().with(|| {
        let state = RwSignal::new(active_state());
        let draft = unobserved_draft(state);
        propose(draft, "e\u{301}");
        draft.value.set("different".into());
        assert!(!accept_displayed_name(draft));
        assert_eq!(draft.value.get_untracked(), "different");
        draft.cancel_idle();
        propose(draft, "e\u{301}");
        assert!(accept_displayed_name(draft));
        assert_eq!(draft.value.get_untracked().as_bytes(), "é".as_bytes());
        assert!(draft.name_error.get_untracked().is_none());
        draft.submit(|| Some("confirmed-operation".into()));
        assert_eq!(
            draft.operation.get_untracked().as_deref(),
            Some("confirmed-operation")
        );
        draft.cancel_idle();
        assert!(draft.pending());
    });
}

// Deliberately uses only the pre-fix API, so this regression can run on the
// audited baseline rather than failing to compile because of new error types.
#[test]
fn nonempty_namespace_input_must_never_be_silently_trimmed() {
    for original in [" 草稿 ", "\u{a0}draft", "draft\u{2003}"] {
        assert_ne!(
            edit_value(original, InputKind::NamespaceName),
            Edit::Save(original.trim().to_owned()),
            "nonempty user input was silently rewritten: {original:?}"
        );
    }
}

#[test]
fn normalization_candidates_do_not_bypass_name_or_branch_byte_budgets() {
    for (prefix, kind) in [
        (237, InputKind::NamespaceName),
        (125, InputKind::BranchName),
    ] {
        // U+0344 expands to U+0308 U+0301 without composing with a digit.
        let original = format!("{}\u{344}", "1".repeat(prefix));
        let Edit::NameError(error) = edit_value(&original, kind) else {
            panic!("expected policy NFC suggestion");
        };
        let candidate = error.candidate().unwrap();
        assert!(candidate.len() > original.len());
        assert!(!matches!(edit_value(candidate, kind), Edit::Save(_)));
    }
}

#[test]
fn a_stale_inline_confirm_click_cannot_accept_a_new_equal_proposal() {
    Owner::new().with(|| {
        let state = RwSignal::new(active_state());
        let draft = unobserved_draft(state);
        propose(draft, "e\u{301}");
        let stale = draft.name_error.get_untracked().unwrap();
        draft.name_error.set(Some(std::sync::Arc::new(
            super::super::naming::validate("e\u{301}").unwrap_err(),
        )));
        assert!(!draft.accept_name(&stale));
        assert_eq!(draft.value.get_untracked(), "e\u{301}");
        assert!(accept_displayed_name(draft));
        assert_eq!(draft.value.get_untracked(), "é");
    });
}
