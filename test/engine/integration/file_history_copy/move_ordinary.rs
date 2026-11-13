//! Ordinary preparation and recovery, never a journal decision, live SMB or
//! power-loss claim: original sources remain and no move is published.
use super::move_decision::plan;
use super::*;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

const STAGE: &str = "file-history-move-ordinary";
#[path = "move_installations.rs"]
mod installations;
#[path = "move_journal.rs"]
mod journal;
fn input(fixture: &mut Fixture) -> (FileHistoryMoveProjection, FileHistoryMoveRecoveryInfo) {
    let (projection, recovery) = plan(fixture);
    let cancel = HistoryWindowCancellation::new();
    fixture
        .engine
        .prepare_file_history_move_decision_inputs(&projection, recovery.digest(), &cancel)
        .unwrap();
    fixture
        .engine
        .prepare_file_history_move_native_operation(&projection, recovery.digest(), &cancel)
        .unwrap();
    fixture
        .engine
        .prepare_file_history_move_scoped_databases(&projection, recovery.digest(), &cancel)
        .unwrap();
    fixture
        .engine
        .select_file_history_move_joint_choice(&projection, recovery.digest(), &cancel)
        .unwrap();
    fixture
        .engine
        .prepare_file_history_move_candidate(&projection, recovery.digest(), &cancel)
        .unwrap();
    (projection, recovery)
}
fn prepare(
    engine: &mut Engine,
    projection: &FileHistoryMoveProjection,
    recovery: &FileHistoryMoveRecoveryInfo,
) -> FileHistoryMoveOrdinaryPreparation {
    engine
        .prepare_file_history_move_ordinary(
            projection,
            recovery.digest(),
            &HistoryWindowCancellation::new(),
        )
        .unwrap()
}
fn target(engine: &Engine) -> PathBuf {
    engine
        .config()
        .disks
        .iter()
        .find(|disk| disk.slot_id == "D04")
        .unwrap()
        .root
        .clone()
}
fn directory(root: &Path, history: &str, version: &str) -> PathBuf {
    root.join("histories")
        .join(history)
        .join("versions")
        .join(version)
}
fn named(root: &Path, history: &str, file: &str) -> PathBuf {
    root.join("histories")
        .join(history)
        .join("file-containers")
        .join(file)
        .join("copied.txt")
}
fn stored_version(root: &Path, history: &str, version: &str) -> FileVersion {
    storage_contract::decode_strict(
        &fs::read(directory(root, history, version).join("version.json")).unwrap(),
    )
    .unwrap()
}
fn saved_databases(root: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            saved_databases(&path, out);
        } else if matches!(
            path.file_name().unwrap().to_str().unwrap(),
            ".zxs-file.sqlite" | ".zxs-disk.sqlite" | ".zxs-disk-history.sqlite"
        ) {
            out.insert(path.clone(), fs::read(path).unwrap());
        }
    }
}
fn unchanged(engine: &Engine) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut saved = BTreeMap::new();
    for disk in &engine.config().disks {
        saved_databases(&disk.root, &mut saved);
        if disk.metadata_member {
            let path = disk.root.join("metadata/application-log/journal.bin");
            saved.insert(path.clone(), fs::read(path).unwrap());
        }
    }
    saved
}

#[test]
fn all_ordinary_copies_are_independent_but_the_same_move_and_databases_stay_unpublished() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = input(&mut fixture);
    let before = fixture.engine.state().clone();
    let saved = unchanged(&fixture.engine);
    let result = prepare(&mut fixture.engine, &projection, &recovery);
    assert_eq!(result.operation_id(), projection.operation_id());
    assert_eq!(result.outstanding().len(), 5);
    assert_eq!(result.copies(), projection.version_mapping().len());
    assert_eq!(result.target_disks(), 1);
    assert_eq!(result.disk_database_intents(), 4);
    assert!(result.file_database_intents() >= 2);
    let root = target(&fixture.engine);
    let container = named(
        &root,
        &fixture.target.history_tree_id,
        projection.target_file_id().as_str(),
    );
    for (source, target) in projection.version_mapping() {
        let version = stored_version(&root, &fixture.target.history_tree_id, target);
        let final_body = installed_body(&container, &version);
        let original = root
            .join("file-history-move-target/versions")
            .join(target)
            .join("content")
            .join(&version.creation_name);
        assert_eq!(
            fs::read(&final_body).unwrap(),
            bytes(&fixture.engine, source)
        );
        assert_eq!(fs::read(&original).unwrap(), fs::read(&final_body).unwrap());
        assert_ne!(
            fs::metadata(&original).unwrap().ino(),
            fs::metadata(&final_body).unwrap().ino()
        );
        assert_eq!(fs::metadata(&final_body).unwrap().nlink(), 1);
        assert!(
            !directory(&root, &fixture.target.history_tree_id, target)
                .join("content")
                .join(&version.creation_name)
                .exists()
        );
        assert!(!root.join(STAGE).join("versions").join(target).exists());
    }
    assert!(!container.join(".zxs-file.sqlite").exists());
    assert!(!container.join("container.json").exists());
    assert_eq!(fixture.engine.state(), &before);
    assert_eq!(unchanged(&fixture.engine), saved);
    assert!(
        fixture
            .engine
            .file_deletion(projection.capture().source_file_id().as_str())
            .is_none()
    );
    assert!(
        fixture
            .engine
            .namespace_tree(&fixture.target.main_branch_id)
            .unwrap()
            .entries()
            .is_empty()
    );
    assert!(
        !fixture
            .engine
            .state()
            .operations
            .contains_key(&projection.operation_id())
    );
    assert!(
        fixture
            .engine
            .inspect_file_history_move_joint_choice()
            .is_some()
    );
}

#[test]
fn cold_retry_preserves_the_ordinary_inode_and_original_pending_scopes() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = input(&mut fixture);
    let first = prepare(&mut fixture.engine, &projection, &recovery);
    let root = target(&fixture.engine);
    let version_id = projection.version_mapping().values().next().unwrap();
    let version = stored_version(&root, &fixture.target.history_tree_id, version_id);
    let body = installed_body(
        &named(
            &root,
            &fixture.target.history_tree_id,
            projection.target_file_id().as_str(),
        ),
        &version,
    );
    let inode = fs::metadata(&body).unwrap().ino();
    let origin = fs::read(
        root.join(STAGE)
            .join("origins")
            .join(format!("{version_id}.json")),
    )
    .unwrap();
    let before = fixture.engine.state().clone();
    let saved = unchanged(&fixture.engine);
    let Fixture { temp, engine, .. } = fixture;
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    let restored = engine
        .resume_file_history_move_recovery(
            recovery.operation_id(),
            recovery.digest(),
            "admin",
            &HistoryWindowCancellation::new(),
        )
        .unwrap();
    let read = engine
        .read_file_history_move_ordinary(
            &restored,
            recovery.digest(),
            &HistoryWindowCancellation::new(),
        )
        .unwrap();
    assert_eq!(read.file_database_intents(), first.file_database_intents());
    assert_eq!(read.disk_database_intents(), first.disk_database_intents());
    let repeated = prepare(&mut engine, &restored, &recovery);
    assert_eq!(repeated.candidate_hash(), first.candidate_hash());
    assert_eq!(
        repeated.file_database_intents(),
        first.file_database_intents()
    );
    assert_eq!(fs::metadata(body).unwrap().ino(), inode);
    assert_eq!(
        fs::read(
            root.join(STAGE)
                .join("origins")
                .join(format!("{version_id}.json"))
        )
        .unwrap(),
        origin
    );
    assert_eq!(engine.state(), &before);
    assert_eq!(unchanged(&engine), saved);
}

#[test]
fn original_two_rename_interruption_resumes_without_recopying_or_publishing() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = input(&mut fixture);
    prepare(&mut fixture.engine, &projection, &recovery);
    let root = target(&fixture.engine);
    let id = projection.version_mapping().values().next().unwrap();
    let version = stored_version(&root, &fixture.target.history_tree_id, id);
    let formal = directory(&root, &fixture.target.history_tree_id, id);
    let body = installed_body(
        &named(
            &root,
            &fixture.target.history_tree_id,
            projection.target_file_id().as_str(),
        ),
        &version,
    );
    let inode = fs::metadata(&body).unwrap().ino();
    fs::rename(&body, formal.join("content").join(&version.creation_name)).unwrap();
    fs::rename(&formal, root.join(STAGE).join("versions").join(id)).unwrap();
    assert!(
        fixture
            .engine
            .read_file_history_move_ordinary(
                &projection,
                recovery.digest(),
                &HistoryWindowCancellation::new()
            )
            .is_err()
    );
    prepare(&mut fixture.engine, &projection, &recovery);
    assert_eq!(fs::metadata(body).unwrap().ino(), inode);
    assert!(
        !fixture
            .engine
            .state()
            .operations
            .contains_key(&projection.operation_id())
    );
}

#[test]
fn equal_bytes_on_a_replacement_inode_cannot_stand_in_for_the_original_ordinary_copy() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = input(&mut fixture);
    prepare(&mut fixture.engine, &projection, &recovery);
    let root = target(&fixture.engine);
    let id = projection.version_mapping().values().next().unwrap();
    let version = stored_version(&root, &fixture.target.history_tree_id, id);
    let body = installed_body(
        &named(
            &root,
            &fixture.target.history_tree_id,
            projection.target_file_id().as_str(),
        ),
        &version,
    );
    let retained = root.join("retained-original-body");
    fs::rename(&body, &retained).unwrap();
    fs::write(&body, fs::read(&retained).unwrap()).unwrap();
    fs::set_permissions(&body, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(
        fixture
            .engine
            .read_file_history_move_ordinary(
                &projection,
                recovery.digest(),
                &HistoryWindowCancellation::new()
            )
            .is_err()
    );
    assert!(
        fixture
            .engine
            .prepare_file_history_move_ordinary(
                &projection,
                recovery.digest(),
                &HistoryWindowCancellation::new()
            )
            .is_err()
    );
    assert!(retained.is_file());
    assert_eq!(fs::read(body).unwrap(), fs::read(retained).unwrap());
}

#[test]
fn missing_origin_and_missing_known_body_are_retained_refusals_not_recopy_authority() {
    for remove_origin in [false, true] {
        let mut fixture = Fixture::new(false);
        let (projection, recovery) = input(&mut fixture);
        prepare(&mut fixture.engine, &projection, &recovery);
        let root = target(&fixture.engine);
        let id = projection.version_mapping().values().next().unwrap();
        let version = stored_version(&root, &fixture.target.history_tree_id, id);
        let body = installed_body(
            &named(
                &root,
                &fixture.target.history_tree_id,
                projection.target_file_id().as_str(),
            ),
            &version,
        );
        let missing = if remove_origin {
            root.join(STAGE).join("origins").join(format!("{id}.json"))
        } else {
            body.clone()
        };
        fs::remove_file(&missing).unwrap();
        assert!(
            fixture
                .engine
                .prepare_file_history_move_ordinary(
                    &projection,
                    recovery.digest(),
                    &HistoryWindowCancellation::new()
                )
                .is_err()
        );
        assert!(!missing.exists());
        assert!(
            root.join("file-history-move-target/versions")
                .join(id)
                .join("content")
                .join(&version.creation_name)
                .is_file()
        );
        assert_eq!(body.exists(), remove_origin);
    }
}

#[test]
fn unclaimed_container_is_not_adopted_and_cancellation_does_not_publish() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = input(&mut fixture);
    let root = target(&fixture.engine);
    let cancel = HistoryWindowCancellation::new();
    cancel.cancel();
    assert!(
        fixture
            .engine
            .prepare_file_history_move_ordinary(&projection, recovery.digest(), &cancel)
            .is_err()
    );
    assert!(!root.join(STAGE).exists());
    let container = named(
        &root,
        &fixture.target.history_tree_id,
        projection.target_file_id().as_str(),
    );
    fs::create_dir_all(&container).unwrap();
    let unknown = container.join("unknown");
    fs::write(&unknown, b"retain me").unwrap();
    assert!(
        fixture
            .engine
            .prepare_file_history_move_ordinary(
                &projection,
                recovery.digest(),
                &HistoryWindowCancellation::new()
            )
            .is_err()
    );
    assert_eq!(fs::read(unknown).unwrap(), b"retain me");
    assert!(!root.join(STAGE).exists());
    assert!(
        !fixture
            .engine
            .state()
            .operations
            .contains_key(&projection.operation_id())
    );
    assert!(
        fixture
            .engine
            .inspect_file_history_move_joint_choice()
            .is_some()
    );
}
