use super::*;
use sqlite_access::Connection;

#[test]
fn huge_cells_are_rejected_without_affecting_another_connection()
-> Result<(), Box<dyn std::error::Error>> {
    crate::sqlite::block_on_sqlite_connection(async {
        let mut first = SqliteConnection::connect("sqlite::memory:").await?;
        let mut second = SqliteConnection::connect("sqlite::memory:").await?;
        sqlite_access::raw_sql(
            "CREATE TABLE data(value BLOB);INSERT INTO data VALUES(zeroblob(2097152))",
        )
        .execute(&mut first)
        .await?;
        let requested = ConnectionLimits::new(64 * 1024);
        assert_eq!(
            apply_connection_limits(&mut first, requested).await?,
            requested
        );
        let error = sqlite_access::query_scalar::<_, Vec<u8>>("SELECT value FROM data")
            .fetch_one(&mut first)
            .await
            .unwrap_err();
        assert_eq!(
            error
                .as_database_error()
                .and_then(|error| error.code())
                .as_deref(),
            Some("18")
        );
        assert!(
            sqlite_access::query("INSERT INTO data VALUES(zeroblob(2097152))")
                .execute(&mut first)
                .await
                .is_err()
        );
        assert_eq!(
            sqlite_access::query_scalar::<_, i64>("SELECT 7")
                .fetch_one(&mut first)
                .await?,
            7
        );
        assert_eq!(
            sqlite_access::query_scalar::<_, i64>("SELECT length(zeroblob(2097152))")
                .fetch_one(&mut second)
                .await?,
            2097152
        );
        first.close().await?;
        second.close().await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })
}

#[test]
fn sql_and_parameter_limits_are_native_and_invalid_input_changes_nothing()
-> Result<(), Box<dyn std::error::Error>> {
    crate::sqlite::block_on_sqlite_connection(async {
        let mut connection = SqliteConnection::connect("sqlite::memory:").await?;
        let limits = ConnectionLimits::new(1024 * 1024)
            .with_max_sql_bytes(1024)
            .with_max_vm_operations(10_000)
            .with_max_bind_parameters(8);
        assert_eq!(
            apply_connection_limits(&mut connection, limits).await?,
            limits
        );
        assert!(matches!(
            apply_connection_limits(&mut connection, ConnectionLimits::new(1)).await,
            Err(ConnectionLimitError::InvalidLimits)
        ));
        {
            let mut handle = connection.lock_handle().await?;
            assert_eq!(
                native_limit(&mut handle, LimitCategory::Value, -1),
                1024 * 1024
            );
            assert_eq!(native_limit(&mut handle, LimitCategory::Sql, -1), 1024);
            assert_eq!(native_limit(&mut handle, LimitCategory::Vm, -1), 10_000);
            assert_eq!(native_limit(&mut handle, LimitCategory::Parameters, -1), 8);
        }
        // Test-owned fixed comment bytes, with no external SQL inputs.
        assert!(
            sqlite_access::query(sqlite_access::AssertSqlSafe(format!(
                "SELECT 1 /*{}*/",
                "x".repeat(1024)
            )))
            .execute(&mut connection)
            .await
            .is_err()
        );
        assert!(
            sqlite_access::query("SELECT ?9")
                .execute(&mut connection)
                .await
                .is_err()
        );
        connection.close().await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })
}
