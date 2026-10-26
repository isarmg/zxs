use super::*;

#[cfg(feature = "sqlite-pool")]
fn test_identity(schema_sha256: String) -> Result<SchemaIdentity, SchemaIdentityError> {
    SchemaIdentity::new("test-product", "0.3.0", 1, schema_sha256)
}

#[tokio::test]
async fn candidate_reader_is_read_only_bounded_and_never_creates_a_database()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let missing = directory.path().join("missing.sqlite");
    assert!(open_index_read_connection(&missing).await.is_err());
    assert!(!missing.exists());

    let path = directory.path().join("index.sqlite");
    let mut writer = open_index_connection(&path, true, None).await?;
    sqlite_access::query("CREATE TABLE items(id INTEGER PRIMARY KEY)")
        .execute(&mut writer)
        .await?;
    sqlite_access::query("INSERT INTO items VALUES(7)")
        .execute(&mut writer)
        .await?;
    writer.close().await?;

    let mut reader = open_index_read_connection(&path).await?;
    let mode: String = sqlite_access::query_scalar("PRAGMA journal_mode")
        .fetch_one(&mut reader)
        .await?;
    let synchronous: i64 = sqlite_access::query_scalar("PRAGMA synchronous")
        .fetch_one(&mut reader)
        .await?;
    let busy: i64 = sqlite_access::query_scalar("PRAGMA busy_timeout")
        .fetch_one(&mut reader)
        .await?;
    let foreign_keys: i64 = sqlite_access::query_scalar("PRAGMA foreign_keys")
        .fetch_one(&mut reader)
        .await?;
    assert_eq!(mode, "wal");
    assert_eq!(synchronous, 2);
    assert_eq!(busy, 20);
    assert_eq!(foreign_keys, 1);
    assert!(
        sqlite_access::query("DELETE FROM items")
            .execute(&mut reader)
            .await
            .is_err()
    );
    assert!(
        sqlite_access::query_scalar::<_, Vec<u8>>("SELECT zeroblob(65537)")
            .fetch_one(&mut reader)
            .await
            .is_err()
    );
    let retained: i64 = sqlite_access::query_scalar("SELECT id FROM items")
        .fetch_one(&mut reader)
        .await?;
    assert_eq!(retained, 7);
    reader.close().await?;
    Ok(())
}

#[tokio::test]
#[cfg(feature = "sqlite-pool")]
async fn explicit_creation_applies_pragmas_to_every_connection_and_reopens()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let database_path = directory.path().join("zxs.sqlite3");
    let options = PoolOptions::new(2).with_min_connections(2);
    let pool = create_if_missing(&database_path, options.clone()).await?;

    let mut first = pool.acquire().await?;
    let mut second = pool.acquire().await?;
    for connection in [&mut first, &mut second] {
        let foreign_keys: i64 = sqlite_access::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&mut **connection)
            .await?;
        let journal_mode: String = sqlite_access::query_scalar("PRAGMA journal_mode")
            .fetch_one(&mut **connection)
            .await?;
        let busy_timeout: i64 = sqlite_access::query_scalar("PRAGMA busy_timeout")
            .fetch_one(&mut **connection)
            .await?;
        let synchronous: i64 = sqlite_access::query_scalar("PRAGMA synchronous")
            .fetch_one(&mut **connection)
            .await?;
        assert_eq!(foreign_keys, 1);
        assert_eq!(journal_mode, "wal");
        assert_eq!(busy_timeout, 5_000);
        assert_eq!(synchronous, 2);
    }
    drop(first);
    drop(second);

    sqlite_access::query("CREATE TABLE parents(id INTEGER PRIMARY KEY)")
        .execute(&pool)
        .await?;
    sqlite_access::query(
        "CREATE TABLE children(\
             id INTEGER PRIMARY KEY,\
             parent_id INTEGER NOT NULL REFERENCES parents(id)\
         )",
    )
    .execute(&pool)
    .await?;
    assert!(
        sqlite_access::query("INSERT INTO children(id,parent_id) VALUES(1,999)")
            .execute(&pool)
            .await
            .is_err()
    );
    sqlite_access::query("INSERT INTO parents(id) VALUES(7)")
        .execute(&pool)
        .await?;
    integrity_check(&pool).await?;
    foreign_key_check(&pool).await?;
    checkpoint(&pool).await?;
    pool.close().await;

    let reopened = open_existing(&database_path, options).await?;
    let parent: i64 = sqlite_access::query_scalar("SELECT id FROM parents")
        .fetch_one(&reopened)
        .await?;
    assert_eq!(parent, 7);
    integrity_check(&reopened).await?;
    foreign_key_check(&reopened).await?;
    Ok(())
}

#[tokio::test]
#[cfg(feature = "sqlite-pool")]
async fn missing_existing_and_invalid_options_are_typed_and_do_not_create()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let missing = directory.path().join("missing.sqlite3");
    assert!(matches!(
        open_existing(&missing, PoolOptions::default()).await,
        Err(Error::Sqlx(_))
    ));
    assert!(!missing.exists());
    assert!(matches!(
        create_if_missing(&missing, PoolOptions::new(0)).await,
        Err(Error::InvalidPoolOptions(
            PoolOptionsError::ZeroMaxConnections
        ))
    ));
    assert!(!missing.exists());
    assert!(matches!(
        PoolOptions::new(1).with_min_connections(2).validate(),
        Err(PoolOptionsError::MinConnectionsExceedMax { .. })
    ));
    Ok(())
}

#[tokio::test]
#[cfg(feature = "sqlite-pool")]
async fn sqlx_adapter_verifies_metadata_shape_values_and_fingerprint()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let database_path = directory.path().join("identity.sqlite3");
    let pool = create_if_missing(&database_path, PoolOptions::new(2)).await?;
    sqlite_access::raw_sql(PRODUCT_METADATA_DDL)
        .execute(&pool)
        .await?;
    sqlite_access::query("CREATE TABLE items(id INTEGER PRIMARY KEY, value TEXT NOT NULL)")
        .execute(&pool)
        .await?;
    let fingerprint = schema_fingerprint(&pool).await?;
    let expected = test_identity(fingerprint.clone())?;
    sqlite_access::query(
        "INSERT INTO product_metadata(\
           singleton, application, application_version, schema_revision, schema_sha256\
         ) VALUES(1, ?, ?, ?, ?)",
    )
    .bind(&expected.application)
    .bind(&expected.application_version)
    .bind(i64::try_from(expected.schema_revision)?)
    .bind(&expected.schema_sha256)
    .execute(&pool)
    .await?;

    assert_eq!(read_pool_schema_identity(&pool).await?, expected);
    assert_eq!(
        require_pool_current_schema(&pool, &expected).await?,
        expected
    );

    sqlite_access::query("UPDATE product_metadata SET application_version='0.2.0'")
        .execute(&pool)
        .await?;
    assert!(matches!(
        require_pool_current_schema(&pool, &expected).await,
        Err(Error::SchemaIdentity(
            SchemaIdentityError::IdentityMismatch {
                field: IdentityField::ApplicationVersion,
                ..
            }
        ))
    ));
    sqlite_access::query("UPDATE product_metadata SET application_version='0.3.0'")
        .execute(&pool)
        .await?;
    sqlite_access::query("CREATE TABLE drift(id INTEGER PRIMARY KEY)")
        .execute(&pool)
        .await?;
    assert!(matches!(
        read_pool_schema_identity(&pool).await,
        Err(Error::SchemaIdentity(
            SchemaIdentityError::SchemaFingerprintMismatch { .. }
        ))
    ));
    Ok(())
}

#[tokio::test]
#[cfg(feature = "sqlite-pool")]
async fn foreign_key_check_reports_the_violating_rows() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let database_path = directory.path().join("foreign-keys.sqlite3");
    let pool = create_if_missing(&database_path, PoolOptions::new(1)).await?;
    sqlite_access::raw_sql(
        "CREATE TABLE parents(id INTEGER PRIMARY KEY);\
         CREATE TABLE children(\
           id INTEGER PRIMARY KEY,\
           parent_id INTEGER NOT NULL REFERENCES parents(id)\
         );",
    )
    .execute(&pool)
    .await?;
    let mut connection = pool.acquire().await?;
    sqlite_access::query("PRAGMA foreign_keys=OFF")
        .execute(&mut *connection)
        .await?;
    sqlite_access::query("INSERT INTO children(id,parent_id) VALUES(9,99)")
        .execute(&mut *connection)
        .await?;
    sqlite_access::query("PRAGMA foreign_keys=ON")
        .execute(&mut *connection)
        .await?;
    drop(connection);

    assert!(matches!(
        foreign_key_check(&pool).await,
        Err(Error::ForeignKeyViolations { violations })
            if violations == vec![ForeignKeyViolation {
                table: "children".to_owned(),
                row_id: Some(9),
                parent: "parents".to_owned(),
                foreign_key_index: 0,
            }]
    ));
    Ok(())
}

#[tokio::test]
async fn metadata_excess_is_detected_with_only_two_rows_materialized()
-> Result<(), Box<dyn std::error::Error>> {
    use sqlite_access::Connection;
    let mut connection = SqliteConnection::connect("sqlite::memory:").await?;
    sqlite_access::raw_sql(PRODUCT_METADATA_DDL)
        .execute(&mut connection)
        .await?;
    sqlite_access::query("PRAGMA ignore_check_constraints=ON")
        .execute(&mut connection)
        .await?;
    sqlite_access::query(
        "WITH RECURSIVE ids(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM ids WHERE n<10000) \
         INSERT INTO product_metadata \
         SELECT n, 'test-product', '0.3.0', 1, ? FROM ids",
    )
    .bind("0".repeat(64))
    .execute(&mut connection)
    .await?;
    let rows = read_metadata_rows(&mut connection).await?;
    assert_eq!(rows.len(), 2);
    assert!(schema_identity_from_metadata_rows(&rows).is_err());
    assert!(matches!(
        read_schema_identity(&mut connection).await,
        Err(Error::SchemaIdentity(
            SchemaIdentityError::ProductMetadataRowCount { actual: 2 }
        ))
    ));
    connection.close().await?;
    Ok(())
}

#[tokio::test]
#[cfg(feature = "sqlite-pool")]
async fn pool_limits_apply_to_replacement_connections_and_invalid_creation_is_readonly()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let missing = directory.path().join("invalid.sqlite3");
    assert!(matches!(
        create_if_missing(
            &missing,
            PoolOptions::new(1).with_connection_limits(ConnectionLimits::new(0))
        )
        .await,
        Err(Error::InvalidPoolOptions(
            PoolOptionsError::InvalidConnectionLimits(_)
        ))
    ));
    assert!(!missing.exists());
    assert_eq!(std::fs::read_dir(directory.path())?.count(), 0);

    let path = directory.path().join("limited.sqlite3");
    let pool = create_if_missing(
        &path,
        PoolOptions::new(1).with_connection_limits(ConnectionLimits::new(64 * 1024)),
    )
    .await?;
    sqlite_access::query("CREATE TABLE values_(value BLOB)")
        .execute(&pool)
        .await?;
    for _ in 0..2 {
        let mut connection = pool.acquire().await?;
        let error = sqlite_access::query("INSERT INTO values_ VALUES(zeroblob(131072))")
            .execute(&mut *connection)
            .await
            .unwrap_err();
        assert!(
            matches!(error, sqlite_access::Error::Database(ref error) if error.code().as_deref() == Some("18"))
        );
        let healthy: i64 = sqlite_access::query_scalar("SELECT 7")
            .fetch_one(&mut *connection)
            .await?;
        assert_eq!(healthy, 7);
        connection.close().await?;
    }
    let count: i64 = sqlite_access::query_scalar("SELECT COUNT(*) FROM values_")
        .fetch_one(&pool)
        .await?;
    assert_eq!(count, 0);
    pool.close().await;
    Ok(())
}

#[tokio::test]
async fn schema_budget_rejects_complete_excess_and_preserves_canonical_bytes()
-> Result<(), Box<dyn std::error::Error>> {
    use sqlite_access::Connection;
    let mut connection = SqliteConnection::connect("sqlite::memory:").await?;
    sqlite_access::raw_sql("CREATE TABLE α(id INTEGER); CREATE INDEX α_id ON α(id);")
        .execute(&mut connection)
        .await?;
    let canonical = sqlite_access::query(crate::schema_identity::SQLITE_SCHEMA_ROWS_QUERY)
        .fetch_all(&mut connection)
        .await?
        .into_iter()
        .map(|row| {
            Ok(SchemaRow::new(
                row.try_get::<String, _>(0)?,
                row.try_get::<String, _>(1)?,
                row.try_get::<String, _>(2)?,
                row.try_get::<String, _>(3)?,
            ))
        })
        .collect::<Result<Vec<_>, sqlite_access::Error>>()?;
    assert_eq!(schema_rows(&mut connection).await?, canonical);
    assert_eq!(
        schema_fingerprint(&mut connection).await?,
        fingerprint_rows(&canonical)?
    );

    // This SQL is generated only from test-owned indices, never input data.
    let ddl = (0..MAX_SCHEMA_OBJECTS - 1)
        .map(|index| format!("CREATE TABLE owned_{index}(id INTEGER);"))
        .collect::<String>();
    sqlite_access::raw_sql(sqlite_access::AssertSqlSafe(ddl.as_str()))
        .execute(&mut connection)
        .await?;
    assert!(matches!(
        schema_rows(&mut connection).await,
        Err(Error::SchemaBudgetExceeded)
    ));
    assert!(matches!(
        schema_fingerprint(&mut connection).await,
        Err(Error::SchemaBudgetExceeded)
    ));
    connection.close().await?;

    let mut connection = SqliteConnection::connect("sqlite::memory:").await?;
    // A stored DDL comment is actual schema text, not merely a large bind.
    let ddl = format!(
        "CREATE TABLE oversized(id INTEGER /*{}*/)",
        "é".repeat(MAX_SCHEMA_BYTES / 2)
    );
    sqlite_access::raw_sql(sqlite_access::AssertSqlSafe(ddl.as_str()))
        .execute(&mut connection)
        .await?;
    let stored_bytes: i64 = sqlite_access::query_scalar(
        "SELECT length(CAST(sql AS BLOB)) FROM sqlite_schema WHERE name='oversized'",
    )
    .fetch_one(&mut connection)
    .await?;
    assert!(stored_bytes > MAX_SCHEMA_BYTES as i64);
    assert!(matches!(
        schema_rows(&mut connection).await,
        Err(Error::SchemaBudgetExceeded)
    ));
    assert!(matches!(
        schema_fingerprint(&mut connection).await,
        Err(Error::SchemaBudgetExceeded)
    ));
    connection.close().await?;
    Ok(())
}

#[tokio::test]
#[cfg(feature = "sqlite-pool")]
async fn foreign_key_evidence_stays_bounded_for_many_violations()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let pool = create_if_missing(
        directory.path().join("many-fk.sqlite3"),
        PoolOptions::new(1),
    )
    .await?;
    sqlite_access::raw_sql("CREATE TABLE parents(id INTEGER PRIMARY KEY); CREATE TABLE children(id INTEGER PRIMARY KEY, parent_id INTEGER REFERENCES parents(id));")
        .execute(&pool).await?;
    let mut connection = pool.acquire().await?;
    sqlite_access::query("PRAGMA foreign_keys=OFF")
        .execute(&mut *connection)
        .await?;
    sqlite_access::query("WITH RECURSIVE ids(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM ids WHERE n<10000) INSERT INTO children SELECT n, n FROM ids")
        .execute(&mut *connection).await?;
    sqlite_access::query("PRAGMA foreign_keys=ON")
        .execute(&mut *connection)
        .await?;
    drop(connection);
    let Err(Error::ForeignKeyViolations { violations }) = foreign_key_check(&pool).await else {
        panic!("all violating data must fail the foreign-key check");
    };
    assert_eq!(violations.len(), MAX_FOREIGN_KEY_EVIDENCE);
    assert!(
        violations
            .iter()
            .all(|row| row.table == "children" && row.parent == "parents")
    );
    pool.close().await;
    Ok(())
}

#[test]
#[cfg(feature = "sqlite-pool")]
fn checkpoint_result_distinguishes_busy_and_incomplete() {
    assert!(matches!(
        pool::validate_checkpoint_result((1, 8, 3)),
        Err(Error::CheckpointBusy {
            log_frames: 8,
            checkpointed_frames: 3
        })
    ));
    assert!(matches!(
        pool::validate_checkpoint_result((0, 8, 3)),
        Err(Error::CheckpointIncomplete {
            log_frames: 8,
            checkpointed_frames: 3
        })
    ));
    assert!(pool::validate_checkpoint_result((0, 8, 8)).is_ok());
}

#[tokio::test]
#[cfg(feature = "sqlite-pool")]
async fn live_reader_makes_truncate_checkpoint_report_busy()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let database_path = directory.path().join("busy.sqlite3");
    let pool = create_if_missing(&database_path, PoolOptions::new(3)).await?;
    sqlite_access::query("CREATE TABLE values_table(value INTEGER NOT NULL)")
        .execute(&pool)
        .await?;
    sqlite_access::query("INSERT INTO values_table(value) VALUES(1)")
        .execute(&pool)
        .await?;
    checkpoint(&pool).await?;

    let mut reader = pool.acquire().await?;
    sqlite_access::query("BEGIN").execute(&mut *reader).await?;
    let _: i64 = sqlite_access::query_scalar("SELECT COUNT(*) FROM values_table")
        .fetch_one(&mut *reader)
        .await?;
    sqlite_access::query("INSERT INTO values_table(value) VALUES(2)")
        .execute(&pool)
        .await?;

    assert!(matches!(
        checkpoint(&pool).await,
        Err(Error::CheckpointBusy { .. })
    ));
    sqlite_access::query("ROLLBACK")
        .execute(&mut *reader)
        .await?;
    checkpoint(&pool).await?;
    Ok(())
}
