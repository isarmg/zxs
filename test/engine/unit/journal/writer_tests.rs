use super::*;
use std::{
    sync::{Barrier, mpsc},
    time::Duration,
};

fn disks(temp: &tempfile::TempDir) -> Vec<DiskConfig> {
    Config::development([
        temp.path().join("one"),
        temp.path().join("two"),
        temp.path().join("three"),
    ])
    .disks
}

#[test]
fn another_journal_can_sync_while_one_writer_is_blocked() {
    let temp = tempfile::tempdir().unwrap();
    let disks = disks(&temp);
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (other_tx, other_rx) = mpsc::channel();
    std::thread::scope(|scope| {
        let first_disk = &disks[0];
        let first = scope.spawn(move || {
            append_with_sync(
                first_disk,
                "PREPARE",
                1,
                &"a".repeat(64),
                &"b".repeat(64),
                None,
                |file, parent| {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    sync_journal(file, parent)
                },
            )
        });
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let second = scope.spawn(|| {
            let result = append(&disks[1], "PREPARE", 1, &"a".repeat(64), &"b".repeat(64));
            other_tx.send(result.is_ok()).unwrap();
            result
        });
        let independent = other_rx.recv_timeout(Duration::from_secs(5));
        // Always release the first writer before assertions, including a broken
        // implementation which accidentally keeps a global lock over fsync.
        release_tx.send(()).unwrap();
        first.join().unwrap().unwrap();
        second.join().unwrap().unwrap();
        assert!(independent.unwrap());
    });
    for disk in &disks[..2] {
        assert_eq!(scan(disk).unwrap().records.len(), 1);
        assert!(!writers().lock().unwrap().contains_key(&log_path(disk)));
    }
}

#[test]
fn simultaneous_same_journal_retries_return_one_frame_and_receipt() {
    let temp = tempfile::tempdir().unwrap();
    let disk = &disks(&temp)[0];
    let start = Barrier::new(16);
    let receipts = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..16)
            .map(|_| {
                scope.spawn(|| {
                    start.wait();
                    append(disk, "PUBLICATION", 1, &"a".repeat(64), &"b".repeat(64)).unwrap()
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert!(
        receipts
            .iter()
            .all(|receipt| receipt.frame_hash == receipts[0].frame_hash)
    );
    assert_eq!(scan(disk).unwrap().records.len(), 1);
    assert!(!writers().lock().unwrap().contains_key(&log_path(disk)));
}

#[test]
fn simultaneous_distinct_frames_preserve_one_verified_hash_chain() {
    let temp = tempfile::tempdir().unwrap();
    let disk = &disks(&temp)[0];
    let start = Barrier::new(16);
    std::thread::scope(|scope| {
        for object in 0..16 {
            let start = &start;
            scope.spawn(move || {
                start.wait();
                append(
                    disk,
                    "PREPARE",
                    1,
                    &"a".repeat(64),
                    &format!("{object:064x}"),
                )
                .unwrap();
            });
        }
    });
    let scanned = scan(disk).unwrap();
    assert!(!scanned.torn_tail);
    assert_eq!(scanned.records.len(), 16);
    let objects: BTreeSet<_> = scanned
        .records
        .iter()
        .map(|(record, _)| &record.object_hash)
        .collect();
    assert_eq!(objects.len(), 16);
}

#[test]
fn writer_lock_outlives_cache_eviction_and_every_waiting_lease() {
    let temp = tempfile::tempdir().unwrap();
    let disk = &disks(&temp)[0];
    append(disk, "PREPARE", 1, &"a".repeat(64), &"b".repeat(64)).unwrap();
    let path = log_path(disk);
    let first = JournalWriter::acquire(&path).unwrap();
    let first_guard = first.lock();
    let waiter = JournalWriter::acquire(&path).unwrap();
    assert!(matches!(
        waiter.lock.as_ref().unwrap().try_lock(),
        Err(std::sync::TryLockError::WouldBlock)
    ));
    for index in 0..MAX_CACHED_JOURNALS {
        let mut another = disk.clone();
        another.root = temp.path().join(format!("z-eviction-{index:03}"));
        append(&another, "PREPARE", 1, &"a".repeat(64), &"b".repeat(64)).unwrap();
    }
    assert!(!tails().lock().unwrap().contains_key(&path));
    assert!(tails().lock().unwrap().len() <= MAX_CACHED_JOURNALS);
    let after_eviction = JournalWriter::acquire(&path).unwrap();
    assert!(Arc::ptr_eq(
        first.lock.as_ref().unwrap(),
        after_eviction.lock.as_ref().unwrap()
    ));
    drop(first_guard);
    drop(first);
    assert!(writers().lock().unwrap().contains_key(&path));
    drop(waiter);
    assert!(writers().lock().unwrap().contains_key(&path));
    drop(after_eviction);
    assert!(!writers().lock().unwrap().contains_key(&path));
}

#[test]
fn simultaneous_last_writer_drops_leave_no_static_paths() {
    let temp = tempfile::tempdir().unwrap();
    for iteration in 0..64 {
        let path = temp.path().join(format!("journal-{iteration}"));
        let leases: Vec<_> = (0..8)
            .map(|_| JournalWriter::acquire(&path).unwrap())
            .collect();
        let release = Barrier::new(leases.len());
        std::thread::scope(|scope| {
            for lease in leases {
                let release = &release;
                scope.spawn(move || {
                    release.wait();
                    drop(lease);
                });
            }
        });
        assert!(!writers().lock().unwrap().contains_key(&path));
    }
}

#[test]
fn writer_panic_retries_verify_visible_and_torn_bytes_after_leases_drain() {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    for tear in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let disk = &disks(&temp)[0];
        let path = log_path(disk);
        append(disk, "PREPARE", 1, &"a".repeat(64), &"b".repeat(64)).unwrap();
        let waiter = JournalWriter::acquire(&path).unwrap();
        let panicked = catch_unwind(AssertUnwindSafe(|| {
            let _ = append_with_sync(
                disk,
                "ACCEPT",
                1,
                &"a".repeat(64),
                &"c".repeat(64),
                None,
                |file, _| {
                    if tear {
                        file.set_len(file.metadata()?.len() - 9)?;
                    }
                    panic!("injected panic during synchronization");
                },
            );
        }));
        assert!(panicked.is_err());
        assert!(!tails().lock().unwrap().contains_key(&path));
        assert!(catch_unwind(AssertUnwindSafe(|| waiter.lock())).is_err());
        assert!(
            catch_unwind(AssertUnwindSafe(|| {
                append(disk, "ACCEPT", 1, &"a".repeat(64), &"c".repeat(64))
            }))
            .is_err()
        );
        assert!(writers().lock().unwrap().contains_key(&path));
        drop(waiter);
        assert!(!writers().lock().unwrap().contains_key(&path));
        let before_scans = SCAN_COUNT.with(std::cell::Cell::get);
        append(disk, "ACCEPT", 1, &"a".repeat(64), &"c".repeat(64)).unwrap();
        assert_eq!(SCAN_COUNT.with(std::cell::Cell::get), before_scans + 1);
        let scanned = scan(disk).unwrap();
        assert_eq!(scanned.records.len(), 2);
        assert!(!scanned.torn_tail);
        assert!(!writers().lock().unwrap().contains_key(&path));
    }
}

#[test]
fn writer_acquire_drop_churn_never_splits_a_same_path_lock() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("churn-journal");
    let inside = AtomicUsize::new(0);
    let start = Barrier::new(16);
    std::thread::scope(|scope| {
        for _ in 0..16 {
            scope.spawn(|| {
                start.wait();
                for iteration in 0..2048 {
                    let lease = JournalWriter::acquire(&path).unwrap();
                    let guard = lease.lock();
                    assert_eq!(inside.fetch_add(1, Ordering::SeqCst), 0);
                    if iteration % 8 == 0 {
                        std::thread::yield_now();
                    }
                    assert_eq!(inside.fetch_sub(1, Ordering::SeqCst), 1);
                    drop(guard);
                    drop(lease);
                }
            });
        }
    });
    assert_eq!(inside.load(Ordering::SeqCst), 0);
    assert!(!writers().lock().unwrap().contains_key(&path));
}
