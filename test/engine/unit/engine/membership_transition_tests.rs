use super::*;
use std::{fs, path::Path};

fn fixture() -> (tempfile::TempDir, Config, Config, Engine) {
    let temporary = tempfile::tempdir().unwrap();
    let config =
        Config::development((0..3).map(|n| temporary.path().join(format!("disk-{n}")))).unwrap();
    let mut target = config.clone();
    target.disks[2].metadata_member = false;
    let mut engine = Engine::initialize(&temporary.path().join("system"), config.clone()).unwrap();
    engine
        .create_repository(
            &RepositoryName::parse("preserved").unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    drop(engine);
    let engine = Engine::open(&temporary.path().join("system")).unwrap();
    (temporary, config, target, engine)
}
fn journal_bytes(config: &Config) -> Vec<Vec<u8>> {
    config
        .disks
        .iter()
        .map(|disk| fs::read(log_path(disk)).unwrap())
        .collect()
}
fn objects(root: &Path) -> BTreeMap<std::path::PathBuf, Vec<u8>> {
    fn visit(root: &Path, path: &Path, output: &mut BTreeMap<std::path::PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(root, &path, output);
            } else {
                output.insert(
                    path.strip_prefix(root).unwrap().to_owned(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut output = BTreeMap::new();
    visit(root, root, &mut output);
    output
}

#[test]
fn membership_fence_preserves_old_bytes_and_reopens_read_only() {
    let (temporary, config, target, mut engine) = fixture();
    let state = canonical_json(engine.state()).unwrap();
    let old = config
        .disks
        .iter()
        .map(|disk| objects(&disk.root))
        .collect::<Vec<_>>();
    let operation = OperationId::generate().unwrap();
    let status = engine
        .begin_membership_handoff_with_effect(
            ValidatedConfig::try_from(target.clone()).unwrap(),
            None,
            "admin",
            operation,
        )
        .unwrap();
    assert!(status.quorum_fenced());
    assert!(status.all_members_fenced());
    assert!(engine.is_read_only());
    assert_eq!(
        engine
            .begin_membership_handoff_with_effect(
                ValidatedConfig::try_from(target.clone()).unwrap(),
                None,
                "admin",
                operation
            )
            .unwrap(),
        status
    );
    let logs = journal_bytes(&config);
    for (disk, files) in config.disks.iter().zip(old) {
        for (path, bytes) in files {
            let current = fs::read(disk.root.join(&path)).unwrap();
            if path == Path::new("metadata/application-log/journal.bin") {
                assert!(current.starts_with(&bytes));
            } else {
                assert_eq!(current, bytes, "{path:?}");
            }
        }
    }
    assert_eq!(canonical_json(engine.state()).unwrap(), state);
    assert_eq!(
        engine.prepare_integrity_scrub().err().unwrap().code(),
        ErrorCode::RecoveryRequired
    );
    assert_eq!(
        engine
            .prepare_degraded_recovery(operation, "admin", "test")
            .err()
            .unwrap()
            .code(),
        ErrorCode::RecoveryRequired
    );
    drop(engine);
    for preserve in [false, true] {
        let mut opened = if preserve {
            Engine::open_for_recovery(&temporary.path().join("system"))
        } else {
            Engine::open(&temporary.path().join("system"))
        }
        .unwrap();
        assert_eq!(opened.membership_fence_status(), Some(status.clone()));
        assert_eq!(canonical_json(opened.state()).unwrap(), state);
        assert_eq!(journal_bytes(&config), logs);
        assert!(opened.is_read_only());
        assert_eq!(
            opened
                .create_repository(
                    &RepositoryName::parse("blocked").unwrap(),
                    "P01",
                    1,
                    "admin",
                    OperationId::generate().unwrap()
                )
                .unwrap_err()
                .code(),
            ErrorCode::RecoveryRequired
        );
        assert_eq!(
            opened
                .begin_membership_handoff_with_effect(
                    ValidatedConfig::try_from(target.clone()).unwrap(),
                    None,
                    "admin",
                    operation
                )
                .unwrap(),
            status
        );
    }
}

#[test]
fn membership_fence_resumes_partial_votes_without_lowering_original_quorum() {
    for complete in [1, 2] {
        let (temporary, config, target, mut engine) = fixture();
        let state = canonical_json(engine.state()).unwrap();
        let operation = OperationId::generate().unwrap();
        engine.inject_fault_once(&format!("after_membership_fence_{complete}"));
        assert_eq!(
            engine
                .begin_membership_handoff_with_effect(
                    ValidatedConfig::try_from(target.clone()).unwrap(),
                    None,
                    "admin",
                    operation
                )
                .unwrap_err()
                .code(),
            ErrorCode::InjectedFailure
        );
        let status = engine.membership_fence_status().unwrap();
        assert_eq!(status.confirmed_members, complete);
        assert_eq!(status.quorum_fenced(), complete >= 2);
        let logs = journal_bytes(&config);
        drop(engine);
        let mut reopened = Engine::open(&temporary.path().join("system")).unwrap();
        assert_eq!(reopened.membership_fence_status(), Some(status));
        assert_eq!(canonical_json(reopened.state()).unwrap(), state);
        assert_eq!(journal_bytes(&config), logs);
        assert!(
            reopened
                .begin_membership_handoff_with_effect(
                    ValidatedConfig::try_from(target).unwrap(),
                    None,
                    "admin",
                    operation
                )
                .unwrap()
                .all_members_fenced()
        );
        for disk in &config.disks {
            let scan = scan(disk).unwrap();
            assert_eq!(
                scan.records
                    .iter()
                    .filter(|(record, _)| record.record_type == RecordType::MembershipFence)
                    .count(),
                1
            );
        }
    }
}

#[test]
fn membership_fence_rejects_changed_intent_and_corrupt_object_before_recovery_writes() {
    let (temporary, config, target, mut engine) = fixture();
    let operation = OperationId::generate().unwrap();
    let status = engine
        .begin_membership_handoff_with_effect(
            ValidatedConfig::try_from(target.clone()).unwrap(),
            None,
            "admin",
            operation,
        )
        .unwrap();
    assert_eq!(
        engine
            .begin_membership_handoff_with_effect(
                ValidatedConfig::try_from(target).unwrap(),
                None,
                "different",
                operation
            )
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyKeyReused
    );
    drop(engine);
    for disk in &config.disks {
        fs::write(
            object_path(disk, INTENT_KIND, &status.transition_hash),
            b"{",
        )
        .unwrap();
    }
    let before = config
        .disks
        .iter()
        .map(|disk| objects(&disk.root))
        .collect::<Vec<_>>();
    assert!(Engine::open(&temporary.path().join("system")).is_err());
    for (disk, bytes) in config.disks.iter().zip(before) {
        assert_eq!(objects(&disk.root), bytes);
    }
}

#[test]
fn membership_fence_rejects_business_records_after_fence() {
    let (temporary, config, target, mut engine) = fixture();
    engine
        .begin_membership_handoff_with_effect(
            ValidatedConfig::try_from(target).unwrap(),
            None,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    // The private wrapper rejects ordinary journal extensions even for callers
    // that bypass require_write (such as explicit degraded recovery).
    let disk = &config.disks[0];
    assert_eq!(
        engine
            .append_recovered_journal(
                disk,
                RecordType::Accept,
                engine.next_slot,
                &"a".repeat(64),
                &"a".repeat(64)
            )
            .unwrap_err()
            .code(),
        ErrorCode::RecoveryRequired
    );
    let sequence = engine.next_slot;
    drop(engine);
    let (mut writer, _) = OpenJournal::scan(disk).unwrap().into_writer().unwrap();
    writer
        .append(
            disk,
            RecordType::Prepare,
            sequence,
            &"a".repeat(64),
            &"a".repeat(64),
        )
        .unwrap();
    drop(writer);
    let logs = journal_bytes(&config);
    assert_eq!(
        Engine::open(&temporary.path().join("system"))
            .err()
            .unwrap()
            .code(),
        ErrorCode::LogFork
    );
    assert_eq!(journal_bytes(&config), logs);
}

#[test]
fn membership_fence_survives_system_directory_loss_and_genesis() {
    let (temporary, config, target, mut engine) = fixture();
    let state = canonical_json(engine.state()).unwrap();
    let status = engine
        .begin_membership_handoff_with_effect(
            ValidatedConfig::try_from(target).unwrap(),
            None,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    drop(engine);
    let restored = Engine::restore_from_disks(
        &temporary.path().join("replacement"),
        &config
            .disks
            .iter()
            .map(|disk| disk.root.clone())
            .collect::<Vec<_>>(),
    )
    .unwrap();
    assert_eq!(restored.membership_fence_status(), Some(status));
    assert_eq!(canonical_json(restored.state()).unwrap(), state);
    assert!(restored.is_read_only());
    drop(restored);
    let temp = tempfile::tempdir().unwrap();
    let config =
        Config::development((0..3).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
    let mut target = config.clone();
    target.disks[2].metadata_member = false;
    let mut empty = Engine::initialize(&temp.path().join("system"), config).unwrap();
    empty
        .begin_membership_handoff_with_effect(
            ValidatedConfig::try_from(target).unwrap(),
            None,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    drop(empty);
    assert!(
        Engine::open(&temp.path().join("system"))
            .unwrap()
            .is_read_only()
    );
}

#[test]
fn membership_fence_append_failures_reopen_and_resume_exact_intent() {
    for failing in 0..3 {
        for fault in [JournalFault::PartialWrite, JournalFault::Sync] {
            let (temporary, config, target, mut engine) = fixture();
            let operation = OperationId::generate().unwrap();
            let state = canonical_json(engine.state()).unwrap();
            engine
                .inject_journal_fault_once(&engine.members()[failing].slot_id, fault)
                .unwrap();
            assert!(
                engine
                    .begin_membership_handoff_with_effect(
                        ValidatedConfig::try_from(target.clone()).unwrap(),
                        None,
                        "admin",
                        operation
                    )
                    .is_err()
            );
            assert!(engine.is_read_only());
            assert_eq!(
                engine.membership_fence_status().unwrap().confirmed_members,
                failing
            );
            drop(engine);
            let mut recovered = Engine::open(&temporary.path().join("system")).unwrap();
            assert_eq!(canonical_json(recovered.state()).unwrap(), state);
            if failing == 0 && matches!(fault, JournalFault::PartialWrite) {
                // No complete fence exists: an incomplete first frame cannot
                // establish a handoff or an old quorum's durable fence.
                assert!(recovered.membership_fence_status().is_none());
                assert!(!recovered.is_read_only());
            } else {
                assert!(recovered.is_read_only());
                assert_eq!(
                    recovered.membership_fence_status().unwrap().operation_id,
                    operation
                );
            }
            assert!(
                recovered
                    .begin_membership_handoff_with_effect(
                        ValidatedConfig::try_from(target).unwrap(),
                        None,
                        "admin",
                        operation
                    )
                    .unwrap()
                    .all_members_fenced()
            );
            for disk in &config.disks {
                let scan = scan(disk).unwrap();
                assert!(!scan.torn_tail);
                assert_eq!(
                    scan.records
                        .iter()
                        .filter(|(record, _)| record.record_type == RecordType::MembershipFence)
                        .count(),
                    1
                );
            }
        }
    }
}

#[test]
fn membership_fence_missing_votes_never_claim_a_lower_quorum() {
    let (temporary, _config, target, mut engine) = fixture();
    let operation = OperationId::generate().unwrap();
    engine.inject_fault_once("after_membership_fence_2");
    assert!(
        engine
            .begin_membership_handoff_with_effect(
                ValidatedConfig::try_from(target.clone()).unwrap(),
                None,
                "admin",
                operation
            )
            .is_err()
    );
    assert!(engine.membership_fence_status().unwrap().quorum_fenced());
    let fenced_member = engine.members()[0].root.clone();
    drop(engine);
    fs::rename(&fenced_member, temporary.path().join("unavailable")).unwrap();
    let mut recovered = Engine::open(&temporary.path().join("system")).unwrap();
    let status = recovered.membership_fence_status().unwrap();
    assert_eq!(status.original_members, 3);
    assert_eq!(status.original_quorum, 2);
    assert_eq!(status.confirmed_members, 1);
    assert!(!status.quorum_fenced());
    assert!(recovered.is_read_only());
    assert!(
        recovered
            .begin_membership_handoff_with_effect(
                ValidatedConfig::try_from(target).unwrap(),
                None,
                "admin",
                operation
            )
            .is_err()
    );
}

#[test]
fn membership_fence_rejects_a_conflicting_intent_or_wrong_prefix() {
    for alter_prefix in [false, true] {
        let (temporary, config, target, mut engine) = fixture();
        engine
            .begin_membership_handoff_with_effect(
                ValidatedConfig::try_from(target).unwrap(),
                None,
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        let mut intent = engine.membership_fence.as_ref().unwrap().intent.clone();
        if alter_prefix {
            intent.boundary.as_mut().unwrap().snapshot_hash = "0".repeat(64);
        } else {
            intent.operation_id = OperationId::generate().unwrap();
        }
        let original_logs = journal_bytes(&config);
        drop(engine);
        let changed = if alter_prefix {
            &config.disks[..]
        } else {
            &config.disks[2..]
        };
        for disk in changed {
            let mut scan = scan(disk).unwrap();
            let fence = scan.records.pop().unwrap();
            assert_eq!(fence.0.record_type, RecordType::MembershipFence);
            // Rebuild only the final adversarial frame, leaving original
            // publication bytes untouched and maintaining a valid hash chain.
            let old_payload = canonical_json(&fence.0).unwrap();
            let old_length = 12 + old_payload.len() + 32;
            let bytes = &original_logs[config
                .disks
                .iter()
                .position(|d| d.disk_id == disk.disk_id)
                .unwrap()];
            fs::write(log_path(disk), &bytes[..bytes.len() - old_length]).unwrap();
            let hash = store_object(disk, INTENT_KIND, &intent).unwrap();
            let (mut writer, _) = OpenJournal::scan(disk).unwrap().into_writer().unwrap();
            writer
                .append(
                    disk,
                    RecordType::MembershipFence,
                    intent.fence_sequence,
                    &hash,
                    &hash,
                )
                .unwrap();
        }
        let before = journal_bytes(&config);
        assert!(Engine::open(&temporary.path().join("system")).is_err());
        assert_eq!(journal_bytes(&config), before);
    }
}

#[test]
fn membership_public_handoff_rejects_unfinishable_shape_before_any_fence() {
    for case in 0..8 {
        let (temp, original, _, mut engine) = fixture();
        let mut target = original.clone();
        let mut added = target.disks[0].clone();
        added.disk_id = new_id("disk").unwrap();
        added.slot_id = "P04".into();
        added.root = temp.path().join("unopened-addition");
        target.disks.push(added);
        match case {
            0 => {
                target.disks.remove(2);
            }
            1 => target.disks[2].metadata_member = false,
            2 => target.disks[0].root = temp.path().join("changed-existing-root"),
            3 => target.disks[0].binding_generation += 1,
            4 => target.disks[0].capacity_bytes = Some(10),
            5 => target.max_entries += 1,
            6 => target.disks[3].binding_generation = 2,
            7 => target.disks[3].root = engine.root.join("nested-storage"),
            _ => unreachable!(),
        }
        let target = ValidatedConfig::try_from(target).unwrap();
        let before = objects(temp.path());
        let state = canonical_json(engine.state()).unwrap();
        let next_slot = engine.next_slot;
        assert!(
            engine
                .begin_membership_handoff(
                    (*target).clone(),
                    "admin",
                    OperationId::generate().unwrap()
                )
                .is_err(),
            "case {case}"
        );
        assert!(engine.membership_fence_status().is_none(), "case {case}");
        assert!(!engine.is_read_only(), "case {case}");
        assert_eq!(engine.next_slot, next_slot);
        assert_eq!(canonical_json(engine.state()).unwrap(), state);
        assert_eq!(objects(temp.path()), before, "case {case}");
        engine
            .create_repository(
                &RepositoryName::parse("still-writable").unwrap(),
                "P01",
                1,
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
    }
}
