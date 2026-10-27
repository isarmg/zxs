use super::*;
use crate::uploads::{self, StagedFile};
use platform_io::fs_safety::PayloadOpenMode;

fn limits(account: u64, total: u64) -> SpoolLimits {
    SpoolLimits {
        bytes_per_account: account,
        bytes_total: total,
        recovery_reserve_bytes: 1,
    }
}
fn retained(budget: &Arc<SpoolBudget>, principal: &str, name: &str, declared: u64, bytes: &[u8]) {
    let reservation = budget
        .reserve_with_available(principal, name, declared, u64::MAX)
        .unwrap();
    let staged = StagedFile::open_budgeted(
        budget.root.join(name),
        PayloadOpenMode::CreateNew,
        reservation,
    )
    .unwrap();
    staged.write(bytes).unwrap();
    staged.sync().unwrap();
}
fn total(budget: &SpoolBudget) -> u64 {
    budget
        .ledger
        .lock()
        .unwrap()
        .files
        .values()
        .map(|entry| entry.bytes)
        .sum()
}
fn record(name: &str, principal: &str) -> UploadRecord {
    UploadRecord {
        workspace_id: "ws".into(),
        item_id: "item".into(),
        principal: principal.into(),
        path: "file".into(),
        generation: 1,
        logical_size: 20,
        content_hash: Some("0".repeat(64).parse().unwrap()),
        persisted_bytes: 3,
        persisted_hash: blake3::hash(b"abc").to_hex().to_string(),
        state: crate::uploads::UploadState::Complete,
        data_file: name.into(),
    }
}
#[test]
fn reservation_settles_to_retained_bytes_not_success_or_declared_length() {
    let root = private_tempdir();
    let budget = SpoolBudget::recover(
        root.path(),
        limits(2 * METADATA_ALLOWANCE + 13, 2 * METADATA_ALLOWANCE + 13),
    )
    .unwrap();
    retained(&budget, "alice", "a.data", 10, b"abc");
    assert_eq!(total(&budget), METADATA_ALLOWANCE + 3);
    let second = budget
        .reserve_with_available("alice", "b.data", 10, u64::MAX)
        .unwrap();
    assert_eq!(total(&budget), 2 * METADATA_ALLOWANCE + 13);
    assert!(
        budget
            .reserve_with_available("bob", "c.data", 0, u64::MAX)
            .is_err()
    );
    drop(second);
    assert_eq!(total(&budget), METADATA_ALLOWANCE + 3);
    // Same physical file has one charge, even at its full quota.
    let retry = budget
        .reserve_with_available("alice", "a.data", 10, u64::MAX)
        .unwrap();
    assert_eq!(total(&budget), METADATA_ALLOWANCE + 10);
    drop(retry);
    assert_eq!(total(&budget), METADATA_ALLOWANCE + 3);
}
#[test]
fn concurrent_reservations_cannot_oversubscribe_account_or_pool() {
    let root = private_tempdir();
    let charge = METADATA_ALLOWANCE + 1;
    let budget = SpoolBudget::recover(root.path(), limits(2 * charge, 3 * charge)).unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(9));
    let mut threads = Vec::new();
    for index in 0..8 {
        let budget = budget.clone();
        let barrier = barrier.clone();
        threads.push(std::thread::spawn(move || {
            let reservation =
                budget.reserve_with_available("alice", &format!("{index}.data"), 1, u64::MAX);
            let accepted = reservation.is_ok();
            barrier.wait();
            barrier.wait();
            accepted
        }));
    }
    barrier.wait();
    assert_eq!(total(&budget), 2 * charge);
    let bob = budget
        .reserve_with_available("bob", "bob.data", 1, u64::MAX)
        .unwrap();
    assert!(
        budget
            .reserve_with_available("charlie", "c.data", 1, u64::MAX)
            .is_err()
    );
    barrier.wait();
    assert_eq!(
        threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .filter(|accepted| *accepted)
            .count(),
        2
    );
    drop(bob);
    assert_eq!(total(&budget), 0);
}
#[test]
fn recovery_counts_retained_generations_failed_unknown_and_orphan_bytes_once() {
    let root = private_tempdir();
    let config = limits(8 * METADATA_ALLOWANCE, 8 * METADATA_ALLOWANCE);
    let budget = SpoolBudget::recover(root.path(), config.clone()).unwrap();
    retained(&budget, "alice", "old.data", 20, b"abc");
    let old = record("old.data", "alice");
    uploads::persist(root.path(), &old).unwrap();
    uploads::retain_generation(root.path(), &old).unwrap();
    uploads::retain_generation(root.path(), &old).unwrap();
    retained(&budget, "alice", "new.data", 20, b"failure");
    let mut new = record("new.data", "alice");
    new.generation = 2;
    new.state = crate::uploads::UploadState::ChecksumMismatch;
    uploads::persist(root.path(), &new).unwrap();
    drop(budget);
    fs::write(root.path().join("unknown.data"), b"unknown").unwrap();
    fs::write(root.path().join("broken.json"), b"{broken").unwrap();
    let budget = SpoolBudget::recover(root.path(), config).unwrap();
    assert_eq!(total(&budget), 4 * METADATA_ALLOWANCE + 3 + 7 + 7 + 7);
    assert_eq!(budget.ledger.lock().unwrap().files.len(), 4);
    assert_eq!(fs::read(root.path().join("old.data")).unwrap(), b"abc");
    assert_eq!(
        fs::read(root.path().join("unknown.data")).unwrap(),
        b"unknown"
    );
}
#[test]
fn unknown_owner_is_not_an_account_quota_escape_and_overbudget_startup_stays_readable() {
    let root = private_tempdir();
    fs::write(root.path().join("unattributed.data"), b"unknown").unwrap();
    let budget = SpoolBudget::recover(
        root.path(),
        limits(METADATA_ALLOWANCE, 4 * METADATA_ALLOWANCE),
    )
    .unwrap();
    assert!(
        budget
            .reserve_with_available("alice", "new.data", 0, u64::MAX)
            .is_err()
    );
    assert!(
        budget
            .reserve_with_available("bob", "new.data", 0, u64::MAX)
            .is_err()
    );
    assert_eq!(
        fs::read(root.path().join("unattributed.data")).unwrap(),
        b"unknown"
    );
}
#[test]
fn headroom_accounts_for_concurrent_reservations_without_filling_a_disk() {
    let root = private_tempdir();
    let budget = SpoolBudget::recover(
        root.path(),
        limits(4 * METADATA_ALLOWANCE, 4 * METADATA_ALLOWANCE),
    )
    .unwrap();
    let charge = METADATA_ALLOWANCE + 5;
    assert!(
        budget
            .reserve_with_available("alice", "one.data", 5, charge)
            .is_err()
    );
    let first = budget
        .reserve_with_available("alice", "one.data", 5, charge + 1)
        .unwrap();
    assert!(
        budget
            .reserve_with_available("bob", "two.data", 5, 2 * charge)
            .is_err()
    );
    assert!(
        budget
            .reserve_with_available("bob", "two.data", 5, 2 * charge + 1)
            .is_ok()
    );
    drop(first);
}
#[test]
fn directory_has_one_inventory_owner_and_aliases_are_rejected() {
    let root = private_tempdir();
    let config = limits(4 * METADATA_ALLOWANCE, 4 * METADATA_ALLOWANCE);
    let budget = SpoolBudget::recover(root.path(), config.clone()).unwrap();
    assert!(SpoolBudget::recover(root.path(), config.clone()).is_err());
    drop(budget);
    fs::write(root.path().join("one.data"), b"abc").unwrap();
    fs::hard_link(root.path().join("one.data"), root.path().join("alias.data")).unwrap();
    assert!(SpoolBudget::recover(root.path(), config).is_err());
}
#[test]
fn sidecar_only_crash_remains_charged_and_rebuilds_without_a_payload() {
    let root = private_tempdir();
    let config = limits(2 * METADATA_ALLOWANCE, 2 * METADATA_ALLOWANCE);
    let budget = SpoolBudget::recover(root.path(), config.clone()).unwrap();
    let reservation = budget
        .reserve_with_available("alice", "missing.data", 10, u64::MAX)
        .unwrap();
    reservation.persist_owner().unwrap();
    drop(reservation);
    assert_eq!(total(&budget), METADATA_ALLOWANCE);
    drop(budget);
    let budget = SpoolBudget::recover(root.path(), config).unwrap();
    assert_eq!(total(&budget), METADATA_ALLOWANCE);
}

#[test]
fn fifo_inventory_lock_is_rejected_without_blocking_management_startup() {
    use std::os::unix::fs::OpenOptionsExt;
    use std::time::Duration;

    let root = private_tempdir();
    let lock_path = root.path().join(".budget.lock");
    rustix::fs::mkfifoat(
        rustix::fs::CWD,
        &lock_path,
        rustix::fs::Mode::from_raw_mode(0o600),
    )
    .unwrap();
    let path = root.path().to_owned();
    let (sent, received) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let result = SpoolBudget::recover(&path, limits(METADATA_ALLOWANCE, METADATA_ALLOWANCE));
        sent.send(result.is_err()).unwrap();
    });
    let immediate = received.recv_timeout(Duration::from_secs(1));
    // Unblock an unsafe write-only open on regression so the test does not
    // strand a native thread. The fixed reader never needs this counterpart.
    let counterpart = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32)
        .open(&lock_path)
        .unwrap();
    if immediate.is_err() {
        assert!(received.recv_timeout(Duration::from_secs(2)).unwrap());
    }
    worker.join().unwrap();
    drop(counterpart);
    assert_eq!(
        immediate,
        Ok(true),
        "optional spool startup blocked on a FIFO"
    );
}

fn private_tempdir() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    directory
}

#[test]
fn rejected_batch_headroom_preserves_actual_tail_and_verified_retry_accounting() {
    let root = private_tempdir();
    let batch = vec![42; 256 * 1024];
    let mut complete = b"abcdef".to_vec();
    complete.extend_from_slice(&batch);
    let size = complete.len() as u64;
    let quota = METADATA_ALLOWANCE + size;
    let budget = SpoolBudget::recover(root.path(), limits(quota, quota)).unwrap();
    retained(&budget, "alice", "batch.data", size, b"abcdef");
    let mut progress = record("batch.data", "alice");
    progress.logical_size = size;
    progress.content_hash = Some(storage_contract::ContentHash::from_digest(blake3::hash(
        &complete,
    )));
    progress.state = crate::uploads::UploadState::Incomplete;
    uploads::persist(root.path(), &progress).unwrap();
    drop(budget);

    // Admission saw space that is no longer available at the write boundary.
    let mut constrained = limits(quota, quota);
    constrained.recovery_reserve_bytes = u64::MAX;
    let budget = SpoolBudget::recover(root.path(), constrained).unwrap();
    let reservation = budget
        .reserve_with_available("alice", "batch.data", size, u64::MAX)
        .unwrap();
    let staged = StagedFile::open_budgeted(
        root.path().join("batch.data"),
        PayloadOpenMode::Existing,
        reservation,
    )
    .unwrap();
    assert_eq!(staged.write(&batch).unwrap_err().status, 429);
    assert_eq!(fs::read(root.path().join("batch.data")).unwrap(), b"abcdef");
    let unchanged = uploads::load(root.path(), "ws", "item").unwrap().unwrap();
    assert_eq!(unchanged.persisted_bytes, 3);
    assert_eq!(
        uploads::status(&unchanged)["durable_ranges"][0]["length"],
        "3"
    );
    drop(staged);
    assert_eq!(total(&budget), METADATA_ALLOWANCE + 6);
    drop(budget);

    let budget = SpoolBudget::recover(root.path(), limits(quota, quota)).unwrap();
    let reservation = budget.reserve("alice", "batch.data", size).unwrap();
    let staged = StagedFile::open_budgeted(
        root.path().join("batch.data"),
        PayloadOpenMode::Existing,
        reservation,
    )
    .unwrap();
    let mut prefix = uploads::ExistingPrefix::open(root.path(), &unchanged)
        .unwrap()
        .unwrap();
    assert_eq!(
        prefix.remaining(),
        6,
        "retry includes the uncheckpointed tail"
    );
    prefix.check(&complete).unwrap();
    assert!(prefix.complete());
    staged.write(&batch).unwrap();
    staged.sync().unwrap();
    progress.persisted_bytes = size;
    progress.persisted_hash = blake3::hash(&complete).to_hex().to_string();
    uploads::persist_staged(&progress, &staged).unwrap();
    drop(staged);
    assert_eq!(total(&budget), quota);
    assert_eq!(fs::read(root.path().join("batch.data")).unwrap(), complete);
    assert_eq!(
        uploads::load(root.path(), "ws", "item")
            .unwrap()
            .unwrap()
            .generation,
        1
    );
}

#[test]
fn reservation_drop_does_not_panic_again_during_unwind() {
    let root = private_tempdir();
    let budget = SpoolBudget::recover(
        root.path(),
        limits(2 * METADATA_ALLOWANCE, 2 * METADATA_ALLOWANCE),
    )
    .unwrap();
    let reservation = budget
        .reserve_with_available("alice", "unwind.data", 10, u64::MAX)
        .unwrap();
    reservation.persist_owner().unwrap();
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _reservation = reservation;
        let _ledger = budget.ledger.lock().unwrap();
        panic!("writer panic while holding the ledger");
    }));
    assert!(panic.is_err());
    assert!(budget.ledger.is_poisoned());
    let retained = match budget.ledger.lock() {
        Err(error) => error.into_inner(),
        Ok(_) => panic!("ledger must remain poisoned"),
    };
    assert_eq!(retained.total, METADATA_ALLOWANCE + 10);
    assert_ne!(retained.files["unwind.data"].reserved, 0);
    assert!(root.path().join("unwind.data.owner.json").is_file());
}

#[test]
fn recovered_metadata_and_longer_payload_survive_short_retry_settlement() {
    let root = private_tempdir();
    let payload = b"retained payload";
    fs::write(root.path().join("retained.data"), payload).unwrap();
    let metadata = json_codec::to_vec(&record("retained.data", "alice")).unwrap();
    let count = METADATA_ALLOWANCE as usize / metadata.len() + 1;
    for index in 0..count {
        fs::write(root.path().join(format!("record-{index}.json")), &metadata).unwrap();
    }
    let overhead = (count * metadata.len()) as u64;
    let expected = overhead + payload.len() as u64;
    let budget = SpoolBudget::recover(root.path(), limits(expected, expected)).unwrap();
    assert_eq!(total(&budget), expected);
    let reservation = budget
        .reserve_with_available("alice", "retained.data", 1, u64::MAX)
        .unwrap();
    drop(reservation);
    let ledger = budget.ledger.lock().unwrap();
    assert!(!ledger.uncertain);
    assert_eq!(ledger.total, expected);
    assert_eq!(ledger.files["retained.data"].overhead, overhead);
    assert_eq!(ledger.files["retained.data"].reserved, 0);
    assert_eq!(
        fs::read(root.path().join("retained.data")).unwrap(),
        payload
    );
}
