use super::*;
use storage_engine::Config;

#[test]
fn readiness_uses_fixed_membership_and_reports_single_disk_no_metadata_redundancy() {
    for count in [1, 2, 3, 4] {
        let temp = tempfile::tempdir().unwrap();
        let disks: Vec<_> = (0..count)
            .map(|n| temp.path().join(format!("disk-{n}")))
            .collect();
        let engine = Engine::initialize(
            &temp.path().join("system"),
            Config::development(disks.clone()),
        )
        .unwrap();
        let backend = EngineBackend::new(engine);
        let crate::Readiness::Engine(ready) = backend.ready().unwrap() else {
            panic!("expected engine readiness")
        };
        assert_eq!(ready.metadata_member_count, count);
        assert_eq!(ready.storage_disk_count, count);
        std::fs::rename(&disks[count - 1], temp.path().join("missing-disk")).unwrap();
        assert_eq!(
            backend.ready().unwrap_err().code,
            storage_contract::ErrorCode::MetadataUnavailable
        );
    }
}

#[test]
fn backend_reports_and_consumes_nonfatal_recovery_checkpoint_warnings() {
    let temp = tempfile::tempdir().unwrap();
    let disk = temp.path().join("disk");
    let root = temp.path().join("system");
    drop(Engine::initialize(&root, Config::development([disk.clone()])).unwrap());
    drop(Engine::open(&root).unwrap());
    std::fs::write(
        disk.join("metadata/replay-checkpoint.json"),
        b"broken checkpoint",
    )
    .unwrap();
    let engine = Engine::open(&root).unwrap();
    let backend = EngineBackend::new(engine);
    let warnings = backend.engine.lock().unwrap().take_recovery_warnings();
    assert!(warnings.seals.is_empty());
    assert!(warnings.checkpoint_load.is_none());
    assert!(warnings.checkpoint_save.is_none());
}

#[test]
fn returned_read_only_engine_reports_the_publication_seal_cause_only_once() {
    use std::io::{Read, Seek};

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("system");
    let disks: Vec<_> = (0..3)
        .map(|i| temp.path().join(format!("disk-{i}")))
        .collect();
    let mut engine = Engine::initialize(&root, Config::development(disks.clone())).unwrap();
    engine
        .create_repository(
            &storage_contract::RepositoryName::parse("unsealed").unwrap(),
            "P01",
            1,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    drop(engine);
    std::fs::rename(&disks[2], temp.path().join("offline")).unwrap();
    for disk in &disks[..2] {
        for entry in std::fs::read_dir(disk.join("metadata/publication-seals")).unwrap() {
            std::fs::remove_file(entry.unwrap().path()).unwrap();
        }
    }
    let mut engine = Engine::open_for_recovery(&root).unwrap();
    assert!(engine.is_read_only());
    assert!(engine.state().repositories.is_empty());
    let mut capture = tempfile::tempfile().unwrap();
    let subscriber = tracing::collector::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(Arc::new(capture.try_clone().unwrap()))
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        report_recovery_warnings(&mut engine);
        report_recovery_warnings(&mut engine);
    });
    capture.rewind().unwrap();
    let mut log = String::new();
    capture.read_to_string(&mut log).unwrap();
    assert_eq!(log.matches("recovery seal verification failed").count(), 1);
    assert!(log.contains("Publication"));
    assert!(log.contains("candidate_hash"));
    assert!(log.contains("NotFound"));
    let consumed = engine.take_recovery_warnings();
    assert!(consumed.seals.is_empty());
    assert!(consumed.checkpoint_load.is_none());
    assert!(consumed.checkpoint_save.is_none());
    assert!(engine.is_read_only());
    assert!(engine.state().repositories.is_empty());
}

#[test]
fn readiness_ignores_missing_nonmember_and_preserves_member_failure_source() {
    let temp = tempfile::tempdir().unwrap();
    let mut config = Config::development([temp.path().join("member"), temp.path().join("backup")]);
    config.disks[1].metadata_member = false;
    let engine = Engine::initialize(&temp.path().join("system"), config.clone()).unwrap();
    let sequence = engine.state().sequence;
    let backend = EngineBackend::new(engine);
    std::fs::rename(&config.disks[1].root, temp.path().join("missing-backup")).unwrap();
    let crate::Readiness::Engine(ready) = backend.ready().unwrap() else {
        panic!("expected engine readiness")
    };
    assert_eq!(ready.metadata_member_count, 1);
    assert_eq!(ready.storage_disk_count, 2);
    assert_eq!(ready.sequence, sequence);
    assert_eq!(ready.mode, config.mode);
    std::fs::rename(&config.disks[0].root, temp.path().join("missing-member")).unwrap();
    let error = backend.ready().unwrap_err();
    assert_eq!(error.status, ::http::StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error.code, storage_contract::ErrorCode::MetadataUnavailable);
    assert!(error.retryable);
    assert!(std::error::Error::source(&error).is_some());
}
