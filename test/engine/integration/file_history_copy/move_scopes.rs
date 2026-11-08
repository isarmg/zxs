//! Unrun source tests for retained historical SQL, never move success or SMB QA.
use super::*;
use super::move_decision::plan;
use sqlite_access::{Connection, Row, SqliteConnection, sqlite::SqliteConnectOptions};
use std::{fs, path::{Path, PathBuf}};

const DIRECTORY: &str = "file-history-move-decision-inputs";
const MANIFEST: &str = "scoped-databases.json";

fn stage(fixture: &Fixture) -> PathBuf { fixture.temp.path().join("system").join(DIRECTORY) }
fn input(fixture: &mut Fixture) -> (FileHistoryMoveProjection, FileHistoryMoveRecoveryInfo) {
    let (projection, recovery) = plan(fixture);
    fixture.engine.prepare_file_history_move_decision_inputs(&projection, recovery.digest(), &HistoryWindowCancellation::new()).unwrap();
    fixture.engine.prepare_file_history_move_native_operation(&projection, recovery.digest(), &HistoryWindowCancellation::new()).unwrap();
    (projection, recovery)
}
fn prepare(fixture: &mut Fixture, projection: &FileHistoryMoveProjection, recovery: &FileHistoryMoveRecoveryInfo) -> FileHistoryMoveScopedPreparation {
    fixture.engine.prepare_file_history_move_scoped_databases(projection, recovery.digest(), &HistoryWindowCancellation::new()).unwrap()
}
fn manifest(fixture: &Fixture) -> json_codec::Value {
    storage_contract::decode_strict(&fs::read(stage(fixture).join(MANIFEST)).unwrap()).unwrap()
}
fn system_rows(path: &Path) -> Vec<(i64, String, json_codec::Value)> {
    platform_io::sqlite::block_on_sqlite_connection(async {
        let mut connection = SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(path).read_only(true)).await.unwrap();
        let result = sqlite_access::query("SELECT kind,id,payload FROM system_changes ORDER BY kind,id").fetch_all(&mut connection).await.unwrap()
            .into_iter().map(|row| (
                row.try_get("kind").unwrap(), row.try_get("id").unwrap(),
                json_codec::from_str(&row.try_get::<String, _>("payload").unwrap()).unwrap(),
            )).collect();
        connection.close().await.unwrap();
        result
    })
}
fn history_header(path: &Path) -> json_codec::Value {
    platform_io::sqlite::block_on_sqlite_connection(async {
        let mut connection = SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(path).read_only(true)).await.unwrap();
        let body: String = sqlite_access::query_scalar("SELECT body FROM identity WHERE singleton=1").fetch_one(&mut connection).await.unwrap();
        connection.close().await.unwrap();
        json_codec::from_str(&body).unwrap()
    })
}

#[test]
fn scoped_images_keep_every_source_publication_separate_from_the_unaccepted_move() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = input(&mut fixture);
    let before = fixture.engine.state().clone();
    let report = prepare(&mut fixture, &projection, &recovery);
    let stored = manifest(&fixture);
    assert_eq!(stored["format"], "file-history-move-unaccepted-scoped-databases");
    assert_eq!(stored["published_source_sequence"], before.sequence.to_string());
    assert_eq!(stored["unaccepted_proposed_sequence"], projection.proposed_sequence().to_string());
    assert_eq!(report.source_sequence(), before.sequence);
    assert_eq!(report.system_publications(), before.sequence as usize);
    assert_eq!(report.databases(), report.system_publications() + 1 + 3 * report.participant_disks());
    assert_eq!(report.outstanding().len(), 5);
    let rows = system_rows(&stage(&fixture).join("proposed-system.sqlite"));
    let operation = rows.iter().find(|(kind, id, _)| *kind == 0 && *id == projection.operation_id().to_string()).unwrap();
    let operation: OperationResult = json_codec::from_value(operation.2.clone()).unwrap();
    assert_eq!(operation.commit_state, CommitState::Staging);
    assert_eq!(operation.result["deletion_node"]["parent_node_ids"], json_codec::json!(projection.capture().parent_node_ids()));
    assert_eq!(rows.iter().filter(|(kind, _, _)| *kind == 0).count(), before.operations.len() + 1);
    assert_eq!(rows.iter().filter(|(kind, _, _)| *kind == 36).count(), before.operation_principals.len() + 1);
    assert_eq!(rows.iter().filter(|(kind, _, _)| *kind == 37).count(), before.events.len() + 1);
    let old = system_rows(&stage(&fixture).join(format!("published-system-{:020}.sqlite", before.sequence)));
    assert!(old.iter().all(|(kind, id, _)| *kind != 0 || *id != projection.operation_id().to_string()));
    assert_eq!(old.iter().filter(|(kind, _, _)| *kind == 0).count(), before.operations.len());
    for disk in &fixture.engine.config().disks {
        let path = stage(&fixture).join(format!("published-history-{}.sqlite", disk.disk_id));
        if path.is_file() {
            assert_eq!(history_header(&path)["through_sequence"], before.sequence.to_string());
        }
    }
    assert_eq!(fixture.engine.state(), &before);
    assert!(fixture.engine.file_deletion(projection.capture().source_file_id().as_str()).is_none());
    assert!(fixture.engine.namespace_tree(&fixture.target.main_branch_id).unwrap().entries().is_empty());
}

#[test]
fn payload_scope_never_receives_system_history_or_another_disks_sql_rows() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = input(&mut fixture);
    prepare(&mut fixture, &projection, &recovery);
    let bytes = fs::read(stage(&fixture).join(MANIFEST)).unwrap();
    let stored = manifest(&fixture);
    for disk in &fixture.engine.config().disks {
        let root = disk.root.join(DIRECTORY);
        if disk.metadata_member || disk.slot_id == "D04" {
            assert_eq!(fs::read(root.join(MANIFEST)).unwrap(), bytes);
            for item in stored["databases"].as_array().unwrap() {
                let name = item["name"].as_str().unwrap();
                let own = item["scope"]["identity"]["disk_id"].as_str() == Some(disk.disk_id.as_str());
                assert_eq!(root.join(name).is_file(), disk.metadata_member || own);
                if disk.metadata_member || own {
                    assert_eq!(fs::read(root.join(name)).unwrap(), fs::read(stage(&fixture).join(name)).unwrap());
                }
            }
        } else { assert!(!root.exists()); }
    }
}

#[test]
fn cold_scoped_read_and_explicit_retry_keep_the_original_images_and_native_digest() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = input(&mut fixture);
    let first = prepare(&mut fixture, &projection, &recovery);
    let path = stage(&fixture).join("proposed-system.sqlite");
    let saved = fs::read(&path).unwrap();
    let inode = fs::metadata(&path).unwrap().ino();
    let before = fixture.engine.state().clone();
    let Fixture { temp, engine, .. } = fixture;
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    let projection = engine.resume_file_history_move_recovery(recovery.operation_id(), recovery.digest(), "admin", &HistoryWindowCancellation::new()).unwrap();
    let read = engine.read_file_history_move_scoped_databases(&projection, recovery.digest(), &HistoryWindowCancellation::new()).unwrap();
    let repeated = engine.prepare_file_history_move_scoped_databases(&projection, recovery.digest(), &HistoryWindowCancellation::new()).unwrap();
    assert_eq!(read.scoped_digest(), first.scoped_digest());
    assert_eq!(repeated.scoped_digest(), first.scoped_digest());
    assert_eq!(read.native_digest(), first.native_digest());
    assert_eq!(fs::read(&path).unwrap(), saved);
    assert_eq!(fs::metadata(path).unwrap().ino(), inode);
    assert_eq!(engine.state(), &before);
}

#[test]
fn missing_one_payload_history_fails_read_only_but_original_manifest_can_resume() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = input(&mut fixture);
    let first = prepare(&mut fixture, &projection, &recovery);
    let disk = fixture.engine.config().disks.iter().find(|disk| disk.slot_id == "D04").unwrap();
    let path = disk.root.join(DIRECTORY).join(format!("published-history-{}.sqlite", disk.disk_id));
    let original = fs::read(&path).unwrap();
    fs::remove_file(&path).unwrap();
    assert!(fixture.engine.read_file_history_move_scoped_databases(&projection, recovery.digest(), &HistoryWindowCancellation::new()).is_err());
    assert!(!path.exists());
    assert_eq!(prepare(&mut fixture, &projection, &recovery).scoped_digest(), first.scoped_digest());
    assert_eq!(fs::read(path).unwrap(), original);
}

#[test]
fn orphaned_scoped_images_cannot_be_adopted_by_recreating_a_missing_manifest() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = input(&mut fixture);
    prepare(&mut fixture, &projection, &recovery);
    let path = stage(&fixture).join(MANIFEST);
    let image = fs::read(stage(&fixture).join("proposed-system.sqlite")).unwrap();
    fs::remove_file(&path).unwrap();
    assert_eq!(fixture.engine.prepare_file_history_move_scoped_databases(&projection, recovery.digest(), &HistoryWindowCancellation::new()).unwrap_err().code(), ErrorCode::RecoveryRequired);
    assert!(!path.exists());
    assert_eq!(fs::read(stage(&fixture).join("proposed-system.sqlite")).unwrap(), image);
}

#[test]
fn valid_sqlite_with_a_different_native_receipt_is_preserved_and_rejected() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = input(&mut fixture);
    prepare(&mut fixture, &projection, &recovery);
    let path = stage(&fixture).join("proposed-system.sqlite");
    platform_io::sqlite::block_on_sqlite_connection(async {
        let mut connection = SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&path)).await.unwrap();
        sqlite_access::query("DELETE FROM system_changes WHERE kind=0 AND id=?")
            .bind(projection.operation_id().to_string()).execute(&mut connection).await.unwrap();
        connection.close().await.unwrap();
    });
    let altered = fs::read(&path).unwrap();
    assert!(fixture.engine.read_file_history_move_scoped_databases(&projection, recovery.digest(), &HistoryWindowCancellation::new()).is_err());
    assert_eq!(fixture.engine.prepare_file_history_move_scoped_databases(&projection, recovery.digest(), &HistoryWindowCancellation::new()).unwrap_err().code(), ErrorCode::RecoveryRequired);
    assert_eq!(fs::read(path).unwrap(), altered);
    assert!(!fixture.engine.state().operations.contains_key(&projection.operation_id()));
}

#[test]
fn scoped_stage_requires_original_native_bytes_and_respects_cancellation_before_writes() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = plan(&mut fixture);
    fixture.engine.prepare_file_history_move_decision_inputs(&projection, recovery.digest(), &HistoryWindowCancellation::new()).unwrap();
    assert!(fixture.engine.prepare_file_history_move_scoped_databases(&projection, recovery.digest(), &HistoryWindowCancellation::new()).is_err());
    assert!(!stage(&fixture).join(MANIFEST).exists());
    fixture.engine.prepare_file_history_move_native_operation(&projection, recovery.digest(), &HistoryWindowCancellation::new()).unwrap();
    let stop = HistoryWindowCancellation::new();
    stop.cancel();
    assert_eq!(fixture.engine.prepare_file_history_move_scoped_databases(&projection, recovery.digest(), &stop).unwrap_err().code(), ErrorCode::OperationAborted);
    assert!(!stage(&fixture).join(MANIFEST).exists());
}

#[test]
fn generation_without_a_complete_historical_recipe_remains_explicitly_unsupported() {
    let mut fixture = Fixture::new(true);
    let (projection, recovery) = input(&mut fixture);
    let source = fixture.engine.state().clone();
    assert_eq!(fixture.engine.prepare_file_history_move_scoped_databases(&projection, recovery.digest(), &HistoryWindowCancellation::new()).unwrap_err().code(), ErrorCode::UnsupportedOperation);
    assert!(!stage(&fixture).join(MANIFEST).exists());
    assert_eq!(fixture.engine.state(), &source);
}
