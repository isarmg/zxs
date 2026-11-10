use super::*;
use crate::ErrorCode;
use sqlite_access::Executor;

#[test]
fn operation_can_consume_owned_input_and_borrow_outer_state() {
    let value = String::from("retained value");
    let mut calls = 0;
    let result = run(
        SqliteConnectOptions::new().in_memory(true),
        async |connection| {
            calls += 1;
            connection
                .execute("CREATE TABLE sample(value TEXT)")
                .await?;
            sqlite_access::query("INSERT INTO sample VALUES(?)")
                .bind(value)
                .execute(&mut *connection)
                .await?;
            sqlite_access::query_scalar::<_, String>("SELECT value FROM sample")
                .fetch_one(&mut *connection)
                .await
                .map_err(Error::from)
        },
    )
    .unwrap();
    assert_eq!(result, "retained value");
    assert_eq!(calls, 1);
}

#[test]
fn operation_failure_keeps_its_original_error() {
    let error = run::<()>(
        SqliteConnectOptions::new().in_memory(true),
        async |connection| {
            connection
                .execute("CREATE TABLE sample(value TEXT)")
                .await?;
            Err(Error::new(
                ErrorCode::ResourceLimit,
                "original operation failure",
            ))
        },
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::ResourceLimit);
    assert_eq!(error.message(), "original operation failure");
}

#[test]
fn loaded_image_keeps_its_rows_and_rejects_writes() {
    let bytes = run(
        SqliteConnectOptions::new()
            .in_memory(true)
            .journal_mode(sqlite_access::sqlite::SqliteJournalMode::Memory),
        async |connection| {
            connection
                .execute("CREATE TABLE sample(value INTEGER)")
                .await?;
            connection.execute("INSERT INTO sample VALUES(7)").await?;
            Ok(connection.serialize(None).await?.to_vec())
        },
    )
    .unwrap();
    let database = SqliteOwnedBuf::try_from(bytes.as_slice()).unwrap();
    run(
        SqliteConnectOptions::new().in_memory(true),
        async |connection| {
            load_read_only(connection, database, 1024 * 1024).await?;
            let value: i64 = sqlite_access::query_scalar("SELECT value FROM sample")
                .fetch_one(&mut *connection)
                .await?;
            assert_eq!(value, 7);
            assert!(matches!(
                connection.execute("UPDATE sample SET value=8").await,
                Err(sqlite_access::Error::Database(_))
            ));
            Ok(())
        },
    )
    .unwrap();
}
