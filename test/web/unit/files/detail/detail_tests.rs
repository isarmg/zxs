use super::*;
use crate::files::test_entries::{entry, update};
use json_codec::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[test]
fn an_unknown_or_loading_note_cannot_be_submitted_as_an_empty_note() {
    use crate::files::history::tests::{chain, window};
    let mut model = Model::default();
    assert!(!file_note_ready(&model));
    assert_eq!(history_notice(&model), HistoryNotice::Unloaded);
    model.history.install(chain(1));
    model.history.begin_main(None);
    assert!(!file_note_ready(&model));
    assert_eq!(history_notice(&model), HistoryNotice::Loading);
    model.history.install(chain(1));
    assert!(file_note_ready(&model));
    assert_eq!(history_notice(&model), HistoryNotice::Loaded);
    model.history.install(window(vec![], json!({})));
    assert!(
        file_note_ready(&model),
        "a known empty note remains editable"
    );
    assert_eq!(history_notice(&model), HistoryNotice::Empty);
    model
        .history
        .install(window(vec![], json!({"next_cursor":"bb"})));
    assert_eq!(
        history_notice(&model),
        HistoryNotice::Loaded,
        "an empty page with a continuation is not an empty history"
    );
    model
        .history
        .mark_stale(crate::files::history::state::StaleReason::RootChanged);
    assert!(
        !file_note_ready(&model),
        "a stale page is not a write basis"
    );
    assert!(matches!(history_notice(&model), HistoryNotice::Stale(_)));
    model.history.clear();
    assert!(
        !file_note_ready(&model),
        "a failed first request never implies an empty note"
    );
    model.history.fail(crate::api::Error {
        code: storage_contract::ErrorCode::InjectedFailure,
        message: "boom".into(),
        retryable: false,
        status: 503,
        request_id: None,
        source: None,
    });
    assert!(matches!(history_notice(&model), HistoryNotice::Failed(_)));
    assert!(!file_note_ready(&model));
}

#[test]
fn branch_options_come_from_the_resident_branch_page_and_mark_read_only_entries() {
    use crate::files::history::tests::{branch_page, branch_wire, chain};
    let mut model = Model::default();
    assert!(branch_options(&model).is_empty());
    model.history.install(chain(1));
    assert!(branch_options(&model).is_empty());
    model.history.branches = Some(branch_page(
        vec![
            branch_wire(json!({"branch_id":"a","name":"main"})),
            branch_wire(json!({"branch_id":"b","name":"old","state":"archived","read_only":true})),
        ],
        None,
    ));
    let options = branch_options(&model);
    assert_eq!(options.len(), 2);
    assert!(!options[0].disabled && options[0].label == "Main");
    assert!(options[1].disabled && options[1].label.contains("Read only"));
}

#[test]
fn selected_branch_identity_is_explicit_when_the_candidate_page_does_not_contain_it() {
    use crate::files::history::tests::{branch_page, branch_wire, chain};
    let mut model = Model::default();
    model.history.install(chain(1));
    model.selected = Some(entry(json!({"selected_branch_id":"branch-000"})));
    model.history.branches = Some(branch_page(
        vec![branch_wire(json!({"branch_id":"branch-200"}))],
        Some("branch-000"),
    ));
    let options = branch_options(&model);
    assert_eq!(options.len(), 2);
    assert_eq!(options[0].value, "branch-000");
    assert!(options[0].disabled && options[0].label.contains("Outside this page"));
    assert_eq!(options[1].value, "branch-200");
}

#[test]
fn file_wide_candidates_cannot_override_the_requested_alias_branch_for_writes() {
    use crate::files::history::tests::{branch_page, branch_wire, node, window_wire};
    Owner::new().with(|| {
        let mut initial = selected_state();
        initial.management.storage = crate::management::Storage::Ready;
        initial.files.writable = true;
        update(initial.files.selected.as_mut().unwrap(),json!({"selected_branch_id":"right","path":"docs/b.txt","repository_path":"docs/b.txt","name":"b.txt"}));
        let mut wire=window_wire(vec![node("n1",1,&[],&["n1"])],json!({}));
        wire["context"]["selected_branch_id"]="right".into();
        wire["context"]["path"]="docs/b.txt".into();
        initial.files.history.install(crate::files::history::HistoryWindow::parse(wire,"file").unwrap());
        initial.files.history.branches=Some(branch_page(vec![
            branch_wire(json!({"branch_id":"left","name":"Alias A","node_id":"left-head","current":true})),
            branch_wire(json!({"branch_id":"right","name":"Alias B","node_id":"right-head"})),
        ],Some("left")));
        assert_eq!(selected_branch(&initial.files),"right");
        assert_eq!(selected_file_branch_record(&initial.files).unwrap().1,"right");
        assert_eq!(tags::check(&mut initial.files,"n1",true),tags::Check::Changed);
        assert_eq!(initial.files.checked_nodes["n1"].branch,"right");
        assert_eq!(initial.files.checked_nodes["n1"].expected,"right-head");
        let state=RwSignal::new(initial);
        merge::start(state);
        state.with_untracked(|s|assert_eq!(s.files.merge.as_ref().unwrap().branches().map(|branch|branch.id.as_str()).collect::<Vec<_>>(),["right"]));
    });
}

fn selected_state() -> State {
    let mut state = State {
        authenticated: true,
        session_epoch: 7,
        ..Default::default()
    };
    state.files.slot = "P01".into();
    state.files.directory = "docs".into();
    state.files.repository_id = "repository".into();
    state.files.repository_path = "docs".into();
    state.files.selected = Some(entry(json_codec::json!({
        "entry_id":"entry", "file_id":"file", "kind":"file", "name":"a.txt",
        "path":"docs/a.txt", "repository_id":"repository", "repository_path":"docs/a.txt",
        "version_id":"v1", "node_id":"v1", "selected_branch_id":"main", "note":"old"
    })));
    state
}

#[test]
fn metadata_completion_keeps_the_current_detail_controls_mounted() {
    Owner::new().with(|| {
        let state = RwSignal::new(selected_state());
        let selected = detail_selection(state);
        let mounts = Arc::new(AtomicUsize::new(0));
        let count = mounts.clone();
        let detail = Memo::new(move |_| {
            let selection = selected.get();
            count.fetch_add(1, Ordering::SeqCst);
            selection
        });
        let identity = detail.get().unwrap();
        state.update(|s| {
            s.files.history.begin_main(None);
            s.files.mutation_pending = true;
        });
        assert_eq!(detail.get().as_ref(), Some(&identity));
        assert_eq!(mounts.load(Ordering::SeqCst), 1);
        state.update(|s| {
            s.files.history.install(crate::files::history::tests::chain(1));
            s.files.history.branches = Some(crate::files::history::tests::branch_page(
                vec![
                    crate::files::history::tests::branch_wire(json!({"branch_id":"draft","name":"draft"})),
                    crate::files::history::tests::branch_wire(json!({"branch_id":"main","name":"main"})),
                ],
                None,
            ));
            s.files.mutation_pending = false;
        });
        assert_eq!(detail.get().as_ref(), Some(&identity));
        assert_eq!(mounts.load(Ordering::SeqCst), 1);
        // A real post-publication listing changes selected_entry.note, not just
        // file_history. Mutable payloads must not recreate editor/tree owners.
        state.update(|s| {
            update(s.files.selected.as_mut().unwrap(), json!({"note":"saved note","read_only":true,"version_id":"v2","node_id":"v2","selected_branch_id":"other"}));
        });
        assert_eq!(detail.get().as_ref(), Some(&identity));
        assert_eq!(mounts.load(Ordering::SeqCst), 1);
        state.with_untracked(|s| {
            let current = identity.entry(s).unwrap();
            assert_eq!(current.note().unwrap(), "saved note");
            assert_eq!(current.name(), "a.txt");
            assert_eq!(current.version_id().unwrap(), "v2");
            assert!(current.read_only());
            assert!(selected_entry_read_only(&s.files));
        });
    });
}

#[test]
fn aliases_replacements_and_context_changes_retire_old_detail_actions() {
    let changes: [fn(&mut State); 9] = [
        |s| s.session_epoch += 1,
        |s| s.files.slot = "B01".into(),
        |s| s.files.directory = "elsewhere".into(),
        |s| {
            update(
                s.files.selected.as_mut().unwrap(),
                json!({"entry_id":"replacement"}),
            )
        },
        |s| {
            update(
                s.files.selected.as_mut().unwrap(),
                json!({"file_id":"replacement"}),
            )
        },
        |s| {
            update(
                s.files.selected.as_mut().unwrap(),
                json!({"path":"alias/a.txt"}),
            )
        },
        |s| {
            update(
                s.files.selected.as_mut().unwrap(),
                json!({"kind":"directory"}),
            )
        },
        |s| {
            update(
                s.files.selected.as_mut().unwrap(),
                json!({"repository_id":"other"}),
            )
        },
        |s| {
            update(
                s.files.selected.as_mut().unwrap(),
                json!({"repository_path":"alias/a.txt"}),
            )
        },
    ];
    for change in changes {
        Owner::new().with(|| {
            let state = RwSignal::new(selected_state());
            let selected = detail_selection(state);
            let original = selected.get().unwrap();
            state.update(change);
            assert_ne!(selected.get().as_ref(), Some(&original));
            state.with_untracked(|s| assert!(original.entry(s).is_none()));
        });
    }
    let mut state = selected_state();
    let original = Selection::capture(&state).unwrap();
    state.authenticated = false;
    assert!(original.action_entry(&state).is_none());
    state.authenticated = true;
    state.files.selected = None;
    assert!(original.entry(&state).is_none());
}

#[test]
fn detail_identity_uses_the_selected_entries_actual_repository_context() {
    let mut state = selected_state();
    let selection = Selection::capture(&state).unwrap();
    state.files.repository_id = "different listing repository".into();
    state.files.repository_path = "different listing path".into();
    assert_eq!(Selection::capture(&state).as_ref(), Some(&selection));
    assert!(selection.entry(&state).is_some());

    // Missing primary location is rejected before a detail control can own it.
    for field in ["repository_id", "repository_path"] {
        let mut value = crate::files::test_entries::wire(state.files.selected.as_ref().unwrap());
        value.as_object_mut().unwrap().remove(field);
        assert!(crate::files::test_entries::parse_entry(value).is_err());
    }
}

#[test]
fn retained_detail_actions_use_current_payload_and_write_permission() {
    let mut state = selected_state();
    state.management.storage = crate::management::Storage::Ready;
    state.files.writable = true;
    let selection = Selection::capture(&state).unwrap();
    update(
        state.files.selected.as_mut().unwrap(),
        json!({"note":"refreshed note"}),
    );
    assert_eq!(
        selection.mutation_entry(&state).unwrap().note().unwrap(),
        "refreshed note"
    );
    update(
        state.files.selected.as_mut().unwrap(),
        json!({"read_only":true}),
    );
    assert!(selection.action_entry(&state).is_some());
    assert!(selection.mutation_entry(&state).is_none());
    update(
        state.files.selected.as_mut().unwrap(),
        json!({"read_only":false}),
    );
    update(
        state.files.selected.as_mut().unwrap(),
        json!({"backup":true}),
    );
    assert!(selection.mutation_entry(&state).is_none());
    update(
        state.files.selected.as_mut().unwrap(),
        json!({"backup":false}),
    );
    state.files.mutation_pending = true;
    assert!(selection.mutation_entry(&state).is_none());
}
