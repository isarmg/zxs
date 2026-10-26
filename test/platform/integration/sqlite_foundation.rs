#[cfg(feature = "sqlite-pool")]
use sqlite_access::sqlite::SqlitePoolOptions;
use sqlite_access::{Connection, Row};
#[cfg(feature = "sqlite-pool")]
use std::time::Duration;

#[tokio::test]
#[cfg(feature = "sqlite-pool")]
async fn pool_timeout_rollback_savepoint_and_statement_eviction() -> Result<(), sqlite_access::Error>
{
    let options = sqlite_access::sqlite::SqliteConnectOptions::new()
        .in_memory(true)
        .statement_cache_capacity(2);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_millis(20))
        .connect_with(options)
        .await?;
    sqlite_access::query("CREATE TABLE entries (value TEXT NOT NULL)")
        .execute(&pool)
        .await?;
    let mut transaction = pool.begin().await?;
    assert!(matches!(
        pool.acquire().await,
        Err(sqlite_access::Error::PoolTimedOut)
    ));
    sqlite_access::query("INSERT INTO entries VALUES (?)")
        .bind("撤销 Ω")
        .execute(&mut *transaction)
        .await?;
    drop(transaction);
    let count: i64 = sqlite_access::query_scalar("SELECT count(*) FROM entries")
        .fetch_one(&pool)
        .await?;
    assert_eq!(count, 0);
    transaction = pool.begin().await?;
    sqlite_access::query("INSERT INTO entries VALUES (?)")
        .bind("保留 Ω")
        .execute(&mut *transaction)
        .await?;
    let mut savepoint = transaction.begin().await?;
    sqlite_access::query("INSERT INTO entries VALUES (?)")
        .bind("嵌套撤销")
        .execute(&mut *savepoint)
        .await?;
    savepoint.rollback().await?;
    transaction.commit().await?;
    let values: Vec<String> = sqlite_access::query_scalar("SELECT value FROM entries")
        .fetch_all(&pool)
        .await?;
    assert_eq!(values, ["保留 Ω"]);
    let mut connection = pool.acquire().await?;
    for query in ["SELECT ?", "SELECT ? + 1", "SELECT ? + 2", "SELECT ? + 3"] {
        let _: i64 = sqlite_access::query_scalar(query)
            .bind(41_i64)
            .fetch_one(&mut *connection)
            .await?;
        assert!(connection.cached_statements_size() <= 2);
    }
    assert_eq!(connection.cached_statements_size(), 2);
    connection.clear_cached_statements().await?;
    assert_eq!(connection.cached_statements_size(), 0);
    drop(connection);
    pool.close().await;
    assert!(matches!(
        pool.acquire().await,
        Err(sqlite_access::Error::PoolClosed)
    ));
    Ok(())
}

#[tokio::test]
async fn sqlite_row_diagnostics_keep_values_null_and_decode_errors()
-> Result<(), sqlite_access::Error> {
    let mut connection = sqlite_access::SqliteConnection::connect("sqlite::memory:").await?;
    let row = sqlite_access::query("SELECT '文字 Ω' AS text_value, NULL AS null_value, X'00ff' AS blob_value, 1.25 AS real_value")
        .fetch_one(&mut connection).await?;
    let formatted = format!("{row:?}");
    assert!(formatted.contains("文字 Ω"), "{formatted}");
    assert!(formatted.contains("NULL"), "{formatted}");
    assert!(formatted.contains("[0, 255]"), "{formatted}");
    assert!(formatted.contains("1.25"), "{formatted}");
    assert!(row.try_get::<i64, _>("text_value").is_err());
    connection.close().await?;
    Ok(())
}

#[tokio::test]
async fn returning_reports_commit_lock_failure() {
    use sqlite_access::{
        ConnectOptions,
        sqlite::{SqliteConnectOptions, SqliteJournalMode},
    };
    let directory = tempfile::tempdir().unwrap();
    let options = SqliteConnectOptions::new()
        .filename(directory.path().join("returning.sqlite"))
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Delete)
        .busy_timeout(std::time::Duration::from_millis(20));
    let mut writer = options.connect().await.unwrap();
    sqlite_access::query("CREATE TABLE entries (value INTEGER)")
        .execute(&mut writer)
        .await
        .unwrap();
    let mut reader = options.connect().await.unwrap();
    let mut transaction = reader.begin().await.unwrap();
    let _: i64 = sqlite_access::query_scalar("SELECT count(*) FROM entries")
        .fetch_one(&mut *transaction)
        .await
        .unwrap();
    let result = sqlite_access::query("INSERT INTO entries VALUES (1) RETURNING value")
        .fetch_one(&mut writer)
        .await;
    let error = result.expect_err("RETURNING must not report success when reset cannot commit");
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("5")
    );
    transaction.rollback().await.unwrap();
    writer.close().await.unwrap();
    reader.close().await.unwrap();
}

#[cfg(feature = "sqlite-pool")]
#[allow(unsafe_code)]
async fn deny_rollback(connection: &mut sqlite_access::SqliteConnection) {
    use std::ffi::{CStr, c_char, c_int, c_void};
    unsafe extern "C" fn authorize(
        _: *mut c_void,
        action: c_int,
        argument: *const c_char,
        _: *const c_char,
        _: *const c_char,
        _: *const c_char,
    ) -> c_int {
        // SAFETY: SQLite supplies a NUL-terminated first argument for SQLITE_TRANSACTION.
        if action == libsqlite3_sys::SQLITE_TRANSACTION
            && unsafe { CStr::from_ptr(argument) }.to_bytes() == b"ROLLBACK"
        {
            libsqlite3_sys::SQLITE_DENY
        } else {
            libsqlite3_sys::SQLITE_OK
        }
    }
    let mut handle = connection.lock_handle().await.unwrap();
    // SAFETY: the exclusive live handle owns this callback, which captures no state.
    let result = unsafe {
        libsqlite3_sys::sqlite3_set_authorizer(
            handle.as_raw_handle().as_ptr(),
            Some(authorize),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(result, libsqlite3_sys::SQLITE_OK);
}

#[tokio::test]
#[cfg(feature = "sqlite-pool")]
async fn failed_drop_rollback_retires_connection() {
    let mut connection = sqlite_access::SqliteConnection::connect("sqlite::memory:")
        .await
        .unwrap();
    let mut transaction = connection.begin().await.unwrap();
    deny_rollback(&mut transaction).await;
    drop(transaction);
    for _ in 0..2 {
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), connection.ping())
                .await
                .unwrap(),
            Err(sqlite_access::Error::WorkerCrashed)
        ));
    }
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), connection.lock_handle())
            .await
            .unwrap(),
        Err(sqlite_access::Error::WorkerCrashed)
    ));
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        sqlite_access::query("CREATE TABLE forbidden (value INTEGER)").execute(&mut connection),
    )
    .await
    .unwrap();
    assert!(matches!(result, Err(sqlite_access::Error::WorkerCrashed)));
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), connection.close())
            .await
            .unwrap(),
        Err(sqlite_access::Error::WorkerCrashed)
    ));
}

#[tokio::test]
#[cfg(feature = "sqlite-pool")]
async fn pool_does_not_reuse_failed_drop_rollback() {
    let directory = tempfile::tempdir().unwrap();
    let options = sqlite_access::sqlite::SqliteConnectOptions::new()
        .filename(directory.path().join("rollback.sqlite"))
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(2))
        .connect_with(options)
        .await
        .unwrap();
    sqlite_access::query("CREATE TABLE entries (value INTEGER)")
        .execute(&pool)
        .await
        .unwrap();
    let mut transaction = pool.begin().await.unwrap();
    sqlite_access::query("INSERT INTO entries VALUES (1)")
        .execute(&mut *transaction)
        .await
        .unwrap();
    deny_rollback(&mut transaction).await;
    drop(transaction);
    let count: i64 = tokio::time::timeout(
        Duration::from_secs(2),
        sqlite_access::query_scalar("SELECT count(*) FROM entries").fetch_one(&pool),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        count, 0,
        "a failed rollback must not leak a transaction to the next borrower"
    );
    pool.close().await;
}

#[tokio::test]
#[cfg(feature = "sqlite-pool")]
async fn pool_close_and_statement_reset_report_errors_once() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct CloseLog(AtomicUsize, AtomicUsize);
    impl log::Log for CloseLog {
        fn enabled(&self, _: &log::Metadata<'_>) -> bool {
            true
        }
        fn log(&self, record: &log::Record<'_>) {
            if record
                .args()
                .to_string()
                .contains("error occurred while closing the pool connection")
            {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
            if record
                .args()
                .to_string()
                .contains("error occurred while resetting the SQLite statement")
            {
                self.1.fetch_add(1, Ordering::SeqCst);
            }
        }
        fn flush(&self) {}
    }
    static LOG: CloseLog = CloseLog(AtomicUsize::new(0), AtomicUsize::new(0));
    log::set_logger(&LOG).unwrap();
    log::set_max_level(log::LevelFilter::Trace);
    for close_on_drop in [false, true] {
        let before = LOG.0.load(Ordering::SeqCst);
        let options = sqlite_access::sqlite::SqliteConnectOptions::new()
            .in_memory(true)
            .optimize_on_close(true, None);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        let mut connection = pool.acquire().await.unwrap();
        connection
            .lock_handle()
            .await
            .unwrap()
            .set_progress_handler(1, || false);
        if close_on_drop {
            connection.close_on_drop();
        } else {
            connection.return_to_pool().await;
        }
        drop(connection);
        tokio::time::timeout(Duration::from_secs(2), pool.close())
            .await
            .unwrap();
        assert_eq!(
            LOG.0.load(Ordering::SeqCst) - before,
            1,
            "close failure must be reported once (close_on_drop={close_on_drop})"
        );
    }

    use futures::TryStreamExt;
    use sqlite_access::{ConnectOptions, sqlite::SqliteJournalMode};
    let directory = tempfile::tempdir().unwrap();
    let options = sqlite_access::sqlite::SqliteConnectOptions::new()
        .filename(directory.path().join("reset.sqlite"))
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Delete)
        .row_buffer_size(0)
        .busy_timeout(Duration::from_millis(20));
    let mut writer = options.connect().await.unwrap();
    sqlite_access::query("CREATE TABLE entries (value INTEGER UNIQUE)")
        .execute(&mut writer)
        .await
        .unwrap();
    sqlite_access::query("INSERT INTO entries VALUES (1)")
        .execute(&mut writer)
        .await
        .unwrap();
    let before = LOG.1.load(Ordering::SeqCst);
    let error = sqlite_access::query("INSERT INTO entries VALUES (1)")
        .execute(&mut writer)
        .await
        .unwrap_err();
    assert!(error.as_database_error().unwrap().is_unique_violation());
    assert!(
        error
            .as_database_error()
            .unwrap()
            .try_downcast_ref::<sqlite_access::sqlite::SqliteError>()
            .is_some()
    );
    assert!(
        sqlite_access::query("SELECT FROM")
            .execute(&mut writer)
            .await
            .is_err()
    );
    assert!(matches!(
        sqlite_access::query("SELECT :unsupported")
            .bind(1_i64)
            .execute(&mut writer)
            .await,
        Err(sqlite_access::Error::Protocol(_))
    ));
    writer.ping().await.unwrap();
    assert_eq!(
        LOG.1.load(Ordering::SeqCst),
        before,
        "reported step/prepare/bind errors must not be logged again by reset"
    );

    let mut reader = options.connect().await.unwrap();
    let mut transaction = reader.begin().await.unwrap();
    let _: i64 = sqlite_access::query_scalar("SELECT count(*) FROM entries")
        .fetch_one(&mut *transaction)
        .await
        .unwrap();
    let error = sqlite_access::query("INSERT INTO entries VALUES (2)")
        .execute(&mut writer)
        .await
        .unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("5")
    );
    writer.ping().await.unwrap();
    assert_eq!(
        LOG.1.load(Ordering::SeqCst),
        before,
        "step BUSY must not be reported again by reset"
    );
    let error = sqlite_access::query("INSERT INTO entries VALUES (2) RETURNING value")
        .fetch_one(&mut writer)
        .await
        .unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("5")
    );
    writer.ping().await.unwrap();
    assert_eq!(
        LOG.1.load(Ordering::SeqCst),
        before,
        "explicit finish reports its reset error to the caller, not again on Drop"
    );
    {
        let mut rows = sqlite_access::query("INSERT INTO entries VALUES (2), (3) RETURNING value")
            .fetch(&mut writer);
        assert!(rows.try_next().await.unwrap().is_some());
        // Dropping before SQLITE_DONE leaves the commit attempt to cleanup.
    }
    tokio::time::timeout(Duration::from_secs(2), writer.ping())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        LOG.1.load(Ordering::SeqCst) - before,
        1,
        "cancelled RETURNING must report its otherwise unobserved reset failure once"
    );
    transaction.rollback().await.unwrap();
    writer.close().await.unwrap();
    reader.close().await.unwrap();
}

#[tokio::test]
#[cfg(feature = "sqlite-pool")]
async fn failed_cancelled_begin_rejects_pending_commands() {
    use std::{future::Future, sync::mpsc, task::Poll};
    let mut connection = sqlite_access::SqliteConnection::connect("sqlite::memory:")
        .await
        .unwrap();
    deny_rollback(&mut connection).await;
    let (entered, started) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let mut first = true;
    connection
        .lock_handle()
        .await
        .unwrap()
        .set_progress_handler(1, move || {
            if first {
                first = false;
                entered.send(()).unwrap();
                released.recv_timeout(Duration::from_secs(2)).unwrap();
            }
            true
        });
    let mut begin = Box::pin(connection.begin());
    std::future::poll_fn(|cx| {
        assert!(begin.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    started.recv_timeout(Duration::from_secs(2)).unwrap();
    drop(begin);
    release.send(()).unwrap();
    for _ in 0..2 {
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), connection.ping())
                .await
                .unwrap(),
            Err(sqlite_access::Error::WorkerCrashed)
        ));
    }
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), connection.close())
            .await
            .unwrap(),
        Err(sqlite_access::Error::WorkerCrashed)
    ));
}

#[tokio::test]
#[cfg(feature = "sqlite-pool")]
async fn cancelled_handle_lock_keeps_worker_usable() {
    use std::{future::Future, task::Poll};
    let mut connection = sqlite_access::SqliteConnection::connect("sqlite::memory:")
        .await
        .unwrap();
    // Poll past the liveness reply to queue UnlockDb behind the fair mutex waiter.
    // Cancelling the request or its just-acquired guard must release both.
    for _ in 0..8 {
        let mut lock = Box::pin(connection.lock_handle());
        let mut polls = 0;
        std::future::poll_fn(|cx| {
            polls += 1;
            match lock.as_mut().poll(cx) {
                Poll::Ready(result) => {
                    drop(result.unwrap());
                    Poll::Ready(())
                }
                Poll::Pending if polls >= 2 => Poll::Ready(()),
                Poll::Pending => Poll::Pending,
            }
        })
        .await;
        drop(lock);
        let handle = tokio::time::timeout(Duration::from_secs(2), connection.lock_handle())
            .await
            .unwrap()
            .unwrap();
        drop(handle);
        tokio::time::timeout(Duration::from_secs(2), connection.ping())
            .await
            .unwrap()
            .unwrap();
    }
    tokio::time::timeout(Duration::from_secs(2), connection.close())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
#[cfg(feature = "sqlite-pool")]
async fn cancelled_nested_begin_preserves_outer_transaction() {
    use std::{future::Future, sync::mpsc, task::Poll};
    let mut connection = sqlite_access::SqliteConnection::connect("sqlite::memory:")
        .await
        .unwrap();
    sqlite_access::query("CREATE TABLE entries (value INTEGER)")
        .execute(&mut connection)
        .await
        .unwrap();
    let mut outer = connection.begin().await.unwrap();
    sqlite_access::query("INSERT INTO entries VALUES (1)")
        .execute(&mut *outer)
        .await
        .unwrap();
    let (entered, started) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let mut first = true;
    outer
        .lock_handle()
        .await
        .unwrap()
        .set_progress_handler(1, move || {
            if first {
                first = false;
                entered.send(()).unwrap();
                released.recv_timeout(Duration::from_secs(2)).unwrap();
            }
            true
        });
    let mut begin = Box::pin(outer.begin());
    std::future::poll_fn(|cx| {
        assert!(begin.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    started.recv_timeout(Duration::from_secs(2)).unwrap();
    drop(begin);
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), outer.commit())
        .await
        .unwrap()
        .unwrap();
    let count: i64 = sqlite_access::query_scalar("SELECT count(*) FROM entries")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert_eq!(
        count, 1,
        "cancelling a savepoint must not roll back its outer transaction"
    );
    connection.close().await.unwrap();
}

#[tokio::test]
#[cfg(feature = "sqlite-pool")]
async fn rejected_nested_begin_preserves_outer_transaction() {
    let mut connection = sqlite_access::SqliteConnection::connect("sqlite::memory:")
        .await
        .unwrap();
    sqlite_access::query("CREATE TABLE entries (value INTEGER)")
        .execute(&mut connection)
        .await
        .unwrap();
    let mut outer = connection.begin().await.unwrap();
    sqlite_access::query("INSERT INTO entries VALUES (1)")
        .execute(&mut *outer)
        .await
        .unwrap();
    assert!(matches!(
        outer.begin_with("BEGIN").await,
        Err(sqlite_access::Error::InvalidSavePointStatement)
    ));
    outer.commit().await.unwrap();
    let count: i64 = sqlite_access::query_scalar("SELECT count(*) FROM entries")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert_eq!(
        count, 1,
        "a rejected savepoint must not roll back its outer transaction"
    );
    connection.close().await.unwrap();
}

#[tokio::test]
#[cfg(feature = "sqlite-pool")]
async fn cancelled_unqueued_begin_does_not_rollback_outer_transaction() {
    use sqlite_access::ConnectOptions;
    use std::{future::Future, sync::mpsc, task::Poll};
    let mut connection = sqlite_access::sqlite::SqliteConnectOptions::new()
        .in_memory(true)
        .command_buffer_size(0)
        .connect()
        .await
        .unwrap();
    sqlite_access::query("CREATE TABLE entries (value INTEGER)")
        .execute(&mut connection)
        .await
        .unwrap();
    let mut outer = connection.begin().await.unwrap();
    sqlite_access::query("INSERT INTO entries VALUES (1)")
        .execute(&mut *outer)
        .await
        .unwrap();
    let (entered, started) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let mut first = true;
    outer
        .lock_handle()
        .await
        .unwrap()
        .set_progress_handler(1, move || {
            if first {
                first = false;
                entered.send(()).unwrap();
                released.recv_timeout(Duration::from_secs(5)).unwrap();
            }
            true
        });
    let mut query = Box::pin(sqlite_access::query("SELECT 1").fetch_one(&mut *outer));
    std::future::poll_fn(|cx| {
        assert!(query.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    started.recv_timeout(Duration::from_secs(2)).unwrap();
    drop(query);
    let mut begin = Box::pin(outer.begin());
    std::future::poll_fn(|cx| {
        assert!(begin.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    // The worker cannot receive Begin yet. Cancellation must not enqueue a
    // blocking rollback; release the worker even if that regression returns.
    let (dropped, drop_finished) = mpsc::channel();
    let release_worker = std::thread::spawn(move || {
        let cancelled_before_release = drop_finished.recv_timeout(Duration::from_secs(2)).is_ok();
        release.send(()).unwrap();
        cancelled_before_release
    });
    drop(begin);
    let notified = dropped.send(()).is_ok();
    let cancelled_before_release = release_worker.join().unwrap();
    tokio::time::timeout(Duration::from_secs(2), outer.commit())
        .await
        .unwrap()
        .unwrap();
    let count: i64 = sqlite_access::query_scalar("SELECT count(*) FROM entries")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert_eq!((notified && cancelled_before_release, count), (true, 1));
    connection.close().await.unwrap();
}
