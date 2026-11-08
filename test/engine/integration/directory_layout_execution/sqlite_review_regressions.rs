//! Unknown SQLite schema and records must survive a refused materialization.
use super::*;
use sqlite_access::{Connection, Executor, SqliteConnection, sqlite::SqliteConnectOptions};

const DATABASE: &str = ".zxs-directory.sqlite";

fn add_unregistered_sqlite_material(path: &Path, statements: &[&'static str]) {
    // The product has no open file-backed connection to this object database;
    // its verifier deserializes bounded bytes into a separate memory database.
    platform_io::sqlite::block_on_sqlite_connection(async {
        let mut connection = SqliteConnection::connect_with(
            &SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(false),
        )
        .await
        .unwrap();
        for statement in statements {
            connection.execute(*statement).await.unwrap();
        }
        connection.close().await.unwrap();
    });
}

fn assert_rejected_without_replacing_database(mut fixture: Fixture, statements: &[&'static str]) {
    let entry = fixture.save("file.txt", b"preserve the actual file body");
    fixture.initialize("");
    fixture
        .engine
        .reconcile_directory_layout(&fixture.repository.repository_id)
        .unwrap();
    let path = fixture.root().join(DATABASE);
    add_unregistered_sqlite_material(&path, statements);
    let unknown = fs::read(&path).unwrap();
    let state = storage_engine::canonical_json(fixture.engine.state()).unwrap();
    let result = fixture
        .engine
        .reconcile_directory_layout(&fixture.repository.repository_id);
    assert!(
        result.is_err(),
        "unregistered SQLite content was accepted as a supported closed schema/record set"
    );
    assert_eq!(fs::read(&path).unwrap(), unknown);
    assert_eq!(
        storage_engine::canonical_json(fixture.engine.state()).unwrap(),
        state
    );
    assert!(!fixture.engine.is_read_only());
    fixture.assert_body(
        &fixture.root().join("file.txt"),
        &entry,
        b"preserve the actual file body",
    );
}

#[test]
fn directory_database_does_not_hide_sqlite_prefix_wildcard_tables_before_replacement() {
    for statements in [
        vec![
            "CREATE TABLE sqliteXhidden(payload TEXT NOT NULL) STRICT",
            "INSERT INTO sqliteXhidden VALUES ('unknown material must not be silently dropped')",
        ],
        vec!["CREATE VIEW sqliteXview AS SELECT body FROM records"],
        vec!["CREATE INDEX sqliteXindex ON records(body)"],
        vec!["CREATE TRIGGER sqliteXtrigger AFTER INSERT ON records BEGIN SELECT 1; END"],
    ] {
        assert_rejected_without_replacing_database(Fixture::new(), &statements);
    }
}

#[test]
fn directory_database_checks_the_first_record_instead_of_skipping_the_empty_cursor_key() {
    assert_rejected_without_replacing_database(
        Fixture::new(),
        &[
            "PRAGMA ignore_check_constraints=ON",
            "INSERT INTO records VALUES ('','','unknown material below the first admitted kind')",
            "PRAGMA ignore_check_constraints=OFF",
        ],
    );
}

#[test]
fn directory_database_in_another_entity_cannot_hide_from_file_absence_inventory() {
    let mut fixture = Fixture::new();
    let entry = fixture.save("plain/kept.txt", b"preserve ordinary directory");
    fixture.initialize("");
    fixture
        .engine
        .reconcile_directory_layout(&fixture.repository.repository_id)
        .unwrap();
    let root_database = fixture.root().join(DATABASE);
    let misplaced = fixture.root().join("plain").join(DATABASE);
    fs::copy(&root_database, &misplaced).unwrap();
    let unknown = fs::read(&misplaced).unwrap();
    let workspace = fixture
        .engine
        .create_workspace(&fixture.repository.main_branch_id, "admin")
        .unwrap();
    fixture
        .engine
        .stage_bytes(&workspace.workspace_id, "new.txt", b"new file")
        .unwrap();
    let before = storage_engine::canonical_json(fixture.engine.state()).unwrap();
    // Preparing a previously absent file exercises the secondary scan, not only
    // the normal layout preflight. A matching filename is not an exemption.
    assert!(
        fixture
            .engine
            .prepare_workspace_commit(
                CommitRequest {
                    workspace_id: workspace.workspace_id,
                    operation_id: op(),
                    principal: "admin".into(),
                    expected_head: workspace.base_head,
                    fencing_token: workspace.fencing_token,
                    message: "must refuse misplaced database".into(),
                    request_digest: None,
                },
                None,
            )
            .and_then(|plan| plan.execute())
            .is_err()
    );
    assert_eq!(fs::read(&misplaced).unwrap(), unknown);
    assert_eq!(
        storage_engine::canonical_json(fixture.engine.state()).unwrap(),
        before
    );
    fixture.assert_body(
        &fixture.root().join("plain/kept.txt"),
        &entry,
        b"preserve ordinary directory",
    );
}

#[test]
fn directory_database_rejects_extra_sqlite_internal_schema_objects() {
    assert_rejected_without_replacing_database(Fixture::new(), &["ANALYZE"]);
}
