use super::*;
use std::os::unix::fs::symlink;

#[tokio::test]
async fn explicit_private_run_directory_keeps_limits_and_cleans_temporary_files() {
    let root = tempfile::tempdir().unwrap();
    let run = root.path().join("run");
    let held = crate::fs_safety::PrivateDirectory::create(&run).unwrap();
    let source = root.path().join("source.sqlite3");
    let pool = crate::sqlite::create_if_missing(&source, crate::sqlite::PoolOptions::new(1))
        .await
        .unwrap();
    sqlite_access::raw_sql("CREATE TABLE records(value INTEGER);INSERT INTO records VALUES(7)")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    let before = generation_bytes(&source);
    assert!(matches!(
        ValidationSnapshot::capture_with_limits_in(
            &source,
            SnapshotLimits {
                max_total_bytes: 1,
                ..SnapshotLimits::default()
            },
            &run
        ),
        Err(SnapshotError::BudgetExceeded)
    ));
    let snapshot = ValidationSnapshot::capture_in(&source, &run).unwrap();
    assert!(snapshot.database_path().starts_with(&run));
    let snapshot = snapshot.into_pool().await.unwrap();
    assert_eq!(
        sqlite_access::query_scalar::<_, i64>("SELECT value FROM records")
            .fetch_one(snapshot.pool())
            .await
            .unwrap(),
        7
    );
    snapshot.close().await;
    assert_eq!(before, generation_bytes(&source));
    assert_eq!(fs::read_dir(&run).unwrap().count(), 0);
    fs::rename(&run, root.path().join("old-run")).unwrap();
    crate::fs_safety::PrivateDirectory::create(&run).unwrap();
    assert!(held.verify_identity().is_err());
}

#[tokio::test]
async fn connection_limits_survive_pool_replacement_without_touching_source() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.sqlite3");
    let original = crate::sqlite::create_if_missing(&source, crate::sqlite::PoolOptions::new(1))
        .await
        .unwrap();
    sqlite_access::raw_sql(
        "CREATE TABLE data(value BLOB);INSERT INTO data VALUES(zeroblob(131072))",
    )
    .execute(&original)
    .await
    .unwrap();
    original.close().await;
    let before = generation_bytes(&source);
    assert!(matches!(
        open_validation_snapshot_with_connection_limits(
            &source,
            crate::sqlite::ConnectionLimits::new(1)
        )
        .await,
        Err(SnapshotError::ConnectionLimits(
            crate::sqlite::ConnectionLimitError::InvalidLimits
        ))
    ));
    let snapshot = open_validation_snapshot_with_connection_limits(
        &source,
        crate::sqlite::ConnectionLimits::new(64 * 1024),
    )
    .await
    .unwrap();
    for _ in 0..2 {
        let mut connection = snapshot.pool().acquire().await.unwrap();
        let error = sqlite_access::query_scalar::<_, Vec<u8>>("SELECT value FROM data")
            .fetch_one(&mut *connection)
            .await
            .unwrap_err();
        assert_eq!(
            error
                .as_database_error()
                .and_then(|error| error.code())
                .as_deref(),
            Some("18")
        );
        assert_eq!(
            sqlite_access::query_scalar::<_, i64>("SELECT 7")
                .fetch_one(&mut *connection)
                .await
                .unwrap(),
            7
        );
        // Force a new native worker instead of returning this connection.
        connection.close().await.unwrap();
    }
    snapshot.close().await;
    assert_eq!(generation_bytes(&source), before);
}

fn generation_bytes(path: &Path) -> Vec<Option<Vec<u8>>> {
    [
        path.to_path_buf(),
        sidecar(path, "-wal"),
        sidecar(path, "-journal"),
        sidecar(path, "-shm"),
    ]
    .into_iter()
    .map(|path| fs::read(path).ok())
    .collect()
}

#[tokio::test]
async fn an_existing_capture_opens_the_same_generation_after_source_disappears() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("database.sqlite3");
    let pool = crate::sqlite::create_if_missing(&source, crate::sqlite::PoolOptions::new(1))
        .await
        .unwrap();
    sqlite_access::query("CREATE TABLE records (value TEXT NOT NULL)")
        .execute(&pool)
        .await
        .unwrap();
    sqlite_access::query("INSERT INTO records VALUES ('captured-generation')")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    let snapshot = ValidationSnapshot::capture(&source).unwrap();
    fs::rename(&source, directory.path().join("original.sqlite3")).unwrap();
    let pool = snapshot.into_pool().await.unwrap();
    let value: String = sqlite_access::query_scalar("SELECT value FROM records")
        .fetch_one(pool.pool())
        .await
        .unwrap();
    assert_eq!(value, "captured-generation");
    assert!(!source.exists());
    assert!(
        sqlite_access::query("DELETE FROM records")
            .execute(pool.pool())
            .await
            .is_err()
    );
    pool.close().await;
}

#[tokio::test]
async fn reads_wal_only_commits_and_preserves_every_source_file() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("database.sqlite3");
    let pool = crate::sqlite::create_if_missing(&source, crate::sqlite::PoolOptions::new(1))
        .await
        .unwrap();
    sqlite_access::query("CREATE TABLE records (value TEXT NOT NULL)")
        .execute(&pool)
        .await
        .unwrap();
    sqlite_access::query("INSERT INTO records VALUES ('wal-only-commit')")
        .execute(&pool)
        .await
        .unwrap();
    assert!(sidecar(&source, "-wal").exists());
    let before = generation_bytes(&source);
    let snapshot = open_validation_snapshot(&source).await.unwrap();
    let value: String = sqlite_access::query_scalar("SELECT value FROM records")
        .fetch_one(snapshot.pool())
        .await
        .unwrap();
    assert_eq!(value, "wal-only-commit");
    assert!(
        sqlite_access::query("INSERT INTO records VALUES ('forbidden')")
            .execute(snapshot.pool())
            .await
            .is_err()
    );
    assert_eq!(generation_bytes(&source), before);
    let retained = snapshot.pool().clone();
    drop(snapshot);
    let value: String = sqlite_access::query_scalar("SELECT value FROM records")
        .fetch_one(&retained)
        .await
        .unwrap();
    assert_eq!(value, "wal-only-commit");
    retained.close().await;
    pool.close().await;
}

#[tokio::test]
async fn active_writer_is_busy_instead_of_an_unlocked_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("database.sqlite3");
    let pool = crate::sqlite::create_if_missing(&source, crate::sqlite::PoolOptions::new(1))
        .await
        .unwrap();
    sqlite_access::query("CREATE TABLE records (value INTEGER)")
        .execute(&pool)
        .await
        .unwrap();
    let mut writer = pool.acquire().await.unwrap();
    sqlite_access::query("BEGIN IMMEDIATE")
        .execute(&mut *writer)
        .await
        .unwrap();
    sqlite_access::query("INSERT INTO records VALUES (1)")
        .execute(&mut *writer)
        .await
        .unwrap();
    assert!(matches!(
        ValidationSnapshot::capture(&source),
        Err(SnapshotError::Busy)
    ));
    sqlite_access::query("ROLLBACK")
        .execute(&mut *writer)
        .await
        .unwrap();
    drop(writer);
    pool.close().await;
}

#[test]
fn unsafe_missing_oversized_and_invalid_sources_fail_without_source_changes() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    assert!(matches!(
        ValidationSnapshot::capture(&source),
        Err(SnapshotError::MissingDatabase)
    ));
    assert!(matches!(
        ValidationSnapshot::capture("relative.sqlite3"),
        Err(SnapshotError::UnsafeSource)
    ));
    fs::write(&source, b"not a sqlite database").unwrap();
    let limits = SnapshotLimits {
        max_total_bytes: 1,
        ..SnapshotLimits::default()
    };
    assert!(matches!(
        ValidationSnapshot::capture_with_limits(&source, limits),
        Err(SnapshotError::BudgetExceeded)
    ));
    let link = directory.path().join("link");
    symlink(&source, &link).unwrap();
    assert!(matches!(
        ValidationSnapshot::capture(&link),
        Err(SnapshotError::UnsafeSource)
    ));
    fs::hard_link(&source, directory.path().join("alias")).unwrap();
    assert!(matches!(
        ValidationSnapshot::capture(&source),
        Err(SnapshotError::UnsafeSource)
    ));
    assert_eq!(fs::read(source).unwrap(), b"not a sqlite database");
}

#[test]
fn snapshot_errors_preserve_io_and_capture_task_sources() {
    let error = SnapshotError::LockUnavailable(io::Error::from_raw_os_error(libc::ENOSYS));
    let source = std::error::Error::source(&error).unwrap();
    assert_eq!(
        source.downcast_ref::<io::Error>().unwrap().raw_os_error(),
        Some(libc::ENOSYS)
    );
    let error = SnapshotError::Filesystem(crate::fs_safety::Error::Io(
        io::Error::from_raw_os_error(libc::EACCES),
    ));
    assert!(
        std::error::Error::source(&error)
            .unwrap()
            .downcast_ref::<crate::fs_safety::Error>()
            .is_some()
    );
}

#[tokio::test]
async fn capture_task_failure_preserves_join_error() {
    let source = tokio::task::spawn_blocking(|| panic!("capture task failure"))
        .await
        .unwrap_err();
    let error = SnapshotError::CaptureTaskFailed(source);
    assert!(
        std::error::Error::source(&error)
            .unwrap()
            .downcast_ref::<tokio::task::JoinError>()
            .unwrap()
            .is_panic()
    );
}
