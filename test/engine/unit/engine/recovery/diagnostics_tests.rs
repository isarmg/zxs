use super::*;
use std::error::Error as _;

fn io_source(failure: &RecoverySealFailure) -> &std::io::Error {
    failure
        .source()
        .unwrap()
        .source()
        .unwrap()
        .source()
        .unwrap()
        .downcast_ref()
        .unwrap()
}

#[test]
fn member_seal_failures_keep_each_identity_and_reset_on_the_next_recovery() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("system");
    let config = Config::development((0..5).map(|i| temp.path().join(format!("disk-{i}"))));
    drop(Engine::initialize(&root, config.clone()).unwrap());
    let mut engine =
        Engine::prepare_workspace_recovery(root, config.clone().try_into().unwrap()).unwrap();
    let expected = canonical_json(engine.state()).unwrap();
    let corrupt = config.disks[0].root.join("disk.json");
    let original = std::fs::read(&corrupt).unwrap();
    let missing = config.disks[1].root.join("disk.json");
    let moved = missing.with_extension("unavailable");
    std::fs::write(&corrupt, b"broken seal").unwrap();
    std::fs::rename(&missing, &moved).unwrap();

    for _ in 0..2 {
        engine.recover_mode(RecoveryMode::PreservePending).unwrap();
        assert!(engine.is_read_only());
        assert_eq!(engine.journals.len(), 3);
        assert_eq!(canonical_json(engine.state()).unwrap(), expected);
        let warnings = engine.take_recovery_warnings();
        assert_eq!(warnings.seals.len(), 2);
        for (index, disk) in config.disks[..2].iter().enumerate() {
            let context = RecoverySealContext::Member {
                disk_id: disk.disk_id.clone(),
                slot_id: disk.slot_id.clone(),
            };
            let failure = warnings
                .seals
                .iter()
                .find(|failure| failure.context() == &context)
                .unwrap();
            assert!(failure.source().unwrap().is::<Error>());
            if index == 0 {
                assert!(matches!(
                    failure.cause(),
                    Error::Contract(storage_contract::Error::Json { .. })
                ));
            } else {
                assert_eq!(io_source(failure).kind(), std::io::ErrorKind::NotFound);
            }
        }
        assert!(warnings.checkpoint_load.is_none());
        assert!(warnings.checkpoint_save.is_none());
        assert!(engine.take_recovery_warnings().seals.is_empty());
    }

    // Leave an unconsumed invocation to verify reset, not just drain semantics.
    engine.recover_mode(RecoveryMode::PreservePending).unwrap();
    std::fs::write(corrupt, original).unwrap();
    std::fs::rename(moved, missing).unwrap();
    engine.recover_mode(RecoveryMode::PreservePending).unwrap();
    assert!(engine.take_recovery_warnings().seals.is_empty());
    assert!(engine.is_read_only());
    assert_eq!(canonical_json(engine.state()).unwrap(), expected);
}

#[test]
fn missing_publication_seal_keeps_the_verified_prefix_and_candidate_cause() {
    for mode in [RecoveryMode::CompletePending, RecoveryMode::PreservePending] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("system");
        let config = Config::development((0..3).map(|i| temp.path().join(format!("disk-{i}"))));
        let mut engine = Engine::initialize(&root, config.clone()).unwrap();
        let first = OperationId::generate();
        engine
            .create_repository(
                &RepositoryName::parse("first").unwrap(),
                "P01",
                1,
                "admin",
                first,
            )
            .unwrap();
        let expected = canonical_json(engine.state()).unwrap();
        let receipt = canonical_json(&engine.operation(first).unwrap()).unwrap();
        let second = OperationId::generate();
        engine
            .create_repository(
                &RepositoryName::parse("second").unwrap(),
                "P01",
                1,
                "admin",
                second,
            )
            .unwrap();
        let record = scan(&config.disks[0])
            .unwrap()
            .records
            .into_iter()
            .rev()
            .find(|(record, _)| record.record_type == RecordType::Publication)
            .unwrap()
            .0;
        drop(engine);
        let mut engine =
            Engine::prepare_workspace_recovery(root, config.clone().try_into().unwrap()).unwrap();
        std::fs::write(
            config.disks[2].root.join("disk.json"),
            b"broken member seal",
        )
        .unwrap();
        let journals: Vec<_> = config.disks[..2]
            .iter()
            .map(|disk| std::fs::read(log_path(disk)).unwrap())
            .collect();
        for disk in &config.disks[..2] {
            std::fs::remove_file(publication_seal_path(disk, &record.candidate_hash)).unwrap();
        }

        engine.recover_mode(mode).unwrap();
        assert!(engine.is_read_only());
        assert_eq!(canonical_json(engine.state()).unwrap(), expected);
        assert_eq!(
            canonical_json(&engine.operation(first).unwrap()).unwrap(),
            receipt
        );
        assert!(!engine.state.operations.contains_key(&second));
        let pending = engine.operation(second).unwrap();
        assert_eq!(pending.commit_state, CommitState::InDoubt);
        assert_eq!(
            pending.protection_at_publish,
            ProtectionAtPublish::Unconfirmed
        );
        assert_eq!(pending.current_health, CurrentHealth::Recovering);
        let warnings = engine.take_recovery_warnings();
        assert_eq!(warnings.seals.len(), 2);
        assert_eq!(
            warnings.seals[0].context(),
            &RecoverySealContext::Member {
                disk_id: config.disks[2].disk_id.clone(),
                slot_id: config.disks[2].slot_id.clone(),
            }
        );
        assert_eq!(
            warnings.seals[1].context(),
            &RecoverySealContext::Publication {
                candidate_hash: record.candidate_hash,
            }
        );
        assert_eq!(
            io_source(&warnings.seals[1]).kind(),
            std::io::ErrorKind::NotFound
        );
        assert!(warnings.checkpoint_load.is_none());
        assert!(warnings.checkpoint_save.is_none());
        assert!(engine.take_recovery_warnings().seals.is_empty());
        for (disk, bytes) in config.disks[..2].iter().zip(journals) {
            assert_eq!(std::fs::read(log_path(disk)).unwrap(), bytes);
        }
    }
}

#[test]
fn warning_aggregate_keeps_permission_source_alongside_both_checkpoint_causes() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temp.path().join("system"),
        Config::development([temp.path().join("disk")]),
    )
    .unwrap();
    let disk = &engine.config.disks[0];
    // The current replay branches cannot naturally emit seal and checkpoint
    // warnings together; exercise their independent storage without changing policy.
    engine.recovery_warnings = RecoveryWarnings {
        seals: vec![crate::engine::RecoverySealFailure {
            context: RecoverySealContext::Member {
                disk_id: disk.disk_id.clone(),
                slot_id: disk.slot_id.clone(),
            },
            source: Error::Filesystem(platform_io::fs_safety::Error::Io(
                std::io::Error::from_raw_os_error(13),
            )),
        }],
        checkpoint_load: Some(std::io::Error::from_raw_os_error(5).into()),
        checkpoint_save: Some(std::io::Error::from_raw_os_error(28).into()),
    };
    let warnings = engine.take_recovery_warnings();
    assert_eq!(warnings.seals.len(), 1);
    assert_eq!(
        io_source(&warnings.seals[0]).kind(),
        std::io::ErrorKind::PermissionDenied
    );
    assert_eq!(io_source(&warnings.seals[0]).raw_os_error(), Some(13));
    for (error, raw) in [
        (warnings.checkpoint_load.unwrap(), 5),
        (warnings.checkpoint_save.unwrap(), 28),
    ] {
        assert_eq!(
            error
                .source()
                .unwrap()
                .downcast_ref::<std::io::Error>()
                .unwrap()
                .raw_os_error(),
            Some(raw)
        );
    }
    let consumed = engine.take_recovery_warnings();
    assert!(consumed.seals.is_empty());
    assert!(consumed.checkpoint_load.is_none());
    assert!(consumed.checkpoint_save.is_none());
    assert!(!engine.is_read_only());
}
