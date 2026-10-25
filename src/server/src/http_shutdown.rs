//! Bound network drain without abandoning admitted blocking storage work.
use http_routing::{
    Router,
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
    sync::oneshot,
};

const NETWORK_GRACE: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Connections {
    closed: bool,
    sockets: BTreeMap<RawFd, std::net::TcpStream>,
}
impl Connections {
    fn close(&mut self) {
        self.closed = true;
        for socket in self.sockets.values() {
            let _ = socket.shutdown(Shutdown::Both);
        }
        self.sockets.clear();
    }
}

#[derive(Default)]
struct NetworkGuard(Arc<Mutex<Connections>>);
impl Drop for NetworkGuard {
    fn drop(&mut self) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).close();
    }
}

struct DrainListener {
    listener: TcpListener,
    connections: Arc<Mutex<Connections>>,
}

struct DrainStream {
    stream: TcpStream,
    cancel: Option<std::net::TcpStream>,
    key: RawFd,
    connections: Arc<Mutex<Connections>>,
}
impl DrainStream {
    // TapIo supplies the standard SocketAddr connect-info implementation while
    // registering each connection before the HTTP library can spawn its task.
    fn register(&mut self) {
        let Some(socket) = self.cancel.take() else {
            return;
        };
        let mut connections = self.connections.lock().unwrap_or_else(|e| e.into_inner());
        if connections.closed {
            let _ = socket.shutdown(Shutdown::Both);
        } else {
            connections.sockets.insert(self.key, socket);
        }
    }
}
impl Drop for DrainStream {
    fn drop(&mut self) {
        self.connections
            .lock()
            .unwrap_or_else(|e| e.into_inner())
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
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }
    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
    }
    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write_vectored(cx, bufs)
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
            // Keep the library's accept-error handling and backoff.
            let (stream, address) = Listener::accept(&mut self.listener).await;
            let prepared = (|| {
                let stream = stream.into_std()?;
                let cancel = stream.try_clone()?;
                let key = cancel.as_raw_fd();
                Ok::<_, io::Error>(DrainStream {
                    stream: TcpStream::from_std(stream)?,
                    cancel: Some(cancel),
                    key,
                    connections: self.connections.clone(),
                })
            })();
            match prepared {
                Ok(stream) => return (stream, address),
                Err(error) => {
                    tracing::warn!(%error, "HTTP connection shutdown registration failed");
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
) -> io::Result<()> {
    serve_with_grace(listener, app, stopped, NETWORK_GRACE).await
}

async fn serve_with_grace(
    listener: TcpListener,
    app: Router,
    stopped: oneshot::Receiver<()>,
    grace: Duration,
) -> io::Result<()> {
    let sockets = NetworkGuard::default();
    let listener = DrainListener {
        listener,
        connections: sockets.0.clone(),
    }
    .tap_io(DrainStream::register);
    let (draining, drain_started) = oneshot::channel();
    let server = async move {
        http_routing::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            let _ = stopped.await;
            let _ = draining.send(());
        })
        .await
    };
    let cutoff = async {
        let _ = drain_started.await;
        tokio::time::sleep(grace).await;
        sockets.0.lock().unwrap_or_else(|e| e.into_inner()).close();
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
