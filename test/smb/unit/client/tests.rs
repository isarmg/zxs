use super::*;
use storage_contract::ErrorCode;

mod admission;
mod read;
mod write_sequence;

#[test]
fn module_client_requires_authenticated_account_and_durable_reservations() {
    for response in [
        json_codec::json!({"protocol":PROTOCOL,"capabilities":REQUIRED_CAPABILITIES}),
        json_codec::json!({"protocol":PROTOCOL,"capabilities":[crate::protocol::WRITE_RESERVATION_CAPABILITY]}),
        json_codec::json!({"protocol":PROTOCOL,"capabilities":[crate::protocol::AUTHENTICATED_ACCOUNT_CAPABILITY]}),
        // A peer that still announces every write with Dirty is a different contract.
        json_codec::json!({"protocol":PROTOCOL,"capabilities":[
            crate::protocol::WRITE_RESERVATION_CAPABILITY,
            crate::protocol::AUTHENTICATED_ACCOUNT_CAPABILITY,
        ]}),
        json_codec::json!({"protocol":PROTOCOL,"capabilities":[
            crate::protocol::WRITE_RESERVATION_CAPABILITY,
            crate::protocol::AUTHENTICATED_ACCOUNT_CAPABILITY,
            crate::protocol::GENERATION_DIRTY_CAPABILITY,
        ]}),
        json_codec::json!({"protocol":PROTOCOL}),
        json_codec::json!({"protocol":PROTOCOL,"capabilities":[]}),
        json_codec::json!({"protocol":PROTOCOL,"capabilities":["unrelated"]}),
        json_codec::json!({"protocol":"unsupported","capabilities":REQUIRED_CAPABILITIES}),
    ] {
        let accepted = response["protocol"] == PROTOCOL
            && response["capabilities"] == json_codec::json!(REQUIRED_CAPABILITIES);
        let (one, two) = UnixStream::pair().unwrap();
        let server = std::thread::spawn(move || {
            let mut stream = FrameStream::new(two).unwrap();
            let (request, descriptor) = stream.receive::<Request>().unwrap();
            assert!(descriptor.is_none());
            let Request::Hello {
                protocol,
                capabilities,
                username,
                repository_id,
            } = request
            else {
                panic!("client must negotiate before issuing handle requests");
            };
            assert_eq!(protocol, PROTOCOL);
            assert_eq!(capabilities, REQUIRED_CAPABILITIES);
            assert_eq!(username, "alice");
            assert_eq!(repository_id, "repository");
            stream
                .send(&Response::Ok { result: response }, None)
                .unwrap();
            assert_eq!(
                stream.receive::<Request>().unwrap_err().kind(),
                io::ErrorKind::UnexpectedEof,
                "rejected negotiation must close the connection without opening a handle"
            );
        });
        // connect() used by the actual VFS module delegates this handshake
        // unchanged after creating its UnixStream.
        let connected = Client::connect_stream(one, "alice", "repository");
        if accepted {
            assert!(connected.is_ok());
            drop(connected);
        } else {
            let error = connected.err().expect("unsupported peer was accepted");
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        }
        server.join().unwrap();
    }
}

#[test]
fn namespace_transport_failure_retains_identity_and_disconnects_every_clone() {
    for send_failure in [false, true] {
        let request = crate::protocol::namespace_request(
            crate::protocol::NamespaceOperation::Mkdir,
            "new".into(),
            None,
            false,
        );
        let Request::Namespace { operation_id, .. } = &request else {
            unreachable!()
        };
        let expected = *operation_id;
        let (one, two) = UnixStream::pair().unwrap();
        let (done, disconnected) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            if !send_failure {
                let mut stream = FrameStream::new(two).unwrap();
                let (received, _) = stream.receive::<Request>().unwrap();
                assert!(matches!(received, Request::Namespace { .. }));
                // The peer consumed the operation, but its reply is lost.
            } else {
                drop(two);
            }
            done.send(()).unwrap();
        });
        if send_failure {
            disconnected.recv().unwrap();
        }
        let client = Client(Arc::new(Mutex::new(Some(FrameStream::new(one).unwrap()))));
        let clone = client.clone();
        let error = client.call(request).unwrap_err();
        assert_eq!(
            error.kind(),
            if send_failure {
                io::ErrorKind::BrokenPipe
            } else {
                io::ErrorKind::UnexpectedEof
            }
        );
        assert_eq!(
            error_errno(&error),
            if send_failure { libc::EPIPE } else { libc::EIO }
        );
        let remote = error
            .get_ref()
            .and_then(|error| error.downcast_ref::<RemoteError>())
            .expect("transport failure lost the caller's operation identity");
        assert_eq!(
            remote.operation_id.as_deref(),
            Some(expected.to_string().as_str())
        );
        assert_eq!(remote.code, RemoteErrorCode::RpcOutcomeUnknown);
        assert!(!remote.retryable);
        assert_eq!(
            clone
                .call(Request::Stat { path: "new".into() })
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotConnected
        );
        server.join().unwrap();
    }
}

#[test]
fn terminal_close_releases_descriptor_on_business_error_poison_or_unwind() {
    use std::io::Read;
    for mode in ["business_error", "poison", "unwind"] {
        let (descriptor, mut peer) = UnixStream::pair().unwrap();
        peer.set_read_timeout(Some(std::time::Duration::from_secs(1)))
            .unwrap();
        let lock = Mutex::new(());
        if mode == "poison" {
            let _ = std::panic::catch_unwind(|| {
                let _guard = lock.lock().unwrap();
                panic!("injected I/O worker failure");
            });
        }
        let result = std::panic::catch_unwind(|| {
            close_descriptor(7, descriptor.into(), &lock, |request| {
                if mode == "poison" {
                    assert!(matches!(request, Request::Detach { handle: 7 }));
                    Ok(())
                } else {
                    assert!(matches!(request, Request::Close { handle: 7 }));
                    assert_ne!(mode, "unwind", "injected close callback panic");
                    Err(io::ErrorKind::PermissionDenied.into())
                }
            })
        });
        if mode == "unwind" {
            assert!(result.is_err());
        } else {
            let error = result.unwrap().unwrap_err();
            if mode == "poison" {
                assert_eq!(error.raw_os_error(), Some(libc::EIO));
            } else {
                assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
            }
        }
        assert_eq!(peer.read(&mut [0; 1]).unwrap(), 0, "descriptor leaked");
    }
}

#[test]
fn remote_error_retains_operation_and_filesystem_errno() {
    for (code, message, errno, retryable, operation) in [
        (
            ErrorCode::BranchNotActive,
            "merged file branch is read-only",
            libc::EROFS,
            false,
            None,
        ),
        (
            ErrorCode::FileWriteBusy,
            "another writer owns this file",
            libc::EBUSY,
            true,
            Some("op_pending"),
        ),
        (
            ErrorCode::FileBranchConflict,
            "file branch changed after the workspace began",
            libc::EAGAIN,
            false,
            None,
        ),
        (
            ErrorCode::DirectoryBranchConflict,
            "directory branch changed after the workspace began",
            libc::EAGAIN,
            false,
            None,
        ),
        (
            ErrorCode::HistoryChanged,
            "history root changed; refresh the history window",
            libc::EAGAIN,
            false,
            None,
        ),
        (
            ErrorCode::CommitPreparationBusy,
            "another workspace commit is being prepared",
            libc::EBUSY,
            true,
            Some("op_preparing"),
        ),
        (
            ErrorCode::CommitPreparationStale,
            "workspace or publication base changed during preparation",
            libc::EAGAIN,
            true,
            Some("op_stale"),
        ),
        (
            ErrorCode::CommitInDoubt,
            "query the original operation before retrying",
            libc::EIO,
            true,
            Some("op_uncertain"),
        ),
        (
            ErrorCode::DurabilityUnknown,
            "publication durability is unknown",
            libc::EIO,
            true,
            Some("op_published"),
        ),
    ] {
        let (one, two) = UnixStream::pair().unwrap();
        let server = std::thread::spawn(move || {
            let mut stream = FrameStream::new(two).unwrap();
            let (_request, fd) = stream.receive::<Request>().unwrap();
            assert!(fd.is_none());
            stream
                .send(
                    &Response::Error {
                        code,
                        message: message.into(),
                        retryable,
                        operation_id: operation.map(str::to_owned),
                    },
                    None,
                )
                .unwrap();
        });
        let client = Client(Arc::new(Mutex::new(Some(FrameStream::new(one).unwrap()))));
        let error = client
            .call(Request::Stat {
                path: "report.txt".into(),
            })
            .unwrap_err();
        assert_eq!(error_errno(&error), errno);
        let remote = error
            .get_ref()
            .unwrap()
            .downcast_ref::<RemoteError>()
            .unwrap();
        assert_eq!(remote.code, RemoteErrorCode::Server(code));
        assert_eq!(remote.message, message);
        assert_eq!(remote.operation_id.as_deref(), operation);
        assert_eq!(remote.retryable, retryable);
        assert!(error.to_string().contains(code.as_str()));
        assert!(error.to_string().contains(message));
        server.join().unwrap();
    }
}

#[test]
fn preparation_conflicts_do_not_reclassify_uncertain_or_integrity_failures() {
    assert_eq!(remote_errno(ErrorCode::CommitPreparationBusy), libc::EBUSY);
    assert_eq!(
        remote_errno(ErrorCode::CommitPreparationStale),
        libc::EAGAIN
    );
    // Only the two explicit workspace preparation conflicts are reclassified.
    // Generic preparation failures and unknown outcomes remain I/O failures,
    // even when their remote diagnostics separately permit a later retry.
    for code in [
        ErrorCode::CommitInDoubt,
        ErrorCode::DurabilityUnknown,
        ErrorCode::ChecksumMismatch,
    ] {
        assert_eq!(remote_errno(code), libc::EIO, "{code:?}");
    }
}

#[test]
fn invalid_frame_closes_every_clone_before_a_stale_success_can_be_reused() {
    use std::io::Write;
    let (one, mut two) = UnixStream::pair().unwrap();
    // Queue the invalid header and a well-formed response together. Merely
    // returning the parse error would leave that success for the next call.
    let stale = json_codec::to_vec(&Response::Ok {
        result: json_codec::json!({"stale": true}),
    })
    .unwrap();
    let mut frames = Vec::new();
    frames.extend_from_slice(&0_u32.to_be_bytes());
    frames.extend_from_slice(&(stale.len() as u32).to_be_bytes());
    frames.extend_from_slice(&stale);
    two.write_all(&frames).unwrap();
    let client = Client(Arc::new(Mutex::new(Some(FrameStream::new(one).unwrap()))));
    let clone = client.clone();
    assert_eq!(
        client
            .call(Request::Stat {
                path: "first".into()
            })
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidData
    );
    for handle in [&clone, &client] {
        assert_eq!(
            handle
                .call(Request::Stat {
                    path: "second".into()
                })
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotConnected
        );
    }
}

#[test]
fn remote_business_error_preserves_the_stream_for_the_next_call() {
    let (one, two) = UnixStream::pair().unwrap();
    let server = std::thread::spawn(move || {
        let mut stream = FrameStream::new(two).unwrap();
        stream.receive::<Request>().unwrap();
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
        let (request, _) = stream.receive::<Request>().unwrap();
        assert!(matches!(request, Request::Stat { path } if path == "exists"));
        stream
            .send(
                &Response::Ok {
                    result: json_codec::json!({"found": true}),
                },
                None,
            )
            .unwrap();
    });
    let client = Client(Arc::new(Mutex::new(Some(FrameStream::new(one).unwrap()))));
    let clone = client.clone();
    assert_eq!(
        error_errno(
            &client
                .call(Request::Stat {
                    path: "missing".into()
                })
                .unwrap_err()
        ),
        libc::ENOENT
    );
    assert_eq!(
        clone
            .call(Request::Stat {
                path: "exists".into()
            })
            .unwrap()["found"],
        true
    );
    server.join().unwrap();
}

#[test]
fn send_failure_also_disconnects_all_clones() {
    let (one, two) = UnixStream::pair().unwrap();
    drop(two);
    let client = Client(Arc::new(Mutex::new(Some(FrameStream::new(one).unwrap()))));
    let clone = client.clone();
    assert!(
        client
            .call(Request::Stat {
                path: "first".into()
            })
            .is_err()
    );
    assert_eq!(
        clone
            .call(Request::Stat {
                path: "second".into()
            })
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotConnected
    );
}

#[test]
fn response_descriptor_contract_failure_disconnects_every_clone() {
    use std::os::fd::AsFd;
    for mode in ["missing_open_fd", "unexpected_stat_fd", "error_fd"] {
        let (one, two) = UnixStream::pair().unwrap();
        let server = std::thread::spawn(move || {
            let mut stream = FrameStream::new(two).unwrap();
            stream.receive::<Request>().unwrap();
            let file = std::fs::File::open("/etc/hostname").unwrap();
            let response = if mode == "error_fd" {
                Response::Error {
                    code: ErrorCode::NotFound,
                    message: "missing".into(),
                    retryable: false,
                    operation_id: None,
                }
            } else {
                Response::Ok {
                    result: json_codec::json!({}),
                }
            };
            stream
                .send(&response, (mode != "missing_open_fd").then(|| file.as_fd()))
                .unwrap();
            assert_eq!(
                stream.receive::<Request>().unwrap_err().kind(),
                io::ErrorKind::UnexpectedEof
            );
        });
        let client = Client(Arc::new(Mutex::new(Some(FrameStream::new(one).unwrap()))));
        let clone = client.clone();
        let request = if mode == "missing_open_fd" {
            Request::Open {
                path: "file".into(),
                writable: false,
                create: false,
                truncate: false,
                exclusive: false,
                write_through: false,
            }
        } else {
            Request::Stat {
                path: "file".into(),
            }
        };
        let error = if mode == "missing_open_fd" {
            client.open(request).unwrap_err()
        } else {
            client.call(request).unwrap_err()
        };
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            error.to_string(),
            "RPC response descriptor does not match the request",
            "descriptor admission must precede malformed open metadata"
        );
        assert_eq!(
            clone
                .call(Request::Stat {
                    path: "next".into()
                })
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotConnected
        );
        server.join().unwrap();
    }
}

#[test]
fn successful_open_descriptor_keeps_the_shared_connection_reusable() {
    use std::os::fd::AsFd;
    let (one, two) = UnixStream::pair().unwrap();
    let server = std::thread::spawn(move || {
        let mut stream = FrameStream::new(two).unwrap();
        assert!(matches!(
            stream.receive::<Request>().unwrap().0,
            Request::Open { .. }
        ));
        let file = std::fs::File::open("/etc/hostname").unwrap();
        stream
            .send(
                &Response::Ok {
                    result: json_codec::json!({
                        "entry_id": "entry", "kind": "file", "path": "file",
                        "handle": "1", "size": "0", "max_file_bytes": "1024",
                        "version_id": null, "workspace_id": null
                    }),
                },
                Some(file.as_fd()),
            )
            .unwrap();
        assert!(
            matches!(stream.receive::<Request>().unwrap().0, Request::Stat { path } if path == "next")
        );
        stream
            .send(
                &Response::Ok {
                    result: json_codec::json!({"found": true}),
                },
                None,
            )
            .unwrap();
    });
    let client = Client(Arc::new(Mutex::new(Some(FrameStream::new(one).unwrap()))));
    let clone = client.clone();
    let (opened, descriptor) = client
        .open(Request::Open {
            path: "file".into(),
            writable: false,
            create: false,
            truncate: false,
            exclusive: false,
            write_through: false,
        })
        .unwrap();
    assert_eq!(opened.handle, 1);
    let file = std::fs::File::from(descriptor);
    assert!(file.metadata().unwrap().is_file());
    let result = clone
        .call(Request::Stat {
            path: "next".into(),
        })
        .unwrap();
    assert_eq!(result["found"], true);
    // The subsequent exchange must not consume or close the caller-owned FD.
    assert!(file.metadata().unwrap().is_file());
    server.join().unwrap();
}

#[test]
fn rejected_response_descriptors_are_closed_before_returning() {
    use std::{io::Read, os::fd::AsFd, time::Duration};
    for error_response in [false, true] {
        let (one, two) = UnixStream::pair().unwrap();
        let (descriptor, mut peer) = UnixStream::pair().unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let server = std::thread::spawn(move || {
            let mut stream = FrameStream::new(two).unwrap();
            stream.receive::<Request>().unwrap();
            let response = if error_response {
                Response::Error {
                    code: ErrorCode::NotFound,
                    message: "missing".into(),
                    retryable: false,
                    operation_id: None,
                }
            } else {
                Response::Ok {
                    result: json_codec::json!({}),
                }
            };
            stream.send(&response, Some(descriptor.as_fd())).unwrap();
            // Only the received SCM_RIGHTS copy remains after this drop.
            drop(descriptor);
        });
        let client = Client(Arc::new(Mutex::new(Some(FrameStream::new(one).unwrap()))));
        assert_eq!(
            client
                .call(Request::Stat {
                    path: "file".into()
                })
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        server.join().unwrap();
        assert_eq!(
            peer.read(&mut [0u8; 1]).unwrap(),
            0,
            "rejected descriptor leaked"
        );
    }
}

#[test]
fn malformed_open_result_closes_descriptor_and_disconnects_every_clone() {
    use std::{io::Read, os::fd::AsFd, time::Duration};
    for result in [
        json_codec::json!({}),
        json_codec::json!({
            "entry_id": "entry", "kind": "file", "path": "file",
            "handle": 1, "size": "0", "max_file_bytes": "1024",
            "version_id": null, "workspace_id": null
        }),
        json_codec::json!({
            "entry_id": "entry", "kind": "file", "path": "file",
            "handle": "1", "size": "0", "max_file_bytes": "1024",
            "version_id": null, "workspace_id": null, "unknown": true
        }),
    ] {
        let (one, two) = UnixStream::pair().unwrap();
        let (descriptor, mut peer) = UnixStream::pair().unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let server = std::thread::spawn(move || {
            let mut stream = FrameStream::with_timeout(two, Duration::from_secs(2)).unwrap();
            stream.receive::<Request>().unwrap();
            stream
                .send(&Response::Ok { result }, Some(descriptor.as_fd()))
                .unwrap();
            drop(descriptor);
            assert_eq!(
                stream.receive::<Request>().unwrap_err().kind(),
                io::ErrorKind::UnexpectedEof
            );
        });
        let client = Client(Arc::new(Mutex::new(Some(FrameStream::new(one).unwrap()))));
        let clone = client.clone();
        let error = client
            .open(Request::Open {
                path: "file".into(),
                writable: false,
                create: false,
                truncate: false,
                exclusive: false,
                write_through: false,
            })
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            clone
                .call(Request::Stat {
                    path: "next".into()
                })
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotConnected
        );
        server.join().unwrap();
        assert_eq!(
            peer.read(&mut [0u8; 1]).unwrap(),
            0,
            "malformed open leaked its descriptor"
        );
    }
}

#[test]
fn portable_name_and_path_rejections_are_invalid_input_not_io_failures() {
    // These are the concrete codes emitted by name-policy and forwarded by
    // the server. Keep malformed SMB names distinguishable from disk errors.
    for code in [
        ErrorCode::NameInvalidCharacter,
        ErrorCode::NameReserved,
        ErrorCode::NameNotNormalized,
        ErrorCode::PathInvalid,
    ] {
        assert_eq!(remote_errno(code), libc::EINVAL, "{code:?}");
    }
    assert_eq!(remote_errno(ErrorCode::NameTooLong), libc::ENAMETOOLONG);
    assert_eq!(remote_errno(ErrorCode::PathTooDeep), libc::ENAMETOOLONG);
    assert_eq!(remote_errno(ErrorCode::ChecksumMismatch), libc::EIO);
}

#[test]
fn close_failure_retains_both_errors_and_primary_errno() {
    let failure = CloseFailure {
        operation: io::Error::from_raw_os_error(libc::ENOSPC),
        close: io::Error::from_raw_os_error(libc::EIO),
    };
    let error = io::Error::new(io::ErrorKind::StorageFull, failure);
    assert_eq!(error_errno(&error), libc::ENOSPC);
    let failure = error
        .get_ref()
        .unwrap()
        .downcast_ref::<CloseFailure>()
        .unwrap();
    assert_eq!(failure.close.raw_os_error(), Some(libc::EIO));
    assert_eq!(
        std::error::Error::source(failure)
            .unwrap()
            .downcast_ref::<io::Error>()
            .unwrap()
            .raw_os_error(),
        Some(libc::ENOSPC)
    );
}

#[test]
fn ffi_errno_translation_keeps_typed_validation_sources() {
    let source = "invalid".parse::<u64>().unwrap_err();
    let error = io::Error::new(io::ErrorKind::InvalidInput, source);
    assert_eq!(error_errno(&error), libc::EINVAL);
    assert!(
        error
            .get_ref()
            .unwrap()
            .downcast_ref::<std::num::ParseIntError>()
            .is_some()
    );
    assert_eq!(
        error_errno(&io::Error::new(io::ErrorKind::PermissionDenied, "revoked")),
        libc::EACCES
    );
    assert_eq!(
        error_errno(&io::Error::new(io::ErrorKind::OutOfMemory, "allocation")),
        libc::ENOMEM
    );
}
