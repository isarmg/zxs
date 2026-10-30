use super::*;
use crate::files::model::FileOperation;

fn editing() -> State {
    let mut state = State {
        authenticated: true,
        ..Default::default()
    };
    state.files.writable = true;
    state.files.merge = Some(super::super::tests::draft());
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
        cancel(&mut state, 1);
        assert!(state.files.merge.is_none());
        assert!(!state.files.publications.contains_key(&review::Kind::Merge));
        assert!(state.files.publications.contains_key(&review::Kind::Backup));
    }
}

#[test]
fn merge_cancel_preserves_pending_and_unknown_operation_drafts() {
    let mut state = editing();
    state.files.mutation_pending = true;
    cancel(&mut state, 1);
    assert!(state.files.merge.is_some());
    state.files.mutation_pending = false;
    state
        .files
        .operations
        .push(FileOperation::unknown("original"));
    cancel(&mut state, 1);
    assert!(state.files.merge.is_some());
    assert!(state.files.publications.contains_key(&review::Kind::Merge));
    state.files.operations.clear();
    cancel(&mut state, 1);
    assert!(state.files.merge.is_none());
}

#[test]
fn selected_branch_pages_are_render_limits_and_preserve_off_page_selection() {
    Owner::new().with(|| {
        let state = RwSignal::new(editing());
        state.update(|s| {
            let draft = s.files.merge.as_mut().unwrap();
            for index in 0..SELECTED_ROWS {
                let id = format!("extra-{index:04}");
                draft.selected.insert(
                    id.clone(),
                    SelectedBranch {
                        id,
                        name: format!("branch {index}"),
                        expected: format!("head-{index}"),
                    },
                );
            }
        });
        let before = state.with_untracked(|s| s.files.merge.as_ref().unwrap().selected.clone());
        move_selected_page(state, 1, true);
        state.with_untracked(|s| {
            let draft = s.files.merge.as_ref().unwrap();
            assert_eq!(draft.selected_page, 1);
            assert_eq!(draft.selected, before);
            assert_eq!(draft.chosen.as_ref().unwrap().id, "fnode-unrelated");
            assert_eq!(
                draft
                    .selected
                    .values()
                    .skip(draft.selected_page * SELECTED_ROWS)
                    .take(SELECTED_ROWS)
                    .count(),
                2
            );
        });
        move_selected_page(state, 1, false);
        assert_eq!(
            state.with_untracked(|s| s.files.merge.as_ref().unwrap().selected_page),
            0
        );
    });
}

#[test]
fn an_old_panel_cancel_cannot_remove_a_reopened_draft_or_cancel_its_review() {
    let mut state = editing();
    let old = ReviewOwner {
        kind: review::Kind::Merge,
        instance: 1,
    };
    state.files.history_aux.begin_review(old);
    cancel(&mut state, 1);
    let mut draft = super::super::tests::draft();
    draft.instance = 2;
    state.files.merge = Some(draft);
    let current = ReviewOwner {
        kind: review::Kind::Merge,
        instance: 2,
    };
    let generation = state.files.history_aux.begin_review(current);
    cancel(&mut state, 1);
    assert_eq!(state.files.merge.as_ref().unwrap().instance, 2);
    assert!(state.files.history_aux.current_review(current, generation));
    cancel(&mut state, 2);
    assert!(state.files.merge.is_none());
    assert!(!state.files.history_aux.loading());
}

#[test]
fn cancelling_a_merge_draft_leaves_another_kinds_review_owned() {
    let mut state = editing();
    let other = ReviewOwner {
        kind: review::Kind::Tags,
        instance: 2,
    };
    let generation = state.files.history_aux.begin_review(other);
    cancel(&mut state, 1);
    assert!(state.files.merge.is_none());
    assert!(state.files.history_aux.current_review(other, generation));
}
