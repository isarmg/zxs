//! Bound socket owners independently of the single native maintenance job.
use http_routing::serve::Listener;
use std::{
    future::Future,
    io::{self, IoSlice},
    net::SocketAddr,
    pin::Pin,
    sync::{Arc, OnceLock},
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::{TcpListener, TcpStream},
    sync::{OwnedSemaphorePermit, Semaphore},
    time::{Instant, Sleep},
};

const MAX_CONNECTIONS: usize = 32;
const DRAIN_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Default)]
pub(super) struct Drain(Arc<OnceLock<Instant>>);
impl Drain {
    pub(super) fn begin(&self) {
        self.0.get_or_init(|| Instant::now() + DRAIN_TIMEOUT);
    }
}

pub(super) struct BoundedListener {
    listener: TcpListener,
    capacity: Arc<Semaphore>,
    drain: Drain,
}
impl BoundedListener {
    pub(super) fn new(listener: TcpListener, drain: Drain) -> Self {
        Self {
            listener,
            capacity: Arc::new(Semaphore::new(MAX_CONNECTIONS)),
            drain,
        }
    }
}
impl Listener for BoundedListener {
    type Io = Connection;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        // Reserve before accept: even an idle keep-alive socket owns a slot.
        // The kernel backlog does not create helper tasks or response queues.
        let permit = Arc::clone(&self.capacity)
            .acquire_owned()
            .await
            .expect("invariant: the private listener semaphore is never closed");
        let (stream, address) = Listener::accept(&mut self.listener).await;
        (
            Connection {
                stream,
                _permit: permit,
                drain: self.drain.clone(),
                deadline: None,
            },
            address,
        )
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.listener.local_addr()
    }
}

pub(super) struct Connection {
    stream: TcpStream,
    _permit: OwnedSemaphorePermit,
    drain: Drain,
    deadline: Option<Pin<Box<Sleep>>>,
}
impl Connection {
    fn poll_drain(&mut self, cx: &mut Context<'_>) -> io::Result<()> {
        if let Some(deadline) = self.drain.0.get() {
            let timer = self
                .deadline
                .get_or_insert_with(|| Box::pin(tokio::time::sleep_until(*deadline)));
            if timer.as_mut().poll(cx).is_ready() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "helper HTTP shutdown drain deadline elapsed",
                ));
            }
        }
        Ok(())
    }
}
impl AsyncRead for Connection {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        this.poll_drain(cx)?;
        Pin::new(&mut this.stream).poll_read(cx, buffer)
    }
}
impl AsyncWrite for Connection {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        this.poll_drain(cx)?;
        Pin::new(&mut this.stream).poll_write(cx, bytes)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffers: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        this.poll_drain(cx)?;
        Pin::new(&mut this.stream).poll_write_vectored(cx, buffers)
    }

    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        this.poll_drain(cx)?;
        Pin::new(&mut this.stream).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        this.poll_drain(cx)?;
        Pin::new(&mut this.stream).poll_shutdown(cx)
    }
}

#[cfg(test)]
#[path = "../../../test/helper/unit/console/transport_tests.rs"]
mod tests;
