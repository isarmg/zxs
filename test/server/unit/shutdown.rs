use super::*;
use std::{
    io::{BufRead, Read, Write},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

const CHILD_MODE: &str = "ZXS_UNIT_TEST_STARTUP_SIGNAL_CHILD";
const READY: &str = "ZXS_TEST_SIGNAL_HANDLERS_INSTALLED";
const DEADLINE: Duration = Duration::from_secs(5);

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

// This entry point exists only in the Rust unit-test executable. Neither the
// daemon CLI nor production startup recognizes CHILD_MODE or this barrier.
#[test]
#[ignore = "invoked only by early_signals_are_retained_through_synchronous_startup"]
fn signal_during_synchronous_startup_child() {
    if std::env::var_os(CHILD_MODE).as_deref() != Some(std::ffi::OsStr::new("1")) {
        return;
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut shutdown = ShutdownSignal::install().unwrap();
        // Model synchronous startup on the only runtime thread. No signal
        // receiver is polled between installation and the parent's release.
        {
            let mut output = std::io::stdout().lock();
            writeln!(output, "{READY}").unwrap();
            output.flush().unwrap();
        }
        let mut release = [0];
        std::io::stdin().read_exact(&mut release).unwrap();
        assert_eq!(release, [b'R']);
        // The OS handler retains the signal while this thread is blocked.
        // Driver delivery is asynchronous: an immediate snapshot is not an
        // atomic observation barrier and may still be false here.
        tokio::time::timeout(DEADLINE, shutdown.wait())
            .await
            .expect("startup signal must be observed after the runtime resumes");
        assert!(shutdown.observed());
        // Once observed, no second signal is required by later startup checks.
        tokio::time::timeout(DEADLINE, shutdown.wait())
            .await
            .unwrap();
        assert!(shutdown.observed());
    });
}

#[test]
fn early_signals_are_retained_through_synchronous_startup() {
    use rustix::process::{Pid, Signal, kill_process};
    for signal in [Signal::INT, Signal::TERM] {
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "shutdown::tests::signal_during_synchronous_startup_child",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD_MODE, "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut child = ChildGuard(child);
        let stdout = child.0.stdout.take().unwrap();
        let (ready, installed) = mpsc::channel();
        let output = std::thread::spawn(move || {
            let mut transcript = String::new();
            for line in std::io::BufReader::new(stdout).lines() {
                let line = line.unwrap();
                if line.contains(READY) {
                    ready.send(()).unwrap();
                }
                transcript.push_str(&line);
                transcript.push('\n');
            }
            transcript
        });
        installed
            .recv_timeout(DEADLINE)
            .expect("child did not install signal handlers");
        assert!(child.0.try_wait().unwrap().is_none());
        let pid = Pid::from_raw(child.0.id().try_into().unwrap()).unwrap();
        kill_process(pid, signal).unwrap();
        child.0.stdin.take().unwrap().write_all(b"R").unwrap();
        let deadline = Instant::now() + DEADLINE;
        let status = loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "child did not finish its startup signal check"
            );
            // Poll only for process completion. The pipe barrier, not this
            // interval, establishes signal installation and startup ordering.
            std::thread::sleep(Duration::from_millis(5));
        };
        let output = output.join().unwrap();
        assert!(status.success(), "{signal:?} child failed: {output}");
        assert!(
            output.contains("1 passed"),
            "child helper did not run: {output}"
        );
    }
}
