use crate::{sqlite::SqliteConnectOptions, Connection, Error, SqliteConnection};
use futures::executor::block_on;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn collation_nul_options_releases_capture() -> TestResult {
    block_on(async {
        let capture = std::sync::Arc::new(());
        let weak = std::sync::Arc::downgrade(&capture);
        let options = SqliteConnectOptions::new().collation("bad\0name", move |left, right| {
            let _keep_alive = &capture;
            left.cmp(right)
        });
        let result = SqliteConnection::connect_with(&options).await;
        assert!(matches!(result, Err(Error::Protocol(_))));
        drop(options);
        assert!(
            weak.upgrade().is_none(),
            "failed registration leaked its capture"
        );
        Ok(())
    })
}

#[test]
fn collation_nul_locked_handle_releases_capture() -> TestResult {
    block_on(async {
        let mut connection = SqliteConnection::connect("sqlite::memory:").await?;
        let capture = std::sync::Arc::new(());
        let weak = std::sync::Arc::downgrade(&capture);
        let result =
            connection
                .lock_handle()
                .await?
                .create_collation("bad\0name", move |left, right| {
                    let _keep_alive = &capture;
                    left.cmp(right)
                });
        assert!(matches!(result, Err(Error::Protocol(_))));
        assert!(
            weak.upgrade().is_none(),
            "failed registration leaked its capture"
        );
        connection.close().await?;
        Ok(())
    })
}

#[test]
fn collation_valid_names_sort_and_release_captures() -> TestResult {
    block_on(async {
        for from_options in [true, false] {
            let capture = std::sync::Arc::new(());
            let weak = std::sync::Arc::downgrade(&capture);
            let compare = move |left: &str, right: &str| {
                let _keep_alive = &capture;
                right.cmp(left)
            };
            let mut connection = if from_options {
                let options = SqliteConnectOptions::new().collation("reverse", compare);
                SqliteConnection::connect_with(&options).await?
            } else {
                let mut connection = SqliteConnection::connect("sqlite::memory:").await?;
                connection
                    .lock_handle()
                    .await?
                    .create_collation("reverse", compare)?;
                connection
            };
            assert!(weak.upgrade().is_some());
            let rows: Vec<String> = crate::query_scalar(
                "SELECT column1 FROM (VALUES ('a'), ('c'), ('b')) ORDER BY column1 COLLATE reverse",
            )
            .fetch_all(&mut connection)
            .await?;
            assert_eq!(rows, ["c", "b", "a"]);
            connection.close().await?;
            assert!(
                weak.upgrade().is_none(),
                "connection close leaked its capture"
            );
        }
        Ok(())
    })
}

#[test]
fn collation_sqlite_busy_releases_rejected_captures() -> TestResult {
    use super::Collation;
    use libsqlite3_sys::{
        sqlite3_finalize, sqlite3_prepare_v2, sqlite3_step, SQLITE_OK, SQLITE_ROW,
    };

    block_on(async {
        let mut connection = SqliteConnection::connect("sqlite::memory:").await?;
        {
            let mut locked = connection.lock_handle().await?;
            locked.create_collation("busy", str::cmp)?;
            let mut statement = std::ptr::null_mut();
            // SAFETY: the locked handle remains live and exclusive until the prepared
            // statement is finalized below; SQL is a valid NUL-terminated literal.
            unsafe {
                assert_eq!(
                    sqlite3_prepare_v2(
                        locked.as_raw_handle().as_ptr(),
                        c"SELECT 'a' UNION ALL SELECT 'b'".as_ptr(),
                        -1,
                        &mut statement,
                        std::ptr::null_mut()
                    ),
                    SQLITE_OK
                );
                assert_eq!(sqlite3_step(statement), SQLITE_ROW);
            }
            // Keep the active statement alive across both registrations so SQLite
            // rejects replacing the existing collation with SQLITE_BUSY.
            let mut results = Vec::new();
            for from_options in [true, false] {
                let capture = std::sync::Arc::new(());
                let weak = std::sync::Arc::downgrade(&capture);
                let compare = move |left: &str, right: &str| {
                    let _keep_alive = &capture;
                    left.cmp(right)
                };
                let result = if from_options {
                    let collation = Collation::new("busy", compare);
                    collation.create(&mut locked.guard.handle)
                } else {
                    locked.create_collation("busy", compare)
                };
                results.push((result, weak));
            }
            // SAFETY: statement came from the successful prepare above and has
            // not been finalized; the locked connection still owns its database.
            assert_eq!(unsafe { sqlite3_finalize(statement) }, SQLITE_OK);
            for (result, weak) in results {
                let Error::Database(error) = result.unwrap_err() else {
                    panic!("expected SQLite registration error");
                };
                assert_eq!(error.code().as_deref(), Some("5"));
                assert!(
                    weak.upgrade().is_none(),
                    "SQLite rejection leaked its capture"
                );
            }
        }
        connection.close().await?;
        Ok(())
    })
}
