//! A single offline scrub owns its native worker through actual I/O completion.
mod wire;

use std::{future::Future, io, path::PathBuf, sync::Arc, thread};
use storage_engine::{
    Engine, IntegrityScrubCancellation, IntegrityScrubReport, RetainedWorkspaceLimits,
};
use tokio::{
    signal::unix::{Signal, SignalKind, signal},
    sync::Notify,
};

type ScanResult = storage_engine::Result<IntegrityScrubReport>;

pub(crate) fn run(root: PathBuf, limits: Option<RetainedWorkspaceLimits>) -> crate::Result<()> {
    // Only signals run on this runtime. The long scan has its own fallible native
    // thread, not a shared blocking-pool slot or an untracked detached task.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .build()?;
    let report = runtime.block_on(async {
        let mut signals = ScrubSignals::install()?;
        let job = ScrubJob::start(root, limits)?;
        job.finish_when(signals.wait()).await
    })?;
    let healthy = report.is_healthy();
    crate::print(wire::Report::from(&report))?;
    if healthy {
        Ok(())
    } else {
        Err("integrity scrub did not complete successfully; see the JSON report".into())
    }
}

struct ScrubSignals {
    interrupt: Signal,
    terminate: Signal,
}
impl ScrubSignals {
    fn install() -> io::Result<Self> {
        Ok(Self {
            interrupt: signal(SignalKind::interrupt())?,
            terminate: signal(SignalKind::terminate())?,
        })
    }
    async fn wait(&mut self) {
        // This local Signal implementation is an infinite stream; recv never
        // returns None. Only installation is a fallible boundary.
        tokio::select! {
            _ = self.interrupt.recv() => {},
            _ = self.terminate.recv() => {},
        }
    }
}

struct ScrubJob {
    worker: Option<thread::JoinHandle<ScanResult>>,
    cancellation: Arc<IntegrityScrubCancellation>,
    completed: Arc<Notify>,
}

// Notification happens on success, error and unwinding. Engine and its writer
// locks are dropped inside the scan before this outer completion guard runs.
struct Completion(Arc<Notify>);
impl Drop for Completion {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}

impl ScrubJob {
    fn start(root: PathBuf, limits: Option<RetainedWorkspaceLimits>) -> io::Result<Self> {
        let cancellation = Arc::new(IntegrityScrubCancellation::new());
        let completed = Arc::new(Notify::new());
        let worker_cancellation = Arc::clone(&cancellation);
        let completion = Completion(Arc::clone(&completed));
        let worker = thread::Builder::new()
            .name("zxs-integrity-scrub".into())
            .spawn(move || {
                let _completion = completion;
                // Preserve pending decisions: this diagnostic must not choose
                // to finish an unpublished operation merely to start a scan.
                let mut engine = Engine::open_for_recovery_with_workspace_limits(&root, limits)?;
                crate::report_recovery_warnings(&mut engine);
                engine.scrub(&worker_cancellation)
            })?;
        Ok(Self {
            worker: Some(worker),
            cancellation,
            completed,
        })
    }

    async fn finish_when(
        mut self,
        stop: impl Future<Output = ()>,
    ) -> crate::Result<IntegrityScrubReport> {
        let completed = Arc::clone(&self.completed).notified_owned();
        tokio::pin!(completed);
        tokio::select! {
            () = &mut completed => {},
            () = stop => {
                self.cancellation.cancel();
                completed.await;
            }
        };
        // A signal requests cancellation, not
        // detachment. Even a failed scan is joined before releasing its owner.
        let worker = self
            .worker
            .take()
            .expect("invariant: scrub worker is joined once");
        join_worker(worker)
    }
}

impl Drop for ScrubJob {
    fn drop(&mut self) {
        self.cancellation.cancel();
        if let Some(worker) = self.worker.take() {
            // Also covers a cancelled caller future or unwinding. This worker
            // never depends on the runtime, so synchronous drain cannot strand it.
            if let Err(error) = join_worker(worker) {
                tracing::error!(error = %error, "integrity scrub drain failed");
            }
        }
    }
}

fn join_worker(worker: thread::JoinHandle<ScanResult>) -> crate::Result<IntegrityScrubReport> {
    let report = worker.join().map_err(WorkerPanic::from)?;
    Ok(report?)
}

#[derive(Debug)]
struct WorkerPanic {
    message: Option<String>,
}
impl From<Box<dyn std::any::Any + Send>> for WorkerPanic {
    fn from(panic: Box<dyn std::any::Any + Send>) -> Self {
        let message = panic.downcast_ref::<String>().cloned().or_else(|| {
            panic
                .downcast_ref::<&str>()
                .map(|value| (*value).to_owned())
        });
        Self { message }
    }
}
impl std::fmt::Display for WorkerPanic {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "integrity scrub worker panicked: {:?}",
            self.message
        )
    }
}
impl std::error::Error for WorkerPanic {}

#[cfg(test)]
#[path = "../../../test/rescue/unit/scrub/tests.rs"]
mod tests;
