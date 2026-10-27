use super::*;
use std::{
    future::{pending, poll_fn, ready},
    sync::mpsc,
    task::Poll,
    time::{Duration, Instant},
};
use storage_engine::{Config, ErrorCode, IntegrityScrubCompletion};

fn fixture() -> (tempfile::TempDir, PathBuf, Engine) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("system");
    let disk = directory.path().join("disk");
    let engine = Engine::initialize(&root, Config::development([disk])).unwrap();
    (directory, root, engine)
}

fn controlled_job(mut engine: Engine) -> (ScrubJob, mpsc::Sender<()>) {
    let cancellation = Arc::new(IntegrityScrubCancellation::new());
    let worker_cancellation = Arc::clone(&cancellation);
    let completed = Arc::new(Notify::new());
    let completion = Completion(Arc::clone(&completed));
    let (release, released) = mpsc::channel();
    let worker = thread::spawn(move || {
        let _completion = completion;
        released.recv().unwrap();
        engine.scrub(&worker_cancellation)
    });
    (
        ScrubJob {
            worker: Some(worker),
            cancellation,
            completed,
        },
        release,
    )
}

fn assert_owned(root: &std::path::Path) {
    assert_eq!(
        Engine::open(root).err().unwrap().code(),
        ErrorCode::WriterAlreadyActive
    );
}

#[test]
fn cancellation_does_not_release_the_engine_before_the_worker_finishes() {
    let (_directory, root, engine) = fixture();
    let (job, release) = controlled_job(engine);
    let cancellation = Arc::clone(&job.cancellation);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let report = runtime.block_on(async {
        let future = job.finish_when(ready(()));
        tokio::pin!(future);
        poll_fn(|cx| {
            assert!(future.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        assert!(cancellation.is_cancelled());
        assert_owned(&root);
        release.send(()).unwrap();
        future.await.unwrap()
    });
    assert!(matches!(
        report.completion,
        IntegrityScrubCompletion::Cancelled
    ));
    assert!(!report.is_healthy());
    Engine::open(&root).unwrap();
}

#[test]
fn dropping_the_caller_cancels_and_joins_before_releasing_ownership() {
    let (_directory, root, engine) = fixture();
    let (job, release) = controlled_job(engine);
    let cancellation = Arc::clone(&job.cancellation);
    let observed_root = root.clone();
    let observer = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !cancellation.is_cancelled() {
            assert!(
                Instant::now() < deadline,
                "caller drop must request cancellation"
            );
            thread::yield_now();
        }
        assert_owned(&observed_root);
        release.send(()).unwrap();
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    runtime.block_on(async {
        let future = job.finish_when(pending());
        tokio::pin!(future);
        poll_fn(|cx| {
            assert!(future.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        // The scope drops a started, pending caller, not merely an unused future.
    });
    observer.join().unwrap();
    Engine::open(&root).unwrap();
}

#[test]
fn worker_panic_notifies_waiter_and_returns_an_explicit_failure() {
    let cancellation = Arc::new(IntegrityScrubCancellation::new());
    let completed = Arc::new(Notify::new());
    let completion = Completion(Arc::clone(&completed));
    let worker = thread::spawn(move || {
        let _completion = completion;
        panic!("scrub panic fixture");
    });
    let job = ScrubJob {
        worker: Some(worker),
        cancellation,
        completed,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let error = runtime.block_on(job.finish_when(pending())).unwrap_err();
    assert!(error.downcast_ref::<WorkerPanic>().is_some());
    assert!(error.to_string().contains("scrub panic fixture"));
}

#[test]
fn actual_worker_rejects_an_active_owner_and_can_run_after_it_drains() {
    let (_directory, root, engine) = fixture();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let error = runtime
        .block_on(
            ScrubJob::start(root.clone(), None)
                .unwrap()
                .finish_when(pending()),
        )
        .unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<storage_engine::Error>()
            .unwrap()
            .code(),
        ErrorCode::WriterAlreadyActive
    );
    drop(engine);
    let report = runtime
        .block_on(
            ScrubJob::start(root.clone(), None)
                .unwrap()
                .finish_when(pending()),
        )
        .unwrap();
    assert!(report.is_healthy());
    Engine::open(&root).unwrap();
}

#[test]
fn json_outcome_never_promotes_partial_cancelled_or_failed_reports_to_success() {
    let (_directory, _root, mut engine) = fixture();
    let mut report = engine.scrub(&IntegrityScrubCancellation::new()).unwrap();
    let json = json_codec::to_value(wire::Report::from(&report)).unwrap();
    assert_eq!(json["state"], "complete_success");
    assert_eq!(json["last_complete_success_at"], json["finished_at"]);
    report.metadata_failures = 1;
    let json = json_codec::to_value(wire::Report::from(&report)).unwrap();
    assert_eq!(json["state"], "partial_failure");
    assert!(json["last_complete_success_at"].is_null());
    for (completion, state) in [
        (IntegrityScrubCompletion::Running, "running"),
        (IntegrityScrubCompletion::Superseded, "expired"),
    ] {
        report.completion = completion;
        let json = json_codec::to_value(wire::Report::from(&report)).unwrap();
        assert_eq!(json["state"], state);
        assert!(json["last_complete_success_at"].is_null());
    }
    report.completion = IntegrityScrubCompletion::Cancelled;
    let json = json_codec::to_value(wire::Report::from(&report)).unwrap();
    assert_eq!(json["state"], "cancelled");
    assert!(json["last_complete_success_at"].is_null());
    report.completion = IntegrityScrubCompletion::Failed(storage_engine::Error::new(
        ErrorCode::IoError,
        "scan failure",
    ));
    let json = json_codec::to_value(wire::Report::from(&report)).unwrap();
    assert_eq!(json["state"], "failed");
    assert_eq!(json["failure"]["code"], "IO_ERROR");
    assert_eq!(json["failure"]["message"], "scan failure");
    assert!(json["last_complete_success_at"].is_null());
}

const SIGNAL_CHILD: &str = "ZXS_TEST_SCRUB_SIGNAL_CHILD";
const SIGNAL_READY: &str = "ZXS_TEST_SCRUB_SIGNALS_INSTALLED";

// Only the unit-test executable recognizes this helper. Production receives
// actual OS signals and has no test environment variable or I/O barrier.
#[test]
#[ignore = "invoked by actual_signals_cancel_and_drain_the_native_worker"]
fn signal_child() {
    use std::io::{Read, Write};
    assert_eq!(std::env::var(SIGNAL_CHILD).unwrap(), "1");
    let (_directory, root, engine) = fixture();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut signals = ScrubSignals::install().unwrap();
        let (job, release) = controlled_job(engine);
        let cancellation = Arc::clone(&job.cancellation);
        let observed_root = root.clone();
        let observer = thread::spawn(move || {
            let mut byte = [0];
            std::io::stdin().read_exact(&mut byte).unwrap();
            assert_eq!(byte, [b'R']);
            let deadline = Instant::now() + Duration::from_secs(10);
            while !cancellation.is_cancelled() {
                assert!(
                    Instant::now() < deadline,
                    "signal must reach the cancellation token"
                );
                thread::yield_now();
            }
            assert_owned(&observed_root);
            release.send(()).unwrap();
        });
        {
            let mut output = std::io::stdout().lock();
            writeln!(output, "{SIGNAL_READY}").unwrap();
            output.flush().unwrap();
        }
        let report = tokio::time::timeout(Duration::from_secs(10), job.finish_when(signals.wait()))
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            report.completion,
            IntegrityScrubCompletion::Cancelled
        ));
        observer.join().unwrap();
        Engine::open(&root).unwrap();
    });
}

#[test]
fn actual_signals_cancel_and_drain_the_native_worker() {
    use rustix::process::{Pid, Signal, kill_process};
    use std::{
        io::{BufRead, Write},
        process::{Child, Command, Stdio},
    };
    struct ChildGuard(Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            if !matches!(self.0.try_wait(), Ok(Some(_))) {
                let _ = self.0.kill();
            }
            let _ = self.0.wait();
        }
    }
    for signal in [Signal::INT, Signal::TERM] {
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "scrub::tests::signal_child",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(SIGNAL_CHILD, "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut child = ChildGuard(child);
        let stdout = child.0.stdout.take().unwrap();
        let (ready, installed) = mpsc::channel();
        let output = thread::spawn(move || {
            let mut transcript = String::new();
            for line in io::BufReader::new(stdout).lines() {
                let line = line.unwrap();
                if line.contains(SIGNAL_READY) {
                    ready.send(()).unwrap();
                }
                transcript.push_str(&line);
                transcript.push('\n');
            }
            transcript
        });
        installed.recv_timeout(Duration::from_secs(10)).unwrap();
        let pid = Pid::from_raw(child.0.id().try_into().unwrap()).unwrap();
        kill_process(pid, signal).unwrap();
        child.0.stdin.take().unwrap().write_all(b"R").unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        let status = loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "scrub signal helper did not finish"
            );
            // Poll only for process exit; the pipe establishes signal readiness.
            thread::sleep(Duration::from_millis(5));
        };
        let output = output.join().unwrap();
        assert!(status.success(), "{signal:?}: {output}");
        assert!(
            output.contains("1 passed"),
            "helper did not execute: {output}"
        );
    }
}
