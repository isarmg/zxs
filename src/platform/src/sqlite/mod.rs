//! Strict SQLx adapters for ZXS's rebuildable metadata index.
//!
//! This crate does not own product DDL, migrations, locking, backup, restore or
//! file-permission policy. Opening an existing database and allowing SQLite to
//! create a missing file are deliberately separate operations.

use crate::schema_identity::{
    schema_fingerprint as fingerprint_rows, schema_identity_from_metadata_rows,
    validate_product_metadata_columns, validate_product_metadata_ddl,
};
use error_definition::Error;
use sqlite_access::{
    Connection, Executor, Row, Sqlite, SqliteConnection,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous},
};
use std::{io, path::Path, time::Duration};

mod blocking;
pub use blocking::block_on_sqlite_connection;
mod native_limits;
pub use native_limits::{
    ConnectionLimitError, ConnectionLimits, EffectiveConnectionLimits, apply_connection_limits,
};
mod native_security;
pub use native_security::{ConnectionSecurityError, disable_checkpoint_on_close, enable_defensive};
mod trusted_ddl;
pub use trusted_ddl::{DdlFingerprintError, fingerprint_trusted_ddl};

mod validation_snapshot;
pub use validation_snapshot::{SnapshotError, SnapshotLimits, ValidationSnapshot};

#[cfg(feature = "sqlite-pool")]
pub use validation_snapshot::{
    ValidationSnapshotPool, open_validation_snapshot,
    open_validation_snapshot_with_connection_limits,
};

#[cfg(feature = "sqlite-pool")]
mod pool;
#[cfg(feature = "sqlite-pool")]
pub use pool::*;

pub use crate::schema_identity::{
    Error as SchemaIdentityError, IdentityField, PRODUCT_METADATA_DDL, ProductMetadataColumn,
    ProductMetadataRow, SCHEMA_FINGERPRINT_ALGORITHM_VERSION, SchemaIdentity, SchemaRow,
};

pub const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
/// Maximum fingerprinted schema objects; an excess is rejected in full.
pub const MAX_SCHEMA_OBJECTS: usize = 1024;
/// Maximum aggregate UTF-8 bytes in the four fingerprinted fields.
pub const MAX_SCHEMA_BYTES: usize = 4 * 1024 * 1024;
/// Maximum retained foreign-key violation evidence, independent of row count.
pub const MAX_FOREIGN_KEY_EVIDENCE: usize = 32;

/// Fixed connection policy for the rebuildable ZXS index.
///
/// Exposed for the synchronous engine's direct-connection worker. This does not
/// create directories, initialize schema, or attest cross-disk commit state.
pub fn index_connect_options(
    database_path: &Path,
    create_if_missing: bool,
) -> SqliteConnectOptions {
    index_open_options(database_path, create_if_missing)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Full)
}

// SQLx drops rather than awaits closure if a connect-time PRAGMA fails. Keep
// schema-reading PRAGMAs out of connect_with so callers own and close the native
// worker before returning any corruption error (and moving database sidecars).
fn index_open_options(database_path: &Path, create_if_missing: bool) -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .filename(database_path)
        .create_if_missing(create_if_missing)
        .foreign_keys(true)
        .busy_timeout(BUSY_TIMEOUT)
}

/// Open a direct SQLite connection, suitable for [`block_on_sqlite_connection`].
/// The caller must explicitly close the connection at the end of its ownership
/// lifetime, before replacing database files. A synchronous owner may reuse the
/// worker across calls. Filesystem permissions and reconstruction remain engine duties.
pub async fn open_index_connection(
    database_path: &Path,
    create_if_missing: bool,
    limits: Option<ConnectionLimits>,
) -> Result<SqliteConnection, Error> {
    // Reject configuration errors before opening or creating an index file.
    if let Some(limits) = limits {
        limits.validate()?;
    }
    let mut connection =
        SqliteConnection::connect_with(&index_open_options(database_path, create_if_missing))
            .await?;
    let initialized = async {
        sqlite_access::query("PRAGMA journal_mode=WAL")
            .execute(&mut connection)
            .await?;
        sqlite_access::query("PRAGMA synchronous=FULL")
            .execute(&mut connection)
            .await?;
        enable_defensive(&mut connection)
            .await
            .map_err(|error| sqlite_access::Error::Io(io::Error::other(error)))?;
        if let Some(limits) = limits {
            apply_connection_limits(&mut connection, limits)
                .await
                .map_err(|error| sqlite_access::Error::Io(io::Error::other(error)))?;
        }
        Ok::<_, sqlite_access::Error>(())
    }
    .await;
    if let Err(error) = initialized {
        if let Err(close_error) = connection.close().await {
            // A caller must not interpret the original corruption code as
            // permission to move files while native closure remains uncertain.
            return Err(Error::InitializationAndClose {
                operation: error,
                close: close_error,
            });
        }
        return Err(connect_error(error));
    }
    Ok(connection)
}

/// Best-effort candidate reads must not hold the engine mutex for the normal
/// writer busy budget. Preserve defensive/WAL/FULL policy, never create a file.
pub async fn open_index_read_connection(database_path: &Path) -> Result<SqliteConnection, Error> {
    let options = index_open_options(database_path, false)
        .read_only(true)
        .busy_timeout(Duration::from_millis(20));
    let mut connection = SqliteConnection::connect_with(&options).await?;
    let initialized = async {
        sqlite_access::query("PRAGMA journal_mode=WAL")
            .execute(&mut connection)
            .await?;
        sqlite_access::query("PRAGMA synchronous=FULL")
            .execute(&mut connection)
            .await?;
        enable_defensive(&mut connection)
            .await
            .map_err(|error| sqlite_access::Error::Io(io::Error::other(error)))?;
        apply_connection_limits(
            &mut connection,
            ConnectionLimits::new(64 * 1024)
                .with_max_sql_bytes(16 * 1024)
                .with_max_vm_operations(10_000)
                .with_max_bind_parameters(16),
        )
        .await
        .map_err(|error| sqlite_access::Error::Io(io::Error::other(error)))?;
        Ok::<_, sqlite_access::Error>(())
    }
    .await;
    if let Err(error) = initialized {
        if let Err(close_error) = connection.close().await {
            return Err(Error::InitializationAndClose {
                operation: error,
                close: close_error,
            });
        }
        return Err(connect_error(error));
    }
    Ok(connection)
}

fn connect_error(error: sqlite_access::Error) -> Error {
    if let sqlite_access::Error::Io(source) = error {
        let kind = source.kind();
        if let Some(inner) = source.into_inner() {
            return match inner.downcast::<ConnectionLimitError>() {
                Ok(error) => Error::ConnectionLimits(*error),
                Err(inner) => Error::Sqlx(sqlite_access::Error::Io(io::Error::new(kind, inner))),
            };
        }
        return Error::Sqlx(sqlite_access::Error::Io(io::Error::from(kind)));
    }
    Error::Sqlx(error)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ForeignKeyViolation {
    pub table: String,
    pub row_id: Option<i64>,
    pub parent: String,
    pub foreign_key_index: i64,
}

/// Read the canonically filtered and ordered rows using any SQLx SQLite
/// executor, including a pool, connection or transaction connection.
///
/// At most 1024 objects / 4 MiB of UTF-8 fields are accepted. The SQL projection
/// suppresses all text before transfer to the driver when either bound is
/// exceeded. No fingerprint is computed from a truncated subset. This bounds
/// the returned data, not SQLite's native schema loading or total process heap.
pub async fn schema_rows<'executor, E>(executor: E) -> Result<Vec<SchemaRow>, Error>
where
    E: Executor<'executor, Database = Sqlite>,
{
    // Keep the version-1 filter, raw SQL bytes and BINARY ordering unchanged.
    // LIMIT 1025 proves an object excess without transferring an unbounded
    // result; window totals gate every text field before SQLx allocates it.
    let rows = sqlite_access::query(
        "WITH bounded AS (\
           SELECT type, name, tbl_name, COALESCE(sql, '') AS sql \
           FROM sqlite_schema \
           WHERE name NOT GLOB 'sqlite_*' AND name <> 'product_metadata' \
           ORDER BY type, name, tbl_name LIMIT 1025\
         ), charged AS (\
           SELECT *, COUNT(*) OVER () AS object_count, \
             SUM(length(CAST(type AS BLOB)) + length(CAST(name AS BLOB)) + \
                 length(CAST(tbl_name AS BLOB)) + length(CAST(sql AS BLOB))) \
               OVER () AS total_bytes FROM bounded\
         ) SELECT \
           CASE WHEN object_count <= 1024 AND total_bytes <= 4194304 THEN type END, \
           CASE WHEN object_count <= 1024 AND total_bytes <= 4194304 THEN name END, \
           CASE WHEN object_count <= 1024 AND total_bytes <= 4194304 THEN tbl_name END, \
           CASE WHEN object_count <= 1024 AND total_bytes <= 4194304 THEN sql END, \
           object_count, total_bytes FROM charged ORDER BY type, name, tbl_name",
    )
    .fetch_all(executor)
    .await?;
    if let Some(row) = rows.first()
        && (row.try_get::<i64, _>(4)? > MAX_SCHEMA_OBJECTS as i64
            || row.try_get::<i64, _>(5)? > MAX_SCHEMA_BYTES as i64)
    {
        return Err(Error::SchemaBudgetExceeded);
    }
    let rows = rows
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
    Ok(rows)
}

/// Calculate the canonical schema fingerprint using an arbitrary SQLx SQLite
/// executor. The pure algorithm lives in `crate::schema_identity`.
pub async fn schema_fingerprint<'executor, E>(executor: E) -> Result<String, Error>
where
    E: Executor<'executor, Database = Sqlite>,
{
    Ok(fingerprint_rows(&schema_rows(executor).await?)?)
}

/// Validate the metadata table and return an identity only after its declared
/// fingerprint has been verified against the actual schema.
pub async fn read_schema_identity(
    connection: &mut SqliteConnection,
) -> Result<SchemaIdentity, Error> {
    validate_metadata_table(connection).await?;
    let metadata_rows = read_metadata_rows(connection).await?;
    let identity = schema_identity_from_metadata_rows(&metadata_rows)?;
    let actual_fingerprint = schema_fingerprint(&mut *connection).await?;
    identity.verify_fingerprint(&actual_fingerprint)?;
    Ok(identity)
}

/// Validate an identity against exact compiled current values.
pub async fn require_current_schema(
    connection: &mut SqliteConnection,
    expected: &SchemaIdentity,
) -> Result<SchemaIdentity, Error> {
    let actual = read_schema_identity(connection).await?;
    actual.require_exact(expected)?;
    Ok(actual)
}

async fn validate_metadata_table(connection: &mut SqliteConnection) -> Result<(), Error> {
    let ddl: Option<String> = sqlite_access::query_scalar(
        "SELECT sql FROM sqlite_schema WHERE type='table' AND name='product_metadata'",
    )
    .fetch_optional(&mut *connection)
    .await?;
    let ddl = ddl.ok_or(Error::ProductMetadataTableMissing)?;
    validate_product_metadata_ddl(&ddl)?;

    let columns = sqlite_access::query(
        "SELECT cid, name, type, \"notnull\", dflt_value, pk \
         FROM pragma_table_info('product_metadata') ORDER BY cid",
    )
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .map(|row| {
        Ok(ProductMetadataColumn {
            cid: row.try_get(0)?,
            name: row.try_get(1)?,
            declared_type: row.try_get(2)?,
            not_null: row.try_get(3)?,
            default_sql: row.try_get(4)?,
            primary_key_position: row.try_get(5)?,
        })
    })
    .collect::<Result<Vec<_>, sqlite_access::Error>>()?;
    validate_product_metadata_columns(&columns)?;
    Ok(())
}

async fn read_metadata_rows(
    connection: &mut SqliteConnection,
) -> Result<Vec<ProductMetadataRow>, Error> {
    let rows = sqlite_access::query(
        "SELECT typeof(singleton), singleton, typeof(application), application, \
                typeof(application_version), application_version, \
                typeof(schema_revision), schema_revision, \
                typeof(schema_sha256), schema_sha256 \
         FROM product_metadata ORDER BY singleton LIMIT 2",
    )
    .fetch_all(connection)
    .await?;

    let mut metadata = Vec::with_capacity(rows.len());
    for row in rows {
        for (field, index, expected) in [
            ("singleton", 0, "integer"),
            ("application", 2, "text"),
            ("application_version", 4, "text"),
            ("schema_revision", 6, "integer"),
            ("schema_sha256", 8, "text"),
        ] {
            let actual: String = row.try_get(index)?;
            if actual != expected {
                return Err(Error::ProductMetadataStorageClass {
                    field,
                    expected,
                    actual,
                });
            }
        }
        metadata.push(ProductMetadataRow {
            singleton: row.try_get(1)?,
            application: row.try_get(3)?,
            application_version: row.try_get(5)?,
            schema_revision: row.try_get(7)?,
            schema_sha256: row.try_get(9)?,
        });
    }
    Ok(metadata)
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("SQLite initialization failed: {operation}; connection close also failed: {close}")]
    InitializationAndClose {
        operation: sqlite_access::Error,
        #[source]
        close: sqlite_access::Error,
    },
    #[cfg(feature = "sqlite-pool")]
    #[error(transparent)]
    InvalidPoolOptions(#[from] PoolOptionsError),
    #[error("SQLite operation failed: {0}")]
    Sqlx(#[from] sqlite_access::Error),
    #[error(transparent)]
    ConnectionLimits(#[from] ConnectionLimitError),
    #[error("SQLite integrity check failed: {messages:?}")]
    IntegrityCheckFailed { messages: Vec<String> },
    #[error("SQLite foreign-key check found violations: {violations:?}")]
    ForeignKeyViolations {
        violations: Vec<ForeignKeyViolation>,
    },
    #[error(
        "SQLite WAL checkpoint is busy ({checkpointed_frames}/{log_frames} frames checkpointed)"
    )]
    CheckpointBusy {
        log_frames: i64,
        checkpointed_frames: i64,
    },
    #[error(
        "SQLite WAL checkpoint was incomplete ({checkpointed_frames}/{log_frames} frames checkpointed)"
    )]
    CheckpointIncomplete {
        log_frames: i64,
        checkpointed_frames: i64,
    },
    #[error("database has no product_metadata table")]
    ProductMetadataTableMissing,
    #[error("SQLite schema exceeds the supported object or byte budget")]
    SchemaBudgetExceeded,
    #[error("product_metadata {field} must use SQLite storage class {expected}, found {actual}")]
    ProductMetadataStorageClass {
        field: &'static str,
        expected: &'static str,
        actual: String,
    },
    #[error(transparent)]
    SchemaIdentity(#[from] SchemaIdentityError),
}

#[cfg(test)]
#[path = "../../../../test/platform/unit/sqlite/tests.rs"]
mod tests;
