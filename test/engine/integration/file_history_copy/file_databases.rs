//! Persisted file-local SQLite over real Engine publication and recovery.
use super::*;
use sqlite_access::{Connection, Row, SqliteConnection, sqlite::SqliteConnectOptions};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Barrier},
};
use storage_contract::history_window::{HistoryFileId, HistoryRepositoryId};

const NAME: &str = ".zxs-file.sqlite";
fn database(
    engine: &Engine,
    repository: &Repository,
    file: &str,
    slot: &str,
    path: &str,
) -> PathBuf {
    let root = &engine
        .config()
        .disks
        .iter()
        .find(|disk| disk.slot_id == slot)
        .unwrap()
        .root;
    root.join("histories")
        .join(&repository.history_tree_id)
        .join("file-containers")
        .join(file)
        .join("namespace")
        .join(path)
        .join(NAME)
}
fn records(path: &Path) -> BTreeMap<(String, String), json_codec::Value> {
    platform_io::sqlite::block_on_sqlite_connection(async {
        let mut connection = SqliteConnection::connect_with(
            &SqliteConnectOptions::new().filename(path).read_only(true),
        )
        .await
        .unwrap();
        let rows = sqlite_access::query("SELECT kind,id,body FROM records ORDER BY kind,id")
            .fetch_all(&mut connection)
            .await
            .unwrap();
        let rows = rows
            .into_iter()
            .map(|row| {
                let body: String = row.try_get("body").unwrap();
                (
                    (row.try_get("kind").unwrap(), row.try_get("id").unwrap()),
                    json_codec::from_str(&body).unwrap(),
                )
            })
            .collect();
        connection.close().await.unwrap();
        rows
    })
}
fn delete(engine: &mut Engine, repo: &Repository, file: &Entry) -> FileDeletionNode {
    engine
        .soft_delete_file(DeleteFileRequest {
            target: FileDeletionTarget {
                repository_id: &HistoryRepositoryId::parse(&repo.repository_id).unwrap(),
                file_id: &HistoryFileId::parse(&file.file_id).unwrap(),
                path: &name_policy::LookupPath::parse(file.path()).unwrap(),
            },
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    engine.file_deletion(&file.file_id).unwrap().clone()
}
#[test]
fn copied_file_has_independent_pathless_database_and_source_remains_visible() {
    let mut fixture = Fixture::new(false);
    let original = entry(&fixture.engine, &fixture.source, "source.txt");
    let branch = fixture
        .engine
        .create_file_branch(
            &original.file_id,
            &original.version_id,
            &BranchName::parse("review").unwrap(),
            "admin",
            op(),
        )
        .unwrap()
        .branch_id
        .unwrap();
    let head = fixture.engine.file_branch_head(&branch).unwrap();
    fixture
        .engine
        .update_file_node_tags(FileNodeTagsUpdateRequest {
            file_id: &original.file_id,
            targets: &[FileTagTarget {
                branch_id: branch,
                source_node_id: head.clone(),
                expected_node_id: head,
            }],
            add: &["persisted-tag".into()],
            remove: &[],
            location: None,
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    let copy = request(&fixture.engine, &fixture.source, &fixture.target);
    fixture
        .engine
        .copy_file_history(&copy, "admin", op())
        .unwrap();
    let copied = entry(&fixture.engine, &fixture.target, "copied.txt");
    let left = database(
        &fixture.engine,
        &fixture.source,
        &original.file_id,
        "P01",
        "source.txt",
    );
    let right = database(
        &fixture.engine,
        &fixture.target,
        &copied.file_id,
        "D04",
        "copied.txt",
    );
    assert_ne!(
        fs::metadata(&left).unwrap().ino(),
        fs::metadata(&right).unwrap().ino()
    );
    let data = records(&right);
    assert_eq!(data.keys().filter(|(kind, _)| kind == "version").count(), 2);
    assert_eq!(data.keys().filter(|(kind, _)| kind == "branch").count(), 2);
    assert!(
        data.values()
            .any(|row| row["node"]["tags"] == json_codec::json!(["persisted-tag"]))
    );
    assert!(
        data.keys()
            .filter(|(kind, _)| kind == "branch")
            .all(
                |(_, id)| fixture.engine.state().branches[id].file_id.as_deref()
                    == Some(copied.file_id.as_str())
            )
    );
    for row in data.values() {
        let text = json_codec::to_string(row).unwrap();
        for field in [
            "\"path\"",
            "\"previous_path\"",
            "\"location\"",
            "\"route_slot_ids\"",
        ] {
            assert!(!text.contains(field));
        }
    }
    assert_eq!(
        entry(&fixture.engine, &fixture.source, "source.txt").file_id,
        original.file_id
    );
    let Fixture {
        temp,
        engine,
        source: _,
        target,
    } = fixture;
    drop(engine);
    let engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(records(&right), data);
    assert_eq!(
        bytes(&engine, &entry(&engine, &target, "copied.txt").version_id),
        b"new full body"
    );
}
#[test]
fn file_database_keeps_undone_deletion_history_and_rebuilds_only_from_admitted_state() {
    let mut fixture = Fixture::new(false);
    let file = entry(&fixture.engine, &fixture.source, "source.txt");
    let first = delete(&mut fixture.engine, &fixture.source, &file);
    fixture
        .engine
        .undo_file_deletion(UndoFileDeletionRequest {
            file_id: &HistoryFileId::parse(&file.file_id).unwrap(),
            deletion_node_id: &first.node_id,
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    let second = delete(&mut fixture.engine, &fixture.source, &file);
    let path = database(
        &fixture.engine,
        &fixture.source,
        &file.file_id,
        "P01",
        "source.txt",
    );
    let data = records(&path);
    assert_eq!(
        data.keys().filter(|(kind, _)| kind == "lifecycle").count(),
        3
    );
    assert_eq!(
        data[&("deletion".into(), second.node_id.clone())]["branch_heads"],
        json_codec::to_value(&second.branch_heads).unwrap()
    );
    assert!(
        data.values()
            .any(|row| row["node"]["node_id"] == first.node_id)
    );
    let Fixture { temp, engine, .. } = fixture;
    drop(engine);
    fs::remove_file(&path).unwrap();
    let engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(records(&path), data);
    assert_eq!(
        engine.file_deletion(&file.file_id).unwrap().node_id,
        second.node_id
    );
}
#[test]
fn file_database_wrong_history_is_preserved_and_refuses_new_publication() {
    let mut fixture = Fixture::new(false);
    let file = entry(&fixture.engine, &fixture.source, "source.txt");
    let path = database(
        &fixture.engine,
        &fixture.source,
        &file.file_id,
        "P01",
        "source.txt",
    );
    platform_io::sqlite::block_on_sqlite_connection(async {
        let mut connection =
            SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&path))
                .await
                .unwrap();
        sqlite_access::query("UPDATE identity SET history_tree_id=?")
            .bind(&fixture.target.history_tree_id)
            .execute(&mut connection)
            .await
            .unwrap();
        connection.close().await.unwrap();
    });
    let preserved = fs::read(&path).unwrap();
    let operation = op();
    let result = fixture.engine.set_file_note(
        &file.file_id,
        None,
        &Note::parse("not admitted".into()).unwrap(),
        "admin",
        operation,
    );
    assert_eq!(
        result.unwrap_err().code(),
        ErrorCode::ContainerLayoutInvalid
    );
    assert!(!fixture.engine.state().operations.contains_key(&operation));
    assert_eq!(fs::read(&path).unwrap(), preserved);
}
#[test]
fn file_database_process_death_at_each_install_stage_recovers_original_operation() {
    const CHILD: &str = "ZXS_FILE_DATABASE_CRASH_CHILD";
    if let Ok(input) = std::env::var(CHILD) {
        let (root, file, operation, point): (PathBuf, String, OperationId, String) =
            json_codec::from_str(&input).unwrap();
        let mut engine = Engine::open(&root).unwrap();
        let barrier = Arc::new(Barrier::new(2));
        engine.inject_pause_once(&point, Arc::clone(&barrier));
        std::thread::spawn(move || {
            barrier.wait();
            std::process::exit(86);
        });
        let _ = engine.set_file_note(
            &file,
            None,
            &Note::parse("recovered note".into()).unwrap(),
            "admin",
            operation,
        );
        panic!("installer never reached {point}");
    }
    for point in [
        "after_database_stage",
        "after_database_intent_prepare",
        "after_database_intent",
        "after_database_publish",
    ] {
        let fixture = Fixture::new(false);
        let file = entry(&fixture.engine, &fixture.source, "source.txt");
        let path = database(
            &fixture.engine,
            &fixture.source,
            &file.file_id,
            "P01",
            "source.txt",
        );
        let operation = op();
        let root = fixture.temp.path().join("system");
        let Fixture { temp, engine, .. } = fixture;
        drop(engine);
        let input =
            json_codec::to_string(&(root.clone(), file.file_id.clone(), operation, point)).unwrap();
        let result=std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact","file_databases::file_database_process_death_at_each_install_stage_recovers_original_operation","--nocapture"]).env(CHILD,input).status().unwrap();
        assert_eq!(result.code(), Some(86), "{point}");
        let inspection = Engine::open_for_recovery(&root).unwrap();
        assert!(inspection.is_read_only());
        drop(inspection);
        let mut engine = Engine::open(&root).unwrap();
        assert_eq!(
            engine
                .operation_record(operation)
                .unwrap()
                .result()
                .commit_state,
            CommitState::Published
        );
        assert_eq!(
            records(&path)[&("file".into(), file.file_id.clone())]["note"],
            "recovered note"
        );
        let replay = engine
            .set_file_note(
                &file.file_id,
                None,
                &Note::parse("recovered note".into()).unwrap(),
                "admin",
                operation,
            )
            .unwrap();
        assert_eq!(replay.operation_id, operation);
        assert_eq!(bytes(&engine, &file.version_id), b"new full body");
        drop(engine);
        drop(temp);
    }
}

fn omit_balanced_pair(path: &Path, ids: [OperationId; 2]) {
    #[derive(data_serialization::Serialize)]
    struct Record {
        kind: String,
        id: String,
        body: String,
    }
    platform_io::sqlite::block_on_sqlite_connection(async {
        let mut connection =
            SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(path))
                .await
                .unwrap();
        for id in ids {
            sqlite_access::query("DELETE FROM records WHERE kind='lifecycle' AND id=?")
                .bind(id.to_string())
                .execute(&mut connection)
                .await
                .unwrap();
        }
        let rows = sqlite_access::query("SELECT kind,id,body FROM records ORDER BY kind,id")
            .fetch_all(&mut connection)
            .await
            .unwrap();
        let rows = rows
            .into_iter()
            .map(|row| Record {
                kind: row.try_get("kind").unwrap(),
                id: row.try_get("id").unwrap(),
                body: row.try_get("body").unwrap(),
            })
            .collect::<Vec<_>>();
        let hash = storage_contract::object_hash("file-local-records", &rows).unwrap();
        sqlite_access::query("UPDATE identity SET records_hash=?")
            .bind(hash)
            .execute(&mut connection)
            .await
            .unwrap();
        connection.close().await.unwrap();
    });
}
#[test]
fn file_database_self_consistent_omitted_history_cannot_replace_captured_state() {
    let mut fixture = Fixture::new(false);
    let file = entry(&fixture.engine, &fixture.source, "source.txt");
    let deletion = delete(&mut fixture.engine, &fixture.source, &file);
    let undo = op();
    fixture
        .engine
        .undo_file_deletion(UndoFileDeletionRequest {
            file_id: &HistoryFileId::parse(&file.file_id).unwrap(),
            deletion_node_id: &deletion.node_id,
            principal: "admin",
            operation_id: undo,
        })
        .unwrap();
    let path = database(
        &fixture.engine,
        &fixture.source,
        &file.file_id,
        "P01",
        "source.txt",
    );
    let retained = fixture
        .temp
        .path()
        .join("disk-0/histories")
        .join(&fixture.source.history_tree_id)
        .join(".zxs-layout/database-installs");
    let barrier = Arc::new(Barrier::new(2));
    fixture
        .engine
        .inject_pause_once("after_database_stage", Arc::clone(&barrier));
    let editor = std::thread::spawn(move || {
        barrier.wait();
        let mut pending = Vec::new();
        for generation in fs::read_dir(&retained).unwrap() {
            for attempt in fs::read_dir(generation.unwrap().path()).unwrap() {
                let attempt = attempt.unwrap().path();
                if attempt.join("payload").is_file() && !attempt.join("intent.json").exists() {
                    pending.push(attempt.join("payload"));
                }
            }
        }
        assert_eq!(pending.len(), 1);
        omit_balanced_pair(&pending[0], [deletion.operation_id, undo]);
        let preserved = fs::read(&pending[0]).unwrap();
        barrier.wait();
        (pending.remove(0), preserved)
    });
    let operation = op();
    let result = fixture.engine.set_file_note(
        &file.file_id,
        None,
        &Note::parse("complete-history-only".into()).unwrap(),
        "admin",
        operation,
    );
    assert!(result.is_err());
    let (orphan, preserved) = editor.join().unwrap();
    let Fixture { temp, engine, .. } = fixture;
    drop(engine);
    let engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(
        engine
            .operation_record(operation)
            .unwrap()
            .result()
            .commit_state,
        CommitState::Published
    );
    let data = records(&path);
    assert_eq!(
        data.keys().filter(|(kind, _)| kind == "lifecycle").count(),
        2
    );
    assert_eq!(
        data[&("file".into(), file.file_id)]["note"],
        "complete-history-only"
    );
    assert_eq!(fs::read(orphan).unwrap(), preserved);
}
