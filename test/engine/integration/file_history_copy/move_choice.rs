//! Source-only coverage for durable input selection. No test below is a native
//! ACCEPT, source-D publication, real SMB rename or physical power-loss claim.
use super::*;
use super::move_decision::plan;
use std::{fs, path::PathBuf};

const DIRECTORY: &str = "file-history-move-decision-inputs";
const NAME: &str = "joint-choice.json";

fn input(fixture: &mut Fixture) -> (FileHistoryMoveProjection, FileHistoryMoveRecoveryInfo) {
    let (projection, recovery) = plan(fixture);
    let stop = HistoryWindowCancellation::new();
    fixture.engine.prepare_file_history_move_decision_inputs(&projection, recovery.digest(), &stop).unwrap();
    fixture.engine.prepare_file_history_move_native_operation(&projection, recovery.digest(), &stop).unwrap();
    fixture.engine.prepare_file_history_move_scoped_databases(&projection, recovery.digest(), &stop).unwrap();
    (projection, recovery)
}
fn select(fixture: &mut Fixture, projection: &FileHistoryMoveProjection, recovery: &FileHistoryMoveRecoveryInfo) -> FileHistoryMoveJointChoice {
    fixture.engine.select_file_history_move_joint_choice(projection, recovery.digest(), &HistoryWindowCancellation::new()).unwrap()
}
fn paths(fixture: &Fixture) -> Vec<PathBuf> {
    let mut result = vec![fixture.temp.path().join("system").join(DIRECTORY).join(NAME)];
    result.extend(fixture.engine.config().disks.iter().filter(|disk| disk.metadata_member || disk.slot_id == "D04")
        .map(|disk| disk.root.join(DIRECTORY).join(NAME)));
    result
}
fn reject_note(engine: &mut Engine, file: &str) {
    assert_eq!(engine.set_file_note(file, None, &Note::parse("must not publish".into()).unwrap(), "admin", op())
        .unwrap_err().code(), ErrorCode::RecoveryRequired);
}

#[test]
fn one_choice_fences_writes_without_publishing_either_end() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = input(&mut fixture);
    let before = fixture.engine.state().clone();
    let source = entry(&fixture.engine, &fixture.source, "source.txt");
    let source_bytes = bytes(&fixture.engine, &source.version_id);
    let report = select(&mut fixture, &projection, &recovery);
    let copies = paths(&fixture);
    let saved = fs::read(&copies[0]).unwrap();
    assert!(copies.iter().all(|path| fs::read(path).unwrap() == saved));
    assert_eq!(report.operation_id(), projection.operation_id());
    assert_eq!(report.source_sequence(), before.sequence);
    assert_eq!(report.decision_slot(), before.sequence + 1);
    assert_eq!(report.participant_disks(), 4);
    assert_eq!(report.outstanding().len(), 5);
    assert_eq!(fixture.engine.read_file_history_move_joint_choice(&projection, recovery.digest(), &HistoryWindowCancellation::new()).unwrap().digest(), report.digest());
    reject_note(&mut fixture.engine, &source.file_id);
    assert_eq!(fixture.engine.create_workspace(&fixture.source.main_branch_id, "admin").unwrap_err().code(), ErrorCode::RecoveryRequired);
    assert_eq!(fixture.engine.state(), &before);
    assert!(fixture.engine.file_deletion(&source.file_id).is_none());
    assert_eq!(bytes(&fixture.engine, &source.version_id), source_bytes);
    assert!(fixture.engine.namespace_tree(&fixture.target.main_branch_id).unwrap().entries().is_empty());
    let inode = fs::metadata(&copies[0]).unwrap().ino();
    assert_eq!(select(&mut fixture, &projection, &recovery).digest(), report.digest());
    assert_eq!(fs::metadata(&copies[0]).unwrap().ino(), inode);
}

#[test]
fn incomplete_scoped_material_cannot_select_or_fence_a_fresh_move() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = input(&mut fixture);
    let image = fixture.temp.path().join("system").join(DIRECTORY).join("proposed-system.sqlite");
    fs::remove_file(&image).unwrap();
    assert!(fixture.engine.select_file_history_move_joint_choice(&projection, recovery.digest(), &HistoryWindowCancellation::new()).is_err());
    assert!(fixture.engine.inspect_file_history_move_joint_choice().is_none());
    assert!(paths(&fixture).iter().all(|path| !path.exists()));
    assert!(!image.exists());
}

#[test]
fn cancellation_before_selection_has_no_durable_or_memory_choice() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = input(&mut fixture);
    let stop = HistoryWindowCancellation::new();
    stop.cancel();
    assert_eq!(fixture.engine.select_file_history_move_joint_choice(&projection, recovery.digest(), &stop).unwrap_err().code(), ErrorCode::OperationAborted);
    assert!(fixture.engine.inspect_file_history_move_joint_choice().is_none());
    assert!(paths(&fixture).iter().all(|path| !path.exists()));
}

#[test]
fn cold_choice_keeps_the_original_d_clock_and_source_readable() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = input(&mut fixture);
    let selected = select(&mut fixture, &projection, &recovery);
    let before = fixture.engine.state().clone();
    let path = paths(&fixture).remove(0);
    let original = fs::read(&path).unwrap();
    let inode = fs::metadata(&path).unwrap().ino();
    let Fixture { temp, engine, source, target } = fixture;
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(engine.inspect_file_history_move_joint_choice().unwrap().digest(), selected.digest());
    reject_note(&mut engine, projection.capture().source_file_id().as_str());
    assert!(engine.capture_file_history_move(FileHistoryMoveRequest {
        namespace: projection.capture().namespace(), source_file_id: projection.capture().source_file_id(),
    }, &HistoryWindowCancellation::new()).is_err());
    assert!(engine.resume_file_history_move_recovery(op(), recovery.digest(), "admin", &HistoryWindowCancellation::new()).is_err());
    let restored = engine.resume_file_history_move_recovery(recovery.operation_id(), recovery.digest(), "admin", &HistoryWindowCancellation::new()).unwrap();
    assert_eq!(restored.proposed_source_deletion(), projection.proposed_source_deletion());
    assert_eq!(restored.version_mapping(), projection.version_mapping());
    assert_eq!(engine.read_file_history_move_joint_choice(&restored, recovery.digest(), &HistoryWindowCancellation::new()).unwrap().digest(), selected.digest());
    assert_eq!(engine.select_file_history_move_joint_choice(&restored, recovery.digest(), &HistoryWindowCancellation::new()).unwrap().digest(), selected.digest());
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
    assert_eq!(engine.state(), &before);
    assert_eq!(entry(&engine, &source, "source.txt").file_id, restored.capture().source_file_id().as_str());
    assert!(engine.namespace_tree(&target.main_branch_id).unwrap().entries().is_empty());
}

#[test]
fn surviving_disk_choice_fences_cold_start_without_recreating_the_missing_system_copy() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = input(&mut fixture);
    let selected = select(&mut fixture, &projection, &recovery);
    let path = paths(&fixture).remove(0);
    let saved = fs::read(&path).unwrap();
    fs::remove_file(&path).unwrap();
    let Fixture { temp, engine, .. } = fixture;
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(engine.inspect_file_history_move_joint_choice().unwrap().digest(), selected.digest());
    assert!(!path.exists());
    reject_note(&mut engine, projection.capture().source_file_id().as_str());
    let restored = engine.resume_file_history_move_recovery(recovery.operation_id(), recovery.digest(), "admin", &HistoryWindowCancellation::new()).unwrap();
    assert!(engine.read_file_history_move_joint_choice(&restored, recovery.digest(), &HistoryWindowCancellation::new()).is_err());
    assert!(!path.exists());
    assert_eq!(engine.select_file_history_move_joint_choice(&restored, recovery.digest(), &HistoryWindowCancellation::new()).unwrap().digest(), selected.digest());
    assert_eq!(fs::read(path).unwrap(), saved);
}

#[test]
fn replication_failure_retains_the_fence_and_original_bytes_until_explicit_same_input_retry() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = input(&mut fixture);
    let blocked = paths(&fixture).pop().unwrap();
    fs::create_dir(&blocked).unwrap();
    assert_eq!(fixture.engine.select_file_history_move_joint_choice(&projection, recovery.digest(), &HistoryWindowCancellation::new()).unwrap_err().code(), ErrorCode::RecoveryRequired);
    assert!(blocked.is_dir());
    let path = paths(&fixture).remove(0);
    let saved = fs::read(&path).unwrap();
    let inode = fs::metadata(&path).unwrap().ino();
    assert!(fixture.engine.inspect_file_history_move_joint_choice().is_some());
    reject_note(&mut fixture.engine, projection.capture().source_file_id().as_str());
    // Only remove the test-created obstructing empty directory, not production
    // recovery material. The production API never performs this removal.
    fs::remove_dir(&blocked).unwrap();
    select(&mut fixture, &projection, &recovery);
    assert_eq!(fs::read(&path).unwrap(), saved);
    assert_eq!(fs::metadata(path).unwrap().ino(), inode);
}

#[test]
fn a_conflicting_or_malformed_choice_refuses_startup_without_overwrite() {
    for malformed in [false, true] {
        let mut fixture = Fixture::new(false);
        let (projection, recovery) = input(&mut fixture);
        select(&mut fixture, &projection, &recovery);
        let copies = paths(&fixture);
        let original = fs::read(&copies[0]).unwrap();
        let damaged = if malformed { b"{".to_vec() } else {
            let mut value: json_codec::Value = storage_contract::decode_strict(&original).unwrap();
            value["native_digest"] = json_codec::json!("0".repeat(64));
            storage_contract::canonical_json(&value).unwrap()
        };
        fs::write(&copies[1], &damaged).unwrap();
        let Fixture { temp, engine, .. } = fixture;
        drop(engine);
        assert!(Engine::open(&temp.path().join("system")).is_err());
        assert_eq!(fs::read(&copies[0]).unwrap(), original);
        assert_eq!(fs::read(&copies[1]).unwrap(), damaged);
    }
}

#[test]
fn an_older_choice_returning_after_a_later_publication_cannot_rewind_or_repair_the_pool() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = input(&mut fixture);
    select(&mut fixture, &projection, &recovery);
    let copies = paths(&fixture);
    let original = fs::read(&copies[0]).unwrap();
    // Simulate unavailable selection evidence, then its later return. There
    // was never a move ACCEPT, so no target or source-D result can be inferred.
    for path in &copies { fs::remove_file(path).unwrap(); }
    let Fixture { temp, engine, .. } = fixture;
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    engine.set_file_note(projection.capture().source_file_id().as_str(), None,
        &Note::parse("a genuinely later publication".into()).unwrap(), "admin", op()).unwrap();
    let later_sequence = engine.state().sequence;
    assert!(later_sequence > projection.capture().source_sequence());
    for path in &copies { fs::write(path, &original).unwrap(); }
    drop(engine);
    assert!(Engine::open(&temp.path().join("system")).is_err());
    assert!(copies.iter().all(|path| fs::read(path).unwrap() == original));
}
