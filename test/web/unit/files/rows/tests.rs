use super::*;
use crate::files::test_entries::update;
use json_codec::json;
use std::cell::Cell;

thread_local! {
    static PROJECTIONS: Cell<usize> = const { Cell::new(0) };
    static PROJECTED: Cell<usize> = const { Cell::new(0) };
}
pub(super) fn record_project(entries: usize) {
    PROJECTIONS.set(PROJECTIONS.get() + 1);
    PROJECTED.set(PROJECTED.get() + entries);
}
fn reset_counts() {
    PROJECTIONS.set(0);
    PROJECTED.set(0);
}
fn state(entries: Vec<Entry>) -> State {
    let mut state = State {
        authenticated: true,
        session_epoch: 7,
        ..Default::default()
    };
    state.files.slot = "disk".into();
    state.files.directory = "docs".into();
    state.files.entries = entries.into();
    state
}
fn entry(id: &str, path: &str, name: &str, kind: &str) -> Entry {
    crate::files::test_entries::entry(
        json!({"entry_id":id,"path":path,"name":name,"kind":kind,"size":if kind == "file" { "1" } else { "0" }}),
    )
}

// Synthetic response metadata only: these tests neither import real files nor
// measure DOM rendering, browser memory, or full-directory capacity.
#[test]
fn unrelated_updates_and_query_changes_do_not_reproject_a_server_page() {
    for size in [1, 100, 200] {
        let owner = Owner::new();
        owner.with(|| {
            reset_counts();
            let state = RwSignal::new(state(
                (0..size)
                    .map(|i| entry(&i.to_string(), &format!("docs/{i}"), &i.to_string(), "file"))
                    .collect(),
            ));
            let rows = visible_rows(state);
            assert_eq!(rows.with_untracked(Vec::len), size);
            for i in 0..100 {
                state.update(|s| {
                    s.notice = i.to_string();
                    s.files.selected = Some(s.files.entries[i % size].clone());
                    s.files.checked_entries.insert(i.to_string());
                    s.files.file_history = crate::files::history::tests::history(json!({"history":i}));
                });
                assert_eq!(rows.with_untracked(Vec::len), size);
            }
            state.update(|s| s.files.query = "does not match this page".into());
            assert_eq!(rows.with_untracked(Vec::len), size);
            assert_eq!((PROJECTIONS.get(), PROJECTED.get()), (1, size));
            println!(
                "synthetic_page_entries={size}; unrelated_updates=100; projections=1; projected={size}; query_reprojections=0"
            );
        });
    }
}

#[test]
fn server_order_is_preserved_without_local_sorting_or_filtering() {
    let owner = Owner::new();
    owner.with(|| {
        let state = RwSignal::new(state(vec![
            entry("f1", "docs/z", "z", "file"),
            entry("f2", "docs/A", "A", "file"),
            entry("d1", "docs/folder-z", "folder-z", "directory"),
            entry("f3", "docs/a2", "a2", "file"),
            entry("d2", "docs/b", "b", "directory"),
        ]));
        let rows = visible_rows(state);
        let ids = || {
            rows.with_untracked(|rows| {
                rows.iter()
                    .map(|row| row.key.entry_id.clone())
                    .collect::<Vec<_>>()
            })
        };
        assert_eq!(ids(), ["f1", "f2", "d1", "f3", "d2"]);
        let retained = rows.get_untracked()[1].clone();
        for query in ["A", "absent", ""] {
            state.update(|s| s.files.query = query.into());
            assert_eq!(ids(), ["f1", "f2", "d1", "f3", "d2"]);
            assert!(Arc::ptr_eq(&retained, &rows.get_untracked()[1]));
        }
    });
}

#[test]
fn replacing_pages_drops_old_keys_and_projects_only_the_new_response() {
    Owner::new().with(|| {
        reset_counts();
        let page = |start: usize| {
            (start..start + 200)
                .map(|i| entry(&i.to_string(), &format!("docs/{i}"), &i.to_string(), "file"))
                .collect()
        };
        let state = RwSignal::new(state(page(0)));
        let rows = visible_rows(state);
        let old = rows.get_untracked()[0].clone();
        let mut seen = std::collections::HashSet::new();
        for start in (0..1_000).step_by(200) {
            if start > 0 {
                state.update(|s| s.files.entries = page(start).into());
            }
            rows.with_untracked(|rows| {
                assert_eq!(rows.len(), 200);
                for (offset, row) in rows.iter().enumerate() {
                    assert_eq!(row.key.entry_id, (start + offset).to_string());
                    assert!(seen.insert(row.key.clone()), "duplicate across pages");
                }
            });
            if start > 0 {
                assert!(state.with_untracked(|s| old.current_entry(s).is_none()));
            }
        }
        assert_eq!(seen.len(), 1_000);
        assert_eq!((PROJECTIONS.get(), PROJECTED.get()), (5, 1_000));
        state.update(|s| s.files.entries = Default::default());
        assert!(rows.with_untracked(Vec::is_empty));
        assert_eq!((PROJECTIONS.get(), PROJECTED.get()), (6, 1_000));
    });
}

#[test]
fn retained_key_refreshes_payload_and_pending_handlers_resolve_current_data() {
    let owner = Owner::new();
    owner.with(|| {
        let state = RwSignal::new(state(vec![entry("f", "docs/a", "a", "file")]));
        let rows = visible_rows(state);
        let old = rows.get_untracked().remove(0);
        state.update(|s| {
            let mut refreshed = s.files.entries[0].clone();
            update(&mut refreshed, json!({"version_id":"new","size":"42"}));
            s.files.entries = vec![refreshed].into();
        });
        // Even before recomputing the row signal, queued handlers cannot use
        // the old version. Current server metadata is authoritative.
        assert_eq!(
            state.with_untracked(|s| old
                .current_entry(s)
                .unwrap()
                .version_id()
                .unwrap()
                .to_owned()),
            "new"
        );
        let new = rows.get_untracked().remove(0);
        assert_eq!(old.key, new.key);
        assert_ne!(old, new);
        assert_eq!(new.name, "a");
        assert_eq!(new.size, Some(42));
        state.update(|s| s.files.entries = Default::default());
        assert!(state.with_untracked(|s| old.current_entry(s).is_none()));
    });
}

#[test]
fn aliases_sessions_and_navigation_do_not_reuse_wrong_identity() {
    let mut state = state(vec![
        entry("f", "docs/a", "a", "file"),
        entry("f", "docs/b", "b", "file"),
    ]);
    let old = Listing::capture(&state).project();
    assert_ne!(old[0].key, old[1].key);
    assert_ne!(old[0].selection_id, old[1].selection_id);
    state.files.selected = Some(state.files.entries[0].clone());
    let selected = entry_key(state.files.selected.as_ref().unwrap());
    assert_eq!(
        old.iter()
            .filter(|row| row.selection_id == selected)
            .count(),
        1
    );
    assert_eq!(old[0].selection_id, selected);
    state.files.directory = "another".into();
    assert!(old[0].current_entry(&state).is_none());
    assert_ne!(old[0].key, Listing::capture(&state).project()[0].key);
    state.files.directory = "docs".into();
    state.files.slot = "other-disk".into();
    assert!(old[0].current_entry(&state).is_none());
    state.files.slot = "disk".into();
    state.session_epoch += 1;
    assert!(old[0].current_entry(&state).is_none());
    assert_ne!(old[0].key, Listing::capture(&state).project()[0].key);
    state.session_epoch -= 1;
    state.session_transition = crate::ui::SessionTransition::LoggingOut;
    assert!(old[0].current_entry(&state).is_none());
}

#[test]
fn copy_on_write_cannot_hide_changed_listing_from_memo() {
    let owner = Owner::new();
    owner.with(|| {
        let state = RwSignal::new(state(vec![entry("f", "docs/a", "a", "file")]));
        let rows = visible_rows(state);
        let before = rows.get_untracked().remove(0);
        state.update(|s| {
            update(
                &mut Arc::make_mut(&mut s.files.entries)[0],
                json!({"size":"42"}),
            )
        });
        let after = rows.get_untracked().remove(0);
        assert_eq!(before.key, after.key);
        assert_ne!(before, after);
        assert_eq!(after.size, Some(42));
        assert_eq!(before.size, Some(1));
    });
}

#[test]
fn display_rows_do_not_retain_old_full_snapshots_or_stale_hidden_metadata() {
    let mut state = state(vec![entry("f", "docs/a", "a", "file")]);
    let old_snapshot = Arc::downgrade(&state.files.entries);
    let old = Listing::capture(&state).project().remove(0);
    let mut updated = state.files.entries[0].clone();
    update(&mut updated, json!({"version_id":"new-version"}));
    state.files.entries = vec![updated].into();
    let new = Listing::capture(&state).project().remove(0);
    assert_eq!(
        old, new,
        "hidden metadata does not invalidate unchanged display data"
    );
    assert!(
        old_snapshot.upgrade().is_none(),
        "retained rows must not pin whole old listings"
    );
    assert_eq!(
        old.current_entry(&state).unwrap().version_id().unwrap(),
        "new-version"
    );
}
