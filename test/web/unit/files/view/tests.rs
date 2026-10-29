use super::super::model::{Disk, Model};
use super::*;
use crate::files::model::FileOperation;

#[test]
fn disk_roots_preserve_loading_empty_and_refresh_states() {
    for loading_disks in [false, true] {
        for disks_loaded in [false, true] {
            for disks in [vec![], vec![disk("offline", false, false, 0)]] {
                let model = Model {
                    loading_disks,
                    disks_loaded,
                    disks: disks.clone(),
                    ..Model::default()
                };
                let expected = if loading_disks && !disks_loaded {
                    DiskRoots::Loading
                } else if disks_loaded && disks.is_empty() {
                    DiskRoots::Empty
                } else {
                    DiskRoots::Disks(disks)
                };
                assert_eq!(disk_roots_projection(&model), expected);
            }
        }
    }
}

#[test]
fn disk_roots_preserve_order_access_capacity_and_ignore_directory_state() {
    let disks = vec![
        disk("offline", false, false, 0),
        disk("read-only", true, false, 123),
        disk("writable", true, true, u64::MAX),
    ];
    let mut model = Model {
        disks: disks.clone(),
        disks_loaded: true,
        ..Model::default()
    };
    model.directory = "unrelated/path".into();
    model.selected = Some(crate::files::test_entries::entry(
        json_codec::json!({"name": "unrelated"}),
    ));
    model.operations = vec![FileOperation::unknown("unrelated")];
    model.checked_entries.insert("unrelated".into());
    assert_eq!(disk_roots_projection(&model), DiskRoots::Disks(disks));
}

fn disk(slot: &str, online: bool, writable: bool, available_bytes: u64) -> Disk {
    Disk {
        slot_id: slot.into(),
        online,
        writable,
        available_bytes,
    }
}

// These exercise the production memo and its downstream dependency, not DOM
// identity, focus, event listeners, or browser scheduling.
#[test]
fn disk_roots_memo_skips_unrelated_and_equal_updates() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    Owner::new().with(|| {
        let state = RwSignal::new(State::default());
        state.update(|s| {
            s.files.disks_loaded = true;
            s.files.disks = vec![disk("P01", true, true, 123)];
        });
        let snapshot = disk_roots_snapshot(state);
        let runs = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&runs);
        let consumer = Memo::new(move |_| {
            observed.fetch_add(1, Ordering::Relaxed);
            snapshot.get()
        });
        let expected = DiskRoots::Disks(vec![disk("P01", true, true, 123)]);
        assert_eq!(consumer.get(), expected);
        for i in 0..10 {
            state.update(|s| {
                s.management.loading = i % 2 == 0;
                s.monitor.loading = i % 2 == 0;
                s.notice = i.to_string();
                s.files.checked_entries.insert(i.to_string());
                s.files.directory = format!("unrelated/{i}");
            });
            assert_eq!(consumer.get(), expected);
        }
        state.update(|s| {
            s.files.loading_disks = true;
            s.files.disks = vec![disk("P01", true, true, 123)];
        });
        assert_eq!(consumer.get(), expected);
        assert_eq!(runs.load(Ordering::Relaxed), 1);
    });
}

#[test]
fn disk_roots_memo_publishes_display_order_loading_and_session_reset_changes() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    Owner::new().with(|| {
        let state = RwSignal::new(State::default());
        let snapshot = disk_roots_snapshot(state);
        let runs = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&runs);
        let consumer = Memo::new(move |_| {
            observed.fetch_add(1, Ordering::Relaxed);
            snapshot.get()
        });
        let check = |expected, count| {
            assert_eq!(consumer.get(), expected);
            assert_eq!(runs.load(Ordering::Relaxed), count);
        };
        check(DiskRoots::Disks(vec![]), 1);
        state.update(|s| s.files.loading_disks = true);
        check(DiskRoots::Loading, 2);
        state.update(|s| {
            s.files.loading_disks = false;
            s.files.disks_loaded = true;
        });
        check(DiskRoots::Empty, 3);
        let mut disks = vec![disk("P01", true, true, 123), disk("B01", true, false, 456)];
        state.update(|s| s.files.disks = disks.clone());
        check(DiskRoots::Disks(disks.clone()), 4);
        disks[0].available_bytes = 124;
        state.update(|s| s.files.disks = disks.clone());
        check(DiskRoots::Disks(disks.clone()), 5);
        disks[0].online = false;
        state.update(|s| s.files.disks = disks.clone());
        check(DiskRoots::Disks(disks.clone()), 6);
        disks[0].writable = false;
        state.update(|s| s.files.disks = disks.clone());
        check(DiskRoots::Disks(disks.clone()), 7);
        disks[0].slot_id = "C01".into();
        state.update(|s| s.files.disks = disks.clone());
        check(DiskRoots::Disks(disks.clone()), 8);
        disks.swap(0, 1);
        state.update(|s| s.files.disks = disks.clone());
        check(DiskRoots::Disks(disks), 9);
        state.update(|s| {
            *s = State {
                session_epoch: s.session_epoch + 1,
                ..Default::default()
            };
        });
        check(DiskRoots::Disks(vec![]), 10);
        state.update(|s| {
            s.authenticated = true;
            s.files.disks_loaded = true;
            s.files.disks = vec![disk("P02", true, false, 789)];
        });
        check(DiskRoots::Disks(vec![disk("P02", true, false, 789)]), 11);
    });
}
