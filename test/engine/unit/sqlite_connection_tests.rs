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

#[test]
fn common_materialization_keeps_page_quota_transaction_and_exact_image() {
    const DDL: &str = "CREATE TABLE sample(value INTEGER)";
    for max_bytes in [16 * 1024 * 1024, 32 * 1024 * 1024] {
        let bytes = materialize(
            max_bytes,
            || Error::new(ErrorCode::ResourceLimit, "image budget"),
            async |connection| {
                let pages: i64 = sqlite_access::query_scalar("PRAGMA max_page_count")
                    .fetch_one(&mut *connection)
                    .await?;
                assert_eq!(pages, (max_bytes / 4096) as i64);
                connection.execute(DDL).await?;
                connection.execute("INSERT INTO sample VALUES(7)").await?;
                Ok(())
            },
        )
        .unwrap();
        let original = run(
            SqliteConnectOptions::new()
                .in_memory(true)
                .journal_mode(SqliteJournalMode::Memory),
            async |connection| {
                connection.execute("PRAGMA page_size=4096").await?;
                let pragma = format!("PRAGMA max_page_count={}", max_bytes / 4096);
                connection.execute(pragma.as_str()).await?;
                let mut tx = connection.begin().await?;
                tx.execute(DDL).await?;
                tx.execute("INSERT INTO sample VALUES(7)").await?;
                tx.commit().await?;
                Ok(connection.serialize(None).await?.to_vec())
            },
        )
        .unwrap();
        assert_eq!(bytes, original);
        read_image(&bytes, max_bytes as u32, async |connection| {
            require_schema(
                connection,
                &[("sample", "table", "sample", Some(DDL))],
                "test",
            )
            .await?;
            let value: i64 = sqlite_access::query_scalar("SELECT value FROM sample")
                .fetch_one(&mut *connection)
                .await?;
            assert_eq!(value, 7);
            assert!(connection.execute("DELETE FROM sample").await.is_err());
            Ok(())
        })
        .unwrap();
    }
}

#[test]
fn materialization_and_image_callbacks_preserve_their_original_errors() {
    let error = materialize(
        16 * 1024 * 1024,
        || Error::new(ErrorCode::ResourceLimit, "image budget"),
        async |connection| {
            connection
                .execute("CREATE TABLE sample(value INTEGER)")
                .await?;
            Err(Error::new(ErrorCode::PublicationInvalid, "write operation"))
        },
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::PublicationInvalid);
    assert_eq!(error.message(), "write operation");
    let bytes = materialize(
        16 * 1024 * 1024,
        || Error::new(ErrorCode::ResourceLimit, "image budget"),
        async |connection| {
            connection
                .execute("CREATE TABLE sample(value INTEGER)")
                .await?;
            Ok(())
        },
    )
    .unwrap();
    let error = read_image::<()>(&bytes, 16 * 1024 * 1024, async |_| {
        Err(Error::new(ErrorCode::PublicationInvalid, "read operation"))
    })
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::PublicationInvalid);
    assert_eq!(error.message(), "read operation");
}

#[test]
fn exact_schema_rejects_extra_objects_and_different_ddl() {
    const DDL: &str = "CREATE TABLE sample(value INTEGER)";
    run(
        SqliteConnectOptions::new().in_memory(true),
        async |connection| {
            connection.execute(DDL).await?;
            let expected = [("sample", "table", "sample", Some(DDL))];
            require_schema(connection, &expected, "test").await?;
            connection
                .execute("CREATE VIEW extra AS SELECT * FROM sample")
                .await?;
            let error = require_schema(connection, &expected, "test")
                .await
                .unwrap_err();
            assert_eq!(error.code(), ErrorCode::ContainerLayoutInvalid);
            assert_eq!(error.message(), "test has unexpected schema objects");
            connection.execute("DROP VIEW extra").await?;
            connection.execute("DROP TABLE sample").await?;
            connection
                .execute("CREATE TABLE sample(value TEXT)")
                .await?;
            let error = require_schema(connection, &expected, "test")
                .await
                .unwrap_err();
            assert_eq!(error.code(), ErrorCode::ContainerLayoutInvalid);
            assert_eq!(
                error.message(),
                "test schema differs from the supported format"
            );
            Ok(())
        },
    )
    .unwrap();
}
