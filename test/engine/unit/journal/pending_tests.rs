use super::*;

fn setup() -> (tempfile::TempDir, DiskConfig, DiskRoot, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let disk = Config::development([temp.path().join("disk")])
        .unwrap()
        .disks
        .remove(0);
    let directory = disk.root.join("metadata/membership-pending/test");
    private_dir(&directory).unwrap();
    private_dir(log_path(&disk).parent().unwrap()).unwrap();
    let root = DiskRoot::open(&disk.root).unwrap();
    (temp, disk, root, directory)
}
fn pending(root: &DiskRoot, directory: &Path) -> PendingJournal {
    PendingJournal::open_at(root, directory, 7, &"a".repeat(64)).unwrap()
}
fn frame(record: &Record) -> Vec<u8> {
    let payload = canonical_json(record).unwrap();
    let mut bytes = Vec::from(LOG_MAGIC.as_slice());
    bytes.extend((payload.len() as u32).to_le_bytes());
    bytes.extend(payload);
    let digest = blake3::hash(&bytes);
    bytes.extend(digest.as_bytes());
    bytes
}

#[test]
fn pending_scan_is_read_only_and_reopening_same_object_is_idempotent() {
    let (_temp, disk, root, directory) = setup();
    let path = directory.join("journal.bin");
    let mut journal = pending(&root, &directory);
    assert!(!path.exists());
    let receipt = journal.synchronize(&disk).unwrap();
    assert_eq!(receipt.record.previous_frame_hash, "0".repeat(64));
    assert_eq!(
        receipt.record.record_type,
        RecordType::MembershipControlBootstrap
    );
    assert!(
        receipt_binds(
            &receipt,
            RecordType::MembershipControlBootstrap,
            7,
            &"a".repeat(64),
            &"a".repeat(64)
        )
        .unwrap()
    );
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(
        journal.synchronize(&disk).unwrap().frame_hash,
        receipt.frame_hash
    );
    drop(journal);
    assert_eq!(
        pending(&root, &directory)
            .synchronize(&disk)
            .unwrap()
            .frame_hash,
        receipt.frame_hash
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert!(!log_path(&disk).exists());
    assert!(PendingJournal::open_at(&root, &directory, 8, &"a".repeat(64)).is_err());
    assert!(PendingJournal::open_at(&root, &directory, 7, &"b".repeat(64)).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn active_and_pending_namespaces_reject_each_others_records() {
    let (_temp, disk, root, directory) = setup();
    pending(&root, &directory).synchronize(&disk).unwrap();
    let bootstrap = std::fs::read(directory.join("journal.bin")).unwrap();
    assert!(scan_frames(bootstrap.as_slice()).is_err());
    std::fs::write(log_path(&disk), &bootstrap).unwrap();
    assert!(scan_at(&root, &disk).is_err());
    assert!(OpenJournal::scan_at(&root, &disk).is_err());
    std::fs::remove_file(log_path(&disk)).unwrap();
    let mut active = JournalWriter::create_at(&root, &disk).unwrap();
    for recovered in [false, true] {
        let result = if recovered {
            active.append_recovered(
                &disk,
                RecordType::MembershipControlBootstrap,
                7,
                &"a".repeat(64),
                &"a".repeat(64),
            )
        } else {
            active.append(
                &disk,
                RecordType::MembershipControlBootstrap,
                7,
                &"a".repeat(64),
                &"a".repeat(64),
            )
        };
        assert!(result.is_err());
    }
    assert!(std::fs::read(log_path(&disk)).unwrap().is_empty());
    for kind in [
        RecordType::Prepare,
        RecordType::Accept,
        RecordType::Finalize,
        RecordType::Publication,
        RecordType::Reprotect,
        RecordType::MembershipFence,
    ] {
        let record = Record {
            record_type: kind,
            sequence: 7,
            candidate_hash: "a".repeat(64),
            object_hash: "a".repeat(64),
            previous_frame_hash: "0".repeat(64),
        };
        let bytes = frame(&record);
        std::fs::write(directory.join("journal.bin"), &bytes).unwrap();
        assert!(PendingJournal::open_at(&root, &directory, 7, &"a".repeat(64)).is_err());
        assert_eq!(std::fs::read(directory.join("journal.bin")).unwrap(), bytes);
    }
}

#[test]
fn pending_rejects_second_frame_partial_suffix_and_nonzero_previous_hash() {
    let (_temp, disk, root, directory) = setup();
    let receipt = pending(&root, &directory).synchronize(&disk).unwrap();
    let first = frame(&receipt.record);
    let mut second = receipt.record.clone();
    second.previous_frame_hash = receipt.frame_hash;
    for suffix in [frame(&second), b"ZXSL".to_vec()] {
        let mut bytes = first.clone();
        bytes.extend(suffix);
        std::fs::write(directory.join("journal.bin"), &bytes).unwrap();
        assert!(PendingJournal::open_at(&root, &directory, 7, &"a".repeat(64)).is_err());
        assert_eq!(std::fs::read(directory.join("journal.bin")).unwrap(), bytes);
    }
    std::fs::write(directory.join("journal.bin"), frame(&second)).unwrap();
    assert!(PendingJournal::open_at(&root, &directory, 7, &"a".repeat(64)).is_err());
}

#[cfg(feature = "fault-injection")]
#[test]
fn pending_partial_append_and_failed_sync_require_reopen() {
    for fault in [JournalFault::PartialWrite, JournalFault::Sync] {
        let (_temp, disk, root, directory) = setup();
        let mut journal = pending(&root, &directory);
        journal.fault = Some(fault);
        assert!(journal.synchronize(&disk).is_err());
        let bytes = std::fs::read(directory.join("journal.bin")).unwrap();
        assert_eq!(
            journal.synchronize(&disk).unwrap_err().code(),
            ErrorCode::RecoveryRequired
        );
        assert_eq!(std::fs::read(directory.join("journal.bin")).unwrap(), bytes);
        let mut reopened = pending(&root, &directory);
        assert_eq!(std::fs::read(directory.join("journal.bin")).unwrap(), bytes);
        let receipt = reopened.synchronize(&disk).unwrap();
        assert_eq!(
            std::fs::read(directory.join("journal.bin")).unwrap(),
            frame(&receipt.record)
        );
        assert_eq!(
            pending(&root, &directory)
                .synchronize(&disk)
                .unwrap()
                .frame_hash,
            receipt.frame_hash
        );
    }
}

#[cfg(feature = "fault-injection")]
#[test]
fn pending_reuse_and_creation_require_file_and_directory_fences() {
    for existing in [false, true] {
        for fault in [OpeningFault::FileSync, OpeningFault::DirectorySync] {
            let (_temp, disk, root, directory) = setup();
            if existing {
                pending(&root, &directory).synchronize(&disk).unwrap();
            }
            let mut journal = pending(&root, &directory);
            journal.opening_fault = Some(fault);
            assert!(journal.synchronize(&disk).is_err());
            assert_eq!(
                journal.synchronize(&disk).unwrap_err().code(),
                ErrorCode::RecoveryRequired
            );
            let mut reopened = pending(&root, &directory);
            let receipt = reopened.synchronize(&disk).unwrap();
            assert_eq!(
                std::fs::read(directory.join("journal.bin")).unwrap(),
                frame(&receipt.record)
            );
            reopened.opening_fault = Some(fault);
            assert!(reopened.synchronize(&disk).is_err());
            assert_eq!(
                reopened.synchronize(&disk).unwrap_err().code(),
                ErrorCode::RecoveryRequired
            );
            assert_eq!(
                pending(&root, &directory)
                    .synchronize(&disk)
                    .unwrap()
                    .frame_hash,
                receipt.frame_hash
            );
        }
    }
}

#[test]
fn pending_writer_cannot_append_business_records_or_a_second_binding() {
    let (_temp, disk, root, directory) = setup();
    let opened = OpenJournal::scan_directory(
        root.journal_directory(&directory).unwrap(),
        JournalNamespace::Pending,
    )
    .unwrap();
    let (mut writer, _) = opened.into_writer().unwrap();
    assert!(
        writer
            .append_mode(
                &disk,
                RecordType::Prepare,
                7,
                &"a".repeat(64),
                &"a".repeat(64),
                AppendMode::Pending
            )
            .is_err()
    );
    assert!(
        writer
            .append(
                &disk,
                RecordType::MembershipControlBootstrap,
                7,
                &"a".repeat(64),
                &"a".repeat(64)
            )
            .is_err()
    );
    assert!(
        writer
            .append_recovered(
                &disk,
                RecordType::MembershipControlBootstrap,
                7,
                &"a".repeat(64),
                &"a".repeat(64)
            )
            .is_err()
    );
    writer
        .append_mode(
            &disk,
            RecordType::MembershipControlBootstrap,
            7,
            &"a".repeat(64),
            &"a".repeat(64),
            AppendMode::Pending,
        )
        .unwrap();
    let bytes = std::fs::read(directory.join("journal.bin")).unwrap();
    assert!(
        writer
            .append_mode(
                &disk,
                RecordType::MembershipControlBootstrap,
                8,
                &"b".repeat(64),
                &"b".repeat(64),
                AppendMode::Pending
            )
            .is_err()
    );
    assert_eq!(std::fs::read(directory.join("journal.bin")).unwrap(), bytes);
}

#[test]
fn pending_record_type_has_one_strict_token_and_bound_candidate() {
    assert_eq!(
        canonical_json(&RecordType::MembershipControlBootstrap).unwrap(),
        br#""MEMBERSHIP_CONTROL_BOOTSTRAP""#
    );
    for token in [
        "membership_control_bootstrap",
        "MembershipControlBootstrap",
        "MEMBERSHIPCONTROLBOOTSTRAP",
    ] {
        assert!(decode_strict::<RecordType>(&canonical_json(token).unwrap()).is_err());
    }
    let (_temp, disk, root, directory) = setup();
    let mut record = pending(&root, &directory)
        .synchronize(&disk)
        .unwrap()
        .record;
    record.candidate_hash = "b".repeat(64);
    let bytes = frame(&record);
    std::fs::write(directory.join("journal.bin"), &bytes).unwrap();
    assert!(PendingJournal::open_at(&root, &directory, 7, &"a".repeat(64)).is_err());
    assert_eq!(std::fs::read(directory.join("journal.bin")).unwrap(), bytes);
}

#[test]
fn joint_pending_and_active_namespaces_are_exhaustively_disjoint() {
    let (_temp, disk, root, directory) = setup();
    let proposal = "a".repeat(64);
    let acceptance = "b".repeat(64);
    let mut joint =
        PendingJournal::open_joint_at(&root, &directory, 7, &proposal, &acceptance).unwrap();
    assert!(joint.require_complete().is_err());
    let receipt = joint.synchronize(&disk).unwrap();
    assert_eq!(
        receipt.record.record_type,
        RecordType::MembershipJointAccept
    );
    let joint_bytes = std::fs::read(directory.join("journal.bin")).unwrap();
    assert!(scan_frames(joint_bytes.as_slice()).is_err());
    assert!(PendingJournal::open_at(&root, &directory, 7, &acceptance).is_err());
    assert!(PendingJournal::open_joint_at(&root, &directory, 7, &acceptance, &proposal).is_err());
    let mut active = JournalWriter::create_at(&root, &disk).unwrap();
    assert!(
        active
            .append(
                &disk,
                RecordType::MembershipJointAccept,
                7,
                &proposal,
                &acceptance
            )
            .is_err()
    );
    assert!(
        active
            .append_recovered(
                &disk,
                RecordType::MembershipJointAccept,
                7,
                &proposal,
                &acceptance
            )
            .is_err()
    );
    for kind in [
        RecordType::Prepare,
        RecordType::Accept,
        RecordType::Finalize,
        RecordType::Publication,
        RecordType::Reprotect,
        RecordType::MembershipFence,
        RecordType::MembershipControlBootstrap,
    ] {
        let mut record = receipt.record.clone();
        record.record_type = kind;
        let bytes = frame(&record);
        std::fs::write(directory.join("journal.bin"), &bytes).unwrap();
        assert!(
            PendingJournal::open_joint_at(&root, &directory, 7, &proposal, &acceptance).is_err()
        );
        assert_eq!(std::fs::read(directory.join("journal.bin")).unwrap(), bytes);
    }
    std::fs::write(directory.join("journal.bin"), joint_bytes).unwrap();
    let (mut writer, _) = OpenJournal::scan_directory(
        root.journal_directory(&directory).unwrap(),
        JournalNamespace::Joint,
    )
    .unwrap()
    .into_writer()
    .unwrap();
    for (kind, mode) in [
        (RecordType::MembershipControlBootstrap, AppendMode::Joint),
        (RecordType::MembershipJointAccept, AppendMode::Pending),
        (RecordType::MembershipJointAccept, AppendMode::Live),
        (RecordType::Prepare, AppendMode::Joint),
    ] {
        assert!(
            writer
                .append_mode(&disk, kind, 7, &proposal, &acceptance, mode)
                .is_err()
        );
    }
    assert_eq!(
        canonical_json(&RecordType::MembershipJointAccept).unwrap(),
        br#""MEMBERSHIP_JOINT_ACCEPT""#
    );
    for token in [
        "membership_joint_accept",
        "MembershipJointAccept",
        "MEMBERSHIPJOINTACCEPT",
    ] {
        assert!(decode_strict::<RecordType>(&canonical_json(token).unwrap()).is_err());
    }
}

#[test]
fn membership_singleton_rejects_file_replacement_after_read_only_open() {
    for joint_namespace in [false, true] {
        let (_temp, disk, root, directory) = setup();
        let mut first = if joint_namespace {
            PendingJournal::open_joint_at(&root, &directory, 7, &"a".repeat(64), &"b".repeat(64))
                .unwrap()
        } else {
            pending(&root, &directory)
        };
        first.synchronize(&disk).unwrap();
        let mut reopened = if joint_namespace {
            PendingJournal::open_joint_at(&root, &directory, 7, &"a".repeat(64), &"b".repeat(64))
                .unwrap()
        } else {
            pending(&root, &directory)
        };
        let path = directory.join("journal.bin");
        std::fs::rename(&path, directory.join("displaced.bin")).unwrap();
        std::fs::write(&path, b"replacement").unwrap();
        assert!(reopened.synchronize(&disk).is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"replacement");
    }
}
