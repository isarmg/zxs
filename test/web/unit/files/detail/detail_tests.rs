use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[test]
fn an_unknown_or_loading_note_cannot_be_submitted_as_an_empty_note() {
    let mut model = Model::default();
    assert!(!file_note_ready(&model));
    model.file_history = json_codec::json!({"note":"existing note"}).into();
    model.history_loading = true;
    assert!(!file_note_ready(&model));
    model.history_loading = false;
    assert!(file_note_ready(&model));
    model.file_history = json_codec::json!({"note":""}).into();
    assert!(
        file_note_ready(&model),
        "a known empty note remains editable"
    );
    model.file_history = Value::Null.into();
    assert!(
        !file_note_ready(&model),
        "a failed first request never implies an empty note"
    );
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
    state.files.selected = Some(json_codec::json!({
        "entry_id":"entry", "file_id":"file", "kind":"file", "name":"a.txt",
        "path":"docs/a.txt", "repository_id":"repository", "repository_path":"docs/a.txt",
        "version_id":"v1", "node_id":"v1", "selected_branch_id":"main", "note":"old"
    }));
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
                json_codec::json!({"branches":[{"name":"main"},{"name":"draft"}]}).into();
            s.files.history_loading = false;
            s.files.mutation_pending = false;
        });
        assert_eq!(detail.get().as_ref(), Some(&identity));
        assert_eq!(mounts.load(Ordering::SeqCst), 1);
        // A real post-publication listing changes selected_entry.note, not just
        // file_history. Mutable payloads must not recreate editor/tree owners.
        state.update(|s| {
            let entry = s.files.selected.as_mut().unwrap();
            entry["note"] = "saved note".into();
            entry["name"] = "refreshed display name".into();
            entry["read_only"] = true.into();
            entry["version_id"] = "v2".into();
            entry["node_id"] = "v2".into();
            entry["selected_branch_id"] = "other".into();
        });
        assert_eq!(detail.get().as_ref(), Some(&identity));
        assert_eq!(mounts.load(Ordering::SeqCst), 1);
        state.with_untracked(|s| {
            let current = identity.entry(s).unwrap();
            assert_eq!(current["note"], "saved note");
            assert_eq!(current["name"], "refreshed display name");
            assert_eq!(current["version_id"], "v2");
            assert_eq!(current["read_only"], true);
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
        |s| s.files.selected.as_mut().unwrap()["entry_id"] = "replacement".into(),
        |s| s.files.selected.as_mut().unwrap()["file_id"] = "replacement".into(),
        |s| s.files.selected.as_mut().unwrap()["path"] = "alias/a.txt".into(),
        |s| s.files.selected.as_mut().unwrap()["kind"] = "directory".into(),
        |s| s.files.selected.as_mut().unwrap()["repository_id"] = "other".into(),
        |s| s.files.selected.as_mut().unwrap()["repository_path"] = "alias/a.txt".into(),
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

    // Primary entries without explicit context use the containing listing.
    // That inherited context is part of identity, not mutable display data.
    for field in ["repository_id", "repository_path"] {
        let mut state = selected_state();
        state
            .files
            .selected
            .as_mut()
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove(field);
        let selection = Selection::capture(&state).unwrap();
        if field == "repository_id" {
            state.files.repository_id = "replacement repository".into();
        } else {
            state.files.repository_path = "replacement directory".into();
        }
        assert!(selection.action_entry(&state).is_none());
    }
}

#[test]
fn retained_detail_actions_use_current_payload_and_write_permission() {
    let mut state = selected_state();
    state.management.storage = crate::management::Storage::Ready;
    state.files.writable = true;
    let selection = Selection::capture(&state).unwrap();
    state.files.selected.as_mut().unwrap()["note"] = "refreshed note".into();
    assert_eq!(
        selection.mutation_entry(&state).unwrap()["note"],
        "refreshed note"
    );
    state.files.selected.as_mut().unwrap()["read_only"] = true.into();
    assert!(selection.action_entry(&state).is_some());
    assert!(selection.mutation_entry(&state).is_none());
    state.files.selected.as_mut().unwrap()["read_only"] = false.into();
    state.files.selected.as_mut().unwrap()["backup"] = true.into();
    assert!(selection.mutation_entry(&state).is_none());
    state.files.selected.as_mut().unwrap()["backup"] = false.into();
    state.files.mutation_pending = true;
    assert!(selection.mutation_entry(&state).is_none());
}
