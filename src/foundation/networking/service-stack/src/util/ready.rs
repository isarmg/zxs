use std::{fmt, marker::PhantomData};

use crate::Service;
use std::{
    future::Future,
    pin::Pin,
    task::{ready, Context, Poll},
};

/// A [`Future`] that yields the service when it is ready to accept a request.
///
/// [`ReadyOneshot`] values are produced by [`ServiceExt::ready_oneshot`].
///
/// [`ServiceExt::ready_oneshot`]: crate::util::ServiceExt::ready_oneshot
pub struct ReadyOneshot<T, Request> {
    inner: Option<T>,
    _p: PhantomData<fn() -> Request>,
}

// The inner service is not structurally pinned and is never projected as Pin<&mut T>.
impl<T, Request> Unpin for ReadyOneshot<T, Request> {}

impl<T, Request> ReadyOneshot<T, Request>
where
    T: Service<Request>,
{
    /// Creates a future that owns the service and returns it once it is ready.
    pub const fn new(service: T) -> Self {
        Self {
            inner: Some(service),
            _p: PhantomData,
        }
    }
}

impl<T, Request> Future for ReadyOneshot<T, Request>
where
    T: Service<Request>,
{
    type Output = Result<T, T::Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        ready!(self
            .inner
            .as_mut()
            .expect("invariant: service is retained until ReadyOneshot returns it")
            .poll_ready(cx))?;

        Poll::Ready(Ok(self.inner.take().expect(
            "invariant: successful readiness does not remove the retained service",
        )))
    }
}

impl<T, Request> fmt::Debug for ReadyOneshot<T, Request>
where
    T: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("ReadyOneshot")
            .field("inner", &self.inner)
            .finish()
    }
}

/// A future that yields a mutable reference to the service when it is ready to accept a request.
///
/// [`Ready`] values are produced by [`ServiceExt::ready`].
///
/// [`ServiceExt::ready`]: crate::util::ServiceExt::ready
pub struct Ready<'a, T, Request>(ReadyOneshot<&'a mut T, Request>);

// Safety: This is safe for the same reason that the impl for ReadyOneshot is safe.
impl<T, Request> Unpin for Ready<'_, T, Request> {}

impl<'a, T, Request> Ready<'a, T, Request>
where
    T: Service<Request>,
{
    /// Creates a future that waits for readiness and returns this mutable service borrow.
    pub fn new(service: &'a mut T) -> Self {
        Self(ReadyOneshot::new(service))
    }
}

impl<'a, T, Request> Future for Ready<'a, T, Request>
where
    T: Service<Request>,
{
    type Output = Result<&'a mut T, T::Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(cx)
    }
}

impl<T, Request> fmt::Debug for Ready<'_, T, Request>
where
    T: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_tuple("Ready").field(&self.0).finish()
    }
}
