//! Unrun ordinary-object/cold-reconstruction cases. No ACCEPT, published move,
//! active database installation, real SMB or physical failure claim.
use super::move_decision::plan;
use super::*;
use std::{
    fs,
    path::{Path, PathBuf},
};

const DIRECTORY: &str = "file-history-move-decision-inputs";
const MANIFEST: &str = "candidate-inputs.json";
fn stage(fixture: &Fixture) -> PathBuf {
    fixture.temp.path().join("system").join(DIRECTORY)
}
fn input(fixture: &mut Fixture) -> (FileHistoryMoveProjection, FileHistoryMoveRecoveryInfo) {
    let (projection, recovery) = plan(fixture);
    let stop = HistoryWindowCancellation::new();
    fixture
        .engine
        .prepare_file_history_move_decision_inputs(&projection, recovery.digest(), &stop)
        .unwrap();
    fixture
        .engine
        .prepare_file_history_move_native_operation(&projection, recovery.digest(), &stop)
        .unwrap();
    fixture
        .engine
        .prepare_file_history_move_scoped_databases(&projection, recovery.digest(), &stop)
        .unwrap();
    (projection, recovery)
}
fn select(
    fixture: &mut Fixture,
    projection: &FileHistoryMoveProjection,
    recovery: &FileHistoryMoveRecoveryInfo,
) {
    fixture
        .engine
        .select_file_history_move_joint_choice(
            projection,
            recovery.digest(),
            &HistoryWindowCancellation::new(),
        )
        .unwrap();
}
fn prepare(
    fixture: &mut Fixture,
    projection: &FileHistoryMoveProjection,
    recovery: &FileHistoryMoveRecoveryInfo,
) -> FileHistoryMoveCandidatePreparation {
    fixture
        .engine
        .prepare_file_history_move_candidate(
            projection,
            recovery.digest(),
            &HistoryWindowCancellation::new(),
        )
        .unwrap()
}
fn object(root: &Path, kind: &str, hash: &str) -> PathBuf {
    root.join("metadata/objects")
        .join(kind)
        .join(&hash[..2])
        .join(format!("{hash}.json"))
}
fn journals(engine: &Engine) -> Vec<(PathBuf, Vec<u8>)> {
    engine
        .config()
        .disks
        .iter()
        .filter(|disk| disk.metadata_member)
        .map(|disk| {
            let path = disk.root.join("metadata/application-log/journal.bin");
            let bytes = fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect()
}
fn assert_journals(saved: &[(PathBuf, Vec<u8>)]) {
    assert!(
        saved
            .iter()
            .all(|(path, bytes)| fs::read(path).unwrap() == *bytes)
    );
}

#[test]
fn ordinary_candidate_requires_the_complete_selected_inputs_and_has_no_journal_authority() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = input(&mut fixture);
    let before = fixture.engine.state().clone();
    assert!(
        fixture
            .engine
            .prepare_file_history_move_candidate(
                &projection,
                recovery.digest(),
                &HistoryWindowCancellation::new()
            )
            .is_err()
    );
    assert!(!stage(&fixture).join(MANIFEST).exists());
    select(&mut fixture, &projection, &recovery);
    let journal = journals(&fixture.engine);
    let report = prepare(&mut fixture, &projection, &recovery);
    assert_eq!(report.operation_id(), projection.operation_id());
    assert_eq!(report.sequence(), projection.proposed_sequence());
    assert_eq!(report.metadata_members(), 3);
    assert_eq!(report.outstanding().len(), 5);
    assert_eq!(fixture.engine.state(), &before);
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
    assert_journals(&journal);
    let stored: json_codec::Value =
        storage_contract::decode_strict(&fs::read(stage(&fixture).join(MANIFEST)).unwrap())
            .unwrap();
    assert_eq!(stored["candidate_hash"], report.candidate_hash());
    assert_eq!(stored["snapshot_hash"], report.snapshot_hash());
    assert_eq!(stored["choice_digest"], report.choice_digest());
    assert_eq!(
        stored["objects"].as_array().unwrap().len(),
        report.objects()
    );
    for disk in &fixture.engine.config().disks {
        if disk.metadata_member {
            for item in stored["objects"].as_array().unwrap() {
                let kind = item["kind"].as_str().unwrap();
                let hash = item["hash"].as_str().unwrap();
                let bytes = fs::read(object(&disk.root, kind, hash)).unwrap();
                let value: json_codec::Value = storage_contract::decode_strict(&bytes).unwrap();
                assert_eq!(storage_contract::object_hash(kind, &value).unwrap(), hash);
                assert_eq!(bytes.len().to_string(), item["bytes"].as_str().unwrap());
            }
            assert_eq!(
                fs::read(object(&disk.root, "candidate", report.candidate_hash())).unwrap(),
                fs::read(stage(&fixture).join("candidate.json")).unwrap()
            );
            assert!(
                disk.root
                    .join(DIRECTORY)
                    .join("candidate-system.sqlite")
                    .is_file()
            );
        } else if disk.slot_id == "D04" {
            assert_eq!(
                fs::read(disk.root.join(DIRECTORY).join(MANIFEST)).unwrap(),
                fs::read(stage(&fixture).join(MANIFEST)).unwrap()
            );
            assert!(!disk.root.join(DIRECTORY).join("candidate.json").exists());
            assert!(
                !disk
                    .root
                    .join(DIRECTORY)
                    .join("candidate-system.sqlite")
                    .exists()
            );
            assert!(!object(&disk.root, "candidate", report.candidate_hash()).exists());
        }
    }
}

#[test]
fn exact_candidate_and_all_dependencies_reconstruct_after_cold_start_without_voting() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = input(&mut fixture);
    select(&mut fixture, &projection, &recovery);
    let first = prepare(&mut fixture, &projection, &recovery);
    let stage = stage(&fixture);
    let original = fs::read(stage.join("candidate.json")).unwrap();
    let inode = fs::metadata(stage.join("candidate.json")).unwrap().ino();
    let before = fixture.engine.state().clone();
    let journal = journals(&fixture.engine);
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
        .read_file_history_move_candidate(
            &restored,
            recovery.digest(),
            &HistoryWindowCancellation::new(),
        )
        .unwrap();
    let repeated = engine
        .prepare_file_history_move_candidate(
            &restored,
            recovery.digest(),
            &HistoryWindowCancellation::new(),
        )
        .unwrap();
    assert_eq!(read.candidate_hash(), first.candidate_hash());
    assert_eq!(repeated.candidate_hash(), first.candidate_hash());
    assert_eq!(read.snapshot_hash(), first.snapshot_hash());
    assert_eq!(fs::read(stage.join("candidate.json")).unwrap(), original);
    assert_eq!(
        fs::metadata(stage.join("candidate.json")).unwrap().ino(),
        inode
    );
    assert_eq!(engine.state(), &before);
    assert_journals(&journal);
}

#[test]
fn missing_one_original_members_object_does_not_fallback_to_other_complete_members() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = input(&mut fixture);
    select(&mut fixture, &projection, &recovery);
    let first = prepare(&mut fixture, &projection, &recovery);
    let disk = fixture
        .engine
        .config()
        .disks
        .iter()
        .find(|disk| disk.metadata_member)
        .unwrap();
    let missing = object(&disk.root, "snapshot", first.snapshot_hash());
    let bytes = fs::read(&missing).unwrap();
    fs::remove_file(&missing).unwrap();
    assert!(
        fixture
            .engine
            .read_file_history_move_candidate(
                &projection,
                recovery.digest(),
                &HistoryWindowCancellation::new()
            )
            .is_err()
    );
    assert!(!missing.exists());
    assert_eq!(
        prepare(&mut fixture, &projection, &recovery).candidate_hash(),
        first.candidate_hash()
    );
    assert_eq!(fs::read(missing).unwrap(), bytes);
}

#[test]
fn an_ordinary_candidate_without_its_original_input_manifest_cannot_be_adopted() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = input(&mut fixture);
    select(&mut fixture, &projection, &recovery);
    let first = prepare(&mut fixture, &projection, &recovery);
    let disk = fixture
        .engine
        .config()
        .disks
        .iter()
        .find(|disk| disk.metadata_member)
        .unwrap();
    let manifest = disk.root.join(DIRECTORY).join(MANIFEST);
    let candidate = object(&disk.root, "candidate", first.candidate_hash());
    let original = fs::read(&candidate).unwrap();
    fs::remove_file(&manifest).unwrap();
    // Remove only the test's staged aliases too, so the ordinary candidate is
    // the surviving origin evidence. Production never deletes these objects.
    fs::remove_file(disk.root.join(DIRECTORY).join("candidate.json")).unwrap();
    fs::remove_file(disk.root.join(DIRECTORY).join("candidate-system.sqlite")).unwrap();
    assert!(
        fixture
            .engine
            .prepare_file_history_move_candidate(
                &projection,
                recovery.digest(),
                &HistoryWindowCancellation::new()
            )
            .is_err()
    );
    assert!(!manifest.exists());
    assert_eq!(fs::read(candidate).unwrap(), original);
}

#[test]
fn corrupt_ordinary_candidate_is_preserved_and_blocks_both_read_and_retry() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = input(&mut fixture);
    select(&mut fixture, &projection, &recovery);
    let first = prepare(&mut fixture, &projection, &recovery);
    let disk = fixture
        .engine
        .config()
        .disks
        .iter()
        .find(|disk| disk.metadata_member)
        .unwrap();
    let path = object(&disk.root, "candidate", first.candidate_hash());
    fs::write(&path, b"{}").unwrap();
    assert!(
        fixture
            .engine
            .read_file_history_move_candidate(
                &projection,
                recovery.digest(),
                &HistoryWindowCancellation::new()
            )
            .is_err()
    );
    assert!(
        fixture
            .engine
            .prepare_file_history_move_candidate(
                &projection,
                recovery.digest(),
                &HistoryWindowCancellation::new()
            )
            .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), b"{}");
    assert!(
        fixture
            .engine
            .inspect_file_history_move_joint_choice()
            .is_some()
    );
}

#[test]
fn candidate_cancellation_does_not_clear_the_choice_or_write_a_journal() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = input(&mut fixture);
    select(&mut fixture, &projection, &recovery);
    let journal = journals(&fixture.engine);
    let cancelled = HistoryWindowCancellation::new();
    cancelled.cancel();
    assert_eq!(
        fixture
            .engine
            .prepare_file_history_move_candidate(&projection, recovery.digest(), &cancelled)
            .unwrap_err()
            .code(),
        ErrorCode::OperationAborted
    );
    assert!(!stage(&fixture).join(MANIFEST).exists());
    assert!(
        fixture
            .engine
            .inspect_file_history_move_joint_choice()
            .is_some()
    );
    assert_journals(&journal);
}
