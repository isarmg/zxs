//! Real Engine I/O gates, not a production disk-offline API.
use super::fixture::{bytes, create_foreign};
use std::{
    collections::BTreeMap,
    fs::File,
    path::{Path, PathBuf},
};
use storage_contract::{Config, ErrorCode, OperationId};
use storage_engine::{
    Engine, ForeignImportIntentRequest, ForeignImportIntentSource, ForeignImportIntentTarget,
    ForeignImportLimits, ForeignImportPreview, PreparedForeignImportIntent,
};

struct Fixture {
    engine: Engine,
    source: PathBuf,
    _root: tempfile::TempDir,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        create_foreign(&root.path().join("foreign-system"), &source, false);
        let mut config = Config::development([
            root.path().join("target-primary"),
            root.path().join("unrelated-primary"),
        ])
        .unwrap();
        config.disks[1].level = 1;
        config.disks[1].upstream_slot_id = None;
        let engine = Engine::initialize(&root.path().join("target-system"), config).unwrap();
        Self {
            _root: root,
            source,
            engine,
        }
    }
    fn prepare(&self) -> PreparedForeignImportIntent {
        prepare(&self.engine, &self.source)
    }
}
fn prepare(engine: &Engine, path: &Path) -> PreparedForeignImportIntent {
    let source = ForeignImportIntentSource::capture(
        ForeignImportPreview::inspect(
            File::open(path).unwrap(),
            path.into(),
            ForeignImportLimits::default(),
        )
        .unwrap(),
    )
    .unwrap();
    let names: BTreeMap<_, _> = source
        .preview()
        .histories()
        .iter()
        .map(|history| (history.group_id().into(), "reviewed-history".into()))
        .collect();
    engine
        .prepare_foreign_import_intent(
            source,
            ForeignImportIntentTarget::new("P01".into(), None, names).unwrap(),
            ForeignImportIntentRequest {
                operation_id: OperationId::generate().unwrap(),
                review_operation_id: OperationId::generate().unwrap(),
                review_digest: "e".repeat(64).parse().unwrap(),
                principal: "admin".into(),
            },
        )
        .unwrap()
}

#[test]
fn unrelated_quiesced_disk_does_not_block_target_intent_save_or_replay() {
    let mut f = Fixture::new();
    let first = f.prepare();
    let target = first.target_identity().clone();
    let operation = first.operation_id();
    let digest = *first.digest();
    let saved = f
        .engine
        .save_foreign_import_intent(first.confirm_save(&digest).unwrap(), "admin")
        .unwrap();
    let second = f.prepare();
    let second_digest = *second.digest();
    let unrelated = f.engine.config().disks[1].root.clone();
    let unrelated_before = bytes(&unrelated);
    let source_before = bytes(&f.source);
    let state_before = json_codec::to_value(f.engine.state()).unwrap();
    let mut closed = f.engine.inject_disk_quiesce("B01").unwrap();
    closed.release_control();
    assert!(closed.is_drained().unwrap());
    assert_eq!(
        f.engine
            .reconcile_foreign_import_intent(&target, operation, "admin", &digest)
            .unwrap(),
        Some(saved)
    );
    f.engine
        .save_foreign_import_intent(second.confirm_save(&second_digest).unwrap(), "admin")
        .unwrap();
    assert_eq!(
        json_codec::to_value(f.engine.state()).unwrap(),
        state_before
    );
    assert_eq!(bytes(&unrelated), unrelated_before);
    assert_eq!(bytes(&f.source), source_before);
    assert_eq!(closed.ordinary_count().unwrap(), 0);
}

#[test]
fn quiesced_target_still_blocks_intent_save_and_replay_without_writes() {
    let mut f = Fixture::new();
    let first = f.prepare();
    let target = first.target_identity().clone();
    let operation = first.operation_id();
    let digest = *first.digest();
    f.engine
        .save_foreign_import_intent(first.confirm_save(&digest).unwrap(), "admin")
        .unwrap();
    let second = f.prepare();
    let second_digest = *second.digest();
    let root = f.engine.config().disks[0].root.clone();
    let before = bytes(&root);
    let mut closed = f.engine.inject_disk_quiesce("P01").unwrap();
    closed.release_control();
    assert!(closed.is_drained().unwrap());
    assert_eq!(
        f.engine
            .reconcile_foreign_import_intent(&target, operation, "admin", &digest)
            .unwrap_err()
            .code(),
        ErrorCode::DiskUnavailable
    );
    assert_eq!(
        f.engine
            .save_foreign_import_intent(second.confirm_save(&second_digest).unwrap(), "admin")
            .unwrap_err()
            .code(),
        ErrorCode::DiskUnavailable
    );
    assert_eq!(bytes(&root), before);
    assert_eq!(closed.ordinary_count().unwrap(), 0);
}
