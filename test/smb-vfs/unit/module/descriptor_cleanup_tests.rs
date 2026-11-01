use super::*;
use crate::module::{authentication, context_for_cleanup};
use smb_protocol::protocol::{FrameStream, PROTOCOL, REQUIRED_CAPABILITIES, Response};
use std::{
    error::Error,
    io::Read,
    os::unix::net::{UnixListener, UnixStream},
};

#[test]
fn close_retains_authentication_sources_and_detach_failure_without_saving() {
    for name in [c"alice", c"bob", c" alice", c"\xff"] {
        for detach_fails in [false, true] {
            let socket = std::env::temp_dir().join(format!(
                "zxs-vfs-auth-close-{}-{}.sock",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let listener = UnixListener::bind(&socket).unwrap();
            let authorized = name == c"alice";
            let worker = std::thread::spawn(move || {
                let mut wire = FrameStream::new(listener.accept().unwrap().0).unwrap();
                assert!(matches!(
                    wire.receive::<Request>().unwrap().0,
                    Request::Hello { .. }
                ));
                wire.send(
                    &Response::Ok {
                        result: json_codec::json!({
                            "protocol": PROTOCOL, "capabilities": REQUIRED_CAPABILITIES
                        }),
                    },
                    None,
                )
                .unwrap();
                let request = wire.receive::<Request>().unwrap().0;
                if authorized {
                    assert!(matches!(request, Request::Close { handle: 7 }));
                } else {
                    assert!(matches!(request, Request::Detach { handle: 7 }));
                }
                wire.send(
                    &if detach_fails {
                        Response::Error {
                            code: storage_contract::ErrorCode::InternalError,
                            message: "injected detach failure".into(),
                            retryable: false,
                            operation_id: None,
                        }
                    } else {
                        Response::Ok {
                            result: json_codec::json!({}),
                        }
                    },
                    None,
                )
                .unwrap();
                assert_eq!(
                    wire.receive::<Request>().unwrap_err().kind(),
                    std::io::ErrorKind::UnexpectedEof,
                    "terminal cleanup must send exactly one request"
                );
            });
            let client = Client::connect(&socket, "alice", "repository").unwrap();
            let (descriptor, mut peer) = UnixStream::pair().unwrap();
            peer.set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .unwrap();
            let opened = Open {
                info: Arc::new(OpenResult {
                    entry_id: "entry".into(),
                    kind: storage_contract::NamespaceEntryKind::File,
                    path: "file".into(),
                    handle: 7,
                    size: 0,
                    max_file_bytes: 1,
                    version_id: None,
                    workspace_id: None,
                    mtime: smb_protocol::protocol::FileMtime::Unpublished {},
                }),
                fd: descriptor.into(),
                writable: true,
                io: Arc::new(Mutex::new(WriteSequence::new(false))),
            };
            let mut owned_context = Box::new(Context {
                samba_username: "alice".into(),
                client,
                files: [(0, opened)].into_iter().collect(),
            });
            let mut session = authentication::tests::AuthenticatedSession::new();
            session.info.account_name = name.as_ptr().cast_mut();
            let mut host = session.handle();
            host.data = (&mut *owned_context as *mut Context).cast();
            // SAFETY: Real SDK/session objects and their Context remain live
            // and exclusively accessible during both helper calls.
            let authorization = unsafe { context(&mut host) }.map(|_| ());
            // SAFETY: The same live Context is borrowed after authorization ends.
            let cleanup = unsafe { context_for_cleanup(&mut host) }.unwrap();
            let opened = cleanup.files.remove(&0).unwrap();
            let result = finish_open(cleanup, opened, authorization);
            if authorized && !detach_fails {
                assert_eq!(result.unwrap(), 0);
            } else {
                let error = result.unwrap_err();
                if authorized {
                    assert_eq!(error_errno(&error), libc::EIO);
                } else {
                    assert_eq!(error_errno(&error), libc::EACCES);
                    let original = if detach_fails {
                        let combined = error
                            .get_ref()
                            .unwrap()
                            .downcast_ref::<DeniedCleanup>()
                            .unwrap();
                        assert!(
                            combined
                                .cleanup
                                .to_string()
                                .contains("injected detach failure")
                        );
                        assert!(combined.source().is_some());
                        &combined.authentication
                    } else {
                        &error
                    };
                    if name == c"bob" {
                        assert_eq!(original.raw_os_error(), Some(libc::EACCES));
                    } else {
                        let source = original
                            .get_ref()
                            .unwrap()
                            .downcast_ref::<std::io::Error>()
                            .unwrap();
                        assert_eq!(source.kind(), std::io::ErrorKind::InvalidInput);
                        if name == c"\xff" {
                            assert!(
                                source
                                    .get_ref()
                                    .unwrap()
                                    .downcast_ref::<std::str::Utf8Error>()
                                    .is_some()
                            );
                        }
                    }
                }
            }
            assert_eq!(peer.read(&mut [0]).unwrap(), 0, "local FD leaked");
            assert!(owned_context.files.is_empty());
            // SAFETY: The live host context has no owner at key zero. A second
            // close must not send another RPC or consume another descriptor.
            let repeated = unsafe { crate::module::close(&mut host, std::ptr::null_mut()) };
            assert_eq!(repeated, -1);
            drop(owned_context);
            worker.join().unwrap();
            std::fs::remove_file(socket).unwrap();
        }
    }
}

#[test]
fn denied_cleanup_maps_the_primary_raw_errno_not_only_its_kind() {
    let error = std::io::Error::new(
        std::io::Error::from_raw_os_error(libc::EBADF).kind(),
        DeniedCleanup {
            authentication: std::io::Error::from_raw_os_error(libc::EBADF),
            cleanup: std::io::Error::from_raw_os_error(libc::EIO),
        },
    );
    assert_eq!(error_errno(&error), libc::EBADF);
}

#[test]
fn missing_cleanup_context_does_not_replace_the_authentication_errno() {
    for name in [c"alice", c"\xff"] {
        let mut session = authentication::tests::AuthenticatedSession::new();
        session.info.account_name = name.as_ptr().cast_mut();
        let mut host = session.handle();
        // SAFETY: The real SDK objects are live; the absent Context and opaque
        // null fsp key must be rejected without dereferencing either.
        let result = unsafe { crate::module::close(&mut host, std::ptr::null_mut()) };
        assert_eq!(result, -1);
        // SAFETY: errno is a live thread-local integer slot.
        let actual_errno = unsafe { *libc::__errno_location() };
        assert_eq!(
            actual_errno,
            if name == c"alice" {
                libc::EINVAL
            } else {
                libc::EACCES
            }
        );
    }
}
