use super::*;
#[test]
fn metadata_metrics_count_flat_and_sharded_files_by_kind() {
    let temp = tempfile::tempdir().unwrap();
    let objects = temp.path().join("member/metadata/objects");
    let hash = "ab".repeat(32);
    let flat = objects.join("snapshot").join(format!("{hash}.json"));
    let shard = objects.join("state-page/ab").join(format!("{hash}.json"));
    fs::create_dir_all(flat.parent().unwrap()).unwrap();
    fs::create_dir_all(shard.parent().unwrap()).unwrap();
    fs::write(flat, b"123").unwrap();
    fs::write(shard, b"12345").unwrap();
    let metrics = disk_bytes(temp.path()).unwrap();
    assert_eq!(metrics["files"], 2);
    assert_eq!(metrics["metadata_object_bytes_all_members"], 8);
    assert_eq!(
        metrics["metadata_objects_by_kind_all_members"],
        json!({"snapshot":{"bytes":3,"files":1},"state-page":{"bytes":5,"files":1}})
    );
    for bad in [
        format!("state-page/ff/{hash}.json"),
        format!("state-page/ab/de/{hash}.json"),
        "state-page/not-a-hash.json".into(),
    ] {
        assert!(metadata_object_kind(&objects.join(bad)).is_err());
    }
}

#[test]
fn sqlite_metrics_count_only_typed_history_rows_and_reset() {
    let temp = tempfile::tempdir().unwrap();
    let index = temp.path().join("index.sqlite");
    let execute = |statement: &'static str| {
        platform_io::sqlite::block_on_sqlite_connection(async {
            let mut conn = platform_io::sqlite::open_index_connection(&index, true, None)
                .await
                .unwrap();
            conn.execute(statement).await.unwrap();
        });
    };
    execute(
        "CREATE TABLE file_history_versions(version_id TEXT PRIMARY KEY,value INTEGER);CREATE TABLE current_state(singleton INTEGER PRIMARY KEY,sequence TEXT);CREATE TABLE entities(id TEXT PRIMARY KEY,json BLOB);",
    );
    assert_eq!(
        sql(&index, true).unwrap(),
        json!({"inserts":0,"updates":0,"deletes":0})
    );
    execute(
        "INSERT INTO current_state VALUES(1,'1');UPDATE current_state SET sequence='2';INSERT INTO entities VALUES('mirror','ignored');UPDATE entities SET json='ignored again';DELETE FROM entities;INSERT INTO file_history_versions VALUES('v1',1);UPDATE file_history_versions SET value=2;DELETE FROM file_history_versions;",
    );
    assert_eq!(
        sql(&index, false).unwrap(),
        json!({"inserts":1,"updates":1,"deletes":1})
    );
    assert_eq!(
        sql(&index, true).unwrap(),
        json!({"inserts":0,"updates":0,"deletes":0})
    );
    // Reopen may rebuild the typed table and remove its triggers.
    execute(
        "DROP TABLE file_history_versions;CREATE TABLE file_history_versions(version_id TEXT PRIMARY KEY,value INTEGER);",
    );
    sql(&index, true).unwrap();
    execute("INSERT INTO file_history_versions VALUES('v2',1);");
    assert_eq!(
        sql(&index, false).unwrap(),
        json!({"inserts":1,"updates":0,"deletes":0})
    );
}

#[test]
fn sqlite_open_failure_retains_platform_error_type() {
    let temp = tempfile::tempdir().unwrap();
    let error = sql(&temp.path().join("absent.sqlite"), false).unwrap_err();
    assert!(error.downcast_ref::<platform_io::sqlite::Error>().is_some());
    assert!(error.source().is_some());
}

#[test]
fn sqlite_pragma_formatting_preserves_connect_and_optimize_close() {
    use sqlite_access::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
    platform_io::sqlite::block_on_sqlite_connection(async {
        let options = SqliteConnectOptions::new()
            .in_memory(true)
            .pragma("cache_size", "1234")
            .optimize_on_close(true, Some(23));
        let mut connection = SqliteConnection::connect_with(&options).await.unwrap();
        let row = sqlite_access::query("PRAGMA cache_size")
            .fetch_one(&mut connection)
            .await
            .unwrap();
        assert_eq!(row.get::<i64, _>(0), 1234);
        connection.close().await.unwrap();
    });
}
