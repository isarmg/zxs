use super::*;
use crate::uploads::{self, StagedFile};
use platform_io::fs_safety::PayloadOpenMode;
use std::os::unix::fs::MetadataExt;
#[path = "retirement.rs"]
mod retirement;

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
    let staged = StagedFile::open_budgeted(PayloadOpenMode::CreateNew, reservation).unwrap();
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
fn last_writer_drop_after_root_replacement_keeps_full_charge_and_blocks_admission() {
    let root = private_tempdir();
    let path = root.path().join("spool");
    PrivateDirectory::create(&path).unwrap();
    let budget = SpoolBudget::recover(
        &path,
        limits(4 * METADATA_ALLOWANCE, 4 * METADATA_ALLOWANCE),
    )
    .unwrap();
    let reservation = budget
        .reserve_with_available("alice", "work.data", 100, u64::MAX)
        .unwrap();
    let staged = StagedFile::open_budgeted(PayloadOpenMode::CreateNew, reservation).unwrap();
    let last_job = Arc::clone(&staged);
    staged.write(b"abc").unwrap();
    staged.sync().unwrap();
    let owner = fs::read(path.join("work.data.owner.json")).unwrap();
    drop(staged);
    let moved = root.path().join("retained");
    fs::rename(&path, &moved).unwrap();
    PrivateDirectory::create(&path).unwrap();
    drop(last_job);
    let ledger = budget.ledger.lock().unwrap();
    assert_eq!(ledger.total, METADATA_ALLOWANCE + 100);
    assert_eq!(ledger.files["work.data"].reserved, METADATA_ALLOWANCE + 100);
    assert!(ledger.uncertain);
    drop(ledger);
    assert!(
        budget
            .reserve_with_available("bob", "next.data", 0, u64::MAX)
            .is_err()
    );
    assert_eq!(fs::read(moved.join("work.data")).unwrap(), b"abc");
    assert_eq!(fs::read(moved.join("work.data.owner.json")).unwrap(), owner);
    assert_eq!(fs::read_dir(path).unwrap().count(), 0);
}

#[test]
fn reservation_drop_keeps_full_charge_when_payload_or_owner_is_unsafe() {
    use std::os::unix::fs::symlink;
    #[derive(Clone, Copy)]
    enum UnsafeEntry {
        Symlink,
        Hardlink,
        OwnerDirectory,
    }
    for unsafe_entry in [
        UnsafeEntry::Symlink,
        UnsafeEntry::Hardlink,
        UnsafeEntry::OwnerDirectory,
    ] {
        let root = private_tempdir();
        let budget = SpoolBudget::recover(
            root.path(),
            limits(4 * METADATA_ALLOWANCE, 4 * METADATA_ALLOWANCE),
        )
        .unwrap();
        let reservation = budget
            .reserve_with_available("alice", "work.data", 100, u64::MAX)
            .unwrap();
        reservation.persist_owner().unwrap();
        let payload = root.path().join("work.data");
        let owner = root.path().join("work.data.owner.json");
        let owner_bytes = fs::read(&owner).unwrap();
        let target = root.path().join("target");
        match unsafe_entry {
            UnsafeEntry::Symlink => {
                fs::write(&target, b"preserve").unwrap();
                symlink(&target, &payload).unwrap();
            }
            UnsafeEntry::Hardlink => {
                fs::write(&target, b"preserve").unwrap();
                fs::hard_link(&target, &payload).unwrap();
            }
            UnsafeEntry::OwnerDirectory => {
                fs::rename(&owner, root.path().join("retained-owner")).unwrap();
                fs::create_dir(&owner).unwrap();
            }
        }
        drop(reservation);
        let ledger = budget.ledger.lock().unwrap();
        assert_eq!(ledger.total, METADATA_ALLOWANCE + 100);
        assert_eq!(ledger.files["work.data"].reserved, METADATA_ALLOWANCE + 100);
        assert!(ledger.uncertain);
        drop(ledger);
        assert!(
            budget
                .reserve_with_available("bob", "new.data", 0, u64::MAX)
                .is_err()
        );
        match unsafe_entry {
            UnsafeEntry::Symlink | UnsafeEntry::Hardlink => {
                assert!(fs::symlink_metadata(payload).is_ok());
                assert_eq!(fs::read(target).unwrap(), b"preserve");
                assert_eq!(fs::read(owner).unwrap(), owner_bytes);
            }
            UnsafeEntry::OwnerDirectory => {
                assert!(owner.is_dir());
                assert!(!payload.exists());
                assert_eq!(
                    fs::read(root.path().join("retained-owner")).unwrap(),
                    owner_bytes
                );
            }
        }
    }
}

#[test]
fn reservation_cannot_publish_owner_or_payload_into_a_rebound_root() {
    for owner_persisted in [false, true] {
        let root = private_tempdir();
        let path = root.path().join("spool");
        PrivateDirectory::create(&path).unwrap();
        let budget = SpoolBudget::recover(
            &path,
            limits(4 * METADATA_ALLOWANCE, 4 * METADATA_ALLOWANCE),
        )
        .unwrap();
        let reservation = budget
            .reserve_with_available("alice", "work.data", 100, u64::MAX)
            .unwrap();
        if owner_persisted {
            reservation.persist_owner().unwrap();
        }
        let moved = root.path().join("retained");
        fs::rename(&path, &moved).unwrap();
        PrivateDirectory::create(&path).unwrap();
        let error = StagedFile::open_budgeted(PayloadOpenMode::CreateNew, reservation)
            .err()
            .unwrap();
        assert!(std::error::Error::source(&error).is_some());
        assert_eq!(total(&budget), METADATA_ALLOWANCE + 100);
        assert!(budget.ledger.lock().unwrap().uncertain);
        assert!(
            budget
                .reserve_with_available("alice", "other.data", 0, u64::MAX)
                .is_err()
        );
        assert!(!moved.join("work.data").exists());
        assert_eq!(moved.join("work.data.owner.json").exists(), owner_persisted);
        assert_eq!(fs::read_dir(path).unwrap().count(), 0);
    }
}

#[test]
fn owner_publication_reuses_only_matching_records_without_overwriting_evidence() {
    #[derive(Clone, Copy)]
    enum Existing {
        Matching,
        Conflicting,
        Corrupt,
    }
    for existing in [Existing::Matching, Existing::Conflicting, Existing::Corrupt] {
        let root = private_tempdir();
        let budget = SpoolBudget::recover(
            root.path(),
            limits(4 * METADATA_ALLOWANCE, 4 * METADATA_ALLOWANCE),
        )
        .unwrap();
        let reservation = budget
            .reserve_with_available("alice", "work.data", 100, u64::MAX)
            .unwrap();
        let bytes = match existing {
            Existing::Matching | Existing::Conflicting => json_codec::to_vec(&Owner {
                data_file: "work.data".into(),
                principal: if matches!(existing, Existing::Matching) {
                    "alice"
                } else {
                    "bob"
                }
                .into(),
            })
            .unwrap(),
            Existing::Corrupt => b"{broken".to_vec(),
        };
        let name = EntryName::new("work.data.owner.json").unwrap();
        platform_io::fs_safety::AtomicFile::create(&budget.directory, &name, &bytes).unwrap();
        let path = root.path().join(name.as_path());
        let inode = fs::metadata(&path).unwrap().ino();
        let result = reservation.persist_owner();
        match existing {
            Existing::Matching => result.unwrap(),
            Existing::Conflicting | Existing::Corrupt => {
                assert!(result.is_err());
                assert!(budget.ledger.lock().unwrap().uncertain);
                assert!(
                    budget
                        .reserve_with_available("bob", "next.data", 0, u64::MAX)
                        .is_err()
                );
            }
        }
        drop(reservation);
        let expected = match existing {
            Existing::Matching => METADATA_ALLOWANCE,
            Existing::Conflicting | Existing::Corrupt => METADATA_ALLOWANCE + 100,
        };
        assert_eq!(total(&budget), expected);
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(fs::metadata(path).unwrap().ino(), inode);
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 2);
    }
}

#[test]
fn lowered_limits_preserve_full_inventory_and_allow_zero_growth_owned_retry() {
    for (account, global) in [
        (METADATA_ALLOWANCE, 8 * METADATA_ALLOWANCE),
        (2 * METADATA_ALLOWANCE, 2 * METADATA_ALLOWANCE),
    ] {
        let root = private_tempdir();
        let budget = SpoolBudget::recover(
            root.path(),
            limits(4 * METADATA_ALLOWANCE, 8 * METADATA_ALLOWANCE),
        )
        .unwrap();
        retained(&budget, "alice", "a.data", 100, b"abc");
        retained(&budget, "bob", "b.data", 100, b"def");
        let owner = fs::read(root.path().join("a.data.owner.json")).unwrap();
        drop(budget);
        let budget = SpoolBudget::recover(root.path(), limits(account, global)).unwrap();
        assert_eq!(total(&budget), 2 * METADATA_ALLOWANCE + 6);
        let retry = budget
            .reserve_with_available("alice", "a.data", 3, u64::MAX)
            .unwrap();
        let staged = StagedFile::open_budgeted(PayloadOpenMode::Existing, retry).unwrap();
        drop(staged);
        assert_eq!(total(&budget), 2 * METADATA_ALLOWANCE + 6);
        let principal = if global == 2 * METADATA_ALLOWANCE {
            "charlie"
        } else {
            "alice"
        };
        assert!(
            budget
                .reserve_with_available(principal, "new.data", 0, u64::MAX)
                .is_err()
        );
        assert_eq!(fs::read(root.path().join("a.data")).unwrap(), b"abc");
        assert_eq!(fs::read(root.path().join("b.data")).unwrap(), b"def");
        assert_eq!(
            fs::read(root.path().join("a.data.owner.json")).unwrap(),
            owner
        );
    }
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
    let temporary = format!(".state-atomic-{}.tmp", "a".repeat(32));
    fs::write(root.path().join(&temporary), b"tail").unwrap();
    fs::write(root.path().join("broken.data.owner.json"), b"{broken").unwrap();
    let budget = SpoolBudget::recover(root.path(), config).unwrap();
    assert_eq!(
        total(&budget),
        6 * METADATA_ALLOWANCE + 3 + 7 + 7 + 7 + 4 + 7
    );
    let ledger = budget.ledger.lock().unwrap();
    assert_eq!(ledger.files.len(), 6);
    assert_eq!(ledger.unknown, 4 * METADATA_ALLOWANCE + 7 + 7 + 4 + 7);
    assert!(ledger.files[&temporary].owner.is_none());
    assert!(ledger.files["broken.data.owner.json"].owner.is_none());
    drop(ledger);
    assert_eq!(fs::read(root.path().join("old.data")).unwrap(), b"abc");
    assert_eq!(
        fs::read(root.path().join("unknown.data")).unwrap(),
        b"unknown"
    );
    assert_eq!(fs::read(root.path().join(temporary)).unwrap(), b"tail");
    assert_eq!(
        fs::read(root.path().join("broken.data.owner.json")).unwrap(),
        b"{broken"
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
    let owner = fs::metadata(root.path().join("missing.data.owner.json")).unwrap();
    assert_eq!(owner.mode() & 0o777, 0o600);
    assert_eq!(owner.nlink(), 1);
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
    let staged = StagedFile::open_budgeted(PayloadOpenMode::Existing, reservation).unwrap();
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
    let staged = StagedFile::open_budgeted(PayloadOpenMode::Existing, reservation).unwrap();
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
fn last_staged_owner_during_ledger_unwind_keeps_charge_without_a_second_panic() {
    let root = private_tempdir();
    let budget = SpoolBudget::recover(
        root.path(),
        limits(2 * METADATA_ALLOWANCE, 2 * METADATA_ALLOWANCE),
    )
    .unwrap();
    let reservation = budget
        .reserve_with_available("alice", "unwind.data", 100, u64::MAX)
        .unwrap();
    let staged = StagedFile::open_budgeted(PayloadOpenMode::CreateNew, reservation).unwrap();
    staged.write(b"retained").unwrap();
    staged.sync().unwrap();
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _owner = staged;
        let _ledger = budget.ledger.lock().unwrap();
        panic!("ledger failed before the final staged owner dropped");
    }));
    assert!(panic.is_err());
    assert!(budget.ledger.is_poisoned());
    let ledger = budget.ledger.lock().err().unwrap().into_inner();
    assert_eq!(ledger.total, METADATA_ALLOWANCE + 100);
    assert_eq!(
        ledger.files["unwind.data"].reserved,
        METADATA_ALLOWANCE + 100
    );
    assert_eq!(
        fs::read(root.path().join("unwind.data")).unwrap(),
        b"retained"
    );
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
