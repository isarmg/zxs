//! Real preparation files only; never a move-success or journal-recovery claim.
use super::*;
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};
use storage_contract::history_window::HistoryFileId;

fn projection(fixture: &Fixture, operation: OperationId) -> FileHistoryMoveProjection {
    projection_for(
        fixture,
        operation,
        &request(&fixture.engine, &fixture.source, &fixture.target),
    )
}

pub(super) fn projection_for(
    fixture: &Fixture,
    operation: OperationId,
    namespace: &FileHistoryCopyRequest,
) -> FileHistoryMoveProjection {
    let file = entry(&fixture.engine, &fixture.source, &namespace.source_path);
    let capture = fixture
        .engine
        .capture_file_history_move(
            FileHistoryMoveRequest {
                namespace,
                source_file_id: &HistoryFileId::parse(&file.file_id).unwrap(),
            },
            &HistoryWindowCancellation::new(),
        )
        .unwrap();
    fixture
        .engine
        .project_file_history_move(
            capture,
            "admin",
            operation,
            &HistoryWindowCancellation::new(),
        )
        .unwrap()
}

fn system_path(fixture: &Fixture) -> PathBuf {
    fixture
        .temp
        .path()
        .join("system/file-history-move-recovery/preparation.json")
}

fn member_paths(engine: &Engine) -> Vec<PathBuf> {
    let mut disks = engine
        .config()
        .disks
        .iter()
        .filter(|disk| disk.metadata_member)
        .collect::<Vec<_>>();
    disks.sort_by(|left, right| left.disk_id.cmp(&right.disk_id));
    disks
        .iter()
        .map(|disk| {
            disk.root
                .join("file-history-move-recovery/preparation.json")
        })
        .collect()
}

fn rewrite_self_hash(path: &std::path::Path, edit: impl FnOnce(&mut json_codec::Value)) -> Vec<u8> {
    let mut envelope: json_codec::Value = json_codec::from_slice(&fs::read(path).unwrap()).unwrap();
    edit(&mut envelope["record"]);
    envelope["digest"] = json_codec::json!(
        storage_contract::object_hash("file-history-move-recovery", &envelope["record"]).unwrap()
    );
    let bytes = storage_contract::canonical_json(&envelope).unwrap();
    fs::write(path, &bytes).unwrap();
    bytes
}

#[test]
fn joint_envelope_is_identical_on_all_members_and_cold_resume_keeps_original_d_and_clock() {
    for current in [false, true] {
        let mut fixture = Fixture::new(current);
        let operation = op();
        let proposed = projection(&fixture, operation);
        let deletion = proposed.proposed_source_deletion().clone();
        let versions = proposed.version_mapping().clone();
        let target_file = proposed.target_file_id().clone();
        let before = fixture.engine.state().clone();
        let info = fixture
            .engine
            .save_file_history_move_recovery(&proposed, &HistoryWindowCancellation::new())
            .unwrap();
        let path = system_path(&fixture);
        let bytes = fs::read(&path).unwrap();
        let members = member_paths(&fixture.engine);
        assert_eq!(info.metadata_members(), members.len());
        assert_eq!(info.operation_id(), operation);
        assert_eq!(info.decision_slot(), proposed.proposed_sequence());
        assert!(members.iter().all(|path| fs::read(path).unwrap() == bytes));
        assert_eq!(fixture.engine.state(), &before);
        let again = fixture
            .engine
            .save_file_history_move_recovery(&proposed, &HistoryWindowCancellation::new())
            .unwrap();
        assert_eq!(info, again);
        assert_eq!(fs::read(&path).unwrap(), bytes);
        let Fixture {
            temp,
            engine,
            source,
            target,
        } = fixture;
        drop(engine);
        let mut engine = Engine::open(&temp.path().join("system")).unwrap();
        let restored = engine
            .resume_file_history_move_recovery(
                operation,
                info.digest(),
                "admin",
                &HistoryWindowCancellation::new(),
            )
            .unwrap();
        assert_eq!(restored.proposed_source_deletion(), &deletion);
        assert_eq!(restored.version_mapping(), &versions);
        assert_eq!(restored.target_file_id(), &target_file);
        assert_eq!(restored.operation_id(), operation);
        assert_eq!(engine.state(), &before);
        assert!(engine.file_deletion(&deletion.file_id).is_none());
        assert_eq!(
            entry(&engine, &source, "source.txt").file_id,
            deletion.file_id
        );
        assert!(
            engine
                .namespace_tree(&target.main_branch_id)
                .unwrap()
                .entries()
                .is_empty()
        );
        assert!(!engine.state().operations.contains_key(&operation));
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn cold_resume_completes_only_a_missing_replica_of_the_same_original_envelope() {
    let mut fixture = Fixture::new(false);
    let operation = op();
    let proposed = projection(&fixture, operation);
    let info = fixture
        .engine
        .save_file_history_move_recovery(&proposed, &HistoryWindowCancellation::new())
        .unwrap();
    let system = fs::read(system_path(&fixture)).unwrap();
    let member = member_paths(&fixture.engine)[0].clone();
    fs::remove_file(&member).unwrap();
    fs::remove_dir(member.parent().unwrap()).unwrap();
    let Fixture { temp, engine, .. } = fixture;
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    let restored = engine
        .resume_file_history_move_recovery(
            operation,
            info.digest(),
            "admin",
            &HistoryWindowCancellation::new(),
        )
        .unwrap();
    assert_eq!(
        restored.proposed_source_deletion(),
        proposed.proposed_source_deletion()
    );
    assert_eq!(fs::read(member).unwrap(), system);
    assert!(!engine.state().operations.contains_key(&operation));
}

#[test]
fn self_consistent_forged_projection_digest_cannot_replace_independent_reconstruction() {
    let mut fixture = Fixture::new(false);
    let operation = op();
    let proposed = projection(&fixture, operation);
    fixture
        .engine
        .save_file_history_move_recovery(&proposed, &HistoryWindowCancellation::new())
        .unwrap();
    let path = system_path(&fixture);
    let forged = rewrite_self_hash(&path, |record| {
        record["projection_hash"] = json_codec::json!("0".repeat(64));
    });
    let observed = fixture
        .engine
        .inspect_file_history_move_recovery()
        .unwrap()
        .unwrap();
    assert_eq!(
        fixture
            .engine
            .resume_file_history_move_recovery(
                operation,
                observed.digest(),
                "admin",
                &HistoryWindowCancellation::new(),
            )
            .map(|_| ())
            .unwrap_err()
            .code(),
        ErrorCode::PreparationStale
    );
    assert_eq!(fs::read(&path).unwrap(), forged);
    assert!(
        fixture
            .engine
            .file_deletion(proposed.capture().source_file_id().as_str())
            .is_none()
    );
}

#[test]
fn original_envelope_fences_source_writes_before_the_later_joint_choice() {
    let mut fixture = Fixture::new(false);
    let operation = op();
    let proposed = projection(&fixture, operation);
    let info = fixture
        .engine
        .save_file_history_move_recovery(&proposed, &HistoryWindowCancellation::new())
        .unwrap();
    let path = system_path(&fixture);
    let original = fs::read(&path).unwrap();
    assert_eq!(
        fixture
            .engine
            .set_file_note(
                proposed.capture().source_file_id().as_str(),
                None,
                &Note::parse("must remain original".into()).unwrap(),
                "admin",
                op()
            )
            .unwrap_err()
            .code(),
        ErrorCode::RecoveryRequired
    );
    let resumed = fixture
        .engine
        .resume_file_history_move_recovery(
            operation,
            info.digest(),
            "admin",
            &HistoryWindowCancellation::new(),
        )
        .unwrap();
    assert_eq!(
        resumed.proposed_source_deletion(),
        proposed.proposed_source_deletion()
    );
    assert_eq!(fs::read(path).unwrap(), original);
}

#[test]
fn partial_member_directory_is_preserved_and_never_promoted_to_a_decision() {
    let mut fixture = Fixture::new(false);
    let operation = op();
    let proposed = projection(&fixture, operation);
    let member = member_paths(&fixture.engine)[0].clone();
    let directory = member.parent().unwrap();
    fs::create_dir(directory).unwrap();
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
    let unknown = directory.join("unclaimed.partial");
    fs::write(&unknown, b"keep original preparation material").unwrap();
    assert_eq!(
        fixture
            .engine
            .save_file_history_move_recovery(&proposed, &HistoryWindowCancellation::new(),)
            .unwrap_err()
            .code(),
        ErrorCode::RecoveryRequired
    );
    let info = fixture
        .engine
        .inspect_file_history_move_recovery()
        .unwrap()
        .unwrap();
    assert_eq!(info.operation_id(), operation);
    assert_eq!(
        fixture
            .engine
            .resume_file_history_move_recovery(
                operation,
                info.digest(),
                "admin",
                &HistoryWindowCancellation::new(),
            )
            .map(|_| ())
            .unwrap_err()
            .code(),
        ErrorCode::RecoveryRequired
    );
    assert_eq!(
        fs::read(unknown).unwrap(),
        b"keep original preparation material"
    );
    assert!(!member.exists());
    assert!(
        fixture
            .engine
            .file_deletion(proposed.capture().source_file_id().as_str())
            .is_none()
    );
    assert!(!fixture.engine.state().operations.contains_key(&operation));
}

#[test]
fn damaged_member_replica_is_not_overwritten_from_the_system_self_hash() {
    let mut fixture = Fixture::new(false);
    let operation = op();
    let proposed = projection(&fixture, operation);
    let info = fixture
        .engine
        .save_file_history_move_recovery(&proposed, &HistoryWindowCancellation::new())
        .unwrap();
    let member = member_paths(&fixture.engine)[0].clone();
    fs::write(&member, b"damaged original envelope").unwrap();
    assert_eq!(
        fixture
            .engine
            .resume_file_history_move_recovery(
                operation,
                info.digest(),
                "admin",
                &HistoryWindowCancellation::new(),
            )
            .map(|_| ())
            .unwrap_err()
            .code(),
        ErrorCode::RecoveryRequired
    );
    assert_eq!(fs::read(member).unwrap(), b"damaged original envelope");
    assert!(!fixture.engine.state().operations.contains_key(&operation));
}

#[test]
fn resume_rejects_other_actor_operation_and_cancelled_request_without_writes() {
    let mut fixture = Fixture::new(false);
    let operation = op();
    let proposed = projection(&fixture, operation);
    let info = fixture
        .engine
        .save_file_history_move_recovery(&proposed, &HistoryWindowCancellation::new())
        .unwrap();
    let path = system_path(&fixture);
    let original = fs::read(&path).unwrap();
    for (id, actor) in [(operation, "another"), (op(), "admin")] {
        assert_eq!(
            fixture
                .engine
                .resume_file_history_move_recovery(
                    id,
                    info.digest(),
                    actor,
                    &HistoryWindowCancellation::new(),
                )
                .map(|_| ())
                .unwrap_err()
                .code(),
            ErrorCode::PreparationStale
        );
    }
    let cancelled = HistoryWindowCancellation::new();
    cancelled.cancel();
    assert_eq!(
        fixture
            .engine
            .resume_file_history_move_recovery(operation, info.digest(), "admin", &cancelled,)
            .map(|_| ())
            .unwrap_err()
            .code(),
        ErrorCode::OperationAborted
    );
    assert_eq!(fs::read(path).unwrap(), original);
}

#[test]
fn unsupported_format_is_preserved_without_source_intent_upgrade() {
    let mut fixture = Fixture::new(false);
    let proposed = projection(&fixture, op());
    fixture
        .engine
        .save_file_history_move_recovery(&proposed, &HistoryWindowCancellation::new())
        .unwrap();
    let path = system_path(&fixture);
    let unsupported = rewrite_self_hash(&path, |record| {
        record["format"] = json_codec::json!("file-history-move-source-intent");
    });
    assert_eq!(
        fixture
            .engine
            .inspect_file_history_move_recovery()
            .unwrap_err()
            .code(),
        ErrorCode::FormatUnsupported
    );
    assert_eq!(fs::read(path).unwrap(), unsupported);
}
