use super::*;

#[tokio::test(flavor = "current_thread")]
async fn startup_io_does_not_pin_timers_or_health_on_a_single_worker() {
    use service_stack::ServiceExt;
    use std::time::Duration;
    let started = Arc::new(tokio::sync::Semaphore::new(0));
    let (release, wait) = std::sync::mpsc::channel();
    let work = tokio::spawn(startup_io({
        let started = started.clone();
        move || {
            started.add_permits(1);
            wait.recv_timeout(Duration::from_secs(3))
                .map_err(io::Error::other)?;
            Ok(())
        }
    }));
    started.acquire().await.unwrap().forget();
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(!work.is_finished());
    let health = http_routing::Router::new()
        .route("/healthz", http_routing::routing::get(|| async { "alive" }))
        .oneshot(
            http_routing::http::Request::builder()
                .uri("/healthz")
                .body(http_routing::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(health.status(), http_routing::http::StatusCode::OK);
    release.send(()).unwrap();
    work.await.unwrap().unwrap();
}

#[tokio::test]
async fn startup_rejects_nonprivate_and_aliased_directories_before_binding() {
    let root = tempfile::tempdir().unwrap();
    let parent = root.path().join("public");
    fs::create_dir(&parent).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o755)).unwrap();
    let socket = parent.join("rpc.sock");
    let spool = root.path().join("management/smb-spool");
    let durable = spool.clone();
    let error =
        startup_io(move || prepare_socket(&socket, &durable, crate::SpoolLimits::default()))
            .await
            .err()
            .unwrap();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert!(!spool.exists());
    let alias = root.path().join("alias");
    std::os::unix::fs::symlink(&parent, &alias).unwrap();
    let socket = alias.join("rpc.sock");
    let error = startup_io(move || prepare_socket(&socket, &spool, crate::SpoolLimits::default()))
        .await
        .err()
        .unwrap();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
}

#[test]
fn durable_spool_and_accounting_survive_runtime_directory_removal() {
    use platform_io::fs_safety::{EntryName, PrivateDirectory};
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let runtime = root.path().join("run/zxs");
    PrivateDirectory::create_trusted(&runtime).unwrap();
    // Model systemd's disposable runtime directory without binding a socket.
    fs::write(runtime.join("socket-marker"), b"disposable").unwrap();
    let durable = root.path().join("management/smb-spool");
    let limits = crate::SpoolLimits {
        bytes_per_account: 65536 + 7,
        bytes_total: 65536 + 7,
        recovery_reserve_bytes: 1,
    };
    let (spool, budget) = prepare_spool(&durable, limits.clone()).unwrap();
    assert_eq!(spool, durable);
    assert!(!runtime.join("smb-spool").exists());
    assert_eq!(fs::symlink_metadata(&spool).unwrap().mode() & 0o777, 0o700);
    let source = spool.join("retained");
    let lease = budget.reserve("owner", &source, 7, 100).unwrap();
    let directory = PrivateDirectory::open_trusted(&spool).unwrap();
    for (name, bytes) in [
        ("retained", b"payload".as_slice()),
        ("retained.work.json", b"unconfirmed operation".as_slice()),
    ] {
        let mut file = directory
            .open_payload(&EntryName::new(name).unwrap(), true)
            .unwrap();
        file.write_all(bytes).unwrap();
        file.sync_all().unwrap();
    }
    let owner_record = fs::read(spool.join("retained.budget.json")).unwrap();
    drop((lease, budget, directory));
    fs::remove_dir_all(&runtime).unwrap();
    assert_eq!(fs::read(&source).unwrap(), b"payload");
    assert_eq!(
        fs::read(spool.join("retained.work.json")).unwrap(),
        b"unconfirmed operation"
    );
    assert_eq!(
        fs::read(spool.join("retained.budget.json")).unwrap(),
        owner_record
    );
    // Recreate only runtime state and reopen the same durable inventory.
    PrivateDirectory::create_trusted(&runtime).unwrap();
    let (_, recovered) = prepare_spool(&durable, limits).unwrap();
    let error = recovered
        .reserve("owner", &durable.join("another"), 0, 100)
        .err()
        .unwrap();
    assert_eq!(
        error.code,
        storage_contract::ErrorCode::ResourceLimitExceeded
    );
    assert!(!durable.join("another.budget.json").exists());
}

#[test]
fn startup_validates_durable_spool_independently_before_binding() {
    use platform_io::fs_safety::PrivateDirectory;
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let runtime = root.path().join("run/zxs");
    PrivateDirectory::create_trusted(&runtime).unwrap();
    let socket = runtime.join("rpc.sock");
    let spool = root.path().join("management/smb-spool");
    PrivateDirectory::create_trusted(&spool).unwrap();
    fs::set_permissions(&spool, fs::Permissions::from_mode(0o755)).unwrap();
    let error = prepare_socket(&socket, &spool, crate::SpoolLimits::default())
        .err()
        .unwrap();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(fs::symlink_metadata(&spool).unwrap().mode() & 0o777, 0o755);
    assert!(!socket.exists());
    let alias = root.path().join("spool-alias");
    std::os::unix::fs::symlink(&spool, &alias).unwrap();
    let error = prepare_socket(&socket, &alias, crate::SpoolLimits::default())
        .err()
        .unwrap();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert!(!socket.exists());
    fs::set_permissions(&spool, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(spool.join("retained.budget.json"), b"invalid").unwrap();
    assert!(prepare_socket(&socket, &spool, crate::SpoolLimits::default()).is_err());
    assert!(!socket.exists());
    assert_eq!(
        fs::read(spool.join("retained.budget.json")).unwrap(),
        b"invalid"
    );
}

// Real AF_UNIX binding/registration is exercised by full-feature CI. Local
// sandboxes denying Unix sockets must not bypass that restriction to run this.
#[cfg(feature = "core")]
#[tokio::test]
async fn bind_and_register_prepared_socket_preserves_permissions_and_identity_cleanup() {
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let socket = root.path().join("rpc.sock");
    let path = socket.clone();
    let spool = root.path().join("management/smb-spool");
    let prepared = startup_io(move || prepare_socket(&path, &spool, crate::SpoolLimits::default()))
        .await
        .unwrap();
    assert_eq!(fs::symlink_metadata(&socket).unwrap().mode() & 0o777, 0o600);
    let listener = tokio::net::UnixListener::from_std(prepared.listener).unwrap();
    let client = tokio::net::UnixStream::connect(&socket).await.unwrap();
    let (server, _) = listener.accept().await.unwrap();
    drop((client, server, listener));
    drop(prepared.guard);
    assert!(!socket.exists());
}

#[test]
fn oversized_success_is_a_business_error_and_the_connection_remains_usable() {
    use std::os::unix::net::UnixStream;
    let (client, server) = UnixStream::pair().unwrap();
    let worker = std::thread::spawn(move || {
        let mut stream = FrameStream::new(server).unwrap();
        for operation_id in [None, Some("result-operation")] {
            let (descriptor, mut peer) = UnixStream::pair().unwrap();
            peer.set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .unwrap();
            let mut result = json!({"large":"x".repeat(smb_protocol::protocol::MAX_FRAME_BYTES)});
            if let Some(operation_id) = operation_id {
                result["operation_id"] = operation_id.into();
            }
            send_success(
                &mut stream,
                result,
                Some(descriptor.into()),
                Some("original-operation".into()),
            )
            .unwrap();
            assert_eq!(peer.read(&mut [0u8; 1]).unwrap(), 0);
        }
        send_success(&mut stream, json!({"alive":true}), None, None).unwrap();
    });
    let mut stream = FrameStream::new(client).unwrap();
    for expected_operation in ["original-operation", "result-operation"] {
        let (response, fd) = stream.receive::<Response>().unwrap();
        assert!(fd.is_none());
        assert!(
            matches!(response, Response::Error { code, operation_id:Some(id), retryable:false, .. } if code == storage_contract::ErrorCode::ResponseTooLarge && id == expected_operation)
        );
    }
    assert!(
        matches!(stream.receive::<Response>().unwrap().0, Response::Ok { result } if result["alive"] == true)
    );
    worker.join().unwrap();
}

#[test]
fn socket_and_connection_cleanup_during_unwind_consume_only_owned_poisoned_fds() {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    for whole_socket in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let (mut client, server) = std::os::unix::net::UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let connections = Arc::new(Mutex::new(BTreeMap::from([(1, server)])));
        assert!(
            catch_unwind(AssertUnwindSafe(|| {
                let _guard = connections.lock().unwrap();
                panic!("poison the FD cleanup inventory");
            }))
            .is_err()
        );
        let cleanup_connections = Arc::clone(&connections);
        assert!(
            catch_unwind(AssertUnwindSafe(move || {
                if whole_socket {
                    // No AF_UNIX bind: the absent path also exercises idempotent
                    // cleanup without touching any unrelated filesystem entry.
                    let _guard = SocketGuard {
                        path: root.path().join("absent.sock"),
                        identity: (0, 0),
                        connections: cleanup_connections,
                    };
                    panic!("drop socket owner during unwind");
                } else {
                    let _guard = ConnectionGuard {
                        id: 1,
                        connections: cleanup_connections,
                    };
                    panic!("drop connection owner during unwind");
                }
            }))
            .is_err()
        );
        assert_eq!(client.read(&mut [0]).unwrap(), 0);
        assert!(connections.is_poisoned());
        assert!(connections.lock().unwrap_err().into_inner().is_empty());
    }
}
