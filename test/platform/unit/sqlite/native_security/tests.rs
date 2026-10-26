use super::*;
use sqlite_access::Connection;

#[tokio::test]
async fn defensive_mode_rejects_schema_writes_and_keeps_normal_sql_working()
-> Result<(), Box<dyn std::error::Error>> {
    let mut connection = SqliteConnection::connect("sqlite::memory:").await?;
    sqlite_access::query("CREATE TABLE items(value INTEGER)")
        .execute(&mut connection)
        .await?;
    let original = crate::sqlite::schema_fingerprint(&mut connection).await?;
    enable_defensive(&mut connection).await?;
    enable_defensive(&mut connection).await?;
    sqlite_access::query("PRAGMA writable_schema=ON")
        .execute(&mut connection)
        .await?;
    let result = sqlite_access::query("UPDATE sqlite_schema SET sql=NULL WHERE name='items'")
        .execute(&mut connection)
        .await;
    assert!(
        matches!(result, Err(sqlite_access::Error::Database(ref error)) if error.code().as_deref() == Some("1"))
    );
    assert_eq!(
        crate::sqlite::schema_fingerprint(&mut connection).await?,
        original
    );
    sqlite_access::query("INSERT INTO items VALUES(7)")
        .execute(&mut connection)
        .await?;
    let value: i64 = sqlite_access::query_scalar("SELECT value FROM items")
        .fetch_one(&mut connection)
        .await?;
    assert_eq!(value, 7);
    connection.close().await?;

    // The setting is local: prove the same native schema write is possible
    // without defensive mode, rather than passing because of a bad query.
    let mut other = SqliteConnection::connect("sqlite::memory:").await?;
    sqlite_access::query("CREATE TABLE items(value INTEGER)")
        .execute(&mut other)
        .await?;
    sqlite_access::query("PRAGMA writable_schema=ON")
        .execute(&mut other)
        .await?;
    let changed = sqlite_access::query("UPDATE sqlite_schema SET sql=NULL WHERE name='items'")
        .execute(&mut other)
        .await?;
    assert_eq!(changed.rows_affected(), 1);
    other.close().await?;
    Ok(())
}

#[tokio::test]
async fn no_checkpoint_on_close_retains_wal_and_releases_native_connection()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("index.sqlite");
    let mut connection = crate::sqlite::open_index_connection(&path, true, None).await?;
    disable_checkpoint_on_close(&mut connection).await?;
    disable_checkpoint_on_close(&mut connection).await?;
    sqlite_access::query("CREATE TABLE items(value INTEGER);INSERT INTO items VALUES(7)")
        .execute(&mut connection)
        .await?;
    let wal = path.with_file_name("index.sqlite-wal");
    let shm = path.with_file_name("index.sqlite-shm");
    let committed = std::fs::read(&wal)?;
    connection.close().await?;
    assert_eq!(std::fs::read(&wal)?, committed);
    assert!(shm.is_file());
    // A new default-policy consumer can recover the committed WAL and acquire
    // an exclusive journal-mode transition: the first native worker is closed,
    // not deliberately leaked to avoid its final checkpoint.
    let mut reopened = crate::sqlite::open_index_connection(&path, false, None).await?;
    let value: i64 = sqlite_access::query_scalar("SELECT value FROM items")
        .fetch_one(&mut reopened)
        .await?;
    assert_eq!(value, 7);
    sqlite_access::query("PRAGMA journal_mode=DELETE")
        .execute(&mut reopened)
        .await?;
    reopened.close().await?;
    assert!(!wal.exists());
    assert!(!shm.exists());
    Ok(())
}
