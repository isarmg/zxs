//! Physical integrity after a bounded closed-schema SQLite image is decoded.
//! Callers own a dedicated read-only memory connection with no progress handler.
//! This checks indexes/pages, not source authority; no file or writer is opened.
use crate::{Error, ErrorCode, Result};
use sqlite_access::SqliteConnection;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

const PROGRESS_INTERVAL: i32 = 1_000;
const PROGRESS_CALLS: usize = 10_000;

pub(crate) async fn verify(connection: &mut SqliteConnection) -> Result<()> {
    let exhausted = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&exhausted);
    let mut remaining = PROGRESS_CALLS;
    connection
        .lock_handle()
        .await?
        .set_progress_handler(PROGRESS_INTERVAL, move || {
            if remaining == 0 {
                observed.store(true, Ordering::Relaxed);
                return false;
            }
            remaining -= 1;
            true
        });
    // quick_check deliberately omits index/table correspondence. A damaged
    // index could otherwise hide retained rows from every ordered read.
    // Bound both the number and size of diagnostics supplied by SQLite.
    let result = async {
        let reports: Vec<String> = sqlite_access::query_scalar(
            "SELECT substr(integrity_check,1,512) FROM pragma_integrity_check(1) LIMIT 2",
        )
        .fetch_all(&mut *connection)
        .await
        .map_err(|error| {
            if exhausted.load(Ordering::Relaxed) {
                Error::from(error).context(
                    ErrorCode::ResourceLimit,
                    "SQLite image integrity scan exceeded its execution budget",
                )
            } else {
                Error::from(error).context(
                    ErrorCode::ContainerLayoutInvalid,
                    "SQLite image physical integrity scan failed",
                )
            }
        })?;
        match reports.as_slice() {
            [report] if report == "ok" => Ok(()),
            [report] => Err(Error::new(
                ErrorCode::ContainerLayoutInvalid,
                format!("SQLite image physical integrity differs: {report}"),
            )),
            _ => Err(Error::new(
                ErrorCode::ContainerLayoutInvalid,
                "SQLite image integrity scan did not return one complete result",
            )),
        }
    }
    .await;
    // Do not let an exhausted query budget interfere with connection teardown.
    let reset = connection
        .lock_handle()
        .await
        .map(|mut handle| handle.remove_progress_handler());
    match (result, reset) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(error.into()),
        (Err(error), Err(reset)) => Err(Error::Cleanup {
            operation: Box::new(error),
            cleanup: std::io::Error::other(reset),
        }),
    }
}
