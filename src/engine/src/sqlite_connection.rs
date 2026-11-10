//! Run an operation and close its SQLite connection before returning.
use crate::{Error, Result};
use sqlite_access::{
    Connection, SqliteConnection,
    sqlite::{SqliteConnectOptions, SqliteOwnedBuf},
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
