use std::task::{Context, Poll};

use tokio::sync::{mpsc, oneshot};

use crate::common::demand;

pub(crate) type RetryPromise<T, U> = oneshot::Receiver<Result<U, TrySendError<T>>>;
pub(crate) type Promise<T> = oneshot::Receiver<Result<T, crate::Error>>;

/// An error when calling `try_send_request`.
///
/// There is a possibility of an error occurring on a connection in-between the
/// time that a request is queued and when it is actually written to the IO
/// transport. If that happens, it is safe to return the request back to the
/// caller, as it was never fully sent.
#[derive(Debug)]
pub struct TrySendError<T> {
    pub(crate) error: crate::Error,
    pub(crate) message: Option<T>,
}

pub(crate) fn channel<T, U>() -> (Sender<T, U>, Receiver<T, U>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let (giver, taker) = demand::new();
    let tx = Sender {
        #[cfg(feature = "http1")]
        buffered_once: false,
        giver,
        inner: tx,
    };
    let rx = Receiver { inner: rx, taker };
    (tx, rx)
}

/// A bounded sender of requests and callbacks for when responses are ready.
///
/// While the inner sender is unbounded, the Giver is used to determine
/// if the Receiver is ready for another request.
pub(crate) struct Sender<T, U> {
    /// One message is always allowed, even if the Receiver hasn't asked
    /// for it yet. This boolean keeps track of whether we've sent one
    /// without notice.
    #[cfg(feature = "http1")]
    buffered_once: bool,
    /// The Giver helps watch that the Receiver side has been polled
    /// when the queue is empty. This helps us know when a request and
    /// response have been fully processed, and a connection is ready
    /// for more.
    giver: demand::Giver,
    /// Actually bounded by the Giver, plus `buffered_once`.
    inner: mpsc::UnboundedSender<Envelope<T, U>>,
}

impl<T, U> Sender<T, U> {
    #[cfg(feature = "http1")]
    pub(crate) fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<crate::Result<()>> {
        self.giver
            .poll_want(cx)
            .map_err(|_| crate::Error::new_closed())
    }

    #[cfg(feature = "http1")]
    pub(crate) fn is_ready(&self) -> bool {
        self.giver.is_wanting()
    }

    #[cfg(feature = "http1")]
    pub(crate) fn is_closed(&self) -> bool {
        self.giver.is_canceled()
    }

    #[cfg(feature = "http1")]
    fn can_send(&mut self) -> bool {
        if self.giver.give() || !self.buffered_once {
            // If the receiver is ready *now*, then of course we can send.
            //
            // If the receiver isn't ready yet, but we don't have anything
            // in the channel yet, then allow one message.
            self.buffered_once = true;
            true
        } else {
            false
        }
    }

    #[cfg(feature = "http1")]
    pub(crate) fn try_send(&mut self, val: T) -> Result<RetryPromise<T, U>, T> {
        if !self.can_send() {
            return Err(val);
        }
        let (tx, rx) = oneshot::channel();
        self.inner
            .send(Envelope(Some((val, Callback::Retry(Some(tx))))))
            .map(move |()| rx)
            .map_err(|mut e| (e.0).0.take().expect("envelope not dropped").0)
    }

    #[cfg(feature = "http1")]
    pub(crate) fn send(&mut self, val: T) -> Result<Promise<U>, T> {
        if !self.can_send() {
            return Err(val);
        }
        let (tx, rx) = oneshot::channel();
        self.inner
            .send(Envelope(Some((val, Callback::NoRetry(Some(tx))))))
            .map(move |()| rx)
            .map_err(|mut e| (e.0).0.take().expect("envelope not dropped").0)
    }
}

pub(crate) struct Receiver<T, U> {
    inner: mpsc::UnboundedReceiver<Envelope<T, U>>,
    taker: demand::Taker,
}

impl<T, U> Receiver<T, U> {
    pub(crate) fn poll_recv(&mut self, cx: &mut Context<'_>) -> Poll<Option<(T, Callback<T, U>)>> {
        match self.inner.poll_recv(cx) {
            Poll::Ready(item) => {
                // A want signaled after finding the queue empty can land after
                // the Sender's `give()` for the message taken here, as can one
                // signaled on a coop-budget Pending with a message queued.
                // Withdraw it, or the Sender reports this connection as ready
                // while it is still serving this message.
                self.taker.unwant();
                Poll::Ready(item.map(|mut env| env.0.take().expect("envelope not dropped")))
            }
            Poll::Pending => {
                self.taker.want();
                Poll::Pending
            }
        }
    }

    #[cfg(feature = "http1")]
    pub(crate) fn close(&mut self) {
        self.taker.cancel();
        self.inner.close();
    }

    #[cfg(feature = "http1")]
    pub(crate) fn poll_close(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        self.close();
        // A send that started before close may still be publishing its envelope.
        // Keep the receiver alive until all such envelopes have been canceled.
        while futures::ready!(self.inner.poll_recv(cx)).is_some() {}
        Poll::Ready(())
    }

    #[cfg(feature = "http1")]
    pub(crate) fn try_recv(&mut self) -> Option<(T, Callback<T, U>)> {
        match crate::common::task::now_or_never(self.inner.recv()) {
            Some(Some(mut env)) => env.0.take(),
            _ => None,
        }
    }
}

impl<T, U> Drop for Receiver<T, U> {
    fn drop(&mut self) {
        // Notify the giver about the closure first, before dropping
        // the mpsc::Receiver.
        self.taker.cancel();
    }
}

struct Envelope<T, U>(Option<(T, Callback<T, U>)>);

impl<T, U> Drop for Envelope<T, U> {
    fn drop(&mut self) {
        if let Some((val, cb)) = self.0.take() {
            cb.send(Err(TrySendError {
                error: crate::Error::new_canceled().with("connection closed"),
                message: Some(val),
            }));
        }
    }
}

pub(crate) enum Callback<T, U> {
    #[allow(unused)]
    Retry(Option<oneshot::Sender<Result<U, TrySendError<T>>>>),
    NoRetry(Option<oneshot::Sender<Result<U, crate::Error>>>),
}

impl<T, U> Drop for Callback<T, U> {
    fn drop(&mut self) {
        match self {
            Callback::Retry(tx) => {
                if let Some(tx) = tx.take() {
                    let _ = tx.send(Err(TrySendError {
                        error: dispatch_gone(),
                        message: None,
                    }));
                }
            }
            Callback::NoRetry(tx) => {
                if let Some(tx) = tx.take() {
                    let _ = tx.send(Err(dispatch_gone()));
                }
            }
        }
    }
}

#[cold]
fn dispatch_gone() -> crate::Error {
    // FIXME(nox): What errors do we want here?
    crate::Error::new_user_dispatch_gone().with(if std::thread::panicking() {
        "user code panicked"
    } else {
        "runtime dropped the dispatch task"
    })
}

impl<T, U> Callback<T, U> {
    pub(crate) fn poll_canceled(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        match self {
            Callback::Retry(Some(tx)) => tx.poll_closed(cx),
            Callback::NoRetry(Some(tx)) => tx.poll_closed(cx),
            _ => unreachable!(),
        }
    }

    pub(crate) fn send(mut self, val: Result<U, TrySendError<T>>) {
        match &mut self {
            Callback::Retry(tx) => {
                let _ = tx
                    .take()
                    .expect("callback sender not dropped before send")
                    .send(val);
            }
            Callback::NoRetry(tx) => {
                let _ = tx
                    .take()
                    .expect("callback sender not dropped before send")
                    .send(val.map_err(|e| e.error));
            }
        }
    }
}

impl<T> TrySendError<T> {
    /// Take the message from this error.
    ///
    /// The message will not always have been recovered. If an error occurs
    /// after the message has been serialized onto the connection, it will not
    /// be available here.
    pub fn take_message(&mut self) -> Option<T> {
        self.message.take()
    }

    /// Returns a reference to the recovered message.
    ///
    /// The message will not always have been recovered. If an error occurs
    /// after the message has been serialized onto the connection, it will not
    /// be available here.
    pub fn message(&self) -> Option<&T> {
        self.message.as_ref()
    }

    /// Consumes this to return the inner error.
    pub fn into_error(self) -> crate::Error {
        self.error
    }

    /// Returns a reference to the inner error.
    pub fn error(&self) -> &crate::Error {
        &self.error
    }
}

#[cfg(test)]
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../../test/foundation/http-transport/unit/src/client/dispatch/tests_394.rs"
));
