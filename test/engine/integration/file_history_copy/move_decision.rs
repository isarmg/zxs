//! Durable joint inputs only. These tests do not claim a move decision or SMB I/O.
use super::*;
use sqlite_access::{Connection, Row, SqliteConnection, sqlite::SqliteConnectOptions};
use std::{fs, os::unix::fs::PermissionsExt, path::{Path, PathBuf}};
use storage_contract::history_window::{HistoryFileId, HistoryRepositoryId};

const DIRECTORY: &str = "file-history-move-decision-inputs";

pub(super) fn plan(fixture: &mut Fixture) -> (FileHistoryMoveProjection, FileHistoryMoveRecoveryInfo) {
    let file = entry(&fixture.engine, &fixture.source, "source.txt");
    let captured = fixture.engine.capture_file_history_move(FileHistoryMoveRequest {
        namespace: &request(&fixture.engine, &fixture.source, &fixture.target),
        source_file_id: &HistoryFileId::parse(&file.file_id).unwrap(),
    }, &HistoryWindowCancellation::new()).unwrap();
    let projection = fixture.engine.project_file_history_move(captured, "admin", op(), &HistoryWindowCancellation::new()).unwrap();
    let recovery = fixture.engine.save_file_history_move_recovery(&projection, &HistoryWindowCancellation::new()).unwrap();
    fixture.engine.prepare_file_history_move_target(&projection, recovery.digest(), &HistoryWindowCancellation::new()).unwrap();
    (projection, recovery)
}
fn disk_root(engine: &Engine, slot: &str) -> PathBuf {
    engine.config().disks.iter().find(|disk| disk.slot_id == slot).unwrap().root.clone()
}
fn system(fixture: &Fixture) -> PathBuf { fixture.temp.path().join("system").join(DIRECTORY) }
fn prepare(fixture: &mut Fixture, projection: &FileHistoryMoveProjection, recovery: &FileHistoryMoveRecoveryInfo) -> FileHistoryMoveDecisionInputs {
    fixture.engine.prepare_file_history_move_decision_inputs(projection, recovery.digest(), &HistoryWindowCancellation::new()).unwrap()
}
fn lifecycle(path: &Path) -> (Vec<json_codec::Value>, Vec<json_codec::Value>) {
    platform_io::sqlite::block_on_sqlite_connection(async {
        let mut connection = SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(path).read_only(true)).await.unwrap();
        let mut events = Vec::new();
        let mut nodes = Vec::new();
        for row in sqlite_access::query("SELECT kind,body FROM records WHERE kind IN ('lifecycle','deletion') ORDER BY kind,id").fetch_all(&mut connection).await.unwrap() {
            let body: json_codec::Value = json_codec::from_str(&row.try_get::<String, _>("body").unwrap()).unwrap();
            if row.try_get::<String, _>("kind").unwrap() == "lifecycle" { events.push(body); } else { nodes.push(body); }
        }
        connection.close().await.unwrap();
        (events, nodes)
    })
}

#[test]
fn one_original_manifest_binds_source_d_target_files_and_every_required_scope() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = plan(&mut fixture);
    let before = fixture.engine.state().clone();
    let result = prepare(&mut fixture, &projection, &recovery);
    assert_eq!(result.operation_id(), projection.operation_id());
    assert_eq!(result.metadata_members(), 3);
    assert_eq!(result.participant_disks(), 4);
    assert_eq!(result.outstanding(), [
        FileHistoryMoveDecisionObligation::UnifiedPermanentReceipt,
        FileHistoryMoveDecisionObligation::SystemHistoricalDatabase,
        FileHistoryMoveDecisionObligation::DiskHistoricalDatabases,
        FileHistoryMoveDecisionObligation::JointJournalDecisionAndRecovery,
        FileHistoryMoveDecisionObligation::TwoEndedReadFences,
    ]);
    let original = fs::read(system(&fixture).join("inputs.json")).unwrap();
    let manifest: json_codec::Value = storage_contract::decode_strict(&original).unwrap();
    assert_eq!(manifest["source_deletion"]["node_id"], projection.proposed_source_deletion().node_id);
    assert_eq!(manifest["source_deletion"]["operation_id"], projection.operation_id().to_string());
    assert_eq!(manifest["target_copy_identities"]["D04"].as_object().unwrap().len(), 2);
    assert_eq!(manifest["source_file"]["records_hash"], result.source_database_records_hash());
    assert_eq!(manifest["target_file"]["records_hash"], result.target_database_records_hash());
    for disk in &fixture.engine.config().disks {
        let root = disk.root.join(DIRECTORY);
        if disk.metadata_member || disk.slot_id == "D04" {
            assert_eq!(fs::read(root.join("inputs.json")).unwrap(), original);
            assert!(root.join("source-file.sqlite").is_file());
            assert!(root.join("target-file.sqlite").is_file());
            assert_eq!(root.join("unaccepted-state.json").is_file(), disk.metadata_member);
        } else { assert!(!root.exists()); }
    }
    let proposal: State = storage_contract::decode_strict(&fs::read(system(&fixture).join("unaccepted-state.json")).unwrap()).unwrap();
    assert_eq!(proposal.operations, before.operations);
    assert_eq!(proposal.operation_principals, before.operation_principals);
    assert_eq!(proposal.events, before.events);
    assert_eq!(proposal.file_deletions[projection.capture().source_file_id().as_str()], *projection.proposed_source_deletion());
    assert!(proposal.versions.values().any(|version| version.file_id == projection.target_file_id().as_str()));
    assert_eq!(fixture.engine.state(), &before);
    assert!(fixture.engine.namespace_tree(&fixture.target.main_branch_id).unwrap().entries().is_empty());
    fixture.engine.verify_file_history_move_decision_inputs(&projection, recovery.digest(), result.digest(), &HistoryWindowCancellation::new()).unwrap();
}

#[test]
fn source_database_adds_only_the_same_move_d_and_target_keeps_only_old_imported_audit() {
    let mut fixture = Fixture::new(false);
    let file = entry(&fixture.engine, &fixture.source, "source.txt");
    let file_id = HistoryFileId::parse(&file.file_id).unwrap();
    fixture.engine.soft_delete_file(DeleteFileRequest {
        target: FileDeletionTarget {
            repository_id: &HistoryRepositoryId::parse(&fixture.source.repository_id).unwrap(),
            file_id: &file_id, path: &name_policy::LookupPath::parse("source.txt").unwrap(),
        }, principal: "admin", operation_id: op(),
    }).unwrap();
    let old_d = fixture.engine.file_deletion(&file.file_id).unwrap().clone();
    fixture.engine.undo_file_deletion(UndoFileDeletionRequest {
        file_id: &file_id, deletion_node_id: &old_d.node_id, principal: "admin", operation_id: op(),
    }).unwrap();
    let (projection, recovery) = plan(&mut fixture);
    prepare(&mut fixture, &projection, &recovery);
    let (source, source_nodes) = lifecycle(&system(&fixture).join("source-file.sqlite"));
    let (target, target_nodes) = lifecycle(&system(&fixture).join("target-file.sqlite"));
    assert_eq!(source.len(), 3);
    assert_eq!(source_nodes.len(), 1);
    assert_eq!(source_nodes[0]["node_id"], projection.proposed_source_deletion().node_id);
    let appended = source.iter().find(|event| event["operation_id"] == projection.operation_id().to_string()).unwrap();
    assert_eq!(appended["action"], "delete");
    assert_eq!(appended["node"]["file_id"], file.file_id);
    assert_eq!(appended["node"]["parent_node_ids"], json_codec::json!(projection.capture().parent_node_ids()));
    assert_eq!(target.len(), 2);
    assert!(target_nodes.is_empty());
    assert!(target.iter().all(|event| event["node"]["file_id"] == projection.target_file_id().as_str()));
    assert!(!fixture.engine.state().operations.contains_key(&projection.operation_id()));
    assert!(fixture.engine.file_deletion(&file.file_id).is_none());
}

#[test]
fn cold_resume_reconstructs_the_same_input_digest_and_never_adopts_it_as_a_decision() {
    for current in [false, true] {
        let mut fixture = Fixture::new(current);
        let (projection, recovery) = plan(&mut fixture);
        let original = prepare(&mut fixture, &projection, &recovery);
        let before = fixture.engine.state().clone();
        let path = system(&fixture).join("source-file.sqlite");
        let inode = fs::metadata(&path).unwrap().ino();
        let Fixture { temp, engine, .. } = fixture;
        drop(engine);
        let mut engine = Engine::open(&temp.path().join("system")).unwrap();
        let projection = engine.resume_file_history_move_recovery(recovery.operation_id(), recovery.digest(), "admin", &HistoryWindowCancellation::new()).unwrap();
        let cold = engine.read_file_history_move_decision_inputs(&projection, recovery.digest(), &HistoryWindowCancellation::new()).unwrap();
        assert_eq!(cold.digest(), original.digest());
        let observed = engine.verify_file_history_move_decision_inputs(&projection, recovery.digest(), original.digest(), &HistoryWindowCancellation::new()).unwrap();
        assert_eq!(observed.digest(), original.digest());
        let resumed = engine.prepare_file_history_move_decision_inputs(&projection, recovery.digest(), &HistoryWindowCancellation::new()).unwrap();
        assert_eq!(resumed.digest(), original.digest());
        assert_eq!(fs::metadata(path).unwrap().ino(), inode);
        assert_eq!(engine.state(), &before);
        assert!(!observed.outstanding().is_empty());
    }
}

#[test]
fn a_missing_required_member_is_not_hidden_by_other_complete_replicas() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = plan(&mut fixture);
    let original = prepare(&mut fixture, &projection, &recovery);
    let root = disk_root(&fixture.engine, "B01").join(DIRECTORY);
    let path = root.join("source-file.sqlite");
    let bytes = fs::read(&path).unwrap();
    fs::remove_file(&path).unwrap();
    assert!(fixture.engine.verify_file_history_move_decision_inputs(&projection, recovery.digest(), original.digest(), &HistoryWindowCancellation::new()).is_err());
    assert!(!path.exists());
    let resumed = prepare(&mut fixture, &projection, &recovery);
    assert_eq!(resumed.digest(), original.digest());
    assert_eq!(fs::read(path).unwrap(), bytes);
}

#[test]
fn an_incomplete_unclaimed_input_directory_is_preserved_without_overwrite() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = plan(&mut fixture);
    let root = disk_root(&fixture.engine, "D04").join(DIRECTORY);
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(root.join("unknown.partial"), b"original interrupted material").unwrap();
    assert_eq!(fixture.engine.prepare_file_history_move_decision_inputs(&projection, recovery.digest(), &HistoryWindowCancellation::new()).unwrap_err().code(), ErrorCode::RecoveryRequired);
    assert!(!root.join("inputs.json").exists());
    assert_eq!(fs::read(root.join("unknown.partial")).unwrap(), b"original interrupted material");
    assert!(fixture.engine.file_deletion(projection.capture().source_file_id().as_str()).is_none());
}

#[test]
fn a_self_consistent_but_foreign_source_database_is_not_accepted_or_repaired() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = plan(&mut fixture);
    let original = prepare(&mut fixture, &projection, &recovery);
    let path = system(&fixture).join("source-file.sqlite");
    platform_io::sqlite::block_on_sqlite_connection(async {
        let mut connection = SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&path)).await.unwrap();
        let row = sqlite_access::query("SELECT body FROM records WHERE kind='file'").fetch_one(&mut connection).await.unwrap();
        let mut body: json_codec::Value = json_codec::from_str(&row.try_get::<String, _>("body").unwrap()).unwrap();
        body["note"] = json_codec::json!("different source state, valid self digest");
        sqlite_access::query("UPDATE records SET body=? WHERE kind='file'").bind(json_codec::to_string(&body).unwrap()).execute(&mut connection).await.unwrap();
        let rows = sqlite_access::query("SELECT kind,id,body FROM records ORDER BY kind,id").fetch_all(&mut connection).await.unwrap();
        let records = rows.iter().map(|row| json_codec::json!({
            "kind": row.try_get::<String, _>("kind").unwrap(),
            "id": row.try_get::<String, _>("id").unwrap(),
            "body": row.try_get::<String, _>("body").unwrap(),
        })).collect::<Vec<_>>();
        let digest = storage_contract::object_hash("file-local-records", &records).unwrap();
        sqlite_access::query("UPDATE identity SET records_hash=?").bind(digest).execute(&mut connection).await.unwrap();
        connection.close().await.unwrap();
    });
    let damaged = fs::read(&path).unwrap();
    assert_eq!(fixture.engine.verify_file_history_move_decision_inputs(&projection, recovery.digest(), original.digest(), &HistoryWindowCancellation::new()).unwrap_err().code(), ErrorCode::ContainerLayoutInvalid);
    assert_eq!(fixture.engine.prepare_file_history_move_decision_inputs(&projection, recovery.digest(), &HistoryWindowCancellation::new()).unwrap_err().code(), ErrorCode::RecoveryRequired);
    assert_eq!(fs::read(path).unwrap(), damaged);
}

#[test]
fn replacing_a_target_inode_and_its_marker_cannot_rebind_the_original_decision_inputs() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = plan(&mut fixture);
    let original = prepare(&mut fixture, &projection, &recovery);
    let (source, target) = projection.version_mapping().iter().next().unwrap();
    let version = disk_root(&fixture.engine, "D04").join("file-history-move-target/versions").join(target);
    let body = version.join("content").join(&fixture.engine.state().versions[source].creation_name);
    let temporary = body.with_extension("replacement");
    fs::write(&temporary, fs::read(&body).unwrap()).unwrap();
    fs::rename(&temporary, &body).unwrap();
    let metadata = fs::metadata(&body).unwrap();
    let marker_path = version.join("copy-identity.json");
    let mut marker: json_codec::Value = storage_contract::decode_strict(&fs::read(&marker_path).unwrap()).unwrap();
    for (key, value) in [
        ("device", metadata.dev().to_string()), ("inode", metadata.ino().to_string()),
        ("modified_seconds", metadata.mtime().to_string()), ("modified_nanoseconds", metadata.mtime_nsec().to_string()),
        ("changed_seconds", metadata.ctime().to_string()), ("changed_nanoseconds", metadata.ctime_nsec().to_string()),
    ] { marker[key] = json_codec::json!(value); }
    fs::write(&marker_path, canonical_json(&marker).unwrap()).unwrap();
    // Local body + marker still agree. The joint immutable manifest independently
    // binds the original verified marker, rather than trusting this new pair.
    fixture.engine.verify_file_history_move_target(&projection, recovery.digest(), &HistoryWindowCancellation::new()).unwrap();
    assert_eq!(fixture.engine.verify_file_history_move_decision_inputs(&projection, recovery.digest(), original.digest(), &HistoryWindowCancellation::new()).unwrap_err().code(), ErrorCode::PreparationStale);
    assert_eq!(fixture.engine.prepare_file_history_move_decision_inputs(&projection, recovery.digest(), &HistoryWindowCancellation::new()).unwrap_err().code(), ErrorCode::RecoveryRequired);
}

#[test]
fn missing_target_fulls_cancellation_and_stale_sources_never_create_a_move_decision() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = plan(&mut fixture);
    let cancel = HistoryWindowCancellation::new();
    cancel.cancel();
    assert_eq!(fixture.engine.prepare_file_history_move_decision_inputs(&projection, recovery.digest(), &cancel).unwrap_err().code(), ErrorCode::OperationAborted);
    assert!(!system(&fixture).exists());
    let (source, target) = projection.version_mapping().iter().next().unwrap();
    let body = disk_root(&fixture.engine, "D04").join("file-history-move-target/versions").join(target).join("content").join(&fixture.engine.state().versions[source].creation_name);
    let content = fs::read(&body).unwrap();
    fs::write(&body, b"incomplete FULL").unwrap();
    assert!(fixture.engine.prepare_file_history_move_decision_inputs(&projection, recovery.digest(), &HistoryWindowCancellation::new()).is_err());
    assert!(!system(&fixture).exists());
    fs::write(&body, content).unwrap();
    fixture.engine.set_file_note(projection.capture().source_file_id().as_str(), None, &Note::parse("changed after original capture".into()).unwrap(), "admin", op()).unwrap();
    assert_eq!(fixture.engine.prepare_file_history_move_decision_inputs(&projection, recovery.digest(), &HistoryWindowCancellation::new()).unwrap_err().code(), ErrorCode::WorkspaceChanged);
    assert!(!system(&fixture).exists());
    assert!(!fixture.engine.state().operations.contains_key(&projection.operation_id()));
}
