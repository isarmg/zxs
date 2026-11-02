//! Direct-worker contracts run with no pool, Tokio or general URL parser.
use crate::{sqlite::SqliteConnectOptions, Connection, Error, SqliteConnection};
use futures::executor::block_on;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct Directory(PathBuf);
impl Directory {
    fn new() -> std::io::Result<Self> {
        static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::current_dir()?.join(format!(
            ".sqlite-direct-test-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path)?;
        Ok(Self(path))
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn direct_memory_transactions_savepoints_rollback_and_errors() -> TestResult {
    block_on(async {
        let mut connection = SqliteConnection::connect_with(
            &SqliteConnectOptions::new().statement_cache_capacity(2),
        )
        .await?;
        crate::query("CREATE TABLE entries(value INTEGER UNIQUE)")
            .execute(&mut connection)
            .await?;
        let mut transaction = connection.begin().await?;
        crate::query("INSERT INTO entries VALUES(1)")
            .execute(&mut *transaction)
            .await?;
        let mut nested = transaction.begin().await?;
        crate::query("INSERT INTO entries VALUES(2)")
            .execute(&mut *nested)
            .await?;
        nested.rollback().await?;
        transaction.commit().await?;
        let mut dropped = connection.begin().await?;
        crate::query("INSERT INTO entries VALUES(3)")
            .execute(&mut *dropped)
            .await?;
        drop(dropped);
        assert_eq!(
            crate::query_scalar::<_, i64>("SELECT value FROM entries")
                .fetch_all(&mut connection)
                .await?,
            [1]
        );
        assert!(crate::query("INSERT INTO entries VALUES(1)")
            .execute(&mut connection)
            .await
            .is_err());
        // A statement error does not poison the native connection or its channels.
        assert_eq!(
            crate::query_scalar::<_, i64>("SELECT 7")
                .fetch_one(&mut connection)
                .await?,
            7
        );
        assert!(connection.cached_statements_size() <= 2);
        connection.clear_cached_statements().await?;
        assert_eq!(connection.cached_statements_size(), 0);
        connection.close().await?;
        Ok(())
    })
}

#[test]
fn filename_absolute_relative_unicode_percent_and_query_characters_are_literal() -> TestResult {
    let directory = Directory::new()?;
    block_on(async {
        for relative in [false, true] {
            // Windows forbids a literal question mark in a filename. The parser-only
            // contract still covers that character on every platform.
            let prefix = if relative { "relative" } else { "absolute" };
            let suffix = if cfg!(windows) {
                "#%25 Ω.sqlite"
            } else {
                "?#%25 Ω.sqlite"
            };
            let absolute = directory.0.join(format!("{prefix} {suffix}"));
            let filename = if relative {
                absolute.strip_prefix(std::env::current_dir()?)?.to_owned()
            } else {
                absolute.clone()
            };
            let options = SqliteConnectOptions::new()
                .filename(&filename)
                .create_if_missing(true);
            let mut connection = SqliteConnection::connect_with(&options).await?;
            crate::query("CREATE TABLE retained(value TEXT)")
                .execute(&mut connection)
                .await?;
            crate::query("INSERT INTO retained VALUES('持久 Ω')")
                .execute(&mut connection)
                .await?;
            connection.close().await?;
            assert!(absolute.is_file());
            // immutable forces SQLite URI serialization, preserving literal filename bytes.
            let options = SqliteConnectOptions::new()
                .filename(&filename)
                .read_only(true)
                .immutable(true);
            let mut reopened = SqliteConnection::connect_with(&options).await?;
            assert_eq!(
                crate::query_scalar::<_, String>("SELECT value FROM retained")
                    .fetch_one(&mut reopened)
                    .await?,
                "持久 Ω"
            );
            assert!(crate::query("DELETE FROM retained")
                .execute(&mut reopened)
                .await
                .is_err());
            reopened.close().await?;
        }
        Ok(())
    })
}

#[test]
fn missing_nul_and_non_utf8_filenames_fail_without_creating_files() -> TestResult {
    let directory = Directory::new()?;
    block_on(async {
        let missing = directory.0.join("missing.sqlite");
        assert!(
            SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&missing))
                .await
                .is_err()
        );
        assert!(!missing.exists());
        let nul = SqliteConnectOptions::new()
            .filename("nul\0.sqlite")
            .create_if_missing(true);
        assert!(
            matches!(SqliteConnection::connect_with(&nul).await, Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::InvalidData)
        );
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let non_utf8 = directory.0.join(std::ffi::OsString::from_vec(
                b"non-utf8-\xff.sqlite".to_vec(),
            ));
            let options = SqliteConnectOptions::new()
                .filename(&non_utf8)
                .create_if_missing(true);
            assert!(
                matches!(SqliteConnection::connect_with(&options).await, Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::InvalidData)
            );
            assert!(!non_utf8.exists());
        }
        Ok(())
    })
}

#[test]
fn parsed_memory_options_share_only_the_selected_database() -> TestResult {
    block_on(async {
        let options: SqliteConnectOptions = "sqlite::memory:?cache=shared".parse()?;
        let mut writer = SqliteConnection::connect_with(&options).await?;
        crate::query("CREATE TABLE entries(value INTEGER)")
            .execute(&mut writer)
            .await?;
        crate::query("INSERT INTO entries VALUES(9)")
            .execute(&mut writer)
            .await?;
        let mut reader = SqliteConnection::connect_with(&options).await?;
        assert_eq!(
            crate::query_scalar::<_, i64>("SELECT value FROM entries")
                .fetch_one(&mut reader)
                .await?,
            9
        );
        let mut separate = SqliteConnection::connect("sqlite::memory:").await?;
        assert!(crate::query("SELECT value FROM entries")
            .fetch_one(&mut separate)
            .await
            .is_err());
        separate.close().await?;
        reader.close().await?;
        writer.close().await?;
        Ok(())
    })
}

#[test]
fn parsed_percent_filename_and_query_options_reopen_readonly() -> TestResult {
    let directory = Directory::new()?;
    block_on(async {
        let path = directory.0.join(if cfg!(windows) {
            "parsed #% Ω.sqlite"
        } else {
            "parsed ?#% Ω.sqlite"
        });
        let encoded = url_encoding::percent::percent_encode(
            path.to_str().unwrap().as_bytes(),
            url_encoding::percent::NON_ALPHANUMERIC,
        );
        let options: SqliteConnectOptions =
            format!("sqlite://{encoded}?mode=rwc&cache=private").parse()?;
        let mut writer = SqliteConnection::connect_with(&options).await?;
        crate::query("CREATE TABLE retained(value INTEGER)")
            .execute(&mut writer)
            .await?;
        writer.close().await?;
        assert!(path.is_file());
        let options: SqliteConnectOptions =
            format!("sqlite:{encoded}?mode=ro&immutable=1").parse()?;
        let mut reader = SqliteConnection::connect_with(&options).await?;
        assert_eq!(
            crate::query_scalar::<_, i64>("SELECT count(*) FROM retained")
                .fetch_one(&mut reader)
                .await?,
            0
        );
        assert!(crate::query("INSERT INTO retained VALUES(1)")
            .execute(&mut reader)
            .await
            .is_err());
        reader.close().await?;
        Ok(())
    })
}

#[test]
fn ordinal_parameters_preserve_repetition_order_and_missing_values() -> TestResult {
    block_on(async {
        let mut connection = SqliteConnection::connect_with(&SqliteConnectOptions::new()).await?;
        for sql in ["SELECT $2, $1, $2, $01", "SELECT ?2, ?1, ?2, ?01"] {
            let row: (i64, i64, i64, i64) = crate::query_as(sql)
                .bind(11_i64)
                .bind(22_i64)
                .fetch_one(&mut connection)
                .await?;
            assert_eq!(row, (22, 11, 22, 11), "{sql}");
        }
        let row: (Option<i64>, i64) = crate::query_as("SELECT $2, $1")
            .bind(11_i64)
            .fetch_one(&mut connection)
            .await?;
        assert_eq!(row, (None, 11));
        let row: (i64, i64) = crate::query_as("SELECT ?, ?")
            .bind(11_i64)
            .bind(22_i64)
            .fetch_one(&mut connection)
            .await?;
        assert_eq!(row, (11, 22));
        connection.close().await?;
        Ok(())
    })
}

#[test]
fn invalid_ordinal_parameters_return_errors_without_poisoning_connection() -> TestResult {
    block_on(async {
        let mut connection = SqliteConnection::connect_with(&SqliteConnectOptions::new()).await?;
        // These are valid SQLite names, but not supported positive decimal ordinals.
        // Check twice to exercise statement reuse after a binding failure.
        for _ in 0..2 {
            for sql in [
                "SELECT $0",
                "SELECT $00",
                "SELECT $1junk",
                "SELECT $1::suffix",
                "SELECT $1(suffix)",
                "SELECT $18446744073709551616",
                "SELECT $name",
                "SELECT :1",
                "SELECT @1",
                "SELECT $2, $0",
            ] {
                let result = crate::query(sql)
                    .bind(11_i64)
                    .fetch_one(&mut connection)
                    .await;
                assert!(
                    matches!(result, Err(Error::Protocol(_))),
                    "{sql}: {result:?}"
                );
                let value: i64 = crate::query_scalar("SELECT $1")
                    .bind(42_i64)
                    .fetch_one(&mut connection)
                    .await?;
                assert_eq!(value, 42);
            }
        }
        connection.close().await?;
        Ok(())
    })
}

#[test]
fn busy_timeout_is_checked_before_creating_the_database() -> TestResult {
    use std::time::Duration;

    let directory = Directory::new()?;
    let filename = directory.0.join("invalid-timeout.sqlite");
    block_on(async {
        for timeout in [
            Duration::from_millis(i32::MAX as u64 + 1),
            Duration::MAX,
        ] {
            let options = SqliteConnectOptions::new()
                .filename(&filename)
                .create_if_missing(true)
                .busy_timeout(timeout);
            assert!(matches!(
                SqliteConnection::connect_with(&options).await,
                Err(Error::InvalidArgument(_))
            ));
            assert!(!filename.exists());
        }
        for timeout in [Duration::ZERO, Duration::from_millis(i32::MAX as u64)] {
            SqliteConnection::connect_with(&SqliteConnectOptions::new().busy_timeout(timeout))
                .await?
                .close()
                .await?;
        }
        Ok(())
    })
}

#[cfg(feature = "sqlite-preupdate-hook")]
#[test]
fn preupdate_hook_resources_are_released_on_replace_remove_and_close() -> TestResult {
    use std::sync::Arc;

    struct DropCount(Arc<AtomicUsize>);
    impl Drop for DropCount {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    block_on(async {
        let drops = Arc::new(AtomicUsize::new(0));
        let calls = Arc::new(AtomicUsize::new(0));
        let mut connection = SqliteConnection::connect_with(&SqliteConnectOptions::new()).await?;
        {
            let mut handle = connection.lock_handle().await?;
            for expected in 0..2 {
                let resource = DropCount(Arc::clone(&drops));
                handle.set_preupdate_hook(move |_| {
                    let _ = &resource;
                });
                assert_eq!(drops.load(Ordering::Relaxed), expected);
            }
            handle.remove_preupdate_hook();
            assert_eq!(drops.load(Ordering::Relaxed), 2);

            let resource = DropCount(Arc::clone(&drops));
            let calls = Arc::clone(&calls);
            handle.set_preupdate_hook(move |_| {
                let _ = &resource;
                calls.fetch_add(1, Ordering::Relaxed);
            });
        }
        crate::query("CREATE TABLE entries(value INTEGER)")
            .execute(&mut connection)
            .await?;
        crate::query("INSERT INTO entries VALUES(1)")
            .execute(&mut connection)
            .await?;
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(drops.load(Ordering::Relaxed), 2);
        connection.close().await?;
        assert_eq!(drops.load(Ordering::Relaxed), 3);
        Ok(())
    })
}

#[test]
fn owned_rows_remain_send_without_shared_decoding() -> TestResult {
    use crate::{sqlite::SqliteRow, Row};

    // This is ambiguous and fails to compile if SqliteRow ever implements Sync.
    trait AmbiguousIfSync<A> {
        fn check() {}
    }
    impl<T: ?Sized> AmbiguousIfSync<()> for T {}
    impl<T: ?Sized + Sync> AmbiguousIfSync<u8> for T {}
    let _ = <SqliteRow as AmbiguousIfSync<_>>::check;

    let row = block_on(async {
        let mut connection = SqliteConnection::connect_with(&SqliteConnectOptions::new()).await?;
        let row = crate::query("SELECT 'retained text', 42")
            .fetch_one(&mut connection)
            .await?;
        connection.close().await?;
        Ok::<_, Error>(row)
    })?;
    let (text, number) = std::thread::spawn(move || {
        let text: &str = row.try_get(0)?;
        let number: i64 = row.try_get(1)?;
        Ok::<_, Error>((text.to_owned(), number))
    })
    .join()
    .expect("row decoding thread panicked")?;
    assert_eq!(text, "retained text");
    assert_eq!(number, 42);
    Ok(())
}
