//! One immutable intent encoding; every installed image still crosses live readback.
use super::*;
use crate::{
    BackupPreparationPhase, BackupPreparationRequest, BackupTopologyTarget,
    BackupTransactionRequest, CommitRequest, Config, Entry, NamespaceOperation,
    NamespaceOperationKind, OperationId, Repository, RepositoryName,
};
use std::{collections::BTreeMap, fs, os::unix::fs::DirBuilderExt, path::Path};

fn operation() -> OperationId {
    OperationId::generate().unwrap()
}

fn save(engine: &mut Engine, repository: &Repository, body: &[u8]) -> Entry {
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "nested/file", body)
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: operation(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "preparation fixture".into(),
            request_digest: None,
        })
        .unwrap();
    engine
        .namespace_tree(&repository.main_branch_id)
        .unwrap()
        .get("nested/file")
        .unwrap()
        .clone()
}

fn target(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
    path
}

fn fixture() -> (tempfile::TempDir, Engine, BackupTransactionRequest) {
    let temp = tempfile::Builder::new()
        .permissions(std::os::unix::fs::PermissionsExt::from_mode(0o700))
        .tempdir()
        .unwrap();
    let config =
        Config::development((0..3).map(|index| temp.path().join(format!("disk-{index}"))))
            .unwrap();
    let mut engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
    engine
        .create_disk_root("P01", "admin", operation())
        .unwrap();
    let repository = engine
        .create_repository(
            &RepositoryName::parse("WholeRoute").unwrap(),
            "P01",
            3,
            "admin",
            operation(),
        )
        .unwrap();
    let old = save(&mut engine, &repository, b"retained first body");
    let latest = save(&mut engine, &repository, b"retained current body");
    assert_eq!(old.file_id, latest.file_id);
    assert_ne!(old.version_id, latest.version_id);
    engine
        .file_namespace(
            &repository.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::Mkdir, "nested/empty", None)
                .unwrap(),
            "admin",
            operation(),
        )
        .unwrap();
    assert_eq!(engine.state().versions.len(), 2);
    assert!(engine.state().directory_git_repositories.is_empty());
    let paths = vec![
        target(temp.path(), "selected-one"),
        target(temp.path(), "selected-two"),
    ];
    let request = BackupTransactionRequest {
        preparation: BackupPreparationRequest {
            operation_id: operation(),
            principal: "admin".into(),
            source_slot_id: "B01".into(),
            target_paths: paths.clone(),
            expected_sequence: engine.state().sequence,
        },
        targets: paths
            .iter()
            .enumerate()
            .map(|(index, _)| BackupTopologyTarget {
                disk_id: crate::new_id("disk").unwrap(),
                slot_id: format!("C{:02}", index + 2),
                upstream_slot_id: "B01".into(),
            })
            .collect(),
    };
    (temp, engine, request)
}

fn prepare_entities(engine: &mut Engine, request: &BackupTransactionRequest) {
    let operation = request.preparation.operation_id;
    engine
        .start_backup_transaction(
            request.clone(),
            request
                .preparation
                .target_paths
                .iter()
                .map(|path| fs::File::open(path).unwrap())
                .collect(),
        )
        .unwrap();
    for phase in [
        BackupPreparationPhase::HistoryPrepared,
        BackupPreparationPhase::NativeControlPrepared,
        BackupPreparationPhase::ObjectDatabasesPrepared,
        BackupPreparationPhase::ScopedDatabasesPrepared,
        BackupPreparationPhase::CutoverInputsPrepared,
    ] {
        assert_eq!(engine.wait_backup_preparation().unwrap().phase, phase);
        assert!(engine.membership_fence_status().is_none());
        assert!(!engine.state().operations.contains_key(&operation));
        if phase != BackupPreparationPhase::CutoverInputsPrepared {
            engine
                .continue_backup_transaction(operation, "admin")
                .unwrap();
        }
    }
    assert_eq!(
        engine.continue_backup_transaction(operation, "admin").unwrap().phase,
        BackupPreparationPhase::CutoverInputsPrepared,
    );
    engine
        .begin_backup_cutover_fence(operation, "admin")
        .unwrap();
    engine
        .continue_backup_transaction(operation, "admin")
        .unwrap();
    assert_eq!(
        engine.wait_backup_preparation().unwrap().phase,
        BackupPreparationPhase::TargetEntitiesPrepared,
    );
    assert_eq!(
        engine.continue_backup_transaction(operation, "admin").unwrap().phase,
        BackupPreparationPhase::TargetEntitiesPrepared,
    );
    for disk in &engine.config().disks {
        assert!(!disk.root.join("metadata/membership-generations").exists());
    }
}


// Evidence covers directory structure and file bytes, not permissions or timestamps.
fn tree(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn visit(root: &Path, path: &Path, entries: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        let metadata = fs::symlink_metadata(path).unwrap();
        let relative = path.strip_prefix(root).unwrap().to_path_buf();
        if metadata.is_dir() {
            assert!(entries.insert(relative, None).is_none());
            for entry in fs::read_dir(path).unwrap() {
                visit(root, &entry.unwrap().path(), entries);
            }
        } else {
            assert!(metadata.is_file());
            assert!(entries.insert(relative, Some(fs::read(path).unwrap())).is_none());
        }
    }
    let mut entries = BTreeMap::new();
    visit(root, root, &mut entries);
    entries
}

fn compare_encoding(task: &Task) {
    // Reference preserves the old hash-then-independent-encoding evaluation order.
    let expected = (|| {
        let hash = object_hash(FORMAT, task)?;
        let intent = task.bytes()?;
        Ok::<_, Error>((intent, hash))
    })();
    let actual = (|| {
        let (intent, hash) = storage_contract::encode_hashed_object(FORMAT, task)?;
        let intent = Task::check_intent_bytes(intent)?;
        Ok::<_, Error>((intent, hash))
    })();
    match (expected, actual) {
        (Ok(expected), Ok(actual)) => assert_eq!(actual, expected),
        (Err(expected), Err(actual)) => {
            assert_eq!(actual.code(), expected.code());
            assert_eq!(error_chain(&actual), error_chain(&expected));
        }
        _ => panic!("encoding differential changed success or rejection"),
    }
}

fn error_chain(error: &Error) -> Vec<String> {
    let mut chain = Vec::new();
    let mut current: Option<&dyn std::error::Error> = Some(error);
    while let Some(error) = current {
        chain.push(error.to_string());
        current = error.source();
    }
    chain
}

fn assert_cancelled(task: &Task, targets: &[Target]) {
    let stop = AtomicBool::new(true);
    let expected = preparation_checkpoint(&stop).unwrap_err();
    let actual = verify_installed(task, targets, &stop).unwrap_err();
    assert_eq!(actual.code(), ErrorCode::OperationAborted);
    assert_eq!(actual.code(), expected.code());
    assert_eq!(error_chain(&actual), error_chain(&expected));
}

#[test]
fn real_intent_encoding_preserves_bytes_limits_and_live_installation_admission() {
    let (temp, mut engine, request) = fixture();
    prepare_entities(&mut engine, &request);
    if let Some(worker) = engine.backup_preparation.as_mut() {
        worker.join().unwrap();
    }
    let task = Task::current(&engine, request.preparation.operation_id, "admin").unwrap();
    let targets = acquire(&engine, &task).unwrap();
    let stop = AtomicBool::new(false);
    let before = tree(temp.path());
    compare_encoding(&task);
    verify_installed(&task, &targets, &stop).unwrap();
    verify_installed(&task, &targets, &stop).unwrap();
    assert_eq!(tree(temp.path()), before);

    // This is only an encoder boundary fixture, never admitted as a valid task.
    // ASCII padding changes exactly its own byte length, not canonical escaping.
    let original_len = task.bytes().unwrap().len();
    assert!(original_len < MAX_INTENT);
    for bound in [MAX_INTENT - 1, MAX_INTENT, MAX_INTENT + 1] {
        let mut padded = task.clone();
        padded.format.push_str(&"x".repeat(bound - original_len));
        assert_eq!(canonical_json(&padded).unwrap().len(), bound);
        compare_encoding(&padded);
        if bound <= MAX_INTENT {
            assert_eq!(padded.bytes().unwrap().len(), bound);
        } else {
            assert_eq!(
                padded.bytes().unwrap_err().code(),
                ErrorCode::ResourceLimit,
            );
        }
    }
    assert_eq!(tree(temp.path()), before);

    for name in [INTENT, ACCEPTANCE, COMPLETE] {
        let path = targets[1].root.path().join(name);
        let original = fs::read(&path).unwrap();
        let mut changed = original.clone();
        assert!(!changed.is_empty());
        changed[0] ^= 1;
        fs::write(&path, changed).unwrap();
        let damaged = tree(temp.path());
        assert!(verify_installed(&task, &targets, &stop).is_err());
        assert_cancelled(&task, &targets);
        assert_eq!(tree(temp.path()), damaged);
        // Restore only bytes deliberately changed by this fixture.
        fs::write(&path, original).unwrap();
        verify_installed(&task, &targets, &stop).unwrap();
        assert_eq!(tree(temp.path()), before);
    }

    assert_cancelled(&task, &targets);
    assert_eq!(tree(temp.path()), before);

    let path = targets[0].root.path().to_owned();
    let displaced = temp.path().join("displaced-target");
    fs::rename(&path, &displaced).unwrap();
    fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
    let replaced = tree(temp.path());
    assert!(verify_installed(&task, &targets, &stop).is_err());
    assert_eq!(tree(temp.path()), replaced);
    fs::remove_dir(&path).unwrap();
    fs::rename(&displaced, &path).unwrap();
    verify_installed(&task, &targets, &stop).unwrap();
    assert_eq!(tree(temp.path()), before);
}
