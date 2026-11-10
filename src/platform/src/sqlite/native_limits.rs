//! Connection-local SQLite limits; no process-global memory settings.

use error_definition::Error;
use sqlite_access::{SqliteConnection, sqlite::LockedSqliteHandle};

/// Limits to apply before preparing product statements on a connection.
///
/// `max_value_bytes` bounds SQLite string/BLOB values and encoded rows. These
/// settings do not bound total process memory, sorting work, or allocations
/// made while initially opening a database. Use query deadlines, bounded
/// results and private database/schema validation in addition to these limits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectionLimits {
    pub max_value_bytes: u32,
    pub max_sql_bytes: u32,
    pub max_vm_operations: u32,
    pub max_bind_parameters: u32,
}

impl ConnectionLimits {
    pub const fn new(max_value_bytes: u32) -> Self {
        Self {
            max_value_bytes,
            max_sql_bytes: 1024 * 1024,
            max_vm_operations: 1_000_000,
            max_bind_parameters: 999,
        }
    }

    pub const fn with_max_sql_bytes(mut self, value: u32) -> Self {
        self.max_sql_bytes = value;
        self
    }

    pub const fn with_max_vm_operations(mut self, value: u32) -> Self {
        self.max_vm_operations = value;
        self
    }

    pub const fn with_max_bind_parameters(mut self, value: u32) -> Self {
        self.max_bind_parameters = value;
        self
    }

    pub(crate) fn validate(self) -> Result<(), ConnectionLimitError> {
        if !(1024..=64 * 1024 * 1024).contains(&self.max_value_bytes)
            || !(1024..=64 * 1024 * 1024).contains(&self.max_sql_bytes)
            || !(1024..=10_000_000).contains(&self.max_vm_operations)
            || !(1..=32766).contains(&self.max_bind_parameters)
        {
            return Err(ConnectionLimitError::InvalidLimits);
        }
        Ok(())
    }
}

/// Values read back from the native connection after applying all four limits.
pub type EffectiveConnectionLimits = ConnectionLimits;

#[derive(Debug, Error)]
pub enum ConnectionLimitError {
    #[error("SQLite connection limits are outside the supported bounds")]
    InvalidLimits,
    #[error("SQLite did not accept the requested connection limits")]
    NotApplied,
    #[error("SQLite connection handle could not be acquired")]
    Connection(#[source] sqlite_access::Error),
}

/// Apply limits to one connection and verify the native values.
///
/// All inputs are validated before changing the connection. Apply this before
/// preparing statements: a VM-operation limit does not invalidate statements
/// compiled earlier. A native rejection leaves the connection with its bounded
/// effective values; callers must not use that connection after an error.
pub async fn apply_connection_limits(
    connection: &mut SqliteConnection,
    limits: ConnectionLimits,
) -> Result<EffectiveConnectionLimits, ConnectionLimitError> {
    limits.validate()?;
    let mut handle = connection
        .lock_handle()
        .await
        .map_err(ConnectionLimitError::Connection)?;
    for (category, value) in [
        (LimitCategory::Value, limits.max_value_bytes),
        (LimitCategory::Sql, limits.max_sql_bytes),
        (LimitCategory::Vm, limits.max_vm_operations),
        (LimitCategory::Parameters, limits.max_bind_parameters),
    ] {
        native_limit(&mut handle, category, value as i32);
        if native_limit(&mut handle, category, -1) != value as i32 {
            return Err(ConnectionLimitError::NotApplied);
        }
    }
    Ok(limits)
}

#[derive(Clone, Copy)]
enum LimitCategory {
    Value,
    Sql,
    Vm,
    Parameters,
}

#[allow(unsafe_code)]
fn native_limit(handle: &mut LockedSqliteHandle<'_>, category: LimitCategory, value: i32) -> i32 {
    let category = match category {
        LimitCategory::Value => libsqlite3_sys::SQLITE_LIMIT_LENGTH,
        LimitCategory::Sql => libsqlite3_sys::SQLITE_LIMIT_SQL_LENGTH,
        LimitCategory::Vm => libsqlite3_sys::SQLITE_LIMIT_VDBE_OP,
        LimitCategory::Parameters => libsqlite3_sys::SQLITE_LIMIT_VARIABLE_NUMBER,
    };
    // SAFETY: SQLx's live exclusive LockedSqliteHandle pauses its connection
    // worker for this borrow. SQLx and this crate use the same pinned native
    // SQLite library/type. The fixed category is valid, and value is either a
    // validated nonnegative i32 or -1 (read-only query). sqlite3_limit retains
    // neither this pointer nor any Rust storage; no handle escapes the borrow.
    unsafe { libsqlite3_sys::sqlite3_limit(handle.as_raw_handle().as_ptr(), category, value) }
}

#[cfg(test)]
#[path = "../../../../test/platform/unit/sqlite/native_limits/tests.rs"]
mod tests;
