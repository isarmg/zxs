//! Owned periodic observers. Stop wakes idle waits; drain waits for actual I/O.
use std::{
    sync::{Condvar, Mutex, mpsc},
    thread::{self, JoinHandle},
    time::Duration,
};

#[derive(Clone, Debug, error_definition::Error)]
#[error("periodic worker panicked: {message:?}")]
struct WorkerPanic {
    message: Option<String>,
}

enum State {
    Idle,
    Running {
        stop: Option<mpsc::Sender<()>>,
        thread: JoinHandle<()>,
    },
    Joining,
    Stopped(Result<(), WorkerPanic>),
}

pub(crate) struct PeriodicWorker {
    state: Mutex<State>,
    joined: Condvar,
}
impl PeriodicWorker {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(State::Idle),
            joined: Condvar::new(),
        }
    }

    pub(crate) fn start(
        &self,
        builder: thread::Builder,
        initial_delay: Duration,
        interval: Duration,
        mut sample: impl FnMut() + Send + 'static,
    ) -> std::io::Result<()> {
        let mut state = crate::lock(&self.state);
        if !matches!(*state, State::Idle) {
            return Err(std::io::Error::other(
                "periodic worker already started or stopped",
            ));
        }
        let (stop, stopped) = mpsc::channel();
        let thread = builder.spawn(move || {
            let mut delay = initial_delay;
            while let Err(mpsc::RecvTimeoutError::Timeout) = stopped.recv_timeout(delay) {
                sample();
                delay = interval;
            }
        })?;
        *state = State::Running {
            stop: Some(stop),
            thread,
        };
        Ok(())
    }

    // Only cleanup recovers this inventory lock; never resume a poisoned owner.
    fn closing_state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| {
            tracing::error!("closing poisoned periodic worker inventory");
            poisoned.into_inner()
        })
    }

    pub(crate) fn stop(&self) {
        let mut state = self.closing_state();
        match &mut *state {
            State::Idle => *state = State::Stopped(Ok(())),
            State::Running { stop, .. } => {
                stop.take();
            }
            State::Joining | State::Stopped(_) => {}
        }
    }

    pub(crate) fn drain(&self) -> Result<(), crate::ApiError> {
        self.stop();
        let mut state = self.closing_state();
        loop {
            match &*state {
                State::Stopped(result) => {
                    return result
                        .clone()
                        .map_err(|error| crate::ApiError::io(std::io::Error::other(error)));
                }
                State::Joining => {
                    state = self
                        .joined
                        .wait(state)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                }
                State::Running { .. } => break,
                State::Idle => unreachable!("invariant: stop closes idle worker"),
            }
        }
        let State::Running { thread, .. } = std::mem::replace(&mut *state, State::Joining) else {
            unreachable!("invariant: running worker owns its join handle");
        };
        drop(state);
        let result = thread.join().map_err(|panic| WorkerPanic {
            message: panic.downcast_ref::<String>().cloned().or_else(|| {
                panic
                    .downcast_ref::<&str>()
                    .map(|message| (*message).to_owned())
            }),
        });
        *self.closing_state() = State::Stopped(result.clone());
        self.joined.notify_all();
        result.map_err(|error| crate::ApiError::io(std::io::Error::other(error)))
    }
}
impl Drop for PeriodicWorker {
    fn drop(&mut self) {
        if let Err(error) = self.drain() {
            tracing::error!(error = ?error, "periodic worker shutdown failed");
        }
    }
}

#[cfg(test)]
#[path = "../../../test/server/unit/periodic_worker.rs"]
mod tests;
