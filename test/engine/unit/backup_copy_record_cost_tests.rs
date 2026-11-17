use super::*;
use crate::disk::DiskRoot;
use crate::engine::backup_preparation::{
    BackupPreparationPhase, BackupPreparationRequest, snapshot::PreparedSnapshot,
};
use crate::{CommitRequest, Config, Engine, ErrorCode};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use storage_contract::OperationId;

#[test]
fn forecast_matches_each_real_target_record_including_escaped_paths_and_complete_batch() {
    let temp = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let config = Config::development(
        ["Primary", "Near archive", "Far archive"]
            .into_iter()
            .enumerate()
            .map(|(i, name)| {
                (
                    name_policy::Name::parse(name).unwrap(),
                    temp.path().join(format!("disk-{i}")),
                )
            }),
    )
    .unwrap();
    let system = temp.path().join("system");
    let mut engine = Engine::initialize(&system, config).unwrap();
    let id = || OperationId::generate().unwrap();
    let repo = engine.create_disk_root("P01", "admin", id()).unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "empty", b"")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "nested/copy", b"independent data")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: id(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "complete source records".into(),
            request_digest: None,
        })
        .unwrap();
    engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Another independent history").unwrap(),
            "P01",
            1,
            "admin",
            id(),
        )
        .unwrap();
    let paths = ["copy-a", "较长的 \"copy-b\""].map(|name| temp.path().join(name));
    for path in &paths {
        fs::DirBuilder::new().mode(0o700).create(path).unwrap();
    }
    let request = BackupPreparationRequest {
        operation_id: id(),
        principal: "admin".into(),
        source_slot_id: "B01".into(),
        target_paths: paths.to_vec(),
        expected_sequence: engine.state().sequence,
    };
    engine
        .start_backup_batch_preparation(
            request,
            paths.iter().map(|p| fs::File::open(p).unwrap()).collect(),
        )
        .unwrap();
    let status = engine.wait_backup_preparation().unwrap();
    assert_eq!(
        status.phase,
        BackupPreparationPhase::HistoryPrepared,
        "{status:?}"
    );
    let batch = batch::read_batch(&DiskRoot::open(&system).unwrap()).unwrap();
    let (envelope, _) = batch::BatchEnvelope::encode(&batch).unwrap();
    let plans = engine.backup_history_plans().unwrap();
    assert_eq!(plans.len(), 2);
    let snapshot = PreparedSnapshot::freeze(&engine, batch.first()).unwrap();
    let mut intent_lengths = Vec::new();
    let stop = AtomicBool::new(false);
    for (job, path) in batch.jobs().iter().zip(&paths) {
        let records = Records::batch(job, envelope);
        let mut sizes = BTreeMap::new();
        records
            .visit(&plans, snapshot.report_counts, &stop, |name, length| {
                let actual = fs::metadata(path.join(name.as_path())).unwrap();
                assert_eq!(actual.len(), length as u64);
                assert!(sizes.insert(name, length).is_none());
                Ok(())
            })
            .unwrap();
        assert_eq!(sizes.len(), 5);
        intent_lengths.push(sizes[&EntryName::new(TARGET_INTENT).unwrap()]);
        let target = DiskRoot::open(path).unwrap();
        let mut census = Census::new(&target).unwrap();
        records
            .charge(&mut census, &plans, snapshot.report_counts, &stop)
            .unwrap();
        assert_eq!(
            (
                census.needed.files,
                census.needed.directories,
                census.needed.bytes
            ),
            (0, 0, 0)
        );
    }
    assert_ne!(intent_lengths[0], intent_lengths[1]);
}

#[test]
fn a_complete_batch_record_fits_exactly_or_is_rejected_without_serializing_the_whole_batch() {
    let envelope = batch::BatchEnvelope {
        intent: 1,
        completion_base: 10,
        targets: 3,
    };
    assert_eq!(batch_length(envelope, 91).unwrap(), 285);
    assert_eq!(with_items(10, 90, 100).unwrap(), 100);
    assert_eq!(
        with_items(10, 91, 100).unwrap_err().code(),
        ErrorCode::ResourceLimit
    );
    assert_eq!(
        with_items(usize::MAX, 1, usize::MAX).unwrap_err().code(),
        ErrorCode::ResourceLimit
    );
    assert_eq!(
        batch_length(envelope, usize::MAX).unwrap_err().code(),
        ErrorCode::ResourceLimit
    );
    assert_eq!(
        batch_length(envelope, batch::MAX_BATCH_BYTES)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceLimit,
    );
}
