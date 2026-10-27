use super::*;
use crate::files::model::FileOperation;

fn editing() -> State {
    let mut state = State {
        authenticated: true,
        ..Default::default()
    };
    state.files.writable = true;
    state.files.merge = Some(Draft {
        file: "file".into(),
        repository: "repo".into(),
        path: "a.txt".into(),
        branches: vec![],
        nodes: vec![],
        selected: BTreeSet::new(),
        chosen: "node".into(),
        survivor: "branch".into(),
    });
    state
        .files
        .publications
        .insert(review::Kind::Merge, Default::default());
    state
        .files
        .publications
        .insert(review::Kind::Backup, Default::default());
    state
}

#[test]
fn merge_cancel_remains_available_when_only_storage_or_listing_blocks_writes() {
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
        assert!(state.files.merge.is_none());
        assert!(!state.files.publications.contains_key(&review::Kind::Merge));
        assert!(state.files.publications.contains_key(&review::Kind::Backup));
    }
}

#[test]
fn merge_cancel_preserves_pending_and_unknown_operation_drafts() {
    let mut state = editing();
    state.files.mutation_pending = true;
    cancel(&mut state);
    assert!(state.files.merge.is_some());
    state.files.mutation_pending = false;
    state
        .files
        .operations
        .push(FileOperation::unknown("original"));
    cancel(&mut state);
    assert!(state.files.merge.is_some());
    assert!(state.files.publications.contains_key(&review::Kind::Merge));
    state.files.operations.clear();
    cancel(&mut state);
    assert!(state.files.merge.is_none());
}
