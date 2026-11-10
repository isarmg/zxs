//! One thread owner and durable diagnostic lifecycle for every Backup stage.
use super::*;

pub(super) struct Progress {
    pub(super) system: DiskRoot,
    state: Arc<Mutex<BackupPreparationStatus>>,
    pub(super) stop: Arc<AtomicBool>,
}

impl Progress {
    pub(super) fn update(&self, action: impl FnOnce(&mut BackupPreparationStatus)) -> Result<()> {
        // Only the live worker writes progress. Never hold its diagnostic lock
        // while persisting; even a failed write must leave its observed result.
        let mut status = self
            .state
            .lock()
            .expect("invariant: backup progress poisoned")
            .clone();
        action(&mut status);
        let persisted = self
            .system
            .write_json(&self.system.path().join(PROGRESS), &status, true);
        *self
            .state
            .lock()
            .expect("invariant: backup progress poisoned") = status;
        persisted
    }

    pub(super) fn finish(&self, result: Result<()>) {
        if let Err(error) = result
            && let Err(persistence) = self.update(|status| {
                status.phase = BackupPreparationPhase::Blocked;
                status.error_code = Some(error.code());
                status.detail = error.to_string();
            })
        {
            self.state
                .lock()
                .expect("invariant: backup progress poisoned")
                .detail = format!("{error}; progress persistence failed: {persistence}");
        }
    }
}

impl Worker {
    pub(super) fn new(
        job: Job,
        batch: Option<batch::Batch>,
        status: BackupPreparationStatus,
    ) -> Self {
        Self {
            job,
            batch,
            state: Arc::new(Mutex::new(status)),
            stop: Arc::new(AtomicBool::new(false)),
            thread: None,
            targets: Vec::new(),
        }
    }

    pub(super) fn spawn(
        mut self,
        locks: WriterLocks,
        system: DiskRoot,
        name: &str,
        execute: impl FnOnce(&Progress) + Send + 'static,
    ) -> Result<Self> {
        let progress = Progress {
            system,
            state: Arc::clone(&self.state),
            stop: Arc::clone(&self.stop),
        };
        self.thread = Some(
            std::thread::Builder::new()
                .name(name.into())
                .spawn(move || {
                    let _locks = locks;
                    // The stage retains its target owners until finish returns. Source
                    // locks outlive that entire closure, including error persistence.
                    execute(&progress);
                })?,
        );
        Ok(self)
    }
}
