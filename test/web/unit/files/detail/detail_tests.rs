use super::*;
use crate::files::test_entries::{entry, update};
use json_codec::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[test]
fn an_unknown_loading_or_stale_note_cannot_be_submitted_as_an_empty_note() {
    let mut model = Model::default();
    assert!(!file_note_ready(&model));
    model.file_history =
        crate::files::history::tests::history(json_codec::json!({"note":"existing note"}));
    model.history_loading = true;
    assert!(!file_note_ready(&model));
    model.history_loading = false;
    assert!(file_note_ready(&model));
    model.file_history = crate::files::history::tests::history(json_codec::json!({"note":""}));
    assert!(
        file_note_ready(&model),
        "a known empty note remains editable"
    );
    model.file_history.invalidate();
    assert!(!file_note_ready(&model));
    assert_eq!(model.file_history.file_note(), Some(""));
    model.file_history = Default::default();
    assert!(
        !file_note_ready(&model),
        "a failed first request never implies an empty note"
    );
}

#[test]
fn branch_options_keep_page_order_without_duplicating_the_current_summary() {
    use crate::files::history::tests::branch_wire;
    let model = Model {
        file_history: crate::files::history::tests::history(json!({
        "selected_branch_id":"current",
        "branches":[
            branch_wire(json!({"branch_id":"other","name":"z-other","state":"merged","read_only":true})),
            branch_wire(json!({"branch_id":"current","name":"a-current","current":true}))
        ]
        })),
        ..Default::default()
    };
    let options = branch_options(&model);
    assert_eq!(options.len(), 2);
    assert_eq!(options[0].value, "other");
    assert!(options[0].disabled);
    assert_eq!(options[1].value, "current");
    assert!(!options[1].disabled);
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
            s.files.history_loading = true;
            s.files.mutation_pending = true;
        });
        assert_eq!(detail.get().as_ref(), Some(&identity));
        assert_eq!(mounts.load(Ordering::SeqCst), 1);
        state.update(|s| {
            s.files.file_history =
                crate::files::history::tests::history(json!({"branches":[
                    crate::files::history::tests::branch_wire(json!({"branch_id":"main","name":"main"})),
                    crate::files::history::tests::branch_wire(json!({"branch_id":"draft","name":"draft"}))
                ]}));
            s.files.history_loading = false;
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
    let changes: [fn(&mut State); 10] = [
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
        |s| update(s.files.selected.as_mut().unwrap(), json!({"backup":true})),
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
