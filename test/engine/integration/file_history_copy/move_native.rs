//! One prospective native receipt, never an accepted or visible move result.
use super::move_decision::plan;
use super::*;
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};
use storage_contract::history_window::{HistoryFileId, HistoryRepositoryId};

fn stage(fixture: &Fixture) -> PathBuf {
    fixture
        .temp
        .path()
        .join("system/file-history-move-decision-inputs")
}
fn input(
    fixture: &mut Fixture,
) -> (
    FileHistoryMoveProjection,
    FileHistoryMoveRecoveryInfo,
    FileHistoryMoveDecisionInputs,
) {
    let (projection, recovery) = plan(fixture);
    let inputs = fixture
        .engine
        .prepare_file_history_move_decision_inputs(
            &projection,
            recovery.digest(),
            &HistoryWindowCancellation::new(),
        )
        .unwrap();
    (projection, recovery, inputs)
}
fn native(
    fixture: &mut Fixture,
    projection: &FileHistoryMoveProjection,
    recovery: &FileHistoryMoveRecoveryInfo,
) -> FileHistoryMoveNativePreparation {
    fixture
        .engine
        .prepare_file_history_move_native_operation(
            projection,
            recovery.digest(),
            &HistoryWindowCancellation::new(),
        )
        .unwrap()
}
fn prepared(fixture: &Fixture) -> (State, OperationResult) {
    let state: State = storage_contract::decode_strict(
        &fs::read(stage(fixture).join("native-state.json")).unwrap(),
    )
    .unwrap();
    let envelope: json_codec::Value = storage_contract::decode_strict(
        &fs::read(stage(fixture).join("native-operation.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        envelope["format"],
        "file-history-move-unaccepted-native-operation"
    );
    (
        state,
        json_codec::from_value(envelope["proposed_operation"].clone()).unwrap(),
    )
}

#[test]
fn native_template_has_one_operation_principal_and_event_for_both_audit_owners() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery, inputs) = input(&mut fixture);
    let before = fixture.engine.state().clone();
    let report = native(&mut fixture, &projection, &recovery);
    assert_eq!(report.operation_id(), projection.operation_id());
    assert_eq!(report.decision_inputs_digest(), inputs.digest());
    assert_eq!(
        report.source_database_records_hash(),
        inputs.source_database_records_hash()
    );
    assert_eq!(
        report.target_database_records_hash(),
        inputs.target_database_records_hash()
    );
    let (next, operation) = prepared(&fixture);
    assert_eq!(next.operations.len(), before.operations.len() + 1);
    assert_eq!(
        next.operation_principals.len(),
        before.operation_principals.len() + 1
    );
    assert_eq!(next.events.len(), before.events.len() + 1);
    assert_eq!(operation.kind, "file.history.move");
    assert_eq!(operation.commit_state, CommitState::Staging);
    assert_eq!(
        operation.protection_at_publish,
        ProtectionAtPublish::NotApplicable
    );
    assert_eq!(operation.current_health, CurrentHealth::Unknown);
    assert_eq!(operation.request_digest, report.request_digest());
    assert_eq!(operation.operation_id, projection.operation_id());
    assert_eq!(next.operations[&operation.operation_id], operation);
    assert_eq!(next.operation_principals[&operation.operation_id], "admin");
    assert_eq!(next.events.last().unwrap().kind, "file.history.move");
    assert_eq!(
        operation.result["deletion_node"]["operation_id"],
        operation.operation_id.to_string()
    );
    assert_eq!(
        operation.result["deletion_node"]["parent_node_ids"],
        json_codec::json!(projection.capture().parent_node_ids())
    );
    assert_eq!(
        operation.result["file_id"],
        projection.target_file_id().as_str()
    );
    assert_eq!(
        operation.result["source_file_id"],
        projection.capture().source_file_id().as_str()
    );
    assert_eq!(
        operation.result["timestamp"],
        projection.proposed_source_deletion().created_at.to_string()
    );
    assert_eq!(
        operation.result["version_protection"]
            .as_array()
            .unwrap()
            .len(),
        projection.version_mapping().len()
    );
    assert_eq!(fixture.engine.state(), &before);
    assert!(
        fixture
            .engine
            .file_deletion(projection.capture().source_file_id().as_str())
            .is_none()
    );
    assert!(!report.outstanding().is_empty());
}

#[test]
fn native_full_state_stays_on_metadata_roots_and_original_inputs_are_unchanged() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery, _) = input(&mut fixture);
    let manifest = fs::read(stage(&fixture).join("inputs.json")).unwrap();
    let old_state = fs::read(stage(&fixture).join("unaccepted-state.json")).unwrap();
    native(&mut fixture, &projection, &recovery);
    let expected = fs::read(stage(&fixture).join("native-operation.json")).unwrap();
    for disk in &fixture.engine.config().disks {
        let root = disk.root.join("file-history-move-decision-inputs");
        if disk.metadata_member || disk.slot_id == "D04" {
            assert_eq!(
                fs::read(root.join("native-operation.json")).unwrap(),
                expected
            );
            assert_eq!(
                root.join("native-state.json").exists(),
                disk.metadata_member
            );
            assert_eq!(fs::read(root.join("inputs.json")).unwrap(), manifest);
        } else {
            assert!(!root.exists());
        }
    }
    assert_eq!(
        fs::read(stage(&fixture).join("unaccepted-state.json")).unwrap(),
        old_state
    );
}

#[test]
fn native_receipt_retains_old_source_audit_and_maps_only_the_old_prefix_to_target() {
    let mut fixture = Fixture::new(false);
    let source = entry(&fixture.engine, &fixture.source, "source.txt");
    let file = HistoryFileId::parse(&source.file_id).unwrap();
    fixture
        .engine
        .soft_delete_file(DeleteFileRequest {
            target: FileDeletionTarget {
                repository_id: &HistoryRepositoryId::parse(&fixture.source.repository_id).unwrap(),
                file_id: &file,
                path: &name_policy::LookupPath::parse("source.txt").unwrap(),
            },
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    let old = fixture
        .engine
        .file_deletion(&source.file_id)
        .unwrap()
        .clone();
    fixture
        .engine
        .undo_file_deletion(UndoFileDeletionRequest {
            file_id: &file,
            deletion_node_id: &old.node_id,
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    let before = fixture.engine.state().clone();
    let (projection, recovery, inputs) = input(&mut fixture);
    let report = native(&mut fixture, &projection, &recovery);
    let (next, operation) = prepared(&fixture);
    for (id, receipt) in before.operations.iter() {
        assert_eq!(next.operations[id], *receipt);
    }
    let events = operation.result["retained_lifecycle"]["events"]
        .as_array()
        .unwrap();
    assert_eq!(events.len(), 2);
    assert!(
        events
            .iter()
            .all(|event| event["node"]["file_id"] == projection.target_file_id().as_str())
    );
    assert!(
        events
            .iter()
            .all(|event| event["operation_id"] != projection.operation_id().to_string())
    );
    assert_eq!(
        next.file_deletions[&source.file_id].operation_id,
        projection.operation_id()
    );
    assert!(
        !next
            .file_deletions
            .contains_key(projection.target_file_id().as_str())
    );
    assert_eq!(
        report.source_database_records_hash(),
        inputs.source_database_records_hash()
    );
    assert_eq!(
        report.target_database_records_hash(),
        inputs.target_database_records_hash()
    );
}

#[test]
fn cold_native_replay_preserves_exact_operation_time_frontier_and_digests() {
    for current in [false, true] {
        let mut fixture = Fixture::new(current);
        let (projection, recovery, _) = input(&mut fixture);
        let report = native(&mut fixture, &projection, &recovery);
        let native_path = stage(&fixture).join("native-operation.json");
        let bytes = fs::read(&native_path).unwrap();
        let inode = fs::metadata(&native_path).unwrap().ino();
        let before = fixture.engine.state().clone();
        let Fixture { temp, engine, .. } = fixture;
        drop(engine);
        let mut engine = Engine::open(&temp.path().join("system")).unwrap();
        let projection = engine
            .resume_file_history_move_recovery(
                recovery.operation_id(),
                recovery.digest(),
                "admin",
                &HistoryWindowCancellation::new(),
            )
            .unwrap();
        let read = engine
            .read_file_history_move_native_operation(
                &projection,
                recovery.digest(),
                &HistoryWindowCancellation::new(),
            )
            .unwrap();
        assert_eq!(read.native_digest(), report.native_digest());
        assert_eq!(read.request_digest(), report.request_digest());
        let repeated = engine
            .prepare_file_history_move_native_operation(
                &projection,
                recovery.digest(),
                &HistoryWindowCancellation::new(),
            )
            .unwrap();
        assert_eq!(repeated.native_digest(), report.native_digest());
        assert_eq!(fs::metadata(&native_path).unwrap().ino(), inode);
        assert_eq!(fs::read(native_path).unwrap(), bytes);
        assert_eq!(engine.state(), &before);
    }
}

#[test]
fn native_read_is_complete_and_read_only_and_same_operation_can_fill_a_missing_template() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery, _) = input(&mut fixture);
    let original = native(&mut fixture, &projection, &recovery);
    let path = fixture
        .engine
        .config()
        .disks
        .iter()
        .find(|disk| disk.slot_id == "D04")
        .unwrap()
        .root
        .join("file-history-move-decision-inputs/native-operation.json");
    let bytes = fs::read(&path).unwrap();
    fs::remove_file(&path).unwrap();
    assert!(
        fixture
            .engine
            .read_file_history_move_native_operation(
                &projection,
                recovery.digest(),
                &HistoryWindowCancellation::new()
            )
            .is_err()
    );
    assert!(!path.exists());
    let repeated = native(&mut fixture, &projection, &recovery);
    assert_eq!(repeated.native_digest(), original.native_digest());
    assert_eq!(fs::read(path).unwrap(), bytes);
}

#[test]
fn native_state_or_operation_rewrites_are_retained_not_accepted_or_overwritten() {
    for filename in ["native-state.json", "native-operation.json"] {
        let mut fixture = Fixture::new(false);
        let (projection, recovery, _) = input(&mut fixture);
        native(&mut fixture, &projection, &recovery);
        let path = stage(&fixture).join(filename);
        let mut value: json_codec::Value =
            storage_contract::decode_strict(&fs::read(&path).unwrap()).unwrap();
        if filename == "native-state.json" {
            value["operation_principals"][&projection.operation_id().to_string()] =
                json_codec::json!("another actor");
        } else {
            value["proposed_operation"]["kind"] = json_codec::json!("file.delete");
        }
        let altered = canonical_json(&value).unwrap();
        fs::write(&path, &altered).unwrap();
        assert!(
            fixture
                .engine
                .read_file_history_move_native_operation(
                    &projection,
                    recovery.digest(),
                    &HistoryWindowCancellation::new()
                )
                .is_err()
        );
        assert_eq!(
            fixture
                .engine
                .prepare_file_history_move_native_operation(
                    &projection,
                    recovery.digest(),
                    &HistoryWindowCancellation::new()
                )
                .unwrap_err()
                .code(),
            ErrorCode::RecoveryRequired
        );
        assert_eq!(fs::read(path).unwrap(), altered);
        assert!(
            !fixture
                .engine
                .state()
                .operations
                .contains_key(&projection.operation_id())
        );
    }
}

#[test]
fn native_preparation_requires_original_inputs_and_honors_initial_cancellation() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = plan(&mut fixture);
    assert!(
        fixture
            .engine
            .prepare_file_history_move_native_operation(
                &projection,
                recovery.digest(),
                &HistoryWindowCancellation::new()
            )
            .is_err()
    );
    assert!(!stage(&fixture).exists());
    fixture
        .engine
        .prepare_file_history_move_decision_inputs(
            &projection,
            recovery.digest(),
            &HistoryWindowCancellation::new(),
        )
        .unwrap();
    let cancellation = HistoryWindowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        fixture
            .engine
            .prepare_file_history_move_native_operation(
                &projection,
                recovery.digest(),
                &cancellation
            )
            .unwrap_err()
            .code(),
        ErrorCode::OperationAborted
    );
    assert!(!stage(&fixture).join("native-operation.json").exists());
}

#[test]
fn native_templates_keep_source_fenced_and_reject_a_damaged_target_full() {
    for changed_source in [false, true] {
        let mut fixture = Fixture::new(false);
        let (projection, recovery, _) = input(&mut fixture);
        native(&mut fixture, &projection, &recovery);
        let retained = fs::read(stage(&fixture).join("native-operation.json")).unwrap();
        if changed_source {
            fixture
                .engine
                .set_file_note(
                    projection.capture().source_file_id().as_str(),
                    None,
                    &Note::parse("different source".into()).unwrap(),
                    "admin",
                    op(),
                )
                .unwrap_err();
        } else {
            let (source, target) = projection.version_mapping().iter().next().unwrap();
            let root = &fixture
                .engine
                .config()
                .disks
                .iter()
                .find(|disk| disk.slot_id == "D04")
                .unwrap()
                .root;
            let body = root
                .join("file-history-move-target/versions")
                .join(target)
                .join("content")
                .join(&fixture.engine.state().versions[source].creation_name);
            let original = bytes(&fixture.engine, source);
            let permissions = fs::metadata(&body).unwrap().permissions();
            assert_eq!(permissions.mode() & 0o777, 0o400);
            let mut damaged = fs::read(&body).unwrap();
            damaged[0] ^= 1;
            fs::set_permissions(&body, fs::Permissions::from_mode(0o600)).unwrap();
            fs::write(&body, &damaged).unwrap();
            fs::set_permissions(&body, permissions).unwrap();
            assert_eq!(fs::read(&body).unwrap(), damaged);
            assert_eq!(bytes(&fixture.engine, source), original);
        }
        assert_eq!(
            fixture
                .engine
                .read_file_history_move_native_operation(
                    &projection,
                    recovery.digest(),
                    &HistoryWindowCancellation::new()
                )
                .is_ok(),
            changed_source
        );
        assert_eq!(
            fs::read(stage(&fixture).join("native-operation.json")).unwrap(),
            retained
        );
        assert!(
            fixture
                .engine
                .file_deletion(projection.capture().source_file_id().as_str())
                .is_none()
        );
    }
}
