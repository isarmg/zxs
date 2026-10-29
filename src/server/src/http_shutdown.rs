//! Bound network drain without abandoning admitted blocking storage work.
use http_routing::{
    Router,
    extract::{ConnectInfo, Request, connect_info::Connected},
    middleware::{Next, from_fn},
    response::Response,
    serve::{Listener, ListenerExt},
};
use std::{
    collections::BTreeMap,
    future::pending,
    io,
    net::{Shutdown, SocketAddr},
    os::fd::{AsRawFd, RawFd},
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::{TcpListener, TcpStream},
    sync::{OwnedSemaphorePermit, Semaphore, oneshot},
};

const NETWORK_GRACE: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Connections {
    closed: bool,
    sockets: BTreeMap<RawFd, Arc<ConnectionSocket>>,
}
impl Connections {
    fn close(&mut self) {
        self.closed = true;
        for socket in self.sockets.values() {
            if let Err(error) = socket.socket.shutdown(Shutdown::Both)
                && error.kind() != io::ErrorKind::NotConnected
            {
                tracing::warn!(error = ?error, "HTTP socket shutdown failed");
            }
        }
        self.sockets.clear();
    }
}

#[derive(Default)]
struct NetworkGuard(Arc<Mutex<Connections>>);
impl Drop for NetworkGuard {
    fn drop(&mut self) {
        self.0.lock().expect("service state lock poisoned").close();
    }
}

// Field order closes the duplicated socket before returning its admission permit.
struct ConnectionSocket {
    socket: std::net::TcpStream,
    _permit: OwnedSemaphorePermit,
}

struct DrainListener {
    slots: Arc<Semaphore>,
    listener: TcpListener,
    connections: Arc<Mutex<Connections>>,
}

struct DrainStream {
    // Close the original socket before releasing any shared connection owner.
    stream: TcpStream,
    cancel: Option<Arc<ConnectionSocket>>,
    response_socket: Arc<ConnectionSocket>,
    write_progress: tokio::sync::watch::Sender<tokio::time::Instant>,
    key: RawFd,
    connections: Arc<Mutex<Connections>>,
}

/// Exact accepted socket identity; unlike a descriptor number this capability
/// cannot accidentally shut down a later connection after descriptor reuse.
#[derive(Clone)]
pub(crate) struct ResponseConnection {
    socket: Arc<ConnectionSocket>,
    write_progress: tokio::sync::watch::Sender<tokio::time::Instant>,
}

pub(crate) struct SendDeadline(tokio::task::JoinHandle<()>);
impl Drop for SendDeadline {
    fn drop(&mut self) {
        self.0.abort();
    }
}
impl ResponseConnection {
    pub(crate) fn shutdown(&self) {
        if let Err(error) = self.socket.socket.shutdown(Shutdown::Both)
            && error.kind() != io::ErrorKind::NotConnected
        {
            tracing::warn!(error = ?error, "HTTP response deadline shutdown failed");
        }
    }
    pub(crate) fn deadline(
        &self,
        limits: &crate::TransferLimits,
        deadline: tokio::time::Instant,
    ) -> SendDeadline {
        let connection = self.clone();
        let idle = limits.idle_timeout;
        let started = tokio::time::Instant::now();
        let mut progress = self.write_progress.subscribe();
        SendDeadline(tokio::spawn(async move {
            loop {
                let next = ((*progress.borrow_and_update()).max(started) + idle).min(deadline);
                tokio::select! {
                    biased;
                    _ = tokio::time::sleep_until(next) => {
                        // A successful socket write may have raced this timer.
                        if tokio::time::Instant::now() < ((*progress.borrow()).max(started) + idle).min(deadline) {
                            continue;
                        }
                        connection.shutdown();
                        return;
                    }
                    result = progress.changed() => { if result.is_err() { return; } }
                }
            }
        }))
    }
}

#[derive(Clone)]
struct ConnectionInfo {
    peer: SocketAddr,
    response: ResponseConnection,
}
impl<F>
    Connected<http_routing::serve::IncomingStream<'_, http_routing::serve::TapIo<DrainListener, F>>>
    for ConnectionInfo
where
    F: FnMut(&mut DrainStream) + Send + 'static,
{
    fn connect_info(
        stream: http_routing::serve::IncomingStream<
            '_,
            http_routing::serve::TapIo<DrainListener, F>,
        >,
    ) -> Self {
        Self {
            peer: *stream.remote_addr(),
            response: ResponseConnection {
                socket: stream.io().response_socket.clone(),
                write_progress: stream.io().write_progress.clone(),
            },
        }
    }
}

async fn connection_context(
    ConnectInfo(info): ConnectInfo<ConnectionInfo>,
    mut request: Request,
    next: Next,
) -> Response {
    // Preserve the existing trusted-proxy/authentication peer contract.
    request.extensions_mut().insert(ConnectInfo(info.peer));
    request.extensions_mut().insert(info.response);
    next.run(request).await
}
impl DrainStream {
    // TapIo registers each connection before the HTTP library can spawn its
    // task; ConnectionInfo retains both its peer and response capability.
    fn register(&mut self) {
        let Some(socket) = self.cancel.take() else {
            return;
        };
        let mut connections = self
            .connections
            .lock()
            .expect("service state lock poisoned");
        if connections.closed {
            if let Err(error) = socket.socket.shutdown(Shutdown::Both)
                && error.kind() != io::ErrorKind::NotConnected
            {
                tracing::warn!(error = ?error, "HTTP socket shutdown failed");
            }
        } else {
            connections.sockets.insert(self.key, socket);
        }
    }
}
impl Drop for DrainStream {
    fn drop(&mut self) {
        self.connections
            .lock()
            .expect("service state lock poisoned")
            .sockets
            .remove(&self.key);
    }
}
impl AsyncRead for DrainStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}
impl AsyncWrite for DrainStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let result = Pin::new(&mut self.stream).poll_write(cx, buf);
        if let Poll::Ready(Ok(written)) = &result
            && *written > 0
        {
            self.write_progress
                .send_replace(tokio::time::Instant::now());
        }
        result
    }
    fn is_write_vectored(&self) -> bool {
        // TcpStream supports vectored writes. The current HTTP/1 server uses
        // its default writev=None/pipeline_flush=false and therefore queues
        // owner-backed Bytes until written, rather than flattening/copying
        // them. History response leases rely on this exact transport path.
        self.stream.is_write_vectored()
    }
    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let result = Pin::new(&mut self.stream).poll_write_vectored(cx, bufs);
        if let Poll::Ready(Ok(written)) = &result
            && *written > 0
        {
            self.write_progress
                .send_replace(tokio::time::Instant::now());
        }
        result
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}
impl Listener for DrainListener {
    type Io = DrainStream;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let permit = Arc::clone(&self.slots)
                .acquire_owned()
                .await
                .expect("invariant: HTTP connection admission is never explicitly closed");
            // Keep the library's accept-error handling and backoff.
            let (stream, address) = Listener::accept(&mut self.listener).await;
            let prepared = (|| {
                let stream = stream.into_std()?;
                let cancel = Arc::new(ConnectionSocket {
                    socket: stream.try_clone()?,
                    _permit: permit,
                });
                let key = cancel.socket.as_raw_fd();
                Ok::<_, io::Error>(DrainStream {
                    stream: TcpStream::from_std(stream)?,
                    response_socket: cancel.clone(),
                    write_progress: tokio::sync::watch::channel(tokio::time::Instant::now()).0,
                    cancel: Some(cancel),
                    key,
                    connections: self.connections.clone(),
                })
            })();
            match prepared {
                Ok(stream) => return (stream, address),
                Err(error) => {
                    tracing::warn!(error = ?error, "HTTP connection shutdown registration failed");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }
}

pub(crate) async fn serve(
    listener: TcpListener,
    app: Router,
    stopped: oneshot::Receiver<()>,
    limit: std::num::NonZeroUsize,
) -> io::Result<()> {
    serve_with_grace(listener, app, stopped, NETWORK_GRACE, limit).await
}

async fn serve_with_grace(
    listener: TcpListener,
    app: Router,
    stopped: oneshot::Receiver<()>,
    grace: Duration,
    limit: std::num::NonZeroUsize,
) -> io::Result<()> {
    let sockets = NetworkGuard::default();
    let listener = DrainListener {
        slots: Arc::new(Semaphore::new(limit.get())),
        listener,
        connections: sockets.0.clone(),
    }
    .tap_io(DrainStream::register);
    let (draining, drain_started) = oneshot::channel();
    let server = async move {
        http_routing::serve(
            listener,
            app.layer(from_fn(connection_context))
                .into_make_service_with_connect_info::<ConnectionInfo>(),
        )
        .with_graceful_shutdown(async move {
            match stopped.await {
                Ok(()) | Err(_) => {} // Sender cancellation also requests shutdown.
            }
            // A cancelled drain timer needs no notification.
            match draining.send(()) {
                Ok(()) | Err(()) => {}
            }
        })
        .await
    };
    let cutoff = async {
        if drain_started.await.is_err() {
            return pending::<io::Result<()>>().await;
        }
        tokio::time::sleep(grace).await;
        sockets
            .0
            .lock()
            .expect("service state lock poisoned")
            .close();
        tracing::info!("HTTP network grace ended; awaiting admitted storage work");
        // Never replace the HTTP drain or durability joins with a timeout.
        pending::<io::Result<()>>().await
    };
    tokio::select! {
        result = server => result,
        result = cutoff => result,
    }
}

#[cfg(test)]
#[path = "../../../test/server/unit/http_shutdown.rs"]
mod tests;
