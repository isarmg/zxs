use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn listener(drain: Drain) -> BoundedListener {
    BoundedListener::new(TcpListener::bind("127.0.0.1:0").await.unwrap(), drain)
}

#[tokio::test]
async fn accepted_and_idle_connections_hold_capacity_until_the_stream_drops() {
    let mut listener = listener(Drain::default()).await;
    let address = listener.local_addr().unwrap();
    let mut clients = Vec::new();
    let mut connections = Vec::new();
    for _ in 0..MAX_CONNECTIONS {
        let mut client = TcpStream::connect(address).await.unwrap();
        let (mut connection, _) = listener.accept().await;
        client.write_all(b"request").await.unwrap();
        let mut bytes = [0; 7];
        connection.read_exact(&mut bytes).await.unwrap();
        assert_eq!(&bytes, b"request");
        connection.write_all(b"response").await.unwrap();
        let mut bytes = [0; 8];
        client.read_exact(&mut bytes).await.unwrap();
        assert_eq!(&bytes, b"response");
        clients.push(client);
        connections.push(connection);
    }
    assert_eq!(listener.capacity.available_permits(), 0);
    let waiting = TcpStream::connect(address).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), listener.accept())
            .await
            .is_err()
    );
    drop(connections.pop());
    let (connection, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
        .await
        .unwrap();
    assert_eq!(listener.capacity.available_permits(), 0);
    drop(connection);
    drop(connections);
    assert_eq!(listener.capacity.available_permits(), MAX_CONNECTIONS);
    drop(waiting);
    drop(clients);
}

#[tokio::test]
async fn cancelling_pending_accept_releases_its_reserved_capacity() {
    let mut listener = listener(Drain::default()).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(20), listener.accept())
            .await
            .is_err()
    );
    assert_eq!(listener.capacity.available_permits(), MAX_CONNECTIONS);
}

#[tokio::test]
async fn draining_wakes_pending_io_and_progress_cannot_extend_the_deadline() {
    let drain = Drain::default();
    let mut listener = listener(drain.clone()).await;
    let mut client = TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (mut connection, _) = listener.accept().await;
    let deadline = Instant::now() + Duration::from_millis(30);
    drain.deadline.set(deadline).unwrap();
    drain.begin();
    assert_eq!(drain.deadline.get(), Some(&deadline));
    connection.write_all(b"still draining").await.unwrap();
    let mut bytes = [0; 14];
    client.read_exact(&mut bytes).await.unwrap();
    assert_eq!(&bytes, b"still draining");
    let error = tokio::time::timeout(Duration::from_secs(5), connection.read(&mut bytes))
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert_eq!(
        connection.write_all(b"too late").await.unwrap_err().kind(),
        io::ErrorKind::TimedOut
    );
    assert_eq!(
        connection.flush().await.unwrap_err().kind(),
        io::ErrorKind::TimedOut
    );
    assert_eq!(
        connection.shutdown().await.unwrap_err().kind(),
        io::ErrorKind::TimedOut
    );
    assert_eq!(listener.capacity.available_permits(), MAX_CONNECTIONS - 1);
    drop(connection);
    assert_eq!(listener.capacity.available_permits(), MAX_CONNECTIONS);
}

#[tokio::test]
async fn shutdown_wakes_a_read_already_pending_before_any_deadline_exists() {
    use std::task::{Wake, Waker};

    struct ObservedWake(std::sync::atomic::AtomicUsize);
    impl Wake for ObservedWake {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    let drain = Drain::default();
    let mut listener = listener(drain.clone()).await;
    let _client = TcpStream::connect(listener.local_addr().unwrap()).await.unwrap();
    let (mut connection, _) = listener.accept().await;
    let observed = Arc::new(ObservedWake(std::sync::atomic::AtomicUsize::new(0)));
    let waker = Waker::from(Arc::clone(&observed));
    let mut context = Context::from_waker(&waker);
    let mut bytes = [0; 1];
    assert!(Pin::new(&mut connection).poll_read(
        &mut context, &mut ReadBuf::new(&mut bytes),
    ).is_pending());
    assert!(drain.deadline.get().is_none());
    let before = observed.0.load(std::sync::atomic::Ordering::SeqCst);

    // Shorten only this private test's absolute deadline; begin still performs
    // the production notification and must not replace the first deadline.
    let deadline = Instant::now() + Duration::from_millis(30);
    drain.deadline.set(deadline).unwrap();
    drain.begin();
    assert!(observed.0.load(std::sync::atomic::Ordering::SeqCst) > before);
    assert_eq!(drain.deadline.get(), Some(&deadline));
    let error = tokio::time::timeout(Duration::from_secs(5), connection.read(&mut bytes))
        .await.unwrap().unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    drop(connection);
    assert_eq!(listener.capacity.available_permits(), MAX_CONNECTIONS);
    let after = observed.0.load(std::sync::atomic::Ordering::SeqCst);
    drain.begin();
    assert_eq!(observed.0.load(std::sync::atomic::Ordering::SeqCst), after);
}
