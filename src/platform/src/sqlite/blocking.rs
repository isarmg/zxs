//! Synchronous entry point for the SQLite driver's connection worker.

use std::future::Future;

/// Complete a trusted direct [`sqlite_access::SqliteConnection`] operation synchronously.
///
/// SQLite connection creation, statements and explicit connection closure use
/// SQLx's native worker thread and channels. They do not require a Tokio reactor.
/// This entry point also works inside a Tokio current-thread or multi-thread
/// runtime without starting a nested Tokio runtime.
///
/// The future must contain only direct SQLite connection work. Pools, Tokio
/// timers/network/filesystem futures and recursively calling this function are
/// outside this contract. The caller owns the connection lifecycle: either close
/// it before completing the future, or retain it in a synchronous owner that
/// explicitly closes it before replacing its files or releasing its root lock.
/// This executor does not replace that ownership.
pub fn block_on_sqlite_connection<F: Future>(future: F) -> F::Output {
    futures::executor::block_on(future)
}

#[cfg(test)]
#[path = "../../../../test/platform/unit/sqlite/blocking/tests.rs"]
mod tests;
