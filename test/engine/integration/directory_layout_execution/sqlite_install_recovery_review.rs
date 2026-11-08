//! Real Engine/file-backed installation regressions prepared by independent review.
//! Faults model interrupted publication; they do not model physical power loss.
use super::*;
use sqlite_access::{
    Connection, Row, SqliteConnection,
    sqlite::{SqliteConnectOptions, SqliteOwnedBuf},
};
use std::{sync::mpsc, thread, time::Duration};

const NAME: &str = ".zxs-directory.sqlite";

#[derive(Clone, Debug, PartialEq)]
struct Snapshot {
    sequence: u64,
    records: BTreeMap<(String, String), json_codec::Value>,
}

async fn snapshot(connection: &mut SqliteConnection) -> Snapshot {
    let checks = sqlite_access::query("PRAGMA integrity_check")
        .fetch_all(&mut *connection)
        .await
        .unwrap();
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0].try_get::<String, _>(0).unwrap(), "ok");
    let identity = sqlite_access::query("SELECT source_sequence FROM identity WHERE singleton=1")
        .fetch_one(&mut *connection)
        .await
        .unwrap();
    let sequence = identity
        .try_get::<String, _>("source_sequence")
        .unwrap()
        .parse()
        .unwrap();
    let rows = sqlite_access::query("SELECT kind,id,body FROM records ORDER BY kind,id")
        .fetch_all(&mut *connection)
        .await
        .unwrap();
    let records = rows
        .into_iter()
        .map(|row| {
            let body: String = row.try_get("body").unwrap();
            (
                (row.try_get("kind").unwrap(), row.try_get("id").unwrap()),
                json_codec::from_str(&body).unwrap(),
            )
        })
        .collect();
    Snapshot { sequence, records }
}

fn read_snapshot(bytes: &[u8]) -> Snapshot {
    platform_io::sqlite::block_on_sqlite_connection(async {
        let mut connection =
            SqliteConnection::connect_with(&SqliteConnectOptions::new().in_memory(true))
                .await
                .unwrap();
        connection
            .deserialize(None, SqliteOwnedBuf::try_from(bytes).unwrap(), true)
            .await
            .unwrap();
        let result = snapshot(&mut connection).await;
        connection.close().await.unwrap();
        result
    })
}

fn contains_version(snapshot: &Snapshot, version: &str) -> bool {
    snapshot
        .records
        .values()
        .filter_map(|record| record.get("entries"))
        .flat_map(|entries| entries.as_array().unwrap())
        .any(|entry| entry["version_id"] == version)
}

fn active() -> Fixture {
    let mut fixture = Fixture::new();
    fixture.save("file.txt", b"published bytes");
    fixture.initialize("");
    fixture
        .engine
        .reconcile_directory_layout(&fixture.repository.repository_id)
        .unwrap();
    fixture
}

fn preserve_pending(fixture: Fixture) -> Fixture {
    let Fixture {
        temporary,
        primary,
        engine,
        repository,
    } = fixture;
    drop(engine);
    let engine = Engine::open_for_recovery(&temporary.path().join("system")).unwrap();
    Fixture {
        temporary,
        primary,
        engine,
        repository,
    }
}

fn prepare_save(
    fixture: &mut Fixture,
    operation: OperationId,
) -> storage_engine::PreparedWorkspaceCommit {
    let workspace = fixture
        .engine
        .create_workspace(&fixture.repository.main_branch_id, "admin")
        .unwrap();
    fixture
        .engine
        .stage_bytes(&workspace.workspace_id, "file.txt", b"decided replacement")
        .unwrap();
    fixture
        .engine
        .prepare_workspace_commit(
            CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: operation,
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "SQLite installation recovery regression".into(),
                request_digest: None,
            },
            None,
        )
        .unwrap()
        .execute()
        .unwrap()
}

#[test]
fn late_corrupt_replacement_is_preserved_until_explicit_quarantine_then_same_decision_recovers() {
    let mut fixture = active();
    let original = fixture.entry("file.txt");
    let container = fixture.root().join("file.txt");
    let container_inode = identity(&container);
    let database = fixture.root().join(NAME);
    let operation = op();
    let prepared = prepare_save(&mut fixture, operation);
    let published = fixture.engine.state().sequence;
    // Prepared layouts have already admitted the original cache. Substitute a
    // different regular inode before finish; the second preflight must see it.
    let late = fixture.temporary.path().join("late-invalid.sqlite");
    let corrupt = b"foreign bytes installed after detached preparation";
    fs::write(&late, corrupt).unwrap();
    let late_inode = fs::symlink_metadata(&late).unwrap().ino();
    fs::rename(&late, &database).unwrap();
    let error = fixture
        .engine
        .finish_workspace_commit(prepared)
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::InDoubt);
    assert_eq!(fixture.engine.state().sequence, published);
    assert_eq!(fs::read(&database).unwrap(), corrupt);
    assert_eq!(fs::symlink_metadata(&database).unwrap().ino(), late_inode);
    assert_eq!(
        fixture
            .engine
            .operation_record(operation)
            .unwrap()
            .result()
            .commit_state,
        storage_engine::CommitState::InDoubt
    );
    let fixture = preserve_pending(fixture);
    assert!(fixture.engine.is_read_only());
    assert_eq!(fixture.engine.state().sequence, published);
    assert_eq!(fs::read(&database).unwrap(), corrupt);
    let Fixture {
        temporary,
        primary,
        engine,
        repository,
    } = fixture;
    drop(engine);
    let system = temporary.path().join("system");
    let error = match Engine::open(&system) {
        Ok(_) => panic!("normal recovery must preserve and reject the corrupt cache"),
        Err(error) => error,
    };
    assert_eq!(error.code(), ErrorCode::ContainerLayoutInvalid);
    assert_eq!(fs::read(&database).unwrap(), corrupt);
    assert_eq!(identity(&container), container_inode);
    // This is an explicit test/operator quarantine, outside managed data, not
    // an assertion that product recovery automatically removes unknown bytes.
    let quarantine = temporary.path().join("retained-corrupt.sqlite");
    fs::rename(&database, &quarantine).unwrap();
    fs::File::open(database.parent().unwrap())
        .unwrap()
        .sync_all()
        .unwrap();
    fs::File::open(temporary.path())
        .unwrap()
        .sync_all()
        .unwrap();
    let engine = Engine::open(&system).unwrap();
    let fixture = Fixture {
        temporary,
        primary,
        engine,
        repository,
    };
    assert!(!fixture.engine.is_read_only());
    assert_eq!(
        fixture
            .engine
            .operation_record(operation)
            .unwrap()
            .result()
            .commit_state,
        storage_engine::CommitState::Published
    );
    let recovered = fixture.entry("file.txt");
    assert_eq!(recovered.file_id, original.file_id);
    assert_ne!(recovered.version_id, original.version_id);
    assert_eq!(identity(&container), container_inode);
    fixture.assert_body(&container, &original, b"published bytes");
    fixture.assert_body(&container, &recovered, b"decided replacement");
    assert_eq!(fs::read(&quarantine).unwrap(), corrupt);
    assert_eq!(
        read_snapshot(&fs::read(&database).unwrap()).sequence,
        fixture.engine.state().sequence
    );
    let sequence = fixture.engine.state().sequence;
    let fixture = fixture.reopen();
    assert_eq!(fixture.engine.state().sequence, sequence);
    assert_eq!(fixture.entry("file.txt").version_id, recovered.version_id);
    assert_eq!(fs::read(&quarantine).unwrap(), corrupt);
}

#[test]
fn installed_unpublished_database_cannot_elect_a_state_during_preserve_pending() {
    let mut fixture = active();
    let original = fixture.entry("file.txt");
    let root = fixture.root();
    let container_inode = identity(&root.join("file.txt"));
    let published = fixture.engine.state().sequence;
    let operation = op();
    fixture.engine.inject_fault_once("after_container_names");
    let error = fixture
        .engine
        .file_namespace(
            &fixture.repository.repository_id,
            NamespaceOperation::parse(
                NamespaceOperationKind::Rename,
                "file.txt",
                Some("renamed.txt"),
            )
            .unwrap(),
            "admin",
            operation,
        )
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::InDoubt);
    assert_eq!(fixture.engine.state().sequence, published);
    let installed = fs::read(root.join(NAME)).unwrap();
    assert!(read_snapshot(&installed).sequence > published);
    assert_eq!(identity(&root.join("renamed.txt")), container_inode);
    let fixture = preserve_pending(fixture);
    assert!(fixture.engine.is_read_only());
    assert_eq!(fixture.engine.state().sequence, published);
    assert_eq!(fixture.entry("file.txt").file_id, original.file_id);
    assert!(
        fixture
            .engine
            .namespace_tree(&fixture.repository.main_branch_id)
            .unwrap()
            .get("renamed.txt")
            .is_none()
    );
    assert_eq!(fs::read(root.join(NAME)).unwrap(), installed);
    assert_eq!(
        fixture
            .engine
            .operation_record(operation)
            .unwrap()
            .result()
            .commit_state,
        storage_engine::CommitState::InDoubt
    );
    let mut fixture = fixture.reopen();
    let completed = fixture
        .engine
        .operation_record(operation)
        .unwrap()
        .into_result();
    assert_eq!(
        completed.commit_state,
        storage_engine::CommitState::Published
    );
    assert_eq!(fixture.entry("renamed.txt").file_id, original.file_id);
    assert_eq!(identity(&root.join("renamed.txt")), container_inode);
    fixture.assert_body(&root.join("renamed.txt"), &original, b"published bytes");
    assert_eq!(
        read_snapshot(&fs::read(root.join(NAME)).unwrap()).sequence,
        fixture.engine.state().sequence
    );
    let sequence = fixture.engine.state().sequence;
    let replay = fixture
        .engine
        .file_namespace(
            &fixture.repository.repository_id,
            NamespaceOperation::parse(
                NamespaceOperationKind::Rename,
                "file.txt",
                Some("renamed.txt"),
            )
            .unwrap(),
            "admin",
            operation,
        )
        .unwrap();
    assert_eq!(replay.commit_state, storage_engine::CommitState::Published);
    assert_eq!(replay.sequence, completed.sequence);
    assert_eq!(fixture.engine.state().sequence, sequence);
    let fixture = fixture.reopen();
    assert_eq!(fixture.engine.state().sequence, sequence);
    assert_eq!(identity(&root.join("renamed.txt")), container_inode);
}

#[test]
fn detached_sqlite_reader_keeps_its_generation_across_real_engine_installation() {
    let mut fixture = active();
    let old_entry = fixture.entry("file.txt");
    let database = fixture.root().join(NAME);
    let old_bytes = fs::read(&database).unwrap();
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    // The product reads bounded bytes then deserializes an owned in-memory
    // SQLite. No long-lived raw-path connection or unsupported writer is used.
    let reader = thread::spawn(move || {
        let mut connection = platform_io::sqlite::block_on_sqlite_connection(async {
            let mut connection =
                SqliteConnection::connect_with(&SqliteConnectOptions::new().in_memory(true))
                    .await
                    .unwrap();
            connection
                .deserialize(
                    None,
                    SqliteOwnedBuf::try_from(old_bytes.as_slice()).unwrap(),
                    true,
                )
                .await
                .unwrap();
            connection
        });
        let first = platform_io::sqlite::block_on_sqlite_connection(snapshot(&mut connection));
        ready_tx.send(first.clone()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(30)).unwrap();
        let after = platform_io::sqlite::block_on_sqlite_connection(snapshot(&mut connection));
        assert_eq!(after, first);
        platform_io::sqlite::block_on_sqlite_connection(connection.close()).unwrap();
        after
    });
    let old = ready_rx.recv_timeout(Duration::from_secs(30)).unwrap();
    assert!(contains_version(&old, &old_entry.version_id));
    let saved = fixture.save("file.txt", b"new generation while reader exists");
    let fresh = read_snapshot(&fs::read(&database).unwrap());
    release_tx.send(()).unwrap();
    let held = reader.join().unwrap();
    assert_eq!(held, old);
    assert!(fresh.sequence > old.sequence);
    assert_eq!(fresh.sequence, fixture.engine.state().sequence);
    assert!(!contains_version(&held, &saved.version_id));
    assert!(contains_version(&fresh, &saved.version_id));
    fixture.assert_body(
        &fixture.root().join("file.txt"),
        &old_entry,
        b"published bytes",
    );
    fixture.assert_body(
        &fixture.root().join("file.txt"),
        &saved,
        b"new generation while reader exists",
    );
}

// The safe installer is exercised by actual child-process termination below.
#[path = "safe_install.rs"]
mod safe_install;
