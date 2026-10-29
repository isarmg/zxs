//! HTTP-only admission and wall-clock body lifetime bounds.
use crate::{ApiError, lock};
use futures::Stream;
use std::{
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll, Waker},
    time::Duration,
};
use tokio::time::Instant;

#[derive(Clone, Debug)]
pub struct TransferLimits {
    /// Independent upload and read pools, each with this total capacity.
    pub slots: usize,
    pub idle_timeout: Duration,
    pub total_timeout: Duration,
}
impl Default for TransferLimits {
    fn default() -> Self {
        Self {
            slots: 2,
            idle_timeout: Duration::from_secs(60),
            total_timeout: Duration::from_secs(3600),
        }
    }
}
impl TransferLimits {
    pub(crate) fn validate(&self) -> Result<(), ApiError> {
        if self.slots == 0
            || self.idle_timeout.is_zero()
            || self.total_timeout < self.idle_timeout
            || self.total_timeout > Duration::from_secs(7 * 24 * 3600)
        {
            return Err(ApiError::invalid(
                "HTTP transfers require positive slots and 0 < idle <= total timeout <= 7 days",
            ));
        }
        Ok(())
    }
}
pub(crate) struct Admission {
    active: Mutex<usize>,
    slots: usize,
}
pub(crate) struct Permit {
    admission: Arc<Admission>,
}
impl Admission {
    pub(crate) fn new(limits: &TransferLimits) -> Arc<Self> {
        Arc::new(Self {
            active: Mutex::new(0),
            slots: limits.slots,
        })
    }
    pub(crate) fn acquire(self: &Arc<Self>) -> Result<Permit, ApiError> {
        let mut active = lock(&self.active);
        if *active >= self.slots {
            return Err(ApiError::limit("HTTP transfer capacity reached"));
        }
        *active += 1;
        Ok(Permit {
            admission: self.clone(),
        })
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        if let Some(mut active) =
            crate::lock_for_terminal_release(&self.admission.active, "HTTP transfer permits")
        {
            *active -= 1;
        }
    }
}
pub(crate) fn timeout_error() -> ApiError {
    ApiError::new(
        ::http::StatusCode::REQUEST_TIMEOUT,
        storage_contract::ErrorCode::TransferTimeout,
        "HTTP transfer deadline exceeded; durable work data is retained",
    )
}

struct StreamState<S> {
    stream: Option<Pin<Box<S>>>,
    expired: bool,
    waker: Option<Waker>,
}
/// The timer drops an idle body even when the HTTP consumer stops polling it.
/// Blocking jobs retain their own lease/permit until they actually finish.
pub(crate) struct BoundedStream<S> {
    state: Arc<Mutex<StreamState<S>>>,
    progress: tokio::sync::watch::Sender<Instant>,
    timer: tokio::task::JoinHandle<()>,
    idle: Duration,
    deadline: Instant,
}
impl<S> BoundedStream<S>
where
    S: Stream<Item = Result<bytes::Bytes, std::io::Error>> + Send + 'static,
{
    pub(crate) fn new(stream: S, limits: &TransferLimits, deadline: Instant) -> Self {
        let state = Arc::new(Mutex::new(StreamState {
            stream: Some(Box::pin(stream)),
            expired: false,
            waker: None,
        }));
        let (progress, mut updates) =
            tokio::sync::watch::channel(Instant::now() + limits.idle_timeout);
        let shared = state.clone();
        let timer = tokio::spawn(async move {
            loop {
                let next = (*updates.borrow_and_update()).min(deadline);
                tokio::select! {
                    biased;
                    _ = tokio::time::sleep_until(next) => {
                        let mut state = shared.lock().expect("service state lock poisoned");
                        // A chunk may have reset idle while this timer became ready.
                        if Instant::now() < (*updates.borrow()).min(deadline) { continue; }
                        if state.stream.is_none() { return; }
                        state.expired = true;
                        state.stream = None;
                        if let Some(waker) = state.waker.take() { waker.wake(); }
                        return;
                    }
                    result = updates.changed() => { if result.is_err() { return; } }
                }
            }
        });
        Self {
            state,
            progress,
            timer,
            idle: limits.idle_timeout,
            deadline,
        }
    }
}
impl<S: Stream<Item = Result<bytes::Bytes, std::io::Error>>> Stream for BoundedStream<S> {
    type Item = Result<bytes::Bytes, std::io::Error>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut state = self.state.lock().expect("service state lock poisoned");
        if state.stream.is_some() && Instant::now() >= (*self.progress.borrow()).min(self.deadline)
        {
            state.stream = None;
            state.expired = true;
            self.timer.abort();
        }
        if state.expired {
            state.expired = false;
            let error = std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "HTTP transfer deadline exceeded",
            );
            tracing::warn!(error = ?error, "HTTP response stream failed");
            return Poll::Ready(Some(Err(error)));
        }
        let Some(stream) = state.stream.as_mut() else {
            return Poll::Ready(None);
        };
        let result = stream.as_mut().poll_next(cx);
        match &result {
            Poll::Ready(Some(Ok(bytes))) if !bytes.is_empty() => {
                self.progress.send_replace(Instant::now() + self.idle);
            }
            Poll::Ready(Some(Err(error))) => {
                tracing::warn!(error = ?error, "HTTP response stream failed");
                state.stream = None;
                self.timer.abort();
            }
            Poll::Ready(None) => {
                state.stream = None;
                self.timer.abort();
            }
            Poll::Pending | Poll::Ready(Some(Ok(_))) => {}
        }
        state.waker = Some(cx.waker().clone());
        result
    }
}
impl<S> Drop for BoundedStream<S> {
    fn drop(&mut self) {
        self.timer.abort();
        if let Some(mut state) =
            crate::lock_for_terminal_release(&self.state, "HTTP bounded stream")
        {
            state.stream = None;
        }
    }
}

#[cfg(test)]
#[path = "../../../test/server/unit/transfer_limits/tests.rs"]
mod tests;
