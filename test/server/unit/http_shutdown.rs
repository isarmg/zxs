use super::*;
use http_routing::{
    body::{Body, to_bytes},
    extract::Request,
    routing::{get, post},
};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::AsyncWriteExt;

const DEADLINE: Duration = Duration::from_secs(5);
const TEST_GRACE: Duration = Duration::from_millis(20);

async fn start(
    app: Router,
) -> (
    TcpStream,
    oneshot::Sender<()>,
    tokio::task::JoinHandle<io::Result<()>>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel();
    let server = tokio::spawn(serve_with_grace(listener, app, stopped, TEST_GRACE));
    let client = TcpStream::connect(address).await.unwrap();
    (client, stop, server)
}

async fn entered(receiver: oneshot::Receiver<()>) {
    tokio::time::timeout(DEADLINE, receiver)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn shutdown_ends_incomplete_request_body_after_network_grace() {
    let (admitted, active) = oneshot::channel();
    let admitted = Arc::new(Mutex::new(Some(admitted)));
    let body_completed = Arc::new(AtomicBool::new(false));
    let app = Router::new().route(
        "/upload",
        post({
            let body_completed = body_completed.clone();
            move |request: Request| {
                let admitted = admitted.lock().unwrap().take().unwrap();
                let body_completed = body_completed.clone();
                async move {
                    admitted.send(()).unwrap();
                    let body = to_bytes(request.into_body(), 1024).await;
                    body_completed.store(body.is_ok(), Ordering::SeqCst);
                    "finished"
                }
            }
        }),
    );
    let (mut client, stop, server) = start(app).await;
    client
        .write_all(b"POST /upload HTTP/1.1\r\nHost: localhost\r\nContent-Length: 10\r\n\r\nx")
        .await
        .unwrap();
    entered(active).await;
    assert!(!server.is_finished());
    stop.send(()).unwrap();
    tokio::time::timeout(DEADLINE, server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        !body_completed.load(Ordering::SeqCst),
        "incomplete body must not hold normal stop forever"
    );
}

struct ResponseDrop(Arc<AtomicBool>);
impl Drop for ResponseDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn shutdown_ends_unread_streaming_response_after_network_grace() {
    let (admitted, active) = oneshot::channel();
    let admitted = Arc::new(Mutex::new(Some(admitted)));
    let dropped = Arc::new(AtomicBool::new(false));
    let app = Router::new().route(
        "/download",
        get({
            let dropped = dropped.clone();
            move || {
                let admitted = admitted.lock().unwrap().take().unwrap();
                let guard = ResponseDrop(dropped.clone());
                async move {
                    admitted.send(()).unwrap();
                    // Reuse one bounded chunk; the peer intentionally never reads.
                    // No finite send-buffer size can finish this response early.
                    let chunk = bytes::Bytes::from(vec![7; 65536]);
                    Body::from_stream(futures::stream::unfold(
                        (guard, chunk),
                        |(guard, chunk)| async move {
                            Some((
                                Ok::<_, std::convert::Infallible>(chunk.clone()),
                                (guard, chunk),
                            ))
                        },
                    ))
                }
            }
        }),
    );
    let (mut client, stop, server) = start(app).await;
    client
        .write_all(b"GET /download HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    entered(active).await;
    assert!(!dropped.load(Ordering::SeqCst));
    stop.send(()).unwrap();
    tokio::time::timeout(DEADLINE, server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        dropped.load(Ordering::SeqCst),
        "response resources must be released"
    );
}

#[test]
fn network_cutoff_keeps_admitted_blocking_work_owned_until_runtime_exit() {
    use std::io::{Read, Write};
    let (admitted, active) = std::sync::mpsc::channel();
    let admitted = Arc::new(Mutex::new(Some(admitted)));
    let (release, released) = std::sync::mpsc::channel();
    let released = Arc::new(Mutex::new(Some(released)));
    let finished = Arc::new(AtomicBool::new(false));
    let app = Router::new().route(
        "/write",
        get({
            let finished = finished.clone();
            move || {
                let admitted = admitted.lock().unwrap().take().unwrap();
                let released = released.lock().unwrap().take().unwrap();
                let finished = finished.clone();
                async move {
                    tokio::task::spawn_blocking(move || {
                        admitted.send(()).unwrap();
                        released.recv_timeout(DEADLINE).unwrap();
                        finished.store(true, Ordering::SeqCst);
                    })
                    .await
                    .unwrap();
                    "persisted"
                }
            }
        }),
    );
    let (bound, address) = std::sync::mpsc::channel();
    let (done, exited) = std::sync::mpsc::channel();
    let (stop, stopped) = oneshot::channel();
    let host = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            bound.send(listener.local_addr().unwrap()).unwrap();
            tokio::time::timeout(
                DEADLINE,
                serve_with_grace(listener, app, stopped, TEST_GRACE),
            )
            .await
            .unwrap()
            .unwrap();
        });
        // A disconnected request may be dropped by the HTTP transport, but
        // runtime teardown must still join its admitted blocking operation.
        drop(runtime);
        done.send(()).unwrap();
    });
    let mut client = std::net::TcpStream::connect(address.recv_timeout(DEADLINE).unwrap()).unwrap();
    client.set_read_timeout(Some(DEADLINE)).unwrap();
    client
        .write_all(b"GET /write HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .unwrap();
    active.recv_timeout(DEADLINE).unwrap();
    stop.send(()).unwrap();
    // EOF/reset witnesses actual cutoff instead of guessing with a sleep.
    let mut bytes = Vec::new();
    if let Err(error) = client.read_to_end(&mut bytes) {
        assert!(
            matches!(
                error.kind(),
                io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted
            ),
            "{error}"
        );
    }
    assert!(!finished.load(Ordering::SeqCst));
    assert!(
        matches!(exited.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)),
        "runtime must not exit before admitted work finishes"
    );
    release.send(()).unwrap();
    exited.recv_timeout(DEADLINE).unwrap();
    host.join().unwrap();
    assert!(finished.load(Ordering::SeqCst));
}
