use super::*;

#[test]
fn poisoned_scope_snapshot_closes_connection_during_unwind() {
    let temp = tempfile::tempdir().unwrap();
    let engine = Engine::initialize(
        &temp.path().join("state"),
        Config::development([
            temp.path().join("p"),
            temp.path().join("b"),
            temp.path().join("t"),
        ]),
    )
    .unwrap();
    let path = temp.path().join("state/index.sqlite");
    let scope = engine
        .capture_history_scope("file", "history")
        .unwrap()
        .unwrap();
    let mut writer = platform_io::sqlite::block_on_sqlite_connection(async {
        let mut writer = platform_io::sqlite::open_index_connection(&path, false, None)
            .await
            .unwrap();
        writer.execute("PRAGMA busy_timeout=0").await.unwrap();
        writer
            .execute("CREATE TABLE drop_probe(value INTEGER)")
            .await
            .unwrap();
        let row = sqlite_access::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .fetch_one(&mut writer)
            .await
            .unwrap();
        assert_eq!(
            row.get::<i64, _>(0),
            1,
            "live snapshot must retain its WAL view"
        );
        writer
    });
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _connection = scope.connection.lock().unwrap();
        let mut failure = scope.failure.lock().unwrap();
        *failure = Some(Error::new(
            ErrorCode::RecoveryRequired,
            "test retained failure",
        ));
        panic!("poison");
    }));
    assert!(scope.connection.is_poisoned());
    assert!(scope.failure.is_poisoned());
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _scope = scope;
            panic!("outer unwind");
        }))
        .is_err()
    );
    platform_io::sqlite::block_on_sqlite_connection(async {
        let row = sqlite_access::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .fetch_one(&mut writer)
            .await
            .unwrap();
        assert_eq!(
            row.get::<i64, _>(0),
            0,
            "poisoned snapshot drop must release the WAL view"
        );
        writer.close().await.unwrap();
    });
}
