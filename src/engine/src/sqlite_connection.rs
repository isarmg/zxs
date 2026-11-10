//! Run an operation and close its SQLite connection before returning.
use crate::{Error, ErrorCode, Result};
use sqlite_access::{
    Connection, Row, SqliteConnection,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteOwnedBuf},
};

pub(crate) fn run<T>(
    options: SqliteConnectOptions,
    operation: impl std::ops::AsyncFnOnce(&mut SqliteConnection) -> Result<T>,
) -> Result<T> {
    platform_io::sqlite::block_on_sqlite_connection(async {
        let mut connection = SqliteConnection::connect_with(&options).await?;
        let result = operation(&mut connection).await;
        match (result, connection.close().await) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), Ok(())) => Err(error),
            (Ok(_), Err(error)) => Err(error.into()),
            (Err(error), Err(close)) => Err(Error::Cleanup {
                operation: Box::new(error),
                cleanup: std::io::Error::other(close),
            }),
        }
    })
}

/// The caller has bounded the immutable image before allocating its SQLite copy.
pub(crate) fn read_image<T>(
    bytes: &[u8],
    max_bytes: u32,
    read: impl std::ops::AsyncFnOnce(&mut SqliteConnection) -> Result<T>,
) -> Result<T> {
    let database = SqliteOwnedBuf::try_from(bytes)?;
    run(
        SqliteConnectOptions::new().in_memory(true),
        async |connection| {
            load_read_only(connection, database, max_bytes).await?;
            read(connection).await
        },
    )
}

pub(crate) fn materialize(
    max_bytes: usize,
    budget: impl Fn() -> Error,
    write: impl std::ops::AsyncFnOnce(&mut SqliteConnection) -> Result<()>,
) -> Result<Vec<u8>> {
    run(
        SqliteConnectOptions::new()
            .in_memory(true)
            .journal_mode(SqliteJournalMode::Memory)
            .page_size(4096)
            .pragma("max_page_count", (max_bytes / 4096).to_string()),
        async move |connection| {
            let mut tx = connection.begin().await?;
            write(&mut tx).await?;
            tx.commit().await?;
            let bytes = connection.serialize(None).await?;
            if bytes.len() > max_bytes {
                return Err(budget());
            }
            Ok(bytes.to_vec())
        },
    )
}

type SchemaEntry = (
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
);

pub(crate) async fn require_schema(
    connection: &mut SqliteConnection,
    expected: &[SchemaEntry],
    label: &str,
) -> Result<()> {
    // BLOB prefixes bound unknown schema strings without hiding trailing NULs.
    let schema = sqlite_access::query(
        "SELECT substr(CAST(name AS BLOB),1,257),substr(CAST(type AS BLOB),1,257),substr(CAST(tbl_name AS BLOB),1,257),substr(CAST(sql AS BLOB),1,4097) FROM sqlite_schema ORDER BY name LIMIT ?",
    )
    .bind((expected.len() + 1) as i64)
    .fetch_all(&mut *connection)
    .await?;
    if schema.len() != expected.len() {
        return Err(schema_error(&format!(
            "{label} has unexpected schema objects"
        )));
    }
    for (row, &(name, kind, table, sql)) in schema.into_iter().zip(expected) {
        if row.try_get::<Vec<u8>, _>(0)? != name.as_bytes()
            || row.try_get::<Vec<u8>, _>(1)? != kind.as_bytes()
            || row.try_get::<Vec<u8>, _>(2)? != table.as_bytes()
            || row.try_get::<Option<Vec<u8>>, _>(3)?.as_deref() != sql.map(str::as_bytes)
        {
            return Err(schema_error(&format!(
                "{label} schema differs from the supported format"
            )));
        }
    }
    Ok(())
}

fn schema_error(message: &str) -> Error {
    Error::new(ErrorCode::ContainerLayoutInvalid, message)
}

/// Apply the metadata reader limits before loading its immutable image.
pub(crate) async fn load_read_only(
    connection: &mut SqliteConnection,
    database: SqliteOwnedBuf,
    max_bytes: u32,
) -> Result<()> {
    platform_io::sqlite::apply_connection_limits(
        connection,
        platform_io::sqlite::ConnectionLimits::new(max_bytes)
            .with_max_sql_bytes(4096)
            .with_max_vm_operations(100_000)
            .with_max_bind_parameters(16),
    )
    .await
    .map_err(|error| Error::from(std::io::Error::other(error)))?;
    platform_io::sqlite::enable_defensive(connection)
        .await
        .map_err(|error| Error::from(std::io::Error::other(error)))?;
    connection.deserialize(None, database, true).await?;
    Ok(())
}

#[cfg(test)]
#[path = "../../../test/engine/unit/sqlite_connection_tests.rs"]
mod tests;
