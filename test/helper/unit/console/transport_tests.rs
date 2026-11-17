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
    drain.0.set(deadline).unwrap();
    drain.begin();
    assert_eq!(drain.0.get(), Some(&deadline));
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
