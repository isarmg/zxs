// Repository-external public API draft; not compiled or executed by this reviewer.
// Original six cases are retained; seven additional cases below remain unexecuted.
// Root must bind native Linux + tokio(rt,fs,io-util) + tempfile and run thirteen tests.
// A failed readonly-fsync setup assertion is not a reproduced File sync bug.
#[cfg(not(target_os = "linux"))]
compile_error!("this readonly regular-file fault fixture requires native Linux");

mod file_sync_error_regressions {
    use std::future::Future;
    use std::io::{self, SeekFrom, Write as _};
    use std::path::PathBuf;
    use tokio::fs::File;
    use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

    const ORIGINAL: &[u8] = b"readable original bytes";
    const REJECTED: &[u8] = b"must not be written";

    struct ReadonlyFixture {
        _directory: tempfile::TempDir,
        file: File,
        expected_write_error: io::Error,
    }

    fn readonly_fixture() -> ReadonlyFixture {
        let directory =
            tempfile::tempdir().expect("fixture setup: create an owned temporary directory");
        let path = directory.path().join("readonly-source");
        std::fs::write(&path, ORIGINAL)
            .expect("invariant: fixture owns this fresh temporary ordinary file");
        let source = std::fs::OpenOptions::new()
            .read(true)
            .open(&path)
            .expect("invariant: fixture just created a readable ordinary file");

        // This only changes descriptor access mode. There is no chmod, umask,
        // system/production permission change, seal, device or raw-fd operation.
        let mut control = source
            .try_clone()
            .expect("invariant: fixture may duplicate its own live descriptor");
        let expected_write_error = control
            .write_all(REJECTED)
            .expect_err("a read-only descriptor cannot accept these bytes");
        assert!(
            expected_write_error.raw_os_error().is_some(),
            "fixture setup must produce a real OS write error",
        );

        // A successful OS sync isolates the earlier buffered write failure.
        control.sync_all().expect(
            "fixture setup: readonly regular-file sync_all must succeed on this filesystem",
        );
        control.sync_data().expect(
            "fixture setup: readonly regular-file sync_data must succeed on this filesystem",
        );

        ReadonlyFixture {
            _directory: directory,
            file: File::from_std(source),
            expected_write_error,
        }
    }

    fn run(future: impl Future<Output = ()>) {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("fixture setup: build the single-thread runtime")
            .block_on(future);
    }

    fn assert_original_error(actual: &io::Error, expected: &io::Error) {
        assert_eq!(actual.kind(), expected.kind());
        assert_eq!(
            actual.raw_os_error(),
            expected.raw_os_error(),
            "an ErrorKind reconstruction must not replace the original OS error",
        );
        assert_eq!(actual.to_string(), expected.to_string());
    }

    async fn queue_failed_write(file: &mut File) {
        // This pins the current buffering transition: a small write is copied
        // and accepted before File polls its mandatory blocking task's result.
        // The backing fd remains read-only, so that task will return the same
        // OS failure observed by the independent std control above.
        file.write_all(REJECTED)
            .await
            .expect("invariant: current File accepts this single small buffered write first");
    }

    #[test]
    fn sync_all_reports_a_preceding_buffered_write_failure() {
        let mut fixture = readonly_fixture();
        run(async {
            queue_failed_write(&mut fixture.file).await;
            let error = fixture
                .file
                .sync_all()
                .await
                .expect_err("successful OS sync cannot erase a preceding write failure");
            assert_original_error(&error, &fixture.expected_write_error);
        });
    }

    #[test]
    fn sync_data_reports_a_preceding_buffered_write_failure() {
        let mut fixture = readonly_fixture();
        run(async {
            queue_failed_write(&mut fixture.file).await;
            let error = fixture
                .file
                .sync_data()
                .await
                .expect_err("successful OS sync cannot erase a preceding write failure");
            assert_original_error(&error, &fixture.expected_write_error);
        });
    }

    #[test]
    fn read_observed_write_failure_keeps_its_original_os_error_for_flush() {
        let mut fixture = readonly_fixture();
        run(async {
            queue_failed_write(&mut fixture.file).await;
            let mut first = [0; 1];
            assert_eq!(fixture.file.read(&mut first).await.unwrap(), 1);
            assert_eq!(first[0], ORIGINAL[0]);
            let error = fixture
                .file
                .flush()
                .await
                .expect_err("read must defer, rather than erase, the failed write");
            assert_original_error(&error, &fixture.expected_write_error);
        });
    }

    #[test]
    fn read_observed_write_failure_is_not_hidden_by_sync_all() {
        let mut fixture = readonly_fixture();
        run(async {
            queue_failed_write(&mut fixture.file).await;
            let mut first = [0; 1];
            assert_eq!(fixture.file.read(&mut first).await.unwrap(), 1);
            let error = fixture
                .file
                .sync_all()
                .await
                .expect_err("sync_all must report the deferred write failure");
            assert_original_error(&error, &fixture.expected_write_error);
        });
    }

    #[test]
    fn seek_observed_write_failure_keeps_its_original_os_error_for_flush() {
        let mut fixture = readonly_fixture();
        run(async {
            queue_failed_write(&mut fixture.file).await;
            assert_eq!(fixture.file.seek(SeekFrom::Start(0)).await.unwrap(), 0);
            let error = fixture
                .file
                .flush()
                .await
                .expect_err("seek must defer, rather than erase, the failed write");
            assert_original_error(&error, &fixture.expected_write_error);
        });
    }

    #[test]
    fn successful_writes_still_sync_and_reopen_with_exact_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let path: PathBuf = directory.path().join("successful-target");
        let source = std::fs::File::create(&path).unwrap();
        let mut file = File::from_std(source);
        run(async {
            file.write_all(ORIGINAL).await.unwrap();
            file.sync_data().await.unwrap();
            file.sync_all().await.unwrap();
        });
        drop(file);
        assert_eq!(std::fs::read(&path).unwrap(), ORIGINAL);
    }

    #[test]
    fn read_observed_write_failure_is_not_hidden_by_sync_data() {
        let mut fixture = readonly_fixture();
        run(async {
            queue_failed_write(&mut fixture.file).await;
            let mut first = [0; 1];
            assert_eq!(fixture.file.read(&mut first).await.unwrap(), 1);
            assert_eq!(first[0], ORIGINAL[0]);
            let error = fixture
                .file
                .sync_data()
                .await
                .expect_err("sync_data must report the deferred write failure");
            assert_original_error(&error, &fixture.expected_write_error);
        });
    }

    #[test]
    fn read_deferred_write_error_is_returned_once_by_write() {
        let mut fixture = readonly_fixture();
        run(async {
            queue_failed_write(&mut fixture.file).await;
            let mut first = [0; 1];
            assert_eq!(fixture.file.read(&mut first).await.unwrap(), 1);
            let error = fixture
                .file
                .write(b"a later write")
                .await
                .expect_err("write must return the previously deferred error first");
            assert_original_error(&error, &fixture.expected_write_error);
            fixture
                .file
                .flush()
                .await
                .expect("the deferred error was consumed exactly once");
        });
    }

    #[test]
    fn seek_deferred_write_error_is_returned_once_by_write_vectored() {
        let mut fixture = readonly_fixture();
        run(async {
            queue_failed_write(&mut fixture.file).await;
            assert_eq!(fixture.file.seek(SeekFrom::Start(0)).await.unwrap(), 0);
            let slices = [io::IoSlice::new(b"later"), io::IoSlice::new(b" bytes")];
            let error = fixture
                .file
                .write_vectored(&slices)
                .await
                .expect_err("vectored write must return the previously deferred error first");
            assert_original_error(&error, &fixture.expected_write_error);
            fixture
                .file
                .flush()
                .await
                .expect("the deferred error was consumed exactly once");
        });
    }

    #[test]
    fn sync_consumes_the_reported_write_error_only_once() {
        let mut fixture = readonly_fixture();
        run(async {
            queue_failed_write(&mut fixture.file).await;
            let error = fixture
                .file
                .sync_all()
                .await
                .expect_err("the first sync must report the failed write");
            assert_original_error(&error, &fixture.expected_write_error);
            fixture.file.flush().await.unwrap();
            fixture.file.sync_data().await.unwrap();
        });
        assert_eq!(
            std::fs::read(fixture._directory.path().join("readonly-source")).unwrap(),
            ORIGINAL,
        );
    }

    #[test]
    fn try_clone_keeps_the_original_deferred_write_error_on_the_original_file() {
        let mut fixture = readonly_fixture();
        run(async {
            queue_failed_write(&mut fixture.file).await;
            let cloned = fixture.file.try_clone().await.unwrap();
            drop(cloned);
            let error =
                fixture.file.flush().await.expect_err(
                    "unit-returning completion must preserve the original deferred error",
                );
            assert_original_error(&error, &fixture.expected_write_error);
            fixture.file.flush().await.unwrap();
        });
    }

    #[derive(Clone, Copy)]
    enum SyncKind {
        All,
        Data,
    }

    async fn perform_sync(kind: SyncKind, file: &File) -> io::Result<()> {
        match kind {
            SyncKind::All => file.sync_all().await,
            SyncKind::Data => file.sync_data().await,
        }
    }

    fn cancelled_sync_keeps_the_pending_write(kind: SyncKind) {
        let mut fixture = readonly_fixture();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .max_blocking_threads(1)
            .build()
            .expect("invariant: fixture enables rt and one blocking worker");
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let blocker = runtime.spawn_blocking(move || {
            entered_tx
                .send(())
                .expect("fixture setup: test awaits worker entry");
            // Sender drop also releases the worker when the test unwinds.
            let _ = release_rx.recv();
        });
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("fixture setup: the sole blocking worker must acknowledge occupation");

        runtime.block_on(async {
            queue_failed_write(&mut fixture.file).await;
            let mut cancelled = Box::pin(perform_sync(kind, &fixture.file));
            std::future::poll_fn(|cx| {
                assert!(
                    cancelled.as_mut().poll(cx).is_pending(),
                    "the acknowledged blocker prevents the queued write from completing",
                );
                std::task::Poll::Ready(())
            })
            .await;
            drop(cancelled);

            release_tx
                .send(())
                .expect("fixture owns the worker release channel");
            blocker.await.expect("fixture worker did not panic");
            let error = perform_sync(kind, &fixture.file)
                .await
                .expect_err("dropping the first sync cannot detach or erase the queued write");
            assert_original_error(&error, &fixture.expected_write_error);
            fixture.file.flush().await.unwrap();
        });
    }

    #[test]
    fn cancelled_sync_all_keeps_the_pending_write_error() {
        cancelled_sync_keeps_the_pending_write(SyncKind::All);
    }

    #[test]
    fn cancelled_sync_data_keeps_the_pending_write_error() {
        cancelled_sync_keeps_the_pending_write(SyncKind::Data);
    }
}
