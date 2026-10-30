use super::*;
use std::{io::Read, os::fd::AsFd, time::Duration};

fn open_request() -> Request {
    Request::Open {
        path: "file".into(),
        writable: false,
        create: false,
        truncate: false,
        exclusive: false,
        write_through: false,
    }
}

#[test]
fn typed_open_transfers_metadata_and_descriptor_ownership_to_the_caller() {
    let (one, two) = UnixStream::pair().unwrap();
    let (descriptor, mut peer) = UnixStream::pair().unwrap();
    let server = std::thread::spawn(move || {
        let mut stream = FrameStream::with_timeout(two, Duration::from_secs(2)).unwrap();
        assert!(matches!(
            stream.receive::<Request>().unwrap().0,
            Request::Open { .. }
        ));
        stream
            .send(
                &Response::Ok {
                    result: json_codec::json!({
                        "entry_id": "entry", "kind": "file", "path": "file",
                        "handle": "18446744073709551615", "size": "9007199254740992",
                        "max_file_bytes": "18446744073709551615",
                        "version_id": "version", "workspace_id": null,
                        "mtime":{"state":"saved","unix_seconds":"1600000000"}
                    }),
                },
                Some(descriptor.as_fd()),
            )
            .unwrap();
        drop(descriptor);
    });
    let client = Client(Arc::new(Mutex::new(Some(FrameStream::new(one).unwrap()))));
    let clone = client.clone();
    let (opened, descriptor) = client.open(open_request()).unwrap();
    assert_eq!(opened.entry_id, "entry");
    assert_eq!(opened.kind, storage_contract::NamespaceEntryKind::File);
    assert_eq!(opened.path, "file");
    assert_eq!(opened.handle, u64::MAX);
    assert_eq!(opened.size, 1 << 53);
    assert_eq!(opened.max_file_bytes, u64::MAX);
    assert_eq!(opened.version_id.as_deref(), Some("version"));
    assert!(opened.workspace_id.is_none());
    server.join().unwrap();
    drop(client);
    drop(clone);
    peer.set_nonblocking(true).unwrap();
    assert_eq!(
        peer.read(&mut [0]).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    drop(descriptor);
    assert_eq!(
        peer.read(&mut [0]).unwrap(),
        0,
        "caller-owned descriptor leaked"
    );
}

#[test]
fn mismatched_public_entrypoints_reject_before_sending_or_closing_the_connection() {
    let (one, two) = UnixStream::pair().unwrap();
    let server = std::thread::spawn(move || {
        let mut stream = FrameStream::with_timeout(two, Duration::from_secs(2)).unwrap();
        assert!(
            matches!(stream.receive::<Request>().unwrap().0, Request::Stat { path } if path == "next")
        );
        stream
            .send(
                &Response::Ok {
                    result: Value::Bool(true),
                },
                None,
            )
            .unwrap();
    });
    let client = Client(Arc::new(Mutex::new(Some(FrameStream::new(one).unwrap()))));
    assert_eq!(
        client.call(open_request()).unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
    assert_eq!(
        client
            .open(Request::Stat {
                path: "invalid".into()
            })
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidInput
    );
    assert_eq!(
        client
            .call(Request::Stat {
                path: "next".into()
            })
            .unwrap(),
        true
    );
    server.join().unwrap();
}

#[test]
fn malformed_open_frames_reject_duplicate_fields_and_release_received_descriptors() {
    for payload in [
        br#"{"status":"ok","result":{"handle":"1","handle":"2"}}"#.as_slice(),
        br#"{"status":"ok","result":{"handle":"1","\u0068andle":"2"}}"#,
        br#"{"status":"ok","result":{"mtime":{"state":"saved","unix_seconds":"1","unix_seconds":"2"}}}"#,
        br#"{"status":"ok","result":{"mtime":{"state":"saved","unix_seconds":"1","\u0075nix_seconds":"2"}}}"#,
        br#"{"status":"ok","status":"error","result":{}}"#,
        br#"{"status":"ok","result":{"path":"\uD800"}}"#,
        br#"{"status":"ok","result":{}} trailing"#,
    ] {
        let (one, two) = UnixStream::pair().unwrap();
        let (descriptor, mut peer) = UnixStream::pair().unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let server = std::thread::spawn(move || {
            let mut stream = FrameStream::with_timeout(two, Duration::from_secs(2)).unwrap();
            stream.receive::<Request>().unwrap();
            stream
                .send_serialized(payload, Some(descriptor.as_fd()))
                .unwrap();
            drop(descriptor);
            assert_eq!(
                stream.receive::<Request>().unwrap_err().kind(),
                io::ErrorKind::UnexpectedEof
            );
        });
        let client = Client(Arc::new(Mutex::new(Some(FrameStream::new(one).unwrap()))));
        let clone = client.clone();
        let error = client.open(open_request()).unwrap_err();
        // FrameStream preserves strict decoder failures as io::Error::other.
        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert!(error.get_ref().is_some());
        for caller in [&client, &clone] {
            assert_eq!(
                caller
                    .call(Request::Stat {
                        path: "next".into()
                    })
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::NotConnected
            );
        }
        server.join().unwrap();
        assert_eq!(
            peer.read(&mut [0]).unwrap(),
            0,
            "malformed frame leaked its descriptor"
        );
    }
}

#[test]
fn open_business_error_does_not_close_the_shared_connection() {
    let (one, two) = UnixStream::pair().unwrap();
    let server = std::thread::spawn(move || {
        let mut stream = FrameStream::with_timeout(two, Duration::from_secs(2)).unwrap();
        assert!(matches!(
            stream.receive::<Request>().unwrap().0,
            Request::Open { .. }
        ));
        stream
            .send(
                &Response::Error {
                    code: ErrorCode::EntryNotFound,
                    message: "missing".into(),
                    operation_id: None,
                    retryable: false,
                },
                None,
            )
            .unwrap();
        assert!(matches!(
            stream.receive::<Request>().unwrap().0,
            Request::Stat { .. }
        ));
        stream
            .send(
                &Response::Ok {
                    result: Value::Bool(true),
                },
                None,
            )
            .unwrap();
    });
    let client = Client(Arc::new(Mutex::new(Some(FrameStream::new(one).unwrap()))));
    let clone = client.clone();
    assert_eq!(
        error_errno(&client.open(open_request()).unwrap_err()),
        libc::ENOENT
    );
    assert_eq!(
        clone
            .call(Request::Stat {
                path: "next".into()
            })
            .unwrap(),
        true
    );
    server.join().unwrap();
}

#[test]
fn namespace_timeout_preserves_original_operation_and_disconnects_every_clone() {
    let request = crate::protocol::namespace_request(
        crate::protocol::NamespaceOperation::Mkdir,
        "new".into(),
        None,
        false,
    );
    let Request::Namespace { operation_id, .. } = &request else {
        unreachable!()
    };
    let operation = operation_id.to_string();
    let (one, two) = UnixStream::pair().unwrap();
    let (done, finished) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let mut stream = FrameStream::with_timeout(two, Duration::from_secs(2)).unwrap();
        assert!(matches!(
            stream.receive::<Request>().unwrap().0,
            Request::Namespace { .. }
        ));
        // Keep the peer alive without a reply until the client reports timeout.
        finished.recv_timeout(Duration::from_secs(2)).unwrap();
    });
    let client = Client(Arc::new(Mutex::new(Some(
        FrameStream::with_timeout(one, Duration::from_millis(30)).unwrap(),
    ))));
    let clone = client.clone();
    let error = client.call(request).unwrap_err();
    assert!(matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    ));
    let remote = error
        .get_ref()
        .unwrap()
        .downcast_ref::<RemoteError>()
        .unwrap();
    assert_eq!(remote.code, RemoteErrorCode::RpcOutcomeUnknown);
    assert_eq!(remote.operation_id.as_deref(), Some(operation.as_str()));
    assert!(!remote.retryable);
    assert_eq!(remote.source.as_ref().unwrap().kind(), error.kind());
    assert_eq!(
        clone
            .call(Request::Stat {
                path: "next".into()
            })
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotConnected
    );
    done.send(()).unwrap();
    server.join().unwrap();
}
