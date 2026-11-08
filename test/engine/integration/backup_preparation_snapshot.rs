//! Real prepared whole-State closure. No restore/adopt or topology switch API.
use super::*;
use storage_engine::{
    FileHistoryCopyRequest, UndoFileDeletionRequest, canonical_json,
    offline_check_backup_preparation,
};

fn prepare(engine: &mut Engine, destination: &Path) {
    engine
        .start_backup_preparation(
            request(engine, vec![destination.to_owned()], op()),
            fs::File::open(destination).unwrap(),
        )
        .unwrap();
    let status = engine.wait_backup_preparation().unwrap();
    assert_eq!(
        status.phase,
        BackupPreparationPhase::HistoryPrepared,
        "{status:?}"
    );
}

fn snapshot_object(destination: &Path, kind: &str, hash: &str) -> PathBuf {
    destination
        .join("pool-snapshot/objects")
        .join(kind)
        .join(format!("{hash}.json"))
}
fn operations_page(destination: &Path) -> PathBuf {
    let job: json_codec::Value =
        json_codec::from_slice(&fs::read(destination.join("backup-preparation.json")).unwrap())
            .unwrap();
    let root: json_codec::Value = json_codec::from_slice(
        &fs::read(snapshot_object(
            destination,
            "snapshot",
            job["snapshot"].as_str().unwrap(),
        ))
        .unwrap(),
    )
    .unwrap();
    let reference = &root["maps"]["operations"];
    assert_eq!(
        reference["kind"], "state-page",
        "fixture must fit one operations page"
    );
    snapshot_object(
        destination,
        "state-page",
        reference["hash"].as_str().unwrap(),
    )
}

#[test]
fn complete_snapshot_keeps_undone_audit_after_source_is_removed() {
    let (temp, mut engine, repository) = setup();
    let original = save(&mut engine, &repository, "source", b"full original content");
    let deleted = engine
        .soft_delete_file(DeleteFileRequest {
            target: FileDeletionTarget {
                repository_id: &HistoryRepositoryId::parse(&repository.repository_id).unwrap(),
                file_id: &HistoryFileId::parse(&original.file_id).unwrap(),
                path: &name_policy::LookupPath::parse("source").unwrap(),
            },
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    let deletion_node = engine
        .file_deletion(&original.file_id)
        .unwrap()
        .node_id
        .clone();
    let undone = engine
        .undo_file_deletion(UndoFileDeletionRequest {
            file_id: &HistoryFileId::parse(&original.file_id).unwrap(),
            deletion_node_id: &deletion_node,
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    save(
        &mut engine,
        &repository,
        "source",
        b"saved after original D was undone",
    );
    let second = engine
        .create_repository(
            &RepositoryName::parse("AlsoComplete").unwrap(),
            "P01",
            1,
            "admin",
            op(),
        )
        .unwrap();
    save(&mut engine, &second, "other", b"another complete history");
    // The latest publication no longer happens to carry either old receipt.
    assert!(engine.state().file_deletions.is_empty());
    let expected = canonical_json(engine.state()).unwrap();
    let expected_hash =
        storage_engine::object_hash("backup-prepared-state", engine.state()).unwrap();
    let operations = engine.state().operations.len();
    let principals = engine.state().operation_principals.len();
    let source_roots: Vec<_> = engine
        .config()
        .disks
        .iter()
        .map(|d| d.root.clone())
        .collect();
    let destination = target(temp.path(), "full-snapshot");
    prepare(&mut engine, &destination);
    assert_eq!(canonical_json(engine.state()).unwrap(), expected);
    let page: json_codec::Value =
        json_codec::from_slice(&fs::read(operations_page(&destination)).unwrap()).unwrap();
    for receipt in [&deleted, &undone] {
        assert_eq!(
            page[receipt.operation_id.to_string()],
            json_codec::to_value(receipt).unwrap()
        );
    }
    drop(engine);
    for (index, root) in source_roots.iter().enumerate() {
        fs::rename(root, temp.path().join(format!("unavailable-{index}"))).unwrap();
    }
    fs::rename(
        temp.path().join("system"),
        temp.path().join("unavailable-system"),
    )
    .unwrap();
    let report = offline_check_backup_preparation(&destination).unwrap();
    assert_eq!(report.state_hash, expected_hash);
    assert_eq!(report.operations, operations);
    assert_eq!(report.operation_principals, principals);
    assert_eq!(report.histories.len(), 2);
    assert_eq!(
        report.histories.iter().map(|h| h.full_files).sum::<usize>(),
        3
    );
    // A complete inspection report is not an Engine root or activation proof.
    assert!(!destination.join("config.json").exists());
    assert!(Engine::open(&destination).is_err());
}

#[test]
fn missing_snapshot_page_blocks_offline_read_then_owner_resumes_exact_material() {
    let (temp, mut engine, repository) = setup();
    save(&mut engine, &repository, "file", b"body remains available");
    let destination = target(temp.path(), "resume-snapshot");
    prepare(&mut engine, &destination);
    let report = offline_check_backup_preparation(&destination).unwrap();
    let marker = fs::read(destination.join("history-prepared.json")).unwrap();
    // The borrowed typed encoder retains the previous marker's exact format.
    assert_eq!(
        marker,
        canonical_json(&json_codec::json!({
            "format": "zxs-backup-history-preparation",
            "job_hash": &report.job_hash,
            "histories": &report.histories,
        }))
        .unwrap()
    );
    let page = operations_page(&destination);
    let expected = fs::read(&page).unwrap();
    drop(engine);
    fs::remove_file(&page).unwrap();
    assert!(offline_check_backup_preparation(&destination).is_err());
    let mut reopened = Engine::open(&temp.path().join("system")).unwrap();
    let status = reopened.wait_backup_preparation().unwrap();
    assert_eq!(
        status.phase,
        BackupPreparationPhase::HistoryPrepared,
        "{status:?}"
    );
    assert_eq!(fs::read(page).unwrap(), expected);
    assert_eq!(
        fs::read(destination.join("history-prepared.json")).unwrap(),
        marker
    );
    assert_eq!(
        canonical_json(&offline_check_backup_preparation(&destination).unwrap()).unwrap(),
        canonical_json(&report).unwrap()
    );
}

#[test]
fn corrupted_snapshot_page_is_preserved_and_never_repaired_over_by_resume() {
    let (temp, mut engine, repository) = setup();
    save(&mut engine, &repository, "file", b"retained original");
    let destination = target(temp.path(), "corrupt-snapshot");
    prepare(&mut engine, &destination);
    let old_state = canonical_json(engine.state()).unwrap();
    let page = operations_page(&destination);
    let marker = fs::read(destination.join("history-prepared.json")).unwrap();
    drop(engine);
    fs::write(&page, b"{}").unwrap();
    assert!(offline_check_backup_preparation(&destination).is_err());
    let mut reopened = Engine::open(&temp.path().join("system")).unwrap();
    let status = reopened.wait_backup_preparation().unwrap();
    assert_eq!(status.phase, BackupPreparationPhase::Blocked, "{status:?}");
    assert_eq!(status.error_code, Some(ErrorCode::ObjectCollision));
    assert_eq!(fs::read(page).unwrap(), b"{}");
    assert_eq!(
        fs::read(destination.join("history-prepared.json")).unwrap(),
        marker
    );
    assert_eq!(canonical_json(reopened.state()).unwrap(), old_state);
}

#[test]
fn task_subset_and_false_completion_do_not_pass_target_local_verification() {
    let (temp, mut engine, repository) = setup();
    save(&mut engine, &repository, "file", b"complete selected unit");
    let destination = target(temp.path(), "reject-subset");
    prepare(&mut engine, &destination);
    let intent = destination.join("backup-preparation.json");
    let original = fs::read(&intent).unwrap();
    let mut job: json_codec::Value = json_codec::from_slice(&original).unwrap();
    job["history_roots"] = json_codec::json!({});
    fs::write(&intent, canonical_json(&job).unwrap()).unwrap();
    assert_eq!(
        offline_check_backup_preparation(&destination)
            .unwrap_err()
            .code(),
        ErrorCode::HistoryPackageInvalid
    );
    fs::write(&intent, &original).unwrap();
    let publication = destination.join("pool-snapshot/publication.json");
    let old_proof = fs::read(&publication).unwrap();
    let mut proof: json_codec::Value = json_codec::from_slice(&old_proof).unwrap();
    proof["config"]["max_entries"] = json_codec::json!(1);
    fs::write(&publication, canonical_json(&proof).unwrap()).unwrap();
    assert_eq!(
        offline_check_backup_preparation(&destination)
            .unwrap_err()
            .code(),
        ErrorCode::HistoryPackageInvalid
    );
    fs::write(&publication, old_proof).unwrap();
    let marker = destination.join("snapshot-prepared.json");
    let mut completed: json_codec::Value =
        json_codec::from_slice(&fs::read(&marker).unwrap()).unwrap();
    completed["operations"] = json_codec::json!(0);
    fs::write(&marker, canonical_json(&completed).unwrap()).unwrap();
    assert_eq!(
        offline_check_backup_preparation(&destination)
            .unwrap_err()
            .code(),
        ErrorCode::HistoryPackageInvalid
    );
    // The original history-only marker does not upgrade missing full evidence.
    assert!(destination.join("history-prepared.json").exists());
}

#[test]
fn complete_snapshot_cannot_mask_a_missing_body_or_borrow_the_live_source() {
    let (temp, mut engine, repository) = setup();
    let original = save(&mut engine, &repository, "file", b"all retained body bytes");
    let destination = target(temp.path(), "missing-body");
    prepare(&mut engine, &destination);
    offline_check_backup_preparation(&destination).unwrap();
    let body = destination
        .join("histories")
        .join(&repository.history_tree_id)
        .join("versions")
        .join(&original.version_id)
        .join("content")
        .join("file");
    assert_eq!(fs::read(&body).unwrap(), b"all retained body bytes");
    fs::remove_file(&body).unwrap();
    assert!(engine.open_content(&original.version_id).is_ok());
    assert!(offline_check_backup_preparation(&destination).is_err());
    assert!(!body.exists());
}

#[test]
fn multi_primary_retained_copy_stays_outside_preparation_without_losing_audit() {
    let temp = tempfile::Builder::new()
        .permissions(std::os::unix::fs::PermissionsExt::from_mode(0o700))
        .tempdir()
        .unwrap();
    let mut config =
        Config::development((0..4).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
    config.disks[3].level = 1;
    config.disks[3].upstream_slot_id = None;
    config.disks[3].metadata_member = false;
    let mut engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
    let source = engine.create_disk_root("P01", "admin", op()).unwrap();
    let other = engine
        .create_repository(
            &RepositoryName::parse("Independent").unwrap(),
            "D04",
            1,
            "admin",
            op(),
        )
        .unwrap();
    let original = save(&mut engine, &source, "source", b"retained source");
    let deleted = engine
        .soft_delete_file(DeleteFileRequest {
            target: FileDeletionTarget {
                repository_id: &HistoryRepositoryId::parse(&source.repository_id).unwrap(),
                file_id: &HistoryFileId::parse(&original.file_id).unwrap(),
                path: &name_policy::LookupPath::parse("source").unwrap(),
            },
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    let node = engine
        .file_deletion(&original.file_id)
        .unwrap()
        .node_id
        .clone();
    let undone = engine
        .undo_file_deletion(UndoFileDeletionRequest {
            file_id: &HistoryFileId::parse(&original.file_id).unwrap(),
            deletion_node_id: &node,
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    let copied = engine
        .copy_retained_file_history(
            &FileHistoryCopyRequest {
                source_repository_id: source.repository_id.clone(),
                source_path: "source".into(),
                expected_source_head: engine.state().branches[&source.main_branch_id]
                    .head_commit_id
                    .clone(),
                target_repository_id: other.repository_id.clone(),
                target_name: "copied".into(),
                expected_target_head: engine.state().branches[&other.main_branch_id]
                    .head_commit_id
                    .clone(),
            },
            "admin",
            op(),
        )
        .unwrap();
    assert_eq!(
        copied.result["retained_lifecycle"]["events"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let before = canonical_json(engine.state()).unwrap();
    let destination = target(temp.path(), "not-yet-supported");
    let error = engine
        .start_backup_preparation(
            request(&engine, vec![destination.clone()], op()),
            fs::File::open(&destination).unwrap(),
        )
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::ConfigConflict);
    assert_eq!(fs::read_dir(&destination).unwrap().count(), 0);
    assert!(
        !temp
            .path()
            .join("system/backup-preparation/job.json")
            .exists()
    );
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    drop(engine);
    let reopened = Engine::open(&temp.path().join("system")).unwrap();
    for receipt in [deleted, undone, copied] {
        assert_eq!(reopened.operation(receipt.operation_id).unwrap(), receipt);
    }
    assert_eq!(canonical_json(reopened.state()).unwrap(), before);
}
