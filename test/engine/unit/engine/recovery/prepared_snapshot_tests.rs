use super::*;

fn publish(engine: &mut Engine, name: &str) -> OperationId {
    let operation = OperationId::generate();
    engine
        .create_repository(
            &storage_contract::RepositoryName::parse(name).unwrap(),
            "P01",
            1,
            "admin",
            operation,
        )
        .unwrap();
    operation
}

#[test]
fn prepared_latest_root_preserves_full_checkpoint_and_suffix_replay() {
    for checkpoint in ["absent", "boundary", "suffix"] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("system");
        let mut engine =
            Engine::initialize(&root, Config::development([temp.path().join("primary")])).unwrap();
        let first = publish(&mut engine, "first");
        if checkpoint == "suffix" {
            drop(engine);
            engine = Engine::open(&root).unwrap();
        }
        let latest = publish(&mut engine, "latest");
        let config = engine.config.clone();
        let expected = canonical_json(engine.state()).unwrap();
        let sequence = engine.state.sequence;
        let first_result = engine.operation(first).unwrap();
        let latest_result = engine.operation(latest).unwrap();
        drop(engine);
        if checkpoint == "boundary" {
            drop(Engine::open(&root).unwrap());
        }
        let journal = log_path(&config.disks[0]);
        let before = std::fs::read(&journal).unwrap();
        let mut engine = Engine::prepare_workspace_recovery(root, config).unwrap();
        let prepared = engine.prepare_recovery().unwrap();
        let (prepared_sequence, paired) = prepared.candidates.as_ref().unwrap();
        assert_eq!(*prepared_sequence, sequence);
        assert_eq!(paired.evidence.candidate.operation_id, latest);
        match checkpoint {
            "absent" => assert!(prepared.checkpoint.is_none()),
            "boundary" => assert_eq!(prepared.checkpoint.as_ref().unwrap().sequence, sequence),
            "suffix" => assert!(prepared.checkpoint.as_ref().unwrap().sequence < sequence),
            _ => unreachable!(),
        }
        assert_eq!(
            canonical_json(
                &prepared
                    .workspace_receipt(&engine, latest)
                    .unwrap()
                    .unwrap()
                    .result
            )
            .unwrap(),
            canonical_json(&latest_result).unwrap(),
        );
        engine.seed_workspace_recovery(&prepared).unwrap();
        SNAPSHOT_LOAD_COUNT.with(|count| count.set(0));
        engine
            .apply_recovery(RecoveryMode::CompletePending, prepared)
            .unwrap();
        assert_eq!(canonical_json(engine.state()).unwrap(), expected);
        assert_eq!(
            canonical_json(&engine.operation(first).unwrap()).unwrap(),
            canonical_json(&first_result).unwrap()
        );
        assert_eq!(
            canonical_json(&engine.operation(latest).unwrap()).unwrap(),
            canonical_json(&latest_result).unwrap()
        );
        assert_eq!(std::fs::read(journal).unwrap(), before);
        SNAPSHOT_LOAD_COUNT
            .with(|count| assert_eq!(count.get(), if checkpoint == "boundary" { 1 } else { 2 }));
    }
}

#[test]
fn prepared_root_still_admits_every_referenced_dependency() {
    for (dependency, missing) in [
        ("state-page", false),
        ("state-page", true),
        ("state-control-auth", false),
        ("state-control-auth", true),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("system");
        let mut engine =
            Engine::initialize(&root, Config::development([temp.path().join("primary")])).unwrap();
        publish(&mut engine, "published");
        let config = engine.config.clone();
        let snapshot: Snapshot = load_object(
            &config.disks[0],
            "snapshot",
            engine.snapshot.as_ref().unwrap(),
        )
        .unwrap();
        let hash = if dependency == "state-page" {
            crate::journal::map_pages(&config.disks[0], &snapshot.maps["repositories"])[0].clone()
        } else {
            snapshot.control_auth.hash().to_owned()
        };
        drop(engine);
        let damaged = object_path(&config.disks[0], dependency, &hash);
        if missing {
            std::fs::remove_file(&damaged).unwrap();
        } else {
            std::fs::write(&damaged, b"corrupt dependency").unwrap();
        }
        let journal = log_path(&config.disks[0]);
        let before = std::fs::read(&journal).unwrap();
        let mut engine = Engine::prepare_workspace_recovery(root, config).unwrap();
        let prepared = engine.prepare_recovery().unwrap();
        assert!(prepared.candidates.is_some());
        engine.seed_workspace_recovery(&prepared).unwrap();
        assert!(
            engine
                .apply_recovery(RecoveryMode::CompletePending, prepared)
                .is_err()
        );
        assert_eq!(engine.state.sequence, 0);
        assert_eq!(std::fs::read(journal).unwrap(), before);
        if missing {
            assert!(!damaged.exists());
        } else {
            assert_eq!(std::fs::read(damaged).unwrap(), b"corrupt dependency");
        }
    }
}

#[test]
fn corrupt_latest_root_is_rejected_during_read_only_preparation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("system");
    let mut engine =
        Engine::initialize(&root, Config::development([temp.path().join("primary")])).unwrap();
    publish(&mut engine, "published");
    let config = engine.config.clone();
    let snapshot = engine.snapshot.clone().unwrap();
    drop(engine);
    let damaged = object_path(&config.disks[0], "snapshot", &snapshot);
    std::fs::write(&damaged, b"corrupt root").unwrap();
    let journal = log_path(&config.disks[0]);
    let before = std::fs::read(&journal).unwrap();
    let mut engine = Engine::prepare_workspace_recovery(root, config).unwrap();
    assert!(engine.prepare_recovery().is_err());
    assert!(engine.journals.is_empty());
    assert_eq!(engine.state.sequence, 0);
    assert_eq!(std::fs::read(journal).unwrap(), before);
    assert_eq!(std::fs::read(damaged).unwrap(), b"corrupt root");
}

#[test]
fn pending_latest_decision_does_not_take_the_last_sealed_root() {
    for mode in [RecoveryMode::PreservePending, RecoveryMode::CompletePending] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("system");
        let mut engine =
            Engine::initialize(&root, Config::development([temp.path().join("primary")])).unwrap();
        let published = publish(&mut engine, "published");
        let published_state = canonical_json(engine.state()).unwrap();
        let published_sequence = engine.state.sequence;
        let pending = OperationId::generate();
        engine.inject_fault_once("after_accept_1");
        assert!(
            engine
                .create_repository(
                    &storage_contract::RepositoryName::parse("pending").unwrap(),
                    "P01",
                    1,
                    "admin",
                    pending,
                )
                .is_err()
        );
        let config = engine.config.clone();
        drop(engine);
        let journal = log_path(&config.disks[0]);
        let before = std::fs::read(&journal).unwrap();
        let mut engine = Engine::prepare_workspace_recovery(root, config).unwrap();
        let prepared = engine.prepare_recovery().unwrap();
        let (sequence, paired) = prepared.candidates.as_ref().unwrap();
        assert_eq!(*sequence, published_sequence);
        assert_eq!(paired.evidence.candidate.operation_id, published);
        assert!(
            prepared
                .workspace_receipt(&engine, pending)
                .unwrap()
                .is_none()
        );
        engine.seed_workspace_recovery(&prepared).unwrap();
        engine.apply_recovery(mode, prepared).unwrap();
        match mode {
            RecoveryMode::PreservePending => {
                assert!(engine.blocked);
                assert_eq!(canonical_json(engine.state()).unwrap(), published_state);
                assert_eq!(std::fs::read(journal).unwrap(), before);
            }
            RecoveryMode::CompletePending => {
                assert!(!engine.blocked);
                assert_eq!(engine.state.repositories.len(), 2);
                assert_eq!(
                    engine.operation(pending).unwrap().commit_state,
                    storage_contract::CommitState::Published
                );
                assert!(std::fs::read(journal).unwrap().starts_with(&before));
            }
        }
    }
}

#[test]
fn schema_admitted_root_and_raw_entry_share_full_state_admission() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("system");
    let mut engine =
        Engine::initialize(&root, Config::development([temp.path().join("primary")])).unwrap();
    publish(&mut engine, "published");
    let disk = &engine.config.disks[0];
    let snapshot: Snapshot =
        load_object(disk, "snapshot", engine.snapshot.as_ref().unwrap()).unwrap();
    let expected = canonical_json(engine.state()).unwrap();
    let admitted = SchemaValidatedSnapshot::new(snapshot.clone()).unwrap();
    assert_eq!(admitted.snapshot().sequence, snapshot.sequence);
    SNAPSHOT_LOAD_COUNT.with(|count| count.set(0));
    let prepared = admitted
        .resolve(|kind, hash| load_authenticated_object(disk, kind, hash))
        .unwrap();
    let raw = resolve_admitted_snapshot(snapshot.clone(), |kind, hash| {
        load_authenticated_object(disk, kind, hash)
    })
    .unwrap();
    assert_eq!(canonical_json(prepared.state()).unwrap(), expected);
    assert_eq!(canonical_json(raw.state()).unwrap(), expected);
    SNAPSHOT_LOAD_COUNT.with(|count| assert_eq!(count.get(), 2));

    for extra in [false, true] {
        let mut malformed = snapshot.clone();
        if extra {
            malformed
                .maps
                .insert("unknown-map".into(), snapshot.maps["plans"].clone());
        } else {
            malformed.maps.remove("plans");
        }
        let error = SchemaValidatedSnapshot::new(malformed.clone())
            .err()
            .unwrap();
        assert_eq!(error.code(), ErrorCode::ObjectCorrupt);
        let error = resolve_admitted_snapshot(malformed, |_, _| {
            panic!("invalid root schema must reject before reading dependencies")
        })
        .err()
        .unwrap();
        assert_eq!(error.code(), ErrorCode::ObjectCorrupt);
    }
}
