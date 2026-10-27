use super::*;
use crate::files::model::FileOperation;

fn editing() -> State {
    let mut state = State {
        authenticated: true,
        ..Default::default()
    };
    state.files.writable = true;
    state.files.tag_edit = Some(Draft {
        file: "file".into(),
        repository: "repo".into(),
        path: "a.txt".into(),
        nodes: vec![],
        branches: vec![],
        targets: vec![],
        add: BTreeSet::from(["draft-tag".into()]),
        remove: BTreeSet::new(),
        input: "unfinished".into(),
    });
    state.files.checked_nodes.insert("node".into());
    state
        .files
        .publications
        .insert(review::Kind::Tags, Default::default());
    state
        .files
        .publications
        .insert(review::Kind::Backup, Default::default());
    state
}

#[test]
fn tag_cancel_remains_available_when_only_storage_or_listing_blocks_writes() {
    for storage in [
        crate::management::Storage::ReadOnly,
        crate::management::Storage::Unavailable,
        crate::management::Storage::Ready,
    ] {
        let mut state = editing();
        state.management.storage = storage;
        state.files.loading = storage == crate::management::Storage::Ready;
        assert!(!actions::write_allowed(&state));
        assert!(!actions::pending(&state));
        cancel(&mut state);
        assert!(state.files.tag_edit.is_none());
        assert!(!state.files.publications.contains_key(&review::Kind::Tags));
        assert!(state.files.publications.contains_key(&review::Kind::Backup));
        assert!(state.files.checked_nodes.contains("node"));
    }
}

#[test]
fn tag_cancel_preserves_pending_and_unknown_operation_drafts() {
    let mut state = editing();
    state.files.mutation_pending = true;
    cancel(&mut state);
    assert!(state.files.tag_edit.is_some());
    state.files.mutation_pending = false;
    state
        .files
        .operations
        .push(FileOperation::unknown("original"));
    cancel(&mut state);
    assert!(state.files.tag_edit.is_some());
    assert!(state.files.publications.contains_key(&review::Kind::Tags));
    state.files.operations.clear();
    cancel(&mut state);
    assert!(state.files.tag_edit.is_none());
}
