//! Bounded evaluation of product-owned trusted DDL for build fingerprints.

use crate::schema_identity::schema_fingerprint;
use error_definition::Error;
use sqlite_access::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use std::time::{Duration, Instant};

const MAX_FRAGMENTS: usize = 128;
const MAX_DDL_BYTES: usize = 1024 * 1024;
use crate::sqlite::MAX_SCHEMA_BYTES;
const DEADLINE: Duration = Duration::from_secs(3);

#[derive(Debug, Error)]
pub enum DdlFingerprintError {
    #[error("trusted DDL input exceeds the supported bounds")]
    InputLimit,
    #[error("trusted DDL native connection limits failed")]
    ConnectionLimits(#[source] crate::sqlite::ConnectionLimitError),
    #[error("trusted DDL schema read failed")]
    SchemaRead(#[source] crate::sqlite::Error),
    #[error("trusted DDL schema result exceeds the supported bounds")]
    ResultLimit,
    #[error("trusted DDL evaluation exceeded its deadline")]
    Deadline,
    #[error("trusted DDL SQLite evaluation failed")]
    Sqlite(#[source] sqlite_access::Error),
    #[error("trusted DDL schema fingerprint failed")]
    Schema(#[source] crate::schema_identity::Error),
    #[error("trusted DDL SQLite connection could not be closed")]
    Close(#[source] sqlite_access::Error),
    #[error("{operation}; SQLite connection close also failed: {close}")]
    EvaluationAndClose {
        operation: Box<DdlFingerprintError>,
        #[source]
        close: sqlite_access::Error,
    },
}

/// Compute a schema fingerprint from owned, trusted product DDL synchronously.
///
/// This is a build-time adapter, not an arbitrary-SQL sandbox. Inputs must be
/// trusted source constants: SQL can access external paths (for example through
/// ATTACH), even though the primary connection is in memory. Never pass user,
/// configuration, downloaded or persisted SQL to this function.
///
/// At most 128 fragments / 1 MiB are accepted. Native execution has a three-second
/// progress deadline; results are limited to 1024 schema objects / 4 MiB of UTF-8
/// fields. The direct connection is explicitly closed on success and failure.
/// Metadata and SQLite internal objects are excluded by fingerprint version 1.
pub fn fingerprint_trusted_ddl(ddl: &[&str]) -> Result<String, DdlFingerprintError> {
    if ddl.len() > MAX_FRAGMENTS
        || ddl
            .iter()
            .try_fold(0usize, |total, fragment| total.checked_add(fragment.len()))
            .is_none_or(|total| total > MAX_DDL_BYTES)
    {
        return Err(DdlFingerprintError::InputLimit);
    }
    crate::sqlite::block_on_sqlite_connection(async {
        let mut connection = SqliteConnection::connect_with(
            &SqliteConnectOptions::new()
                .in_memory(true)
                .foreign_keys(true),
        )
        .await
        .map_err(DdlFingerprintError::Sqlite)?;
        let deadline = Instant::now() + DEADLINE;
        let result = evaluate(&mut connection, ddl, deadline).await;
        let close = connection.close().await;
        match (result, close) {
            (Err(operation), Err(close)) => Err(DdlFingerprintError::EvaluationAndClose {
                operation: Box::new(operation),
                close,
            }),
            (Err(error), Ok(())) => Err(error),
            (Ok(_), Err(error)) => Err(DdlFingerprintError::Close(error)),
            (Ok(fingerprint), Ok(())) => Ok(fingerprint),
        }
    })
}

async fn evaluate(
    connection: &mut SqliteConnection,
    ddl: &[&str],
    deadline: Instant,
) -> Result<String, DdlFingerprintError> {
    crate::sqlite::apply_connection_limits(
        connection,
        crate::sqlite::ConnectionLimits::new(MAX_SCHEMA_BYTES as u32),
    )
    .await
    .map_err(DdlFingerprintError::ConnectionLimits)?;
    {
        let mut handle = connection
            .lock_handle()
            .await
            .map_err(DdlFingerprintError::Sqlite)?;
        handle.set_progress_handler(1000, move || Instant::now() < deadline);
    }
    for fragment in ddl {
        if Instant::now() >= deadline {
            return Err(DdlFingerprintError::Deadline);
        }
        // Audited boundary: fragments are owned source constants, never data
        // received from configuration, persisted state, or external callers.
        sqlite_access::raw_sql(sqlite_access::AssertSqlSafe(*fragment))
            .execute(&mut *connection)
            .await
            .map_err(|error| {
                if Instant::now() >= deadline {
                    DdlFingerprintError::Deadline
                } else {
                    DdlFingerprintError::Sqlite(error)
                }
            })?;
    }
    let rows = crate::sqlite::schema_rows(&mut *connection)
        .await
        .map_err(|error| {
            if Instant::now() >= deadline {
                return DdlFingerprintError::Deadline;
            }
            match error {
                crate::sqlite::Error::SchemaBudgetExceeded => DdlFingerprintError::ResultLimit,
                crate::sqlite::Error::Sqlx(error) => DdlFingerprintError::Sqlite(error),
                error => DdlFingerprintError::SchemaRead(error),
            }
        })?;
    let fingerprint = schema_fingerprint(&rows).map_err(DdlFingerprintError::Schema)?;
    if Instant::now() >= deadline {
        return Err(DdlFingerprintError::Deadline);
    }
    Ok(fingerprint)
}

#[cfg(test)]
#[path = "../../../../test/platform/unit/sqlite/trusted_ddl/tests.rs"]
mod tests;
