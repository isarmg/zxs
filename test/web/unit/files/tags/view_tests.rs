use super::*;
use crate::files::model::FileOperation;

fn editing() -> State {
    let mut state = State {
        authenticated: true,
        ..Default::default()
    };
    state.files.writable = true;
    state.files.tag_edit = Some(super::super::tests::draft_with_pending_input("unfinished"));
    state.files.checked_nodes = super::super::tests::selection(&["node"]);
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
        cancel(&mut state, 1);
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
    cancel(&mut state, 1);
    assert!(state.files.tag_edit.is_some());
    state.files.mutation_pending = false;
    state
        .files
        .operations
        .push(FileOperation::unknown("original"));
    cancel(&mut state, 1);
    assert!(state.files.tag_edit.is_some());
    assert!(state.files.publications.contains_key(&review::Kind::Tags));
    state.files.operations.clear();
    cancel(&mut state, 1);
    assert!(state.files.tag_edit.is_none());
}

#[test]
fn selected_target_and_delta_pages_preserve_the_complete_action() {
    Owner::new().with(|| {
        let state = RwSignal::new(editing());
        state.update(|s| {
            let draft = s.files.tag_edit.as_mut().unwrap();
            for index in 0..SELECTED_ROWS {
                draft
                    .targets
                    .insert(format!("source-{index:04}"), Some("main".into()));
                draft.add.insert(format!("tag-{index:04}"));
            }
        });
        let before = state.with_untracked(|s| s.files.tag_edit.as_ref().unwrap().targets.clone());
        move_page(state, 1, true, false);
        move_page(state, 1, true, true);
        state.with_untracked(|s| {
            let draft = s.files.tag_edit.as_ref().unwrap();
            assert_eq!(draft.target_page, 1);
            assert_eq!(draft.delta_page, 1);
            assert_eq!(draft.targets, before);
            assert_eq!(draft.targets.len(), SELECTED_ROWS + 1);
            assert_eq!(
                draft
                    .targets
                    .keys()
                    .skip(draft.target_page * SELECTED_ROWS)
                    .take(SELECTED_ROWS)
                    .count(),
                1
            );
            assert_eq!(draft.input, "unfinished");
        });
        move_page(state, 1, false, false);
        assert_eq!(
            state.with_untracked(|s| s.files.tag_edit.as_ref().unwrap().target_page),
            0
        );
    });
}

#[test]
fn an_old_tag_panel_cancel_cannot_remove_a_successor_or_its_review() {
    let mut state = editing();
    state.files.history_aux.begin_review(ReviewOwner {
        kind: review::Kind::Tags,
        instance: 1,
    });
    cancel(&mut state, 1);
    state.files.tag_edit = Some(super::super::tests::draft_for_instance(2, "new input"));
    let current = ReviewOwner {
        kind: review::Kind::Tags,
        instance: 2,
    };
    let generation = state.files.history_aux.begin_review(current);
    cancel(&mut state, 1);
    assert_eq!(state.files.tag_edit.as_ref().unwrap().instance, 2);
    assert!(state.files.history_aux.current_review(current, generation));
    cancel(&mut state, 2);
    assert!(state.files.tag_edit.is_none());
    assert!(!state.files.history_aux.loading());
}
