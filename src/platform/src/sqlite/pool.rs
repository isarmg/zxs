//! Optional Tokio-backed multi-connection adapters.

use super::*;
use sqlite_access::{SqlitePool, sqlite::SqlitePoolOptions};

pub const DEFAULT_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(10);

/// The intentionally small set of product-selectable pool settings.
///
/// Durability and correctness PRAGMAs are not configurable: every connection
/// uses WAL, foreign keys, a five-second busy timeout and FULL synchronous.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PoolOptions {
    max_connections: u32,
    min_connections: u32,
    acquire_timeout: Duration,
    connection_limits: Option<ConnectionLimits>,
}

impl PoolOptions {
    pub const fn new(max_connections: u32) -> Self {
        Self {
            max_connections,
            min_connections: 0,
            acquire_timeout: DEFAULT_ACQUIRE_TIMEOUT,
            connection_limits: None,
        }
    }

    pub const fn with_min_connections(mut self, min_connections: u32) -> Self {
        self.min_connections = min_connections;
        self
    }

    pub const fn with_acquire_timeout(mut self, acquire_timeout: Duration) -> Self {
        self.acquire_timeout = acquire_timeout;
        self
    }

    /// Apply native limits before product statements on every new connection,
    /// including connections replacing closed or failed pool members.
    pub const fn with_connection_limits(mut self, limits: ConnectionLimits) -> Self {
        self.connection_limits = Some(limits);
        self
    }

    pub const fn max_connections(&self) -> u32 {
        self.max_connections
    }

    pub const fn min_connections(&self) -> u32 {
        self.min_connections
    }

    pub const fn acquire_timeout(&self) -> Duration {
        self.acquire_timeout
    }

    pub fn validate(&self) -> Result<(), PoolOptionsError> {
        if self.max_connections == 0 {
            return Err(PoolOptionsError::ZeroMaxConnections);
        }
        if self.min_connections > self.max_connections {
            return Err(PoolOptionsError::MinConnectionsExceedMax {
                min_connections: self.min_connections,
                max_connections: self.max_connections,
            });
        }
        if self.acquire_timeout.is_zero() {
            return Err(PoolOptionsError::ZeroAcquireTimeout);
        }
        if let Some(limits) = self.connection_limits {
            limits
                .validate()
                .map_err(PoolOptionsError::InvalidConnectionLimits)?;
        }
        Ok(())
    }
}

impl Default for PoolOptions {
    fn default() -> Self {
        Self::new(10)
    }
}

/// Open an existing SQLite file. This operation never asks SQLite to create a
/// missing file. Native open failures retain their SQLite source.
pub async fn open_existing(
    database_path: impl AsRef<Path>,
    options: PoolOptions,
) -> Result<SqlitePool, Error> {
    options.validate()?;
    let database_path = database_path.as_ref();
    connect(database_path, false, options).await
}

/// Open a SQLite file, explicitly allowing SQLite to create it when missing.
/// This is not a schema initializer and may also open an already-existing file.
pub async fn create_if_missing(
    database_path: impl AsRef<Path>,
    options: PoolOptions,
) -> Result<SqlitePool, Error> {
    options.validate()?;
    connect(database_path.as_ref(), true, options).await
}

async fn connect(
    database_path: &Path,
    create_if_missing: bool,
    options: PoolOptions,
) -> Result<SqlitePool, Error> {
    let connect_options = index_connect_options(database_path, create_if_missing);
    let pool = SqlitePoolOptions::new()
        .max_connections(options.max_connections)
        .min_connections(options.min_connections)
        .acquire_timeout(options.acquire_timeout)
        .after_connect(move |connection, _| {
            Box::pin(async move {
                // The query index never needs writable_schema or raw page writes.
                enable_defensive(connection)
                    .await
                    .map_err(|error| sqlite_access::Error::Io(io::Error::other(error)))?;
                if let Some(limits) = options.connection_limits {
                    apply_connection_limits(connection, limits)
                        .await
                        .map_err(|error| sqlite_access::Error::Io(io::Error::other(error)))?;
                }
                Ok(())
            })
        })
        .connect_with(connect_options)
        .await
        .map_err(connect_error)?;
    Ok(pool)
}

/// Require the complete integrity-check result to be the single value `ok`.
pub async fn integrity_check(pool: &SqlitePool) -> Result<(), Error> {
    let messages: Vec<String> = sqlite_access::query_scalar("PRAGMA integrity_check")
        .fetch_all(pool)
        .await?;
    if messages.len() != 1 || !messages[0].eq_ignore_ascii_case("ok") {
        return Err(Error::IntegrityCheckFailed { messages });
    }
    Ok(())
}

/// Require `PRAGMA foreign_key_check` to return no violations.
///
/// Failure retains at most the first 32 violations as evidence. It never treats
/// a partial evidence collection as a successful integrity check.
pub async fn foreign_key_check(pool: &SqlitePool) -> Result<(), Error> {
    let violations = sqlite_access::query(
        "SELECT \"table\", rowid, parent, fkid FROM pragma_foreign_key_check LIMIT 32",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|row| {
        Ok(ForeignKeyViolation {
            table: row.try_get(0)?,
            row_id: row.try_get(1)?,
            parent: row.try_get(2)?,
            foreign_key_index: row.try_get(3)?,
        })
    })
    .collect::<Result<Vec<_>, sqlite_access::Error>>()?;
    if !violations.is_empty() {
        return Err(Error::ForeignKeyViolations { violations });
    }
    Ok(())
}

/// Truncate-checkpoint the WAL and reject both busy and incomplete results.
pub async fn checkpoint(pool: &SqlitePool) -> Result<(), Error> {
    let result: (i64, i64, i64) = sqlite_access::query_as("PRAGMA wal_checkpoint(TRUNCATE)")
        .fetch_one(pool)
        .await?;
    validate_checkpoint_result(result)
}

pub(super) fn validate_checkpoint_result(
    (busy, log_frames, checkpointed_frames): (i64, i64, i64),
) -> Result<(), Error> {
    if busy != 0 {
        return Err(Error::CheckpointBusy {
            log_frames,
            checkpointed_frames,
        });
    }
    if log_frames != checkpointed_frames {
        return Err(Error::CheckpointIncomplete {
            log_frames,
            checkpointed_frames,
        });
    }
    Ok(())
}

/// Pool convenience wrapper around [`read_schema_identity`].
pub async fn read_pool_schema_identity(pool: &SqlitePool) -> Result<SchemaIdentity, Error> {
    let mut connection = pool.acquire().await?;
    read_schema_identity(&mut connection).await
}

/// Pool convenience wrapper around [`require_current_schema`].
pub async fn require_pool_current_schema(
    pool: &SqlitePool,
    expected: &SchemaIdentity,
) -> Result<SchemaIdentity, Error> {
    let mut connection = pool.acquire().await?;
    require_current_schema(&mut connection, expected).await
}

#[derive(Debug, Error)]
pub enum PoolOptionsError {
    #[error("max_connections must be greater than zero")]
    ZeroMaxConnections,
    #[error(
        "min_connections ({min_connections}) must not exceed max_connections ({max_connections})"
    )]
    MinConnectionsExceedMax {
        min_connections: u32,
        max_connections: u32,
    },
    #[error("acquire_timeout must be greater than zero")]
    ZeroAcquireTimeout,
    #[error("connection limits are outside the supported bounds")]
    InvalidConnectionLimits(#[source] ConnectionLimitError),
}
