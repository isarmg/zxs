use super::*;
use std::error::Error as _;

fn disk(temp: &tempfile::TempDir) -> DiskConfig {
    let disk = Config::development([temp.path().join("disk")])
        .disks
        .remove(0);
    private_dir(log_path(&disk).parent().unwrap()).unwrap();
    disk
}
fn put(writer: &mut JournalWriter, disk: &DiskConfig, kind: RecordType, sequence: u64) -> Receipt {
    writer
        .append(
            disk,
            kind,
            sequence,
            &format!("{sequence:064x}"),
            &"b".repeat(64),
        )
        .unwrap()
}

#[test]
fn held_writer_preserves_golden_bytes_and_idempotent_current_receipts() {
    let temp = tempfile::tempdir().unwrap();
    let disk = disk(&temp);
    let mut writer = JournalWriter::create(&disk).unwrap();
    let first = put(&mut writer, &disk, RecordType::Prepare, 1);
    let accepted = put(&mut writer, &disk, RecordType::Accept, 1);
    let bytes = std::fs::read(log_path(&disk)).unwrap();
    assert_eq!(
        put(&mut writer, &disk, RecordType::Prepare, 1).frame_hash,
        first.frame_hash
    );
    assert_eq!(
        put(&mut writer, &disk, RecordType::Accept, 1).frame_hash,
        accepted.frame_hash
    );
    assert_eq!(std::fs::read(log_path(&disk)).unwrap(), bytes);
    for sequence in 2..=40 {
        let previous = writer.records.last().unwrap().1.clone();
        assert_eq!(
            put(&mut writer, &disk, RecordType::Prepare, sequence)
                .record
                .previous_frame_hash,
            previous
        );
        put(&mut writer, &disk, RecordType::Accept, sequence);
    }
    assert_eq!(writer.records.len(), 2);
    assert_eq!(scan(&disk).unwrap().records.len(), 80);
}

#[test]
fn failed_write_or_sync_requires_explicit_recovery_before_retry() {
    for fault in [JournalFault::PartialWrite, JournalFault::Sync] {
        let temp = tempfile::tempdir().unwrap();
        let disk = disk(&temp);
        let mut writer = JournalWriter::create(&disk).unwrap();
        put(&mut writer, &disk, RecordType::Prepare, 1);
        writer.fault = Some(fault);
        let failed = writer.append(
            &disk,
            RecordType::Accept,
            1,
            &format!("{:064x}", 1),
            &"b".repeat(64),
        );
        assert!(failed.is_err());
        assert!(writer.needs_recovery());
        let bytes = std::fs::read(log_path(&disk)).unwrap();
        assert_eq!(
            writer
                .append(
                    &disk,
                    RecordType::Accept,
                    1,
                    &format!("{:064x}", 1),
                    &"b".repeat(64)
                )
                .unwrap_err()
                .code(),
            ErrorCode::RecoveryRequired
        );
        assert_eq!(std::fs::read(log_path(&disk)).unwrap(), bytes);
        drop(writer);
        let opened = OpenJournal::scan(&disk).unwrap();
        assert_eq!(
            opened.scan.torn_tail,
            matches!(fault, JournalFault::PartialWrite)
        );
        // Scan alone is read-only, including a damaged suffix.
        assert_eq!(std::fs::read(log_path(&disk)).unwrap(), bytes);
        let mut writer = opened.into_writer(&disk).unwrap();
        writer.repair_torn_tail().unwrap();
        let receipt = put(&mut writer, &disk, RecordType::Accept, 1);
        assert!(
            receipt_binds(
                &receipt,
                RecordType::Accept,
                1,
                &format!("{:064x}", 1),
                &"b".repeat(64)
            )
            .unwrap()
        );
        let scanned = scan(&disk).unwrap();
        assert!(!scanned.torn_tail);
        assert_eq!(scanned.records.len(), 2);
    }
}

#[test]
fn real_enospc_blocks_engine_before_first_prepare_and_preserves_publication() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("system");
    let config = Config::development([temp.path().join("disk")]);
    let disk = config.disks[0].clone();
    let mut engine = Engine::initialize(&root, config).unwrap();
    engine.journals.get_mut(&disk.disk_id).unwrap().file = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let error = engine
        .create_repository("failed", "P01", 1, "admin", &new_id("op"))
        .unwrap_err();
    assert!(error.source().is_some());
    assert!(engine.is_read_only());
    assert!(engine.state().repositories.is_empty());
    assert!(scan(&disk).unwrap().records.is_empty());
    drop(engine);
    let mut recovered = Engine::open(&root).unwrap();
    assert!(!recovered.is_read_only());
    recovered
        .create_repository("saved", "P01", 1, "admin", &new_id("op"))
        .unwrap();
    assert_eq!(scan(&disk).unwrap().records.len(), 4);
}

#[test]
fn full_recovery_keeps_historical_receipts_and_wire_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("system");
    let config = Config::development((0..3).map(|i| temp.path().join(format!("disk-{i}"))));
    let mut engine = Engine::initialize(&root, config.clone()).unwrap();
    for i in 0..6 {
        engine
            .create_repository(&format!("repo-{i}"), "P01", 1, "admin", &new_id("op"))
            .unwrap();
    }
    let expected = canonical_json(engine.state()).unwrap();
    let bytes: Vec<_> = config
        .disks
        .iter()
        .map(|disk| std::fs::read(log_path(disk)).unwrap())
        .collect();
    drop(engine);
    let mut recovered = Engine::open(&root).unwrap();
    assert_eq!(canonical_json(recovered.state()).unwrap(), expected);
    assert!(!recovered.is_read_only());
    for (disk, bytes) in config.disks.iter().zip(bytes) {
        assert_eq!(std::fs::read(log_path(disk)).unwrap(), bytes);
    }
    recovered
        .create_repository("after-recovery", "P01", 1, "admin", &new_id("op"))
        .unwrap();
    drop(recovered);
    assert_eq!(Engine::open(&root).unwrap().state().repositories.len(), 7);
}

#[test]
fn prepare_io_failure_requires_reopen_and_retry_keeps_operation_identity() {
    for fault in [JournalFault::PartialWrite, JournalFault::Sync] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("system");
        let config = Config::development([temp.path().join("disk")]);
        let mut engine = Engine::initialize(&root, config).unwrap();
        let operation = new_id("op");
        engine.inject_journal_fault_once("P01", fault).unwrap();
        assert!(
            engine
                .create_repository("retry", "P01", 1, "admin", &operation)
                .is_err()
        );
        assert!(engine.is_read_only());
        assert!(engine.state().operations.is_empty());
        assert!(
            engine
                .create_repository("retry", "P01", 1, "admin", &operation)
                .is_err()
        );
        drop(engine);
        let mut recovered = Engine::open(&root).unwrap();
        let repo = recovered
            .create_repository("retry", "P01", 1, "admin", &operation)
            .unwrap();
        assert_eq!(recovered.state().repositories.len(), 1);
        assert_eq!(
            recovered
                .create_repository("retry", "P01", 1, "admin", &operation)
                .unwrap()
                .repository_id,
            repo.repository_id
        );
        drop(recovered);
        let recovered = Engine::open(&root).unwrap();
        assert_eq!(recovered.state().operations.len(), 1);
        assert!(recovered.operation(&operation).is_ok());
    }
}

#[test]
fn recovery_modes_preserve_evidence_or_repair_the_same_torn_tail() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("system");
    let config = Config::development([temp.path().join("disk")]);
    let disk = config.disks[0].clone();
    let mut engine = Engine::initialize(&root, config).unwrap();
    engine
        .create_repository("published", "P01", 1, "admin", &new_id("op"))
        .unwrap();
    drop(engine);
    let path = log_path(&disk);
    let valid = std::fs::read(&path).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    file.write_all(b"ZXSL").unwrap();
    file.sync_all().unwrap();
    drop(file);
    let torn = std::fs::read(&path).unwrap();
    let preserving = Engine::open_for_recovery(&root).unwrap();
    assert!(preserving.is_read_only());
    assert_eq!(preserving.state().repositories.len(), 1);
    assert_eq!(std::fs::read(&path).unwrap(), torn);
    drop(preserving);
    let mut recovered = Engine::open(&root).unwrap();
    assert!(!recovered.is_read_only());
    assert_eq!(std::fs::read(&path).unwrap(), valid);
    recovered
        .create_repository("next", "P01", 1, "admin", &new_id("op"))
        .unwrap();
    assert!(!scan(&disk).unwrap().torn_tail);
    assert_eq!(scan(&disk).unwrap().records.len(), 8);
}

#[test]
fn leftover_created_leaf_requires_both_opening_durability_fences() {
    for fault in [OpeningFault::FileSync, OpeningFault::DirectorySync] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("system");
        let config = Config::development([temp.path().join("disk")]);
        let disk = config.disks[0].clone();
        drop(Engine::initialize(&root, config).unwrap());
        let path = log_path(&disk);
        let parent = path.parent().unwrap();
        std::fs::remove_file(&path).unwrap();
        sync_dir(parent).unwrap();
        let file = create_journal(&path).unwrap();
        let error = sync_open_journal(&file, parent, Some(fault)).unwrap_err();
        assert!(error.source().is_some());
        drop(file);
        assert!(path.is_file());

        let mut opened = OpenJournal::scan(&disk).unwrap();
        opened.opening_fault = Some(OpeningFault::DirectorySync);
        let error = opened.into_writer(&disk).err().unwrap();
        assert!(error.source().is_some());
        assert!(scan(&disk).unwrap().records.is_empty());

        let mut recovered = Engine::open(&root).unwrap();
        assert!(!recovered.is_read_only());
        recovered
            .create_repository("durable", "P01", 1, "admin", &new_id("op"))
            .unwrap();
        drop(recovered);
        assert_eq!(Engine::open(&root).unwrap().state().repositories.len(), 1);
    }
}
