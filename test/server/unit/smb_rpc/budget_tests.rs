use super::*;

#[test]
fn fifo_inventory_lock_is_rejected_without_stranding_smb_startup() {
    use std::os::unix::fs::OpenOptionsExt;
    use std::time::Duration;

    let root = crate::tests::private_tempdir();
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
        sent.send(Budget::recover(&path, SpoolLimits::default()).is_err())
            .unwrap();
    });
    let immediate = received.recv_timeout(Duration::from_secs(1));
    // Release an unsafe open on regression, without letting the test hang.
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
    assert_eq!(immediate, Ok(true), "SMB startup blocked on a FIFO");
}

fn limits(account: u64, total: u64) -> SpoolLimits {
    SpoolLimits {
        bytes_per_account: account,
        bytes_total: total,
        recovery_reserve_bytes: 1,
    }
}
fn reserve(
    budget: &Arc<Budget>,
    principal: &str,
    name: &str,
    ceiling: u64,
) -> Result<Lease, ApiError> {
    budget.reserve_available(
        principal,
        &budget.root.join(name),
        ceiling,
        ceiling,
        u64::MAX,
    )
}
fn total(budget: &Budget) -> u64 {
    budget
        .ledger
        .lock()
        .unwrap()
        .files
        .values()
        .map(|entry| entry.bytes)
        .sum()
}
#[test]
fn retained_descriptor_ceiling_survives_disconnect_and_restart() {
    let root = crate::tests::private_tempdir();
    let limit = OVERHEAD + 100;
    let budget = Budget::recover(root.path(), limits(limit, 2 * limit)).unwrap();
    let lease = reserve(&budget, "alice", "copy", 100).unwrap();
    fs::write(&lease.source, b"one").unwrap();
    // A flushed or disconnected descriptor can still expand later. Charge the
    // full ceiling, not the current three bytes, even after its server drops.
    drop(lease);
    assert_eq!(total(&budget), limit);
    assert!(reserve(&budget, "alice", "other", 0).is_err());
    drop(budget);
    let budget = Budget::recover(root.path(), limits(limit, 2 * limit)).unwrap();
    assert_eq!(total(&budget), limit);
    assert!(reserve(&budget, "alice", "other", 0).is_err());
    assert!(reserve(&budget, "bob", "other", 0).is_ok());
    assert_eq!(fs::read(root.path().join("copy")).unwrap(), b"one");
}
#[test]
fn successful_cleanup_releases_capacity_but_failed_cleanup_does_not() {
    let root = crate::tests::private_tempdir();
    let limit = OVERHEAD + 100;
    let budget = Budget::recover(root.path(), limits(limit, limit)).unwrap();
    let lease = reserve(&budget, "alice", "copy", 100).unwrap();
    fs::write(&lease.source, b"one").unwrap();
    fs::write(lease.source.with_extension("work.json"), b"evidence").unwrap();
    fs::remove_file(&lease.source).unwrap();
    drop(lease);
    assert_eq!(total(&budget), limit);
    assert!(root.path().join("copy.budget.json").exists());
    // A separate clean lease demonstrates the normal Close path.
    let clean = crate::tests::private_tempdir();
    let budget = Budget::recover(clean.path(), limits(limit, limit)).unwrap();
    let lease = reserve(&budget, "alice", "copy", 100).unwrap();
    fs::write(&lease.source, b"one").unwrap();
    fs::remove_file(&lease.source).unwrap();
    drop(lease);
    assert_eq!(total(&budget), 0);
    assert!(!clean.path().join("copy.budget.json").exists());
}
#[test]
fn global_account_and_recovery_admission_precede_payload_expansion() {
    let root = crate::tests::private_tempdir();
    let limit = OVERHEAD + 100;
    let budget = Budget::recover(root.path(), limits(limit, 2 * limit)).unwrap();
    assert!(
        budget
            .reserve_available("alice", &root.path().join("a"), 100, 100, limit)
            .is_err()
    );
    assert!(!root.path().join("a.budget.json").exists());
    let _a = reserve(&budget, "alice", "a", 100).unwrap();
    assert!(reserve(&budget, "alice", "b", 1).is_err());
    let _b = reserve(&budget, "bob", "b", 100).unwrap();
    assert!(reserve(&budget, "charlie", "c", 0).is_err());
}
#[test]
fn orphan_bytes_charge_every_account_without_deletion() {
    let root = crate::tests::private_tempdir();
    fs::write(root.path().join("legacy"), b"unknown").unwrap();
    let budget = Budget::recover(root.path(), limits(OVERHEAD + 100, 3 * OVERHEAD)).unwrap();
    assert!(reserve(&budget, "alice", "new", 0).is_err());
    assert!(reserve(&budget, "bob", "new", 0).is_err());
    assert_eq!(fs::read(root.path().join("legacy")).unwrap(), b"unknown");
}
#[test]
fn ceiling_checks_happen_before_write_and_metadata_corruption_fails_closed() {
    let root = crate::tests::private_tempdir();
    let budget = Budget::recover(root.path(), limits(OVERHEAD + 100, OVERHEAD + 100)).unwrap();
    let mut lease = reserve(&budget, "alice", "copy", 100).unwrap();
    assert!(lease.before_write_available(101, 0, u64::MAX).is_err());
    assert!(lease.before_write_available(100, 100, 100).is_err());
    assert!(lease.before_write_available(100, 100, 101).is_ok());
    fs::write(&lease.source, b"one").unwrap();
    drop(lease);
    drop(budget);
    fs::write(root.path().join("copy.budget.json"), b"invalid").unwrap();
    assert!(Budget::recover(root.path(), limits(OVERHEAD + 100, OVERHEAD + 100)).is_err());
    assert_eq!(fs::read(root.path().join("copy")).unwrap(), b"one");
}

#[test]
fn concurrent_admission_cannot_overbook_a_single_account() {
    let root = crate::tests::private_tempdir();
    let one = OVERHEAD + 1;
    let budget = Budget::recover(root.path(), limits(2 * one, 8 * one)).unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(8));
    let workers = (0..8)
        .map(|index| {
            let budget = budget.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let admitted = reserve(&budget, "alice", &format!("copy{index}"), 1);
                barrier.wait();
                let success = admitted.is_ok();
                barrier.wait();
                success
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        workers
            .into_iter()
            .filter_map(|worker| worker.join().ok())
            .filter(|accepted| *accepted)
            .count(),
        2
    );
}

#[test]
fn tiny_open_reserves_current_extent_not_static_maximum_and_growth_is_durable() {
    let root = crate::tests::private_tempdir();
    let limit = 2 * OVERHEAD + 20;
    let budget = Budget::recover(root.path(), limits(limit, limit)).unwrap();
    let mut lease = budget
        .reserve_available(
            "alice",
            &root.path().join("tiny"),
            3,
            1024 * 1024 * 1024,
            u64::MAX,
        )
        .unwrap();
    fs::write(&lease.source, b"old").unwrap();
    let _other = budget
        .reserve_available(
            "alice",
            &root.path().join("other"),
            0,
            1024 * 1024 * 1024,
            u64::MAX,
        )
        .unwrap();
    assert_eq!(total(&budget), 2 * OVERHEAD + 3);
    lease.before_write_available(20, 17, u64::MAX).unwrap();
    assert_eq!(total(&budget), limit);
    assert!(lease.before_write_available(21, 1, u64::MAX).is_err());
    assert_eq!(
        fs::read(&lease.source).unwrap(),
        b"old",
        "admission does not write payload"
    );
    // A later truncate/overwrite cannot release an earlier admitted extent.
    lease.before_write_available(1, 1, u64::MAX).unwrap();
    drop(lease);
    drop(_other);
    drop(budget);
    let budget = Budget::recover(root.path(), limits(limit, limit)).unwrap();
    assert_eq!(total(&budget), OVERHEAD + 20);
    assert_eq!(fs::read(root.path().join("tiny")).unwrap(), b"old");
}

#[test]
fn failed_growth_record_keeps_data_and_blocks_new_admission() {
    let root = crate::tests::private_tempdir();
    let budget = Budget::recover(root.path(), limits(4 * OVERHEAD, 4 * OVERHEAD)).unwrap();
    let mut lease = budget
        .reserve_available("alice", &root.path().join("copy"), 3, 100, u64::MAX)
        .unwrap();
    fs::write(&lease.source, b"old").unwrap();
    // Force atomic record replacement to fail without relying on root-sensitive
    // permission tests or actually exhausting the shared filesystem.
    fs::remove_file(record_path(&lease.source)).unwrap();
    fs::create_dir(record_path(&lease.source)).unwrap();
    assert!(lease.before_write_available(20, 17, u64::MAX).is_err());
    // The admissible grant reaches the handle maximum and stays charged even
    // though its record could not be persisted.
    assert_eq!(total(&budget), OVERHEAD + 100);
    assert!(reserve(&budget, "bob", "other", 0).is_err());
    assert_eq!(fs::read(&lease.source).unwrap(), b"old");
    drop(lease);
    drop(budget);
    assert!(Budget::recover(root.path(), limits(4 * OVERHEAD, 4 * OVERHEAD)).is_err());
    assert_eq!(fs::read(root.path().join("copy")).unwrap(), b"old");
}

#[test]
fn pending_unwind_closes_admission_without_releasing_high_water() {
    let root = crate::tests::private_tempdir();
    let budget = Budget::recover(root.path(), limits(4 * OVERHEAD, 4 * OVERHEAD)).unwrap();
    let lease = reserve(&budget, "alice", "copy", 100).unwrap();
    fs::write(&lease.source, b"old").unwrap();
    let outcome = std::panic::catch_unwind(|| {
        let _pending = Pending {
            budget: &budget,
            complete: false,
        };
        panic!("persistence unwound");
    });
    assert!(outcome.is_err());
    assert!(budget.ledger.lock().unwrap().uncertain);
    assert!(reserve(&budget, "alice", "other", 0).is_err());
    drop(lease);
    assert_eq!(total(&budget), OVERHEAD + 100);
    assert!(root.path().join("copy.budget.json").is_file());
    assert_eq!(fs::read(root.path().join("copy")).unwrap(), b"old");
}

#[test]
fn poisoned_pending_and_lease_drop_preserve_evidence_without_second_panic() {
    let root = crate::tests::private_tempdir();
    let budget = Budget::recover(root.path(), limits(4 * OVERHEAD, 4 * OVERHEAD)).unwrap();
    let lease = reserve(&budget, "alice", "copy", 100).unwrap();
    let outcome = std::panic::catch_unwind(|| {
        let _pending = Pending {
            budget: &budget,
            complete: false,
        };
        let _ledger = budget.ledger.lock().unwrap();
        panic!("ledger invariant failed");
    });
    assert!(outcome.is_err());
    assert!(budget.ledger.is_poisoned());
    assert!(std::panic::catch_unwind(|| reserve(&budget, "alice", "other", 0)).is_err());
    drop(lease);
    assert!(root.path().join("copy.budget.json").is_file());
    let ledger = budget.ledger.lock().err().unwrap().into_inner();
    assert_eq!(ledger.total, OVERHEAD + 100);
    assert_eq!(ledger.accounts["alice"], OVERHEAD + 100);
}

#[test]
fn failed_reservation_never_clobbers_existing_record_and_keeps_charge() {
    let root = crate::tests::private_tempdir();
    let budget = Budget::recover(root.path(), limits(4 * OVERHEAD, 4 * OVERHEAD)).unwrap();
    let source = root.path().join("copy");
    fs::write(&source, b"old").unwrap();
    fs::write(record_path(&source), b"existing evidence").unwrap();
    let error = reserve(&budget, "alice", "copy", 100).err().unwrap();
    assert!(std::error::Error::source(&error).is_some());
    assert_eq!(
        fs::read(record_path(&source)).unwrap(),
        b"existing evidence"
    );
    assert_eq!(fs::read(&source).unwrap(), b"old");
    assert_eq!(total(&budget), OVERHEAD + 100);
    assert!(budget.ledger.lock().unwrap().uncertain);
    assert!(reserve(&budget, "bob", "other", 0).is_err());
}

#[test]
fn pending_persistence_charges_capacity_but_does_not_block_other_sources() {
    for grow in [false, true] {
        let root = crate::tests::private_tempdir();
        let limit = 2 * (OVERHEAD + 100);
        let budget = Budget::recover(root.path(), limits(limit, limit)).unwrap();
        let source = root.path().join("a");
        let lease = grow.then(|| {
            budget
                .reserve_available("alice", &source, 0, 100, u64::MAX)
                .unwrap()
        });
        fs::write(&source, b"old").unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let a = {
            let budget = budget.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                persist::pause_at(persist::Point::Before, barrier);
                if let Some(mut lease) = lease {
                    lease.before_write_available(100, 100, u64::MAX).unwrap();
                    lease
                } else {
                    reserve(&budget, "alice", "a", 100).unwrap()
                }
            })
        };
        barrier.wait();
        let (sent, received) = std::sync::mpsc::channel();
        let b = {
            let budget = budget.clone();
            std::thread::spawn(move || {
                assert!(reserve(&budget, "alice", "a", 0).is_err());
                let lease = reserve(&budget, "alice", "b", 100).unwrap();
                assert!(reserve(&budget, "alice", "c", 0).is_err());
                let ledger = budget.ledger.lock().unwrap();
                assert_eq!(ledger.total, limit);
                assert_eq!(ledger.accounts["alice"], limit);
                assert_eq!(ledger.unknown, 0);
                drop(ledger);
                sent.send(()).unwrap();
                lease
            })
        };
        let progressed = received.recv_timeout(std::time::Duration::from_secs(5));
        // Always release A, including when a lock regression prevented B's progress.
        barrier.wait();
        let _a = a.join().unwrap();
        let _b = b.join().unwrap();
        assert!(
            progressed.is_ok(),
            "different source waited for persistence"
        );
    }
}

#[test]
fn completed_persistence_cannot_issue_permission_after_concurrent_failure() {
    for grow in [false, true] {
        let root = crate::tests::private_tempdir();
        let budget = Budget::recover(root.path(), limits(4 * OVERHEAD, 4 * OVERHEAD)).unwrap();
        let source = root.path().join("a");
        let lease = grow.then(|| {
            budget
                .reserve_available("alice", &source, 0, 100, u64::MAX)
                .unwrap()
        });
        fs::write(&source, b"one").unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let worker = {
            let budget = budget.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                persist::pause_at(persist::Point::After, barrier);
                if let Some(mut lease) = lease {
                    lease.before_write_available(100, 100, u64::MAX)
                } else {
                    reserve(&budget, "alice", "a", 100).map(|_| ())
                }
            })
        };
        barrier.wait();
        let failed_source = root.path().join("b");
        fs::write(&failed_source, b"two").unwrap();
        fs::write(record_path(&failed_source), b"conflicting evidence").unwrap();
        let error = reserve(&budget, "alice", "b", 100).err().unwrap();
        assert!(std::error::Error::source(&error).is_some());
        barrier.wait();
        assert_eq!(
            worker.join().unwrap().unwrap_err().code,
            storage_contract::ErrorCode::SmbSpoolUnavailable
        );
        assert!(reserve(&budget, "bob", "c", 0).is_err());
        assert_eq!(total(&budget), 2 * (OVERHEAD + 100));
        assert_eq!(fs::read(&source).unwrap(), b"one");
        assert_eq!(fs::read(&failed_source).unwrap(), b"two");
        let owner: Owner =
            storage_contract::decode_strict(&fs::read(record_path(&source)).unwrap()).unwrap();
        assert_eq!(owner.admitted_end, 100);
    }
}

#[test]
fn temporary_creation_failure_retains_growth_and_original_evidence() {
    let root = crate::tests::private_tempdir();
    let directory = root.path().join("spool");
    fs::create_dir(&directory).unwrap();
    fs::set_permissions(
        &directory,
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    let budget = Budget::recover(&directory, limits(4 * OVERHEAD, 4 * OVERHEAD)).unwrap();
    let mut lease = budget
        .reserve_available("alice", &directory.join("copy"), 3, 100, u64::MAX)
        .unwrap();
    fs::write(&lease.source, b"old").unwrap();
    let moved = root.path().join("retained");
    fs::rename(&directory, &moved).unwrap();
    fs::write(&directory, b"not a directory").unwrap();
    let error = lease.before_write_available(100, 97, u64::MAX).unwrap_err();
    assert!(std::error::Error::source(&error).is_some());
    assert_eq!(total(&budget), OVERHEAD + 100);
    assert!(budget.ledger.lock().unwrap().uncertain);
    drop(lease);
    assert_eq!(fs::read(moved.join("copy")).unwrap(), b"old");
    assert!(moved.join("copy.budget.json").is_file());
}

#[test]
#[ignore = "requires budget_io_eio.c in an isolated LD_PRELOAD child"]
fn real_budget_io_failure_preserves_source_charge_and_error() {
    let root = PathBuf::from(std::env::var_os("ZXS_SMB_BUDGET_FAULT_ROOT").unwrap());
    let grow = std::env::var("ZXS_SMB_BUDGET_GROW").unwrap() == "1";
    let budget = Budget::recover(&root, limits(4 * OVERHEAD, 4 * OVERHEAD)).unwrap();
    let source = root.join("copy");
    let lease = grow.then(|| {
        budget
            .reserve_available("alice", &source, 3, 100, u64::MAX)
            .unwrap()
    });
    fs::write(&source, b"old").unwrap();
    fs::write(root.join(".inject"), b"enable child-only interposer").unwrap();
    let result = if let Some(mut lease) = lease {
        lease.before_write_available(100, 97, u64::MAX)
    } else {
        reserve(&budget, "alice", "copy", 100).map(|_| ())
    };
    let error = result.unwrap_err();
    let cause = std::error::Error::source(&error).unwrap().source().unwrap();
    assert_eq!(
        cause.downcast_ref::<io::Error>().unwrap().raw_os_error(),
        Some(5)
    );
    assert_eq!(total(&budget), OVERHEAD + 100);
    let ledger = budget.ledger.lock().unwrap();
    assert_eq!(ledger.accounts["alice"], OVERHEAD + 100);
    assert_eq!(ledger.unknown, 0);
    assert!(ledger.uncertain);
    drop(ledger);
    assert!(reserve(&budget, "bob", "other", 0).is_err());
    assert_eq!(fs::read(&source).unwrap(), b"old");
    assert!(record_path(&source).is_file());
}

#[test]
fn unshared_preparation_cleanup_cannot_release_uncertain_budget() {
    let root = crate::tests::private_tempdir();
    let budget = Budget::recover(root.path(), limits(4 * OVERHEAD, 4 * OVERHEAD)).unwrap();
    let lease = reserve(&budget, "alice", "copy", 100).unwrap();
    fs::write(&lease.source, b"unshared preparation").unwrap();
    drop(Pending {
        budget: &budget,
        complete: false,
    });
    // Open's error cleanup owns this newly created source: no FD was handed out.
    fs::remove_file(&lease.source).unwrap();
    drop(lease);
    assert_eq!(total(&budget), OVERHEAD + 100);
    assert!(root.path().join("copy.budget.json").is_file());
    assert!(reserve(&budget, "bob", "other", 0).is_err());
}

#[test]
fn failed_budget_cleanup_keeps_accounted_high_water() {
    let root = crate::tests::private_tempdir();
    let budget = Budget::recover(root.path(), limits(4 * OVERHEAD, 4 * OVERHEAD)).unwrap();
    let lease = reserve(&budget, "alice", "copy", 100).unwrap();
    fs::remove_file(record_path(&lease.source)).unwrap();
    fs::create_dir(record_path(&lease.source)).unwrap();
    drop(lease);
    assert_eq!(total(&budget), OVERHEAD + 100);
    assert!(root.path().join("copy.budget.json").is_dir());
}

fn grown_end(root: &Path, name: &str) -> u64 {
    let owner: Owner = storage_contract::decode_strict(
        &fs::read(root.join(format!("{name}.budget.json"))).unwrap(),
    )
    .unwrap();
    owner.admitted_end
}

#[test]
fn growth_grants_a_bounded_extent_that_later_writes_reuse() {
    const MIB: u64 = 1 << 20;
    let root = crate::tests::private_tempdir();
    let budget = Budget::recover(root.path(), limits(1 << 40, 1 << 40)).unwrap();
    let mut lease = budget
        .reserve_available("alice", &root.path().join("copy"), 0, 1 << 40, u64::MAX)
        .unwrap();
    let start = persist::record_writes().1;
    // Small ends receive the 1 MiB minimum; the grant is durable before ready.
    lease
        .before_write_available(65536, 65536, u64::MAX)
        .unwrap();
    assert_eq!(grown_end(root.path(), "copy"), 65536 + MIB);
    assert_eq!(persist::record_writes().1 - start, 1);
    for end in (2..=17).map(|index| index * 65536) {
        lease.before_write_available(end, 65536, u64::MAX).unwrap();
    }
    assert_eq!(
        persist::record_writes().1 - start,
        1,
        "writes inside the grant reuse it"
    );
    // Beyond the grant: a quarter of the new end, capped at 64 MiB.
    lease
        .before_write_available(18 * 65536, 65536, u64::MAX)
        .unwrap();
    assert_eq!(grown_end(root.path(), "copy"), 18 * 65536 + MIB);
    lease
        .before_write_available(40 * MIB, MIB, u64::MAX)
        .unwrap();
    assert_eq!(grown_end(root.path(), "copy"), 50 * MIB);
    lease
        .before_write_available(1024 * MIB, MIB, u64::MAX)
        .unwrap();
    assert_eq!(grown_end(root.path(), "copy"), 1088 * MIB);
    assert_eq!(persist::record_writes().1 - start, 4);
    assert_eq!(total(&budget), OVERHEAD + 1088 * MIB);
    assert_eq!(budget.ledger.lock().unwrap().granted, 64 * MIB);
    // A retained copy keeps the whole grant charged across restart.
    fs::write(&lease.source, b"bytes").unwrap();
    drop(lease);
    assert_eq!(budget.ledger.lock().unwrap().granted, 0);
    drop(budget);
    let budget = Budget::recover(root.path(), limits(1 << 40, 1 << 40)).unwrap();
    assert_eq!(total(&budget), OVERHEAD + 1088 * MIB);
    assert_eq!(budget.ledger.lock().unwrap().granted, 0);
}

#[test]
fn grants_never_reject_a_write_that_its_exact_end_admits() {
    const MIB: u64 = 1 << 20;
    let root = crate::tests::private_tempdir();
    // The account admits exactly 10 MiB of extent: no room for any grant.
    let budget = Budget::recover(root.path(), limits(OVERHEAD + 10 * MIB, 1 << 40)).unwrap();
    let mut lease = budget
        .reserve_available("alice", &root.path().join("copy"), 0, 1 << 30, u64::MAX)
        .unwrap();
    lease
        .before_write_available(10 * MIB, MIB, u64::MAX)
        .unwrap();
    assert_eq!(grown_end(root.path(), "copy"), 10 * MIB);
    assert!(
        lease
            .before_write_available(10 * MIB + 1, 1, u64::MAX)
            .is_err()
    );
    assert_eq!(budget.ledger.lock().unwrap().granted, 0);
    // Free space admits the exact end plus recovery headroom but not the grant.
    let other = crate::tests::private_tempdir();
    let budget = Budget::recover(other.path(), limits(1 << 40, 1 << 40)).unwrap();
    let mut lease = budget
        .reserve_available("alice", &other.path().join("copy"), 0, 1 << 30, u64::MAX)
        .unwrap();
    let exact = OVERHEAD + 4 * MIB + 1;
    lease.before_write_available(4 * MIB, MIB, exact).unwrap();
    assert_eq!(grown_end(other.path(), "copy"), 4 * MIB);
    assert_eq!(total(&budget), OVERHEAD + 4 * MIB);
}

#[test]
fn optional_grant_accounting_overflow_is_not_hidden_by_exact_end_fallback() {
    let root = crate::tests::private_tempdir();
    let budget = Budget::recover(root.path(), limits(u64::MAX, u64::MAX)).unwrap();
    let source = root.path().join("copy");
    let mut lease = budget
        .reserve_available("alice", &source, 0, u64::MAX, u64::MAX)
        .unwrap();
    fs::write(&source, b"").unwrap();
    let end = u64::MAX - OVERHEAD;
    // Admitted external u64 limits can make the optional ceiling overflow its
    // metadata charge even though the exact requested end is representable.
    budget
        .check_admission(&budget.lock_ledger(), "alice", &source, end, u64::MAX)
        .unwrap();
    let grows = persist::record_writes().1;
    let error = lease.before_write_available(end, 1, u64::MAX).unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::SmbSpoolUnavailable);
    assert_eq!(error.message, "SMB spool accounting overflow");
    assert_eq!(persist::record_writes().1, grows);
    assert_eq!(grown_end(root.path(), "copy"), 0);
    assert_eq!(total(&budget), OVERHEAD);
    assert_eq!(budget.ledger.lock().unwrap().granted, 0);
    assert_eq!(fs::read(&source).unwrap(), b"");
}

#[test]
fn every_write_inside_a_grant_still_checks_actual_space() {
    let root = crate::tests::private_tempdir();
    let budget = Budget::recover(root.path(), limits(1 << 40, 1 << 40)).unwrap();
    let mut lease = budget
        .reserve_available("alice", &root.path().join("copy"), 0, 1 << 30, u64::MAX)
        .unwrap();
    lease.before_write_available(4096, 4096, u64::MAX).unwrap();
    let start = persist::record_writes().1;
    // Overwrites and writes inside the granted extent need no growth, but the
    // bytes plus recovery headroom must still fit the current free space.
    let error = lease.before_write_available(4096, 4096, 4096).unwrap_err();
    assert_eq!(
        error.code,
        storage_contract::ErrorCode::ResourceLimitExceeded
    );
    lease.before_write_available(4096, 4096, 4097).unwrap();
    assert_eq!(persist::record_writes().1, start);
}

#[test]
fn the_live_grant_pool_bounds_outstanding_grants_and_returns_them_on_drop() {
    const MIB: u64 = 1 << 20;
    let root = crate::tests::private_tempdir();
    // bytes_total / 64 leaves a 2 MiB pool: two minimum grants.
    let budget = Budget::recover(root.path(), limits(128 * MIB, 128 * MIB)).unwrap();
    let mut leases = ["a", "b", "c"]
        .map(|name| {
            budget
                .reserve_available("alice", &root.path().join(name), 0, 1 << 30, u64::MAX)
                .unwrap()
        })
        .into_iter()
        .collect::<Vec<_>>();
    for lease in &mut leases {
        lease.before_write_available(1, 1, u64::MAX).unwrap();
    }
    assert_eq!(grown_end(root.path(), "a"), 1 + MIB);
    assert_eq!(grown_end(root.path(), "b"), 1 + MIB);
    assert_eq!(
        grown_end(root.path(), "c"),
        1,
        "an exhausted pool admits the exact end"
    );
    assert_eq!(budget.ledger.lock().unwrap().granted, 2 * MIB);
    drop(leases.remove(0));
    assert_eq!(budget.ledger.lock().unwrap().granted, MIB);
    leases[1].before_write_available(2, 1, u64::MAX).unwrap();
    assert_eq!(grown_end(root.path(), "c"), 2 + MIB);
    // Regrowing replaces, not accumulates, a lease's share of the pool.
    leases[0]
        .before_write_available(2 * MIB, MIB, u64::MAX)
        .unwrap();
    assert_eq!(grown_end(root.path(), "b"), 3 * MIB);
    assert_eq!(budget.ledger.lock().unwrap().granted, 2 * MIB);
}

#[test]
fn retained_grants_can_exceed_the_live_pool_but_remain_charged_after_restart() {
    const MIB: u64 = 1 << 20;
    const LIMIT: u64 = 128 * MIB;
    let root = crate::tests::private_tempdir();
    let budget = Budget::recover(root.path(), limits(LIMIT, LIMIT)).unwrap();
    for name in ["a", "b", "c"] {
        let mut lease = budget
            .reserve_available("alice", &root.path().join(name), 0, LIMIT, u64::MAX)
            .unwrap();
        lease.before_write_available(1, 1, u64::MAX).unwrap();
        fs::write(&lease.source, b"x").unwrap();
        drop(lease);
        assert_eq!(budget.ledger.lock().unwrap().granted, 0);
    }
    let retained = 3 * (OVERHEAD + MIB + 1);
    assert!(3 * MIB > budget.grant_pool());
    assert_eq!(total(&budget), retained);
    assert_eq!(budget.ledger.lock().unwrap().accounts["alice"], retained);
    drop(budget);
    let budget = Budget::recover(root.path(), limits(LIMIT, LIMIT)).unwrap();
    assert_eq!(total(&budget), retained);
    assert_eq!(budget.ledger.lock().unwrap().accounts["alice"], retained);
    assert_eq!(budget.ledger.lock().unwrap().granted, 0);
    // Another account may use only the real remaining global reservation budget.
    let remaining = LIMIT - retained - OVERHEAD;
    let _other = reserve(&budget, "bob", "other", remaining).unwrap();
    assert_eq!(total(&budget), LIMIT);
    assert_eq!(
        reserve(&budget, "charlie", "blocked", 0)
            .err()
            .unwrap()
            .code,
        storage_contract::ErrorCode::ResourceLimitExceeded
    );
    for name in ["a", "b", "c"] {
        assert_eq!(fs::read(root.path().join(name)).unwrap(), b"x");
        assert_eq!(grown_end(root.path(), name), MIB + 1);
    }
}
