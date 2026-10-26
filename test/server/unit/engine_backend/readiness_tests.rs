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
