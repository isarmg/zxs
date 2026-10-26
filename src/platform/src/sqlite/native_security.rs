//! One-way connection hardening through SQLx's exclusive native handle.
use error_definition::Error;
use sqlite_access::{SqliteConnection, sqlite::LockedSqliteHandle};

#[derive(Debug, Error)]
pub enum ConnectionSecurityError {
    #[error("SQLite connection handle could not be acquired")]
    Connection(#[source] sqlite_access::Error),
    #[error("SQLite refused defensive mode")]
    DefensiveModeRejected,
    #[error("SQLite refused to disable checkpoint-on-close")]
    CheckpointOnCloseRejected,
}

/// Enable SQLite's defensive mode and read back the effective setting.
///
/// This is connection-local and cannot disable the setting. SQLx has no safe
/// db-config API, and SQLite has no equivalent defensive PRAGMA. Call before
/// product statements; a failed connection must not be used afterwards.
pub async fn enable_defensive(
    connection: &mut SqliteConnection,
) -> Result<(), ConnectionSecurityError> {
    let mut handle = connection
        .lock_handle()
        .await
        .map_err(ConnectionSecurityError::Connection)?;
    if set_and_verify(&mut handle, libsqlite3_sys::SQLITE_DBCONFIG_DEFENSIVE) {
        Ok(())
    } else {
        Err(ConnectionSecurityError::DefensiveModeRejected)
    }
}

/// Disable implicit WAL checkpointing and sidecar unlinking on connection close.
///
/// Configure before admitting a long-lived worker. Its shutdown then releases
/// resources without mutating names that may have been replaced while it lived.
/// FULL commits and explicit/automatic checkpoints retain their normal policy.
/// WAL and SHM can remain after close and must accompany the main database.
pub async fn disable_checkpoint_on_close(
    connection: &mut SqliteConnection,
) -> Result<(), ConnectionSecurityError> {
    let mut handle = connection
        .lock_handle()
        .await
        .map_err(ConnectionSecurityError::Connection)?;
    if set_and_verify(
        &mut handle,
        libsqlite3_sys::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
    ) {
        Ok(())
    } else {
        Err(ConnectionSecurityError::CheckpointOnCloseRejected)
    }
}

#[allow(unsafe_code)]
fn set_and_verify(handle: &mut LockedSqliteHandle<'_>, option: std::ffi::c_int) -> bool {
    let mut enabled: std::ffi::c_int = 0;
    // SAFETY: this live SQLx exclusive handle pauses its worker and uses the
    // same pinned native SQLite library. Both private callers pass an integer
    // db-config option (DEFENSIVE or NO_CKPT_ON_CLOSE), which requires a
    // promoted C int and an initialized int output pointer. Only fixed 1 (set)
    // and -1 (read) are passed; output storage stays live and never escapes.
    let (set, read) = unsafe {
        let database = handle.as_raw_handle().as_ptr();
        let set = libsqlite3_sys::sqlite3_db_config(
            database,
            option,
            1 as std::ffi::c_int,
            &mut enabled as *mut std::ffi::c_int,
        );
        let read = libsqlite3_sys::sqlite3_db_config(
            database,
            option,
            -1 as std::ffi::c_int,
            &mut enabled as *mut std::ffi::c_int,
        );
        (set, read)
    };
    set == libsqlite3_sys::SQLITE_OK && read == libsqlite3_sys::SQLITE_OK && enabled == 1
}

#[cfg(test)]
#[path = "../../../../test/platform/unit/sqlite/native_security/tests.rs"]
mod tests;
