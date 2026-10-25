use super::*;
use std::{
    sync::{Mutex, mpsc},
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
    }
}

#[test]
fn serialized_engine_mutations_preserve_one_verified_hash_chain() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("state");
    let config = Config::development([temp.path().join("disk")]);
    let disk = config.disks[0].clone();
    let engine = Mutex::new(Engine::initialize(&root, config).unwrap());
    assert_eq!(
        Engine::open(&root).err().unwrap().code(),
        ErrorCode::WriterAlreadyActive
    );
    std::thread::scope(|scope| {
        for index in 0..8 {
            let engine = &engine;
            scope.spawn(move || {
                engine
                    .lock()
                    .unwrap()
                    .create_repository(
                        &format!("repository-{index}"),
                        "P01",
                        1,
                        "admin",
                        &new_id("op"),
                    )
                    .unwrap();
            });
        }
    });
    let scanned = scan(&disk).unwrap();
    assert!(!scanned.torn_tail);
    assert_eq!(
        scanned
            .records
            .iter()
            .filter(|(r, _)| r.record_type == "PUBLICATION")
            .count(),
        8
    );
    drop(engine);
    assert_eq!(Engine::open(&root).unwrap().state().repositories.len(), 8);
}

#[test]
fn sequential_retries_and_cache_eviction_preserve_receipts() {
    let temp = tempfile::tempdir().unwrap();
    let disk = &disks(&temp)[0];
    let first = append(disk, "PUBLICATION", 1, &"a".repeat(64), &"b".repeat(64)).unwrap();
    for _ in 0..16 {
        let repeated = append(disk, "PUBLICATION", 1, &"a".repeat(64), &"b".repeat(64)).unwrap();
        assert_eq!(first.frame_hash, repeated.frame_hash);
    }
    for index in 0..MAX_CACHED_JOURNALS {
        let mut another = disk.clone();
        another.root = temp.path().join(format!("z-eviction-{index:03}"));
        append(&another, "PREPARE", 1, &"a".repeat(64), &"b".repeat(64)).unwrap();
    }
    assert!(!tails().lock().unwrap().contains_key(&log_path(disk)));
    let before_scans = SCAN_COUNT.with(std::cell::Cell::get);
    let repeated = append(disk, "PUBLICATION", 1, &"a".repeat(64), &"b".repeat(64)).unwrap();
    assert_eq!(first.frame_hash, repeated.frame_hash);
    assert_eq!(SCAN_COUNT.with(std::cell::Cell::get), before_scans + 1);
    assert_eq!(scan(disk).unwrap().records.len(), 1);
}

#[test]
fn writer_panic_retries_verify_visible_and_torn_bytes() {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    for tear in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let disk = &disks(&temp)[0];
        let path = log_path(disk);
        append(disk, "PREPARE", 1, &"a".repeat(64), &"b".repeat(64)).unwrap();
        let panicked = catch_unwind(AssertUnwindSafe(|| {
            append_with_sync(
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
            )
            .unwrap();
        }));
        assert!(panicked.is_err());
        assert!(!tails().lock().unwrap().contains_key(&path));
        let before_scans = SCAN_COUNT.with(std::cell::Cell::get);
        append(disk, "ACCEPT", 1, &"a".repeat(64), &"c".repeat(64)).unwrap();
        assert_eq!(SCAN_COUNT.with(std::cell::Cell::get), before_scans + 1);
        let scanned = scan(disk).unwrap();
        assert_eq!(scanned.records.len(), 2);
        assert!(!scanned.torn_tail);
    }
}

#[test]
fn detached_preparation_retains_writer_ownership_without_appending() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("state");
    let config = Config::development([temp.path().join("disk")]);
    let disk = config.disks[0].clone();
    let mut engine = Engine::initialize(&root, config).unwrap();
    let repo = engine
        .create_repository("repo", "P01", 1, "admin", &new_id("op"))
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "file.txt", b"payload")
        .unwrap();
    let preparation = engine
        .prepare_workspace_commit(
            CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: new_id("op"),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: String::new(),
                request_digest: None,
            },
            None,
        )
        .unwrap();
    let before = std::fs::read(log_path(&disk)).unwrap();
    drop(engine);
    assert_eq!(
        Engine::open(&root).err().unwrap().code(),
        ErrorCode::WriterAlreadyActive
    );
    let prepared = preparation.execute().unwrap();
    assert_eq!(std::fs::read(log_path(&disk)).unwrap(), before);
    assert_eq!(
        Engine::open(&root).err().unwrap().code(),
        ErrorCode::WriterAlreadyActive
    );
    drop(prepared);
    let reopened = Engine::open(&root).unwrap();
    assert_eq!(reopened.state().repositories.len(), 1);
}
