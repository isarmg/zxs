//! Explicit complete-copy batches, not an authority switch or foreign admission.
//! New test source only; this slice was not compiled or run under the current policy.
use super::*;
use storage_engine::{
    canonical_json, offline_check_backup_preparation, offline_check_backup_preparation_batch,
};

fn start(
    engine: &mut Engine,
    targets: &[PathBuf],
    operation: OperationId,
) -> BackupPreparationRequest {
    let request = request(engine, targets.to_vec(), operation);
    engine
        .start_backup_batch_preparation(
            request.clone(),
            targets.iter().map(|p| fs::File::open(p).unwrap()).collect(),
        )
        .unwrap();
    request
}

#[test]
fn every_explicit_target_keeps_all_histories_nested_repositories_and_original_receipts() {
    let (temp, mut engine, repository) = setup();
    save(
        &mut engine,
        &repository,
        "parent/child/body",
        b"old retained bytes",
    );
    save(
        &mut engine,
        &repository,
        "parent/child/body",
        b"new retained bytes",
    );
    for path in ["parent/child", "parent"] {
        let head = engine.state().branches[&repository.main_branch_id]
            .head_commit_id
            .clone();
        engine
            .initialize_directory_git(
                &repository.repository_id,
                &storage_contract::directory_git::DirectoryGitInitializeBody {
                    path: path.into(),
                    expected_namespace_head: head,
                    operation_id: op(),
                },
                "admin",
            )
            .unwrap();
    }
    let second = engine
        .create_repository(
            &RepositoryName::parse("AnotherUnit").unwrap(),
            "P01",
            1,
            "admin",
            op(),
        )
        .unwrap();
    save(&mut engine, &second, "file", b"another complete repository");
    let expected = canonical_json(engine.state()).unwrap();
    let state_hash = storage_engine::object_hash("backup-prepared-state", engine.state()).unwrap();
    let paths = vec![target(temp.path(), "copy-a"), target(temp.path(), "copy-b")];
    let request = start(&mut engine, &paths, op());
    let done = engine.wait_backup_preparation().unwrap();
    assert_eq!(
        done.phase,
        BackupPreparationPhase::HistoryPrepared,
        "{done:?}"
    );
    assert_eq!((done.completed_files, done.total_files), (6, 6));
    assert_eq!(canonical_json(engine.state()).unwrap(), expected);
    let report = offline_check_backup_preparation_batch(&paths).unwrap();
    assert_eq!(report.targets.len(), 2);
    for target in &report.targets {
        assert_eq!(target.state_hash, state_hash);
        assert_eq!(target.histories.len(), 2);
    }
    let task = fs::read(temp.path().join("system/backup-preparation/batch.json")).unwrap();
    engine
        .start_backup_batch_preparation(
            request,
            paths.iter().map(|p| fs::File::open(p).unwrap()).collect(),
        )
        .unwrap();
    assert_eq!(
        engine.wait_backup_preparation().unwrap().phase,
        BackupPreparationPhase::HistoryPrepared
    );
    assert_eq!(
        fs::read(temp.path().join("system/backup-preparation/batch.json")).unwrap(),
        task
    );
    let sources = engine
        .config()
        .disks
        .iter()
        .map(|d| d.root.clone())
        .collect::<Vec<_>>();
    drop(engine);
    for (i, source) in sources.iter().enumerate() {
        fs::rename(source, temp.path().join(format!("removed-source-{i}"))).unwrap();
    }
    fs::rename(
        temp.path().join("system"),
        temp.path().join("removed-system"),
    )
    .unwrap();
    let report = offline_check_backup_preparation_batch(&paths).unwrap();
    assert_eq!(report.targets.len(), 2);
    fs::rename(&paths[1], temp.path().join("absent-peer")).unwrap();
    assert!(offline_check_backup_preparation_batch(&paths).is_err());
    assert!(offline_check_backup_preparation_batch(&paths[..1]).is_err());
    assert_eq!(
        offline_check_backup_preparation(&paths[0])
            .unwrap()
            .state_hash,
        state_hash
    );
    assert!(!paths[0].join("disk.json").exists());
    assert!(Engine::open(&paths[0]).is_err());
}

#[test]
fn nonempty_or_overlapping_batch_is_rejected_before_system_or_other_target_write() {
    let (temp, mut engine, repository) = setup();
    save(&mut engine, &repository, "file", b"original remains");
    let a = target(temp.path(), "clean");
    let b = target(temp.path(), "occupied");
    fs::write(b.join("unrelated"), b"do not replace").unwrap();
    let expected = canonical_json(engine.state()).unwrap();
    let paths = vec![a.clone(), b.clone()];
    let result = engine.start_backup_batch_preparation(
        request(&engine, paths.clone(), op()),
        paths.iter().map(|p| fs::File::open(p).unwrap()).collect(),
    );
    assert_eq!(result.unwrap_err().code(), ErrorCode::ObjectCollision);
    assert!(fs::read_dir(&a).unwrap().next().is_none());
    assert!(
        !temp
            .path()
            .join("system/backup-preparation/batch.json")
            .exists()
    );
    assert_eq!(fs::read(b.join("unrelated")).unwrap(), b"do not replace");
    let paths = vec![a.clone(), a.clone()];
    assert!(
        engine
            .start_backup_batch_preparation(
                request(&engine, paths.clone(), op()),
                paths.iter().map(|p| fs::File::open(p).unwrap()).collect()
            )
            .is_err()
    );
    assert_eq!(canonical_json(engine.state()).unwrap(), expected);
}

#[test]
fn blocked_peer_is_not_hidden_by_complete_target_and_same_task_resumes_without_resampling() {
    let (temp, mut engine, repository) = setup();
    let file = save(
        &mut engine,
        &repository,
        "file",
        b"every target must retain these bytes",
    );
    let paths = vec![target(temp.path(), "one"), target(temp.path(), "two")];
    let original_request = start(&mut engine, &paths, op());
    assert_eq!(
        engine.wait_backup_preparation().unwrap().phase,
        BackupPreparationPhase::HistoryPrepared
    );
    let marker = fs::read(paths[0].join("backup-batch-prepared.json")).unwrap();
    let intent = fs::read(temp.path().join("system/backup-preparation/batch.json")).unwrap();
    let version = &engine.state().versions[&file.version_id];
    // The package format's helper supplies the exact stored version path.
    let actual = find_body(&paths[1], &version.creation_name);
    assert_eq!(
        fs::read(&actual).unwrap(),
        b"every target must retain these bytes"
    );
    fs::remove_file(&actual).unwrap();
    assert!(offline_check_backup_preparation_batch(&paths).is_err());
    drop(engine);
    let mut reopened = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(
        reopened.wait_backup_preparation().unwrap().phase,
        BackupPreparationPhase::HistoryPrepared
    );
    assert_eq!(
        fs::read(&actual).unwrap(),
        b"every target must retain these bytes"
    );
    assert_eq!(
        fs::read(paths[0].join("backup-batch-prepared.json")).unwrap(),
        marker
    );
    assert_eq!(
        fs::read(temp.path().join("system/backup-preparation/batch.json")).unwrap(),
        intent
    );
    let mut different = original_request.clone();
    different.target_paths.reverse();
    assert_eq!(
        reopened
            .start_backup_batch_preparation(
                different,
                paths
                    .iter()
                    .rev()
                    .map(|p| fs::File::open(p).unwrap())
                    .collect()
            )
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyKeyReused
    );
    // A retained batch cannot be silently replaced with the old one-target API.
    assert_eq!(
        reopened
            .start_backup_preparation(
                request(&reopened, vec![paths[0].clone()], op()),
                fs::File::open(&paths[0]).unwrap()
            )
            .unwrap_err()
            .code(),
        ErrorCode::ConfigConflict
    );
}

fn find_body(root: &Path, name: &str) -> PathBuf {
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            if let Some(found) = find_body_optional(&path, name) {
                return found;
            }
        } else if path.file_name().unwrap() == name {
            return path;
        }
    }
    panic!("target body missing");
}
fn find_body_optional(root: &Path, name: &str) -> Option<PathBuf> {
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            if let Some(found) = find_body_optional(&path, name) {
                return Some(found);
            }
        } else if path.file_name().unwrap() == name {
            return Some(path);
        }
    }
    None
}
