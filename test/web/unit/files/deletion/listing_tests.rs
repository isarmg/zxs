use super::*;
use json_codec::json;

fn id(prefix: &str, number: u32) -> String {
    format!("{prefix}_{number:032x}")
}
fn page(first: u32, count: u32, next: bool) -> Arc<wire::Page> {
    let entries = (first..first + count)
        .map(|number| {
            json!({
                "file_id": id("file", number),
                "deletion_node_id": id("fdel", number),
                "repository_id": id("repo", 1),
                "path": "same.txt"
            })
        })
        .collect::<Vec<_>>();
    Arc::new(
        wire::page(json!({
            "snapshot": "original-snapshot",
            "entries": entries,
            "next_after": next.then(|| id("file", first + count - 1))
        }))
        .unwrap(),
    )
}
fn state(page: Arc<wire::Page>) -> State {
    let mut state = State::default();
    state.authenticated = true;
    state.session_epoch = 7;
    state.files.deletion.open = true;
    state.files.deletion.page = Some(page);
    state
}
fn target(state: &State) -> Target {
    Target {
        epoch: state.session_epoch,
        page: state.files.deletion.page.clone().unwrap(),
    }
}

#[test]
fn selection_uses_the_displayed_identity_when_paths_have_the_same_name() {
    let mut state = state(page(1, 2, false));
    let first = Selection {
        target: target(&state),
        index: 0,
    };
    let second = Selection {
        target: target(&state),
        index: 1,
    };
    let (epoch, generation, selected) = second.begin(&mut state).unwrap();
    assert_eq!(epoch, 7);
    assert!(state.files.deletion.accepts(generation));
    assert_eq!(selected.file_id.as_str(), id("file", 2));
    assert_eq!(selected.deletion_node_id, id("fdel", 2));
    assert!(state.files.deletion.loading);
    assert!(first.begin(&mut state).is_none());
    state.files.deletion.loading = false;
    let (_, _, selected) = first.begin(&mut state).unwrap();
    assert_eq!(selected.file_id.as_str(), id("file", 1));
}

#[test]
fn old_row_and_next_page_controls_cannot_adopt_a_replacement_page() {
    let mut state = state(page(1, 100, true));
    let old_target = target(&state);
    let selection = Selection {
        target: old_target.clone(),
        index: 0,
    };
    let next = Read::Next(old_target);
    assert_eq!(
        next.endpoint(&state).unwrap(),
        format!(
            "/api/file-deletions?after={}&snapshot=original-snapshot",
            id("file", 100),
        )
    );
    state.files.deletion.page = Some(page(101, 100, true));
    assert!(next.endpoint(&state).is_none());
    assert!(selection.begin(&mut state).is_none());
    assert!(state.files.deletion.selected.is_none());
    assert!(!state.files.deletion.loading);
    assert_eq!(state.files.deletion.generation, 0);
    assert_eq!(
        Read::Next(target(&state)).endpoint(&state).unwrap(),
        format!(
            "/api/file-deletions?after={}&snapshot=original-snapshot",
            id("file", 200),
        )
    );
}

#[test]
fn even_equal_reloaded_rows_require_their_own_displayed_page() {
    let mut state = state(page(1, 2, false));
    let stale = Selection {
        target: target(&state),
        index: 0,
    };
    state.files.deletion.page = Some(page(1, 2, false));
    assert!(stale.begin(&mut state).is_none());
    let fresh = Selection {
        target: target(&state),
        index: 0,
    };
    assert!(fresh.begin(&mut state).is_some());
}

#[test]
fn old_controls_are_inert_after_session_close_or_read_changes() {
    let changes: [fn(&mut State); 5] = [
        |state| state.session_epoch += 1,
        |state| state.authenticated = false,
        |state| state.files.deletion.close(),
        |state| state.files.deletion.loading = true,
        |state| state.files.deletion.page = None,
    ];
    for change in changes {
        let mut state = state(page(1, 100, true));
        let target = target(&state);
        let selection = Selection {
            target: target.clone(),
            index: 0,
        };
        change(&mut state);
        assert!(Read::Next(target).endpoint(&state).is_none());
        assert!(selection.begin(&mut state).is_none());
    }
}

#[test]
fn last_page_has_no_continuation_and_explicit_reload_starts_at_page_one() {
    let state = state(page(101, 1, false));
    assert!(Read::Next(target(&state)).endpoint(&state).is_none());
    assert_eq!(Read::First.endpoint(&state).unwrap(), "/api/file-deletions");
}
