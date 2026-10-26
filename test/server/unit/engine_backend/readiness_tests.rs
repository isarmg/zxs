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
    assert!(warnings.checkpoint_load.is_none());
    assert!(warnings.checkpoint_save.is_none());
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
