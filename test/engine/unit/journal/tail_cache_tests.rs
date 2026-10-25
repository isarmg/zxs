use super::*;
use std::io::{Seek, SeekFrom};

fn disk(temp: &tempfile::TempDir) -> DiskConfig {
    Config::development([
        temp.path().join("one"),
        temp.path().join("two"),
        temp.path().join("three"),
    ])
    .disks
    .remove(0)
}
fn put(d: &DiskConfig, kind: RecordType, sequence: u64) -> Receipt {
    append(
        d,
        kind,
        sequence,
        &format!("{sequence:064x}"),
        &"b".repeat(64),
    )
    .unwrap()
}

#[test]
fn unnumbered_frame_header_binds_length_payload_and_receipt() {
    let temp = tempfile::tempdir().unwrap();
    let d = disk(&temp);
    let receipt = put(&d, RecordType::Prepare, 1);
    let bytes = std::fs::read(log_path(&d)).unwrap();
    let payload = canonical_json(&receipt.record).unwrap();
    assert_eq!(&bytes[..8], LOG_MAGIC);
    assert_eq!(
        u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize,
        payload.len()
    );
    assert_eq!(bytes.len(), LOG_HEADER_BYTES + payload.len() + 32);
    assert_eq!(
        &bytes[LOG_HEADER_BYTES..LOG_HEADER_BYTES + payload.len()],
        payload
    );
    assert!(
        receipt_binds(
            &receipt,
            RecordType::Prepare,
            1,
            &format!("{:064x}", 1),
            &"b".repeat(64)
        )
        .unwrap()
    );
    assert_eq!(scan(&d).unwrap().valid_bytes, bytes.len() as u64);
    let mut invalid = bytes;
    invalid[..8].copy_from_slice(b"ZXSLOG2\0");
    std::fs::write(log_path(&d), &invalid).unwrap();
    assert_eq!(
        scan(&d).err().unwrap().code(),
        storage_contract::ErrorCode::LogCorrupt
    );
    invalid[..8].copy_from_slice(LOG_MAGIC);
    invalid[8..12].copy_from_slice(&((MAX_FRAME + 1) as u32).to_le_bytes());
    std::fs::write(log_path(&d), invalid).unwrap();
    assert!(scan(&d).is_err());
}

#[test]
fn latest_receipts_are_idempotent_and_old_retries_rescan() {
    let temp = tempfile::tempdir().unwrap();
    let d = disk(&temp);
    let first = put(&d, RecordType::Prepare, 1);
    let latest = put(&d, RecordType::Accept, 1);
    assert_eq!(put(&d, RecordType::Prepare, 1).frame_hash, first.frame_hash);
    assert_eq!(put(&d, RecordType::Accept, 1).frame_hash, latest.frame_hash);
    put(&d, RecordType::Prepare, 2);
    let len = std::fs::metadata(log_path(&d)).unwrap().len();
    let before = SCAN_COUNT.with(|count| count.get());
    assert_eq!(put(&d, RecordType::Prepare, 1).frame_hash, first.frame_hash);
    assert_eq!(SCAN_COUNT.with(|count| count.get()) - before, 1);
    assert_eq!(std::fs::metadata(log_path(&d)).unwrap().len(), len);
    assert_eq!(scan(&d).unwrap().records.len(), 3);
    assert_eq!(
        append(
            &d,
            RecordType::Finalize,
            1,
            &format!("{:064x}", 1),
            &"b".repeat(64)
        )
        .unwrap_err()
        .code(),
        storage_contract::ErrorCode::LogFork
    );
}

#[test]
fn visible_publication_retry_repeats_failed_durability_barrier() {
    let temp = tempfile::tempdir().unwrap();
    let d = disk(&temp);
    let candidate = "a".repeat(64);
    let object = "b".repeat(64);
    let failure = append_with_sync(
        &d,
        RecordType::Publication,
        1,
        &candidate,
        &object,
        None,
        |_, _| {
            Err(Error::new(
                storage_contract::ErrorCode::InjectedFailure,
                "frame written, file sync failed",
            ))
        },
    );
    assert_eq!(
        failure.unwrap_err().code(),
        storage_contract::ErrorCode::InjectedFailure
    );
    assert_eq!(scan(&d).unwrap().records.len(), 1);
    let length = std::fs::metadata(log_path(&d)).unwrap().len();

    let mut repeated = false;
    let failure = append_with_sync(
        &d,
        RecordType::Publication,
        1,
        &candidate,
        &object,
        None,
        |file, _| {
            repeated = true;
            file.sync_all()?;
            Err(Error::new(
                storage_contract::ErrorCode::InjectedFailure,
                "parent sync failed",
            ))
        },
    );
    assert!(repeated, "visible matching frames must still synchronize");
    assert_eq!(
        failure.unwrap_err().code(),
        storage_contract::ErrorCode::InjectedFailure
    );
    assert!(!tails().lock().unwrap().contains_key(&log_path(&d)));

    let receipt = append(&d, RecordType::Publication, 1, &candidate, &object).unwrap();
    assert!(receipt_binds(&receipt, RecordType::Publication, 1, &candidate, &object).unwrap());
    assert_eq!(std::fs::metadata(log_path(&d)).unwrap().len(), length);
    assert_eq!(scan(&d).unwrap().records.len(), 1);
}

#[test]
fn cached_append_detects_same_length_prefix_corruption() {
    let temp = tempfile::tempdir().unwrap();
    let d = disk(&temp);
    put(&d, RecordType::Prepare, 1);
    put(&d, RecordType::Prepare, 2);
    let path = log_path(&d);
    let old = std::fs::metadata(&path).unwrap();
    let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.seek(SeekFrom::Start(20)).unwrap();
    file.write_all(b"!").unwrap();
    file.set_times(std::fs::FileTimes::new().set_modified(old.modified().unwrap()))
        .unwrap();
    file.sync_all().unwrap();
    assert_eq!(
        append(
            &d,
            RecordType::Accept,
            2,
            &format!("{:064x}", 2),
            &"b".repeat(64)
        )
        .unwrap_err()
        .code(),
        storage_contract::ErrorCode::LogCorrupt
    );
}

#[test]
fn cached_append_repairs_torn_external_tail() {
    let temp = tempfile::tempdir().unwrap();
    let d = disk(&temp);
    put(&d, RecordType::Prepare, 1);
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(log_path(&d))
        .unwrap();
    file.write_all(b"ZXSLO").unwrap();
    file.sync_all().unwrap();
    put(&d, RecordType::Accept, 1);
    let scanned = scan(&d).unwrap();
    assert!(!scanned.torn_tail);
    assert_eq!(scanned.records.len(), 2);
}

#[test]
fn replacement_and_truncation_invalidate_cached_receipts() {
    let temp = tempfile::tempdir().unwrap();
    let d = disk(&temp);
    put(&d, RecordType::Prepare, 1);
    let old = put(&d, RecordType::Accept, 1);
    let path = log_path(&d);
    std::fs::rename(&path, path.with_extension("old")).unwrap();
    let fresh = put(&d, RecordType::Accept, 1);
    assert_ne!(fresh.frame_hash, old.frame_hash);
    assert_eq!(scan(&d).unwrap().records.len(), 1);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(0)
        .unwrap();
    put(&d, RecordType::Prepare, 2);
    let scanned = scan(&d).unwrap();
    assert_eq!(scanned.records.len(), 1);
    assert_eq!(scanned.records[0].0.sequence, 2);
}

#[test]
fn cold_append_revalidates_chain_and_cache_retains_only_latest_slot() {
    // Keep this key after ordinary .tmp* fixtures in deterministic eviction
    // order, so unrelated parallel tests cannot evict this measured tail.
    let temp = tempfile::Builder::new()
        .prefix("zzzz-tail-")
        .tempdir()
        .unwrap();
    let d = disk(&temp);
    let before = SCAN_COUNT.with(|count| count.get());
    for sequence in 1..=40 {
        put(&d, RecordType::Prepare, sequence);
        put(&d, RecordType::Accept, sequence);
    }
    assert_eq!(
        SCAN_COUNT.with(|count| count.get()) - before,
        1,
        "ordinary appends must scan only the initial cold journal"
    );
    {
        let mut cache = tails().lock().unwrap();
        let tail = cache.remove(&log_path(&d)).unwrap();
        assert_eq!(tail.records.len(), 2);
        assert_eq!(tail.records.capacity(), 2);
        assert!(tail.records.iter().all(|(r, _)| r.sequence == 40));
    }
    put(&d, RecordType::Finalize, 40);
    assert_eq!(
        SCAN_COUNT.with(|count| count.get()) - before,
        2,
        "evicted/cold state must verify the full journal again"
    );
    assert_eq!(scan(&d).unwrap().records.len(), 81);
}

#[test]
fn full_recovery_preserves_publications_with_bounded_journal_scans() {
    for publications in [2, 6] {
        // Keep these measured journals after ordinary .tmp* fixtures in the
        // cache eviction order, as in the cold-append test above.
        let temp = tempfile::Builder::new()
            .prefix("zzzz-replay-scan-")
            .tempdir()
            .unwrap();
        let root = temp.path().join("system");
        let config = Config::development([
            temp.path().join("one"),
            temp.path().join("two"),
            temp.path().join("three"),
        ]);
        let mut engine = Engine::initialize(&root, config.clone()).unwrap();
        for n in 0..publications {
            engine
                .create_repository(&format!("repo-{n}"), "P01", 3, "admin", &new_id("op"))
                .unwrap();
        }
        let expected = engine.state().clone();
        drop(engine);
        let members: Vec<_> = config.disks.iter().filter(|d| d.metadata_member).collect();
        let journals: Vec<_> = members
            .iter()
            .map(|d| {
                assert!(!d.root.join("metadata/replay-checkpoint.json").exists());
                let scanned = scan(d).unwrap();
                assert!(!scanned.torn_tail);
                assert_eq!(
                    scanned
                        .records
                        .iter()
                        .filter(|(r, _)| r.record_type == RecordType::Publication)
                        .count(),
                    publications
                );
                std::fs::read(log_path(d)).unwrap()
            })
            .collect();
        // Simulate a fresh process, without changing any journal bytes.
        {
            let mut cache = tails().lock().unwrap();
            for d in &members {
                cache.remove(&log_path(d));
            }
        }
        let before = SCAN_COUNT.with(|count| count.get());
        let recovered = Engine::open(&root).unwrap();
        let scans = SCAN_COUNT.with(|count| count.get()) - before;
        eprintln!(
            "full recovery: members={} publications={publications} journal_scans={scans}",
            members.len()
        );
        assert_eq!(recovered.state(), &expected);
        assert!(!recovered.is_read_only());
        for (d, bytes) in members.iter().zip(journals) {
            assert_eq!(std::fs::read(log_path(d)).unwrap(), bytes);
        }
        // Initial verification and checkpoint creation each scan every member.
        // Reusing same-recovery publication evidence must not add scans per slot.
        assert!(scans >= members.len(), "recovery must verify every journal");
        assert!(scans <= members.len() * 2, "unexpected scans: {scans}");
    }
}

fn scanned_receipt<'a>(
    scan: &'a Scan,
    hashes: &'a BTreeSet<String>,
    record: &'a Record,
) -> Option<ScannedReceipt<'a>> {
    scan.stamp.as_ref().map(|stamp| ScannedReceipt {
        stamp,
        hashes,
        record,
    })
}

#[test]
fn replayed_old_receipt_preserves_bytes_and_never_seeds_latest_tail() {
    let temp = tempfile::tempdir().unwrap();
    let d = disk(&temp);
    let first = put(&d, RecordType::Publication, 1);
    let last = put(&d, RecordType::Publication, 2);
    let scanned = scan(&d).unwrap();
    let hashes = scanned.records.iter().map(|(_, h)| h.clone()).collect();
    let bytes = std::fs::read(log_path(&d)).unwrap();
    tails().lock().unwrap().remove(&log_path(&d));
    let before = SCAN_COUNT.with(|count| count.get());
    let receipt = replay_publication(
        &d,
        1,
        &first.record.candidate_hash,
        &first.record.object_hash,
        scanned_receipt(&scanned, &hashes, &scanned.records[0].0),
    )
    .unwrap();
    assert_eq!(
        canonical_json(&receipt).unwrap(),
        canonical_json(&first).unwrap()
    );
    assert_eq!(std::fs::read(log_path(&d)).unwrap(), bytes);
    assert_eq!(SCAN_COUNT.with(|count| count.get()) - before, 0);
    assert!(!tails().lock().unwrap().contains_key(&log_path(&d)));
    let next = put(&d, RecordType::Prepare, 3);
    assert_eq!(next.record.previous_frame_hash, last.frame_hash);
    assert_eq!(SCAN_COUNT.with(|count| count.get()) - before, 1);
}

#[test]
fn replay_receipt_missing_or_mismatched_evidence_uses_normal_append() {
    let temp = tempfile::tempdir().unwrap();
    let d = disk(&temp);
    let expected = put(&d, RecordType::Publication, 1);
    put(&d, RecordType::Publication, 2);
    let scanned = scan(&d).unwrap();
    let hashes = scanned.records.iter().map(|(_, h)| h.clone()).collect();
    for mismatch in 0..7 {
        let mut record = scanned.records[0].0.clone();
        match mismatch {
            0 => record.record_type = RecordType::Accept,
            1 => record.sequence = 2,
            2 => record.candidate_hash = "c".repeat(64),
            3 => record.object_hash = "d".repeat(64),
            4 => record.previous_frame_hash = "e".repeat(64),
            _ => {}
        }
        let empty = BTreeSet::new();
        let proof = if mismatch == 6 {
            None
        } else {
            scanned_receipt(
                &scanned,
                if mismatch == 5 { &empty } else { &hashes },
                &record,
            )
        };
        tails().lock().unwrap().remove(&log_path(&d));
        let before = SCAN_COUNT.with(|count| count.get());
        let receipt = replay_publication(
            &d,
            1,
            &expected.record.candidate_hash,
            &expected.record.object_hash,
            proof,
        )
        .unwrap();
        assert_eq!(receipt.frame_hash, expected.frame_hash);
        assert_eq!(SCAN_COUNT.with(|count| count.get()) - before, 1);
    }
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(log_path(&d))
        .unwrap();
    file.write_all(b"ZXSLO").unwrap();
    file.sync_all().unwrap();
    let torn = scan(&d).unwrap();
    assert!(torn.torn_tail);
    assert!(torn.stamp.is_none());
    let before = SCAN_COUNT.with(|count| count.get());
    let receipt = replay_publication(
        &d,
        1,
        &expected.record.candidate_hash,
        &expected.record.object_hash,
        scanned_receipt(&torn, &hashes, &torn.records[0].0),
    )
    .unwrap();
    assert_eq!(receipt.frame_hash, expected.frame_hash);
    assert_eq!(SCAN_COUNT.with(|count| count.get()) - before, 1);
}

#[test]
fn replay_receipt_revalidates_replaced_truncated_and_rewritten_journals() {
    for change in 0..3 {
        let temp = tempfile::tempdir().unwrap();
        let d = disk(&temp);
        let expected = put(&d, RecordType::Publication, 1);
        let prefix_bytes = std::fs::metadata(log_path(&d)).unwrap().len();
        put(&d, RecordType::Publication, 2);
        let scanned = scan(&d).unwrap();
        let hashes = scanned.records.iter().map(|(_, h)| h.clone()).collect();
        let path = log_path(&d);
        match change {
            0 => {
                let bytes = std::fs::read(&path).unwrap();
                std::fs::rename(&path, path.with_extension("old")).unwrap();
                let mut file = open_append_regular(&path).unwrap();
                file.write_all(&bytes).unwrap();
                file.sync_all().unwrap();
            }
            1 => std::fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .unwrap()
                .set_len(prefix_bytes)
                .unwrap(),
            _ => {
                let old = std::fs::metadata(&path).unwrap();
                let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
                file.seek(SeekFrom::Start(20)).unwrap();
                file.write_all(b"!").unwrap();
                file.set_times(std::fs::FileTimes::new().set_modified(old.modified().unwrap()))
                    .unwrap();
                file.sync_all().unwrap();
            }
        }
        let before = SCAN_COUNT.with(|count| count.get());
        let result = replay_publication(
            &d,
            1,
            &expected.record.candidate_hash,
            &expected.record.object_hash,
            scanned_receipt(&scanned, &hashes, &scanned.records[0].0),
        );
        if change == 2 {
            assert_eq!(
                result.unwrap_err().code(),
                storage_contract::ErrorCode::LogCorrupt
            );
        } else {
            assert_eq!(result.unwrap().frame_hash, expected.frame_hash);
        }
        assert_eq!(SCAN_COUNT.with(|count| count.get()) - before, 1);
    }
}

#[test]
fn replay_receipt_repeats_both_durability_barriers_after_failures() {
    let temp = tempfile::tempdir().unwrap();
    let d = disk(&temp);
    let expected = put(&d, RecordType::Publication, 1);
    let scanned = scan(&d).unwrap();
    let hashes = scanned.records.iter().map(|(_, h)| h.clone()).collect();
    let bytes = std::fs::read(log_path(&d)).unwrap();
    let before = SCAN_COUNT.with(|count| count.get());
    for stage in 0..3 {
        let mut synchronized = false;
        let result = append_with_sync(
            &d,
            RecordType::Publication,
            1,
            &expected.record.candidate_hash,
            &expected.record.object_hash,
            scanned_receipt(&scanned, &hashes, &scanned.records[0].0),
            |file, parent| {
                synchronized = true;
                if stage == 0 {
                    return Err(Error::new(
                        storage_contract::ErrorCode::InjectedFailure,
                        "file sync failed",
                    ));
                }
                file.sync_all()?;
                if stage == 1 {
                    return Err(Error::new(
                        storage_contract::ErrorCode::InjectedFailure,
                        "directory sync failed",
                    ));
                }
                sync_dir(parent)
            },
        );
        assert!(synchronized);
        if stage < 2 {
            assert_eq!(
                result.unwrap_err().code(),
                storage_contract::ErrorCode::InjectedFailure
            );
        } else {
            assert_eq!(result.unwrap().frame_hash, expected.frame_hash);
        }
        assert!(!tails().lock().unwrap().contains_key(&log_path(&d)));
        assert_eq!(std::fs::read(log_path(&d)).unwrap(), bytes);
    }
    assert_eq!(SCAN_COUNT.with(|count| count.get()) - before, 0);
}

#[test]
fn replay_receipt_rechecks_descriptor_and_path_after_synchronization() {
    for replace in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let d = disk(&temp);
        let expected = put(&d, RecordType::Publication, 1);
        let scanned = scan(&d).unwrap();
        let hashes = scanned.records.iter().map(|(_, h)| h.clone()).collect();
        let path = log_path(&d);
        let result = append_with_sync(
            &d,
            RecordType::Publication,
            1,
            &expected.record.candidate_hash,
            &expected.record.object_hash,
            scanned_receipt(&scanned, &hashes, &scanned.records[0].0),
            |file, parent| {
                sync_journal(file, parent)?;
                if replace {
                    let bytes = std::fs::read(&path)?;
                    std::fs::rename(&path, path.with_extension("old"))?;
                    open_append_regular(&path)?.write_all(&bytes)?;
                } else {
                    open_append_regular(&path)?.write_all(b"ZXSLO")?;
                }
                Ok(())
            },
        );
        assert_eq!(
            result.unwrap_err().code(),
            storage_contract::ErrorCode::RecoveryRequired
        );
        assert!(!tails().lock().unwrap().contains_key(&path));
    }
}
