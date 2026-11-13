//! Real target staging and the two-copy object-move route on temporary disks.
//! These sources are not an executed result or SMB certification.
use super::*;
use sqlite_access::{Connection, Row, SqliteConnection, sqlite::SqliteConnectOptions};
use std::{fs, path::PathBuf};
use storage_contract::history_window::{HistoryFileId, HistoryRepositoryId};

fn two_targets() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let mut config =
        Config::development((0..5).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
    for disk in config.disks.iter_mut().skip(3) {
        disk.metadata_member = false;
    }
    config.disks[3].level = 1;
    config.disks[3].upstream_slot_id = None;
    config.disks[4].level = 2;
    config.disks[4].upstream_slot_id = Some(config.disks[3].slot_id.clone());
    let mut engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
    let source = engine.create_disk_root("P01", "admin", op()).unwrap();
    let target = engine
        .create_repository(
            &RepositoryName::parse("target").unwrap(),
            "D04",
            2,
            "admin",
            op(),
        )
        .unwrap();
    save(&mut engine, &source, "source.txt", b"old full body");
    save(&mut engine, &source, "source.txt", b"new full body");
    Fixture {
        temp,
        engine,
        source,
        target,
    }
}

#[test]
fn two_copy_regular_repository_is_not_a_low_level_disk_root_capture() {
    let fixture = two_targets();
    let file = entry(&fixture.engine, &fixture.source, "source.txt");
    let before = fixture.engine.state().clone();
    assert!(!fixture.target.file_manager_root);
    assert_eq!(fixture.target.target_copies, 2);
    let capture = fixture.engine.capture_file_history_move(
        FileHistoryMoveRequest {
            namespace: &request(&fixture.engine, &fixture.source, &fixture.target),
            source_file_id: &HistoryFileId::parse(&file.file_id).unwrap(),
        },
        &HistoryWindowCancellation::new(),
    );
    assert_eq!(
        capture.err().unwrap().code(),
        ErrorCode::UnsupportedOperation
    );
    assert_eq!(fixture.engine.state(), &before);
    assert!(fixture.engine.file_deletion(&file.file_id).is_none());
    assert_eq!(bytes(&fixture.engine, &file.version_id), b"new full body");
    for slot in ["D04", "D05"] {
        assert!(!stage(&fixture.engine, slot).exists());
    }
}

#[test]
fn object_move_publishes_independent_fulls_and_equal_file_databases_on_both_targets() {
    use super::file_databases::{database, records};
    let mut fixture = two_targets();
    let source = entry(&fixture.engine, &fixture.source, "source.txt");
    let originals = fixture.engine.file_history(&source.file_id).unwrap();
    let request = FileHistoryMovePathRequest {
        source_repository_id: fixture.source.repository_id.clone(),
        source_path: "source.txt".into(),
        target_repository_id: fixture.target.repository_id.clone(),
        target_name: "moved.txt".into(),
    };
    let operation = op();
    let receipt = fixture
        .engine
        .move_current_file(&request, "admin", operation)
        .unwrap();
    assert_eq!(
        receipt.kind,
        storage_contract::file_relocation::OBJECT_MOVE_KIND
    );
    assert_eq!(receipt.commit_state, CommitState::Published);
    assert_eq!(
        receipt.protection_at_publish,
        ProtectionAtPublish::TargetMet
    );
    let moved = entry(&fixture.engine, &fixture.target, "moved.txt");
    assert_ne!(moved.file_id, source.file_id);
    assert_eq!(
        fixture
            .engine
            .file_deletion(&source.file_id)
            .unwrap()
            .operation_id,
        operation
    );
    assert!(
        fixture
            .engine
            .namespace_tree(&fixture.source.main_branch_id)
            .unwrap()
            .get("source.txt")
            .is_none()
    );
    let paths = ["D04", "D05"].map(|slot| {
        database(
            &fixture.engine,
            &fixture.target,
            &moved.file_id,
            slot,
            "moved.txt",
        )
    });
    let expected = records(&paths[0]);
    assert_eq!(expected, records(&paths[1]));
    assert_eq!(
        expected
            .keys()
            .filter(|(kind, _)| kind == "version")
            .count(),
        originals.len()
    );
    let source_database = database(
        &fixture.engine,
        &fixture.source,
        &source.file_id,
        "P01",
        "source.txt",
    );
    let mut database_identities = BTreeSet::new();
    let metadata = fs::metadata(source_database).unwrap();
    database_identities.insert((metadata.dev(), metadata.ino()));
    for path in &paths {
        let metadata = fs::metadata(path).unwrap();
        assert_eq!(metadata.nlink(), 1);
        assert!(database_identities.insert((metadata.dev(), metadata.ino())));
    }
    let mut retained = Vec::new();
    for original in originals {
        let target = receipt.result["version_mapping"][&original.version_id]
            .as_str()
            .unwrap();
        let version = &fixture.engine.state().versions[target];
        let placement = &fixture.engine.state().placements[target];
        assert_eq!(placement.route_slot_ids, ["D04", "D05"]);
        assert_eq!(placement.target_copies, 2);
        let old = fixture
            .engine
            .open_content(&original.version_id)
            .unwrap()
            .file
            .metadata()
            .unwrap();
        let mut identities = BTreeSet::from([(old.dev(), old.ino())]);
        for database in &paths {
            let body = installed_body(database.parent().unwrap(), &version);
            let contents = fs::read(&body).unwrap();
            assert_eq!(contents, bytes(&fixture.engine, &original.version_id));
            let metadata = fs::metadata(&body).unwrap();
            assert_eq!(metadata.nlink(), 1);
            assert!(identities.insert((metadata.dev(), metadata.ino())));
            retained.push((body, contents, (metadata.dev(), metadata.ino())));
        }
    }
    let Fixture {
        temp,
        engine,
        target,
        ..
    } = fixture;
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(
        engine
            .move_current_file(&request, "admin", operation)
            .unwrap(),
        receipt
    );
    assert_eq!(entry(&engine, &target, "moved.txt"), moved);
    for path in paths {
        assert_eq!(records(&path), expected);
    }
    for (path, contents, identity) in retained {
        assert_eq!(fs::read(&path).unwrap(), contents);
        let metadata = fs::metadata(path).unwrap();
        assert_eq!((metadata.dev(), metadata.ino()), identity);
    }
}

fn plan(fixture: &mut Fixture) -> (FileHistoryMoveProjection, FileHistoryMoveRecoveryInfo) {
    let file = entry(&fixture.engine, &fixture.source, "source.txt");
    let capture = fixture
        .engine
        .capture_file_history_move(
            FileHistoryMoveRequest {
                namespace: &request(&fixture.engine, &fixture.source, &fixture.target),
                source_file_id: &HistoryFileId::parse(&file.file_id).unwrap(),
            },
            &HistoryWindowCancellation::new(),
        )
        .unwrap();
    let projection = fixture
        .engine
        .project_file_history_move(capture, "admin", op(), &HistoryWindowCancellation::new())
        .unwrap();
    let info = fixture
        .engine
        .save_file_history_move_recovery(&projection, &HistoryWindowCancellation::new())
        .unwrap();
    (projection, info)
}

fn stage(engine: &Engine, slot: &str) -> PathBuf {
    engine
        .config()
        .disks
        .iter()
        .find(|disk| disk.slot_id == slot)
        .unwrap()
        .root
        .join("file-history-move-target")
}

fn staged_body(engine: &Engine, projection: &FileHistoryMoveProjection, slot: &str) -> PathBuf {
    let (source, target) = projection.version_mapping().iter().next().unwrap();
    stage(engine, slot)
        .join("versions")
        .join(target)
        .join("content")
        .join(&engine.state().versions[source].creation_name)
}

#[test]
fn disk_root_staging_keeps_independent_full_history_files_unpublished() {
    let mut fixture = Fixture::new(false);
    let (projection, info) = plan(&mut fixture);
    let before = fixture.engine.state().clone();
    let report = fixture
        .engine
        .prepare_file_history_move_target(
            &projection,
            info.digest(),
            &HistoryWindowCancellation::new(),
        )
        .unwrap();
    assert_eq!(report.target_slots(), ["D04"]);
    assert_eq!(report.versions_per_slot(), 2);
    assert_eq!(report.body_bytes_per_slot(), 26);
    assert_eq!(
        report.target_file_id(),
        projection.target_file_id().as_str()
    );
    for (source, target) in projection.version_mapping() {
        let original = fixture
            .engine
            .open_content(source)
            .unwrap()
            .file
            .metadata()
            .unwrap();
        let mut identities = BTreeSet::new();
        for slot in report.target_slots() {
            let path = stage(&fixture.engine, slot)
                .join("versions")
                .join(target)
                .join("content")
                .join(&fixture.engine.state().versions[source].creation_name);
            assert_eq!(fs::read(&path).unwrap(), bytes(&fixture.engine, source));
            let metadata = fs::metadata(path).unwrap();
            assert_eq!(metadata.nlink(), 1);
            assert_ne!(
                (metadata.dev(), metadata.ino()),
                (original.dev(), original.ino())
            );
            assert!(identities.insert((metadata.dev(), metadata.ino())));
        }
    }
    fixture
        .engine
        .verify_file_history_move_target(
            &projection,
            info.digest(),
            &HistoryWindowCancellation::new(),
        )
        .unwrap();
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
}

#[test]
fn staged_database_retains_imported_lifecycle_without_a_fabricated_operation_receipt() {
    let mut fixture = Fixture::new(false);
    let file = entry(&fixture.engine, &fixture.source, "source.txt");
    let file_id = HistoryFileId::parse(&file.file_id).unwrap();
    fixture
        .engine
        .soft_delete_file(DeleteFileRequest {
            target: FileDeletionTarget {
                repository_id: &HistoryRepositoryId::parse(&fixture.source.repository_id).unwrap(),
                file_id: &file_id,
                path: &name_policy::LookupPath::parse("source.txt").unwrap(),
            },
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    let deletion = fixture.engine.file_deletion(&file.file_id).unwrap().clone();
    fixture
        .engine
        .undo_file_deletion(UndoFileDeletionRequest {
            file_id: &file_id,
            deletion_node_id: &deletion.node_id,
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    let (projection, info) = plan(&mut fixture);
    fixture
        .engine
        .prepare_file_history_move_target(
            &projection,
            info.digest(),
            &HistoryWindowCancellation::new(),
        )
        .unwrap();
    let path = stage(&fixture.engine, "D04").join(".zxs-file.sqlite");
    platform_io::sqlite::block_on_sqlite_connection(async {
        let mut connection = SqliteConnection::connect_with(
            &SqliteConnectOptions::new().filename(path).read_only(true),
        )
        .await
        .unwrap();
        let rows = sqlite_access::query("SELECT body FROM records WHERE kind='lifecycle'")
            .fetch_all(&mut connection)
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        for row in rows {
            let body: String = row.try_get("body").unwrap();
            let event: json_codec::Value = json_codec::from_str(&body).unwrap();
            assert_eq!(
                event["node"]["file_id"],
                projection.target_file_id().as_str()
            );
            assert_ne!(event["node"]["node_id"], deletion.node_id);
            assert_ne!(event["operation_id"], projection.operation_id().to_string());
        }
        assert!(
            sqlite_access::query("SELECT id FROM records WHERE kind='deletion'")
                .fetch_all(&mut connection)
                .await
                .unwrap()
                .is_empty()
        );
        connection.close().await.unwrap();
    });
    assert!(
        !fixture
            .engine
            .state()
            .operations
            .contains_key(&projection.operation_id())
    );
}

#[test]
fn cold_replay_verifies_existing_materials_under_the_same_original_envelope() {
    let mut fixture = Fixture::new(false);
    let (projection, info) = plan(&mut fixture);
    fixture
        .engine
        .prepare_file_history_move_target(
            &projection,
            info.digest(),
            &HistoryWindowCancellation::new(),
        )
        .unwrap();
    let path = staged_body(&fixture.engine, &projection, "D04");
    let inode = fs::metadata(&path).unwrap().ino();
    let Fixture { temp, engine, .. } = fixture;
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    let resumed = engine
        .resume_file_history_move_recovery(
            info.operation_id(),
            info.digest(),
            "admin",
            &HistoryWindowCancellation::new(),
        )
        .unwrap();
    engine
        .verify_file_history_move_target(&resumed, info.digest(), &HistoryWindowCancellation::new())
        .unwrap();
    engine
        .prepare_file_history_move_target(
            &resumed,
            info.digest(),
            &HistoryWindowCancellation::new(),
        )
        .unwrap();
    assert_eq!(fs::metadata(path).unwrap().ino(), inode);
    assert_eq!(
        resumed.proposed_source_deletion(),
        projection.proposed_source_deletion()
    );
    assert!(!engine.state().operations.contains_key(&info.operation_id()));
}

#[test]
fn an_unclaimed_body_is_not_adopted_even_when_its_bytes_match() {
    let mut fixture = Fixture::new(false);
    let (projection, info) = plan(&mut fixture);
    fixture
        .engine
        .prepare_file_history_move_target(
            &projection,
            info.digest(),
            &HistoryWindowCancellation::new(),
        )
        .unwrap();
    let body = staged_body(&fixture.engine, &projection, "D04");
    let original = fs::read(&body).unwrap();
    let marker = body
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("copy-identity.json");
    fs::remove_file(&marker).unwrap();
    assert_eq!(
        fixture
            .engine
            .prepare_file_history_move_target(
                &projection,
                info.digest(),
                &HistoryWindowCancellation::new()
            )
            .unwrap_err()
            .code(),
        ErrorCode::RecoveryRequired
    );
    assert_eq!(fs::read(body).unwrap(), original);
    assert!(!marker.exists());
    assert!(
        fixture
            .engine
            .file_deletion(projection.capture().source_file_id().as_str())
            .is_none()
    );
}

#[test]
fn identical_replacement_inode_and_new_hardlink_both_invalidate_target_materials() {
    for hardlink in [false, true] {
        let mut fixture = Fixture::new(false);
        let (projection, info) = plan(&mut fixture);
        fixture
            .engine
            .prepare_file_history_move_target(
                &projection,
                info.digest(),
                &HistoryWindowCancellation::new(),
            )
            .unwrap();
        let body = staged_body(&fixture.engine, &projection, "D04");
        let alternate = body.with_extension("other");
        if hardlink {
            fs::hard_link(&body, &alternate).unwrap();
        } else {
            fs::write(&alternate, fs::read(&body).unwrap()).unwrap();
            fs::rename(&alternate, &body).unwrap();
        }
        assert!(
            fixture
                .engine
                .verify_file_history_move_target(
                    &projection,
                    info.digest(),
                    &HistoryWindowCancellation::new()
                )
                .is_err()
        );
        assert_eq!(
            fixture
                .engine
                .prepare_file_history_move_target(
                    &projection,
                    info.digest(),
                    &HistoryWindowCancellation::new()
                )
                .unwrap_err()
                .code(),
            ErrorCode::RecoveryRequired
        );
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
fn database_with_recomputed_self_digest_still_must_match_the_complete_original_projection() {
    let mut fixture = Fixture::new(false);
    let (projection, info) = plan(&mut fixture);
    fixture
        .engine
        .prepare_file_history_move_target(
            &projection,
            info.digest(),
            &HistoryWindowCancellation::new(),
        )
        .unwrap();
    let path = stage(&fixture.engine, "D04").join(".zxs-file.sqlite");
    platform_io::sqlite::block_on_sqlite_connection(async {
        let mut connection =
            SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&path))
                .await
                .unwrap();
        let row = sqlite_access::query("SELECT body FROM records WHERE kind='file'")
            .fetch_one(&mut connection)
            .await
            .unwrap();
        let mut body: json_codec::Value =
            json_codec::from_str(&row.try_get::<String, _>("body").unwrap()).unwrap();
        body["note"] = json_codec::json!("forged but well-formed note");
        sqlite_access::query("UPDATE records SET body=? WHERE kind='file'")
            .bind(json_codec::to_string(&body).unwrap())
            .execute(&mut connection)
            .await
            .unwrap();
        let rows = sqlite_access::query("SELECT kind,id,body FROM records ORDER BY kind,id")
            .fetch_all(&mut connection)
            .await
            .unwrap();
        let records = rows
            .iter()
            .map(|row| {
                json_codec::json!({
                    "kind":row.try_get::<String,_>("kind").unwrap(),
                    "id":row.try_get::<String,_>("id").unwrap(),
                    "body":row.try_get::<String,_>("body").unwrap(),
                })
            })
            .collect::<Vec<_>>();
        let digest = storage_contract::object_hash("file-local-records", &records).unwrap();
        sqlite_access::query("UPDATE identity SET records_hash=?")
            .bind(digest)
            .execute(&mut connection)
            .await
            .unwrap();
        connection.close().await.unwrap();
    });
    let forged = fs::read(&path).unwrap();
    assert_eq!(
        fixture
            .engine
            .verify_file_history_move_target(
                &projection,
                info.digest(),
                &HistoryWindowCancellation::new()
            )
            .unwrap_err()
            .code(),
        ErrorCode::ContainerLayoutInvalid
    );
    assert_eq!(
        fixture
            .engine
            .prepare_file_history_move_target(
                &projection,
                info.digest(),
                &HistoryWindowCancellation::new()
            )
            .unwrap_err()
            .code(),
        ErrorCode::RecoveryRequired
    );
    assert_eq!(fs::read(path).unwrap(), forged);
}

#[test]
fn target_preparation_requires_all_original_recovery_replicas_and_honors_initial_cancellation() {
    let mut fixture = Fixture::new(false);
    let (projection, info) = plan(&mut fixture);
    let cancel = HistoryWindowCancellation::new();
    cancel.cancel();
    assert_eq!(
        fixture
            .engine
            .prepare_file_history_move_target(&projection, info.digest(), &cancel)
            .unwrap_err()
            .code(),
        ErrorCode::OperationAborted
    );
    assert!(!stage(&fixture.engine, "D04").exists());
    let member = fixture
        .engine
        .config()
        .disks
        .iter()
        .find(|disk| disk.metadata_member)
        .unwrap();
    let path = member
        .root
        .join("file-history-move-recovery/preparation.json");
    fs::remove_file(path).unwrap();
    assert!(
        fixture
            .engine
            .prepare_file_history_move_target(
                &projection,
                info.digest(),
                &HistoryWindowCancellation::new()
            )
            .is_err()
    );
    assert!(!stage(&fixture.engine, "D04").exists());
    assert!(
        fixture
            .engine
            .file_deletion(projection.capture().source_file_id().as_str())
            .is_none()
    );
}

#[test]
fn staged_target_keeps_the_earlier_original_source_write_fence() {
    let mut fixture = Fixture::new(false);
    let (projection, info) = plan(&mut fixture);
    fixture
        .engine
        .prepare_file_history_move_target(
            &projection,
            info.digest(),
            &HistoryWindowCancellation::new(),
        )
        .unwrap();
    let path = stage(&fixture.engine, "D04").join("joint-metadata.json");
    let retained = fs::read(&path).unwrap();
    fixture
        .engine
        .set_file_note(
            projection.capture().source_file_id().as_str(),
            None,
            &Note::parse("new source fact".into()).unwrap(),
            "admin",
            op(),
        )
        .unwrap_err();
    fixture
        .engine
        .verify_file_history_move_target(
            &projection,
            info.digest(),
            &HistoryWindowCancellation::new(),
        )
        .unwrap();
    assert_eq!(fs::read(path).unwrap(), retained);
    assert!(
        fixture
            .engine
            .namespace_tree(&fixture.target.main_branch_id)
            .unwrap()
            .entries()
            .is_empty()
    );
}
