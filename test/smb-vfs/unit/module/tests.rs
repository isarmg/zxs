use super::descriptors::close_opened;
use super::*;

#[test]
fn typed_handle_stat_matches_path_stat_without_json_reencoding() {
    for (kind, size) in [
        (NamespaceEntryKind::File, 0),
        (NamespaceEntryKind::File, 513),
        (NamespaceEntryKind::File, i64::MAX),
        (NamespaceEntryKind::Directory, 0),
    ] {
        let mut direct = sdk::SMB_STRUCT_STAT::default();
        let mut decoded = sdk::SMB_STRUCT_STAT::default();
        let node = json_codec::json!({"kind": kind, "entry_id": "entry", "size": size.to_string()});
        // SAFETY: Both outputs are live, exclusive local SDK stat structures.
        unsafe {
            fill_stat_fields(kind, "entry", size, &mut direct).unwrap();
            fill_stat(&node, &mut decoded).unwrap();
        }
        assert_eq!(direct.st_ex_dev, decoded.st_ex_dev);
        assert_eq!(direct.st_ex_ino, inode("entry"));
        assert_eq!(direct.st_ex_ino, decoded.st_ex_ino);
        assert_eq!(direct.st_ex_mode, decoded.st_ex_mode);
        assert_eq!(
            direct.st_ex_mode & libc::S_IFMT,
            if kind == NamespaceEntryKind::Directory {
                libc::S_IFDIR
            } else {
                libc::S_IFREG
            }
        );
        assert_eq!(direct.st_ex_nlink, 1);
        assert_eq!(direct.st_ex_uid, decoded.st_ex_uid);
        assert_eq!(direct.st_ex_gid, decoded.st_ex_gid);
        assert_eq!(direct.st_ex_size, size);
        assert_eq!(direct.st_ex_size, decoded.st_ex_size);
        assert_eq!(direct.st_ex_blksize, decoded.st_ex_blksize);
        assert_eq!(direct.st_ex_blocks, (size as u64).div_ceil(512) as i64);
        assert_eq!(direct.st_ex_blocks, decoded.st_ex_blocks);
    }
    let mut output = sdk::SMB_STRUCT_STAT::default();
    // SAFETY: The local output is writable; null is rejected before dereference.
    unsafe {
        assert!(fill_stat_fields(NamespaceEntryKind::File, "entry", -1, &mut output).is_err());
        assert!(
            fill_stat_fields(NamespaceEntryKind::File, "entry", 0, std::ptr::null_mut()).is_err()
        );
        let too_large =
            json_codec::json!({"kind": "file", "entry_id": "entry", "size": u64::MAX.to_string()});
        assert!(fill_stat(&too_large, &mut output).is_err());
        let unknown = json_codec::json!({"kind":"future_kind", "entry_id":"entry", "size":"0"});
        assert!(fill_stat(&unknown, &mut output).is_err());
    }
}

#[test]
fn input_buffer_checks_null_empty_and_oversize_before_borrowing() {
    // SAFETY: null/zero and invalid sizes must be rejected before dereference;
    // the only nonempty valid buffer is a live local array borrowed here.
    unsafe {
        assert!(input_bytes(std::ptr::null(), 0).unwrap().is_empty());
        assert!(input_bytes(std::ptr::null(), 1).is_err());
        let bytes = [1_u8, 2, 3];
        assert!(input_bytes(bytes.as_ptr().cast(), usize::MAX).is_err());
        assert_eq!(
            input_bytes(bytes.as_ptr().cast(), bytes.len()).unwrap(),
            &bytes
        );
    }
}

#[test]
fn removed_open_descriptor_is_closed_on_poisoned_lock_and_remote_save_failure() {
    use std::{io::Read, os::unix::net::UnixStream};
    for poisoned in [true, false] {
        let (descriptor, mut peer) = UnixStream::pair().unwrap();
        peer.set_read_timeout(Some(std::time::Duration::from_secs(1)))
            .unwrap();
        let lock = Arc::new(Mutex::new(WriteSequence::new(false)));
        if poisoned {
            let _ = std::panic::catch_unwind(|| {
                let _guard = lock.lock().unwrap();
                panic!("simulate a failed I/O worker");
            });
        }
        let opened = Open {
            info: Arc::new(OpenResult {
                entry_id: "entry".into(),
                kind: storage_contract::NamespaceEntryKind::File,
                path: "file.txt".into(),
                handle: 7,
                size: 0,
                max_file_bytes: 1,
                version_id: None,
                workspace_id: None,
            }),
            fd: descriptor.into(),
            writable: true,
            io: lock,
        };
        let error = close_opened(opened, |request| {
            if poisoned {
                assert!(matches!(request, Request::Detach { handle: 7 }));
                Ok(())
            } else {
                assert!(matches!(request, Request::Close { handle: 7 }));
                Err(std::io::ErrorKind::PermissionDenied.into())
            }
        })
        .unwrap_err();
        if poisoned {
            assert_eq!(error.raw_os_error(), Some(libc::EIO));
        } else {
            assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        }
        assert_eq!(
            peer.read(&mut [0; 1]).unwrap(),
            0,
            "removed descriptor leaked"
        );
    }
}
#[test]
fn failed_name_lookup_preserves_the_callers_filename() {
    let original = c"new.txt".as_ptr().cast_mut();
    let mut found = original;
    // SAFETY: The output points to a live local slot and the static invalid component is
    // rejected before any null host handle is accessed.
    let status = unsafe {
        real_filename(
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            c"..".as_ptr(),
            std::ptr::null_mut(),
            &mut found,
        )
    };
    assert_ne!(status.v, 0);
    assert_eq!(found, original);
}
#[test]
fn absolute_names_are_confined_to_the_exact_share_root() {
    let root = c"/safe/share";
    let mut connection = sdk::connection_struct {
        connectpath: root.as_ptr().cast_mut(),
        ..Default::default()
    };
    let mut handle = sdk::vfs_handle_struct {
        conn: &mut connection,
        ..Default::default()
    };
    let mut file: sdk::smb_filename = Default::default();
    for (input, expected) in [
        ("/safe/share", Some("")),
        ("/safe/share/./文件.txt", Some("文件.txt")),
        ("./documents/file.txt", Some("documents/file.txt")),
        ("/safe/share-other/file", None),
        ("/safe/share/../secret", None),
        ("/outside/file", None),
        ("documents/../../secret", None),
    ] {
        let input = std::ffi::CString::new(input).unwrap();
        file.base_name = input.as_ptr().cast_mut();
        // SAFETY: All SDK objects and CString storage are local, initialized, and live until
        // name returns; their pointers reference those objects.
        let result = unsafe { name(&mut handle, &file) };
        match expected {
            Some(expected) => assert_eq!(result.unwrap(), expected),
            None => assert!(result.is_err()),
        }
    }
}

#[test]
fn readdir_keeps_one_page_and_rewind_requests_a_fresh_snapshot() {
    use smb_protocol::protocol::{FrameStream, PROTOCOL, REQUIRED_CAPABILITIES, Response};
    use std::{
        ffi::CStr,
        os::unix::net::UnixListener,
        time::{SystemTime, UNIX_EPOCH},
    };
    let socket = std::env::temp_dir().join(format!(
        "zxs-vfs-{}-{}.sock",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let listener = UnixListener::bind(&socket).unwrap();
    let worker = std::thread::spawn(move || {
        let mut stream = FrameStream::new(listener.accept().unwrap().0).unwrap();
        assert!(matches!(
            stream.receive::<Request>().unwrap().0,
            Request::Hello { .. }
        ));
        stream.send(&Response::Ok { result: json_codec::json!({"protocol": PROTOCOL, "capabilities": REQUIRED_CAPABILITIES}) }, None).unwrap();
        for (restart, names, more) in [
            (false, vec!["one", "two"], true),
            (false, vec!["three"], false),
            (true, vec!["fresh"], false),
        ] {
            assert!(
                matches!(stream.receive::<Request>().unwrap().0, Request::ReadDirectory {handle: 7, restart: actual} if actual == restart)
            );
            let entries: Vec<_> = names
                .iter()
                .map(|name| {
                    json_codec::json!({
                        "entry_id": name, "name": name, "lookup_key": name, "path": name,
                        "kind": "file", "file_id": "file", "version_id": "version", "size": "0",
                    })
                })
                .collect();
            stream
                .send(
                    &Response::Ok {
                        result: json_codec::json!({"entries": entries, "more": more}),
                    },
                    None,
                )
                .unwrap();
        }
        assert!(matches!(
            stream.receive::<Request>().unwrap().0,
            Request::Close { handle: 7 }
        ));
        stream
            .send(
                &Response::Ok {
                    result: json_codec::json!({"closed": true}),
                },
                None,
            )
            .unwrap();
    });
    let client = Client::connect(&socket, "alice", "repository").unwrap();
    let mut fsp: sdk::files_struct = Default::default();
    let key = &mut fsp as *mut sdk::files_struct as usize;
    let descriptor = std::fs::File::open("/dev/null").unwrap().into();
    let opened = Open {
        info: Arc::new(OpenResult {
            entry_id: "directory".into(),
            kind: storage_contract::NamespaceEntryKind::Directory,
            path: "".into(),
            handle: 7,
            size: 0,
            max_file_bytes: 1,
            version_id: None,
            workspace_id: None,
        }),
        fd: descriptor,
        writable: false,
        io: Arc::new(Mutex::new(WriteSequence::new(false))),
    };
    let context = Box::new(Context {
        samba_username: "alice".into(),
        client,
        files: [(key, opened)].into_iter().collect(),
    });
    let mut authenticated = authentication::tests::AuthenticatedSession::new();
    let mut host = authenticated.handle();
    host.data = Box::into_raw(context).cast();
    // SAFETY: The host handle and nested authenticated SDK objects stay live.
    // fsp is an opaque owner key. fdopendir returns the unique Directory and
    // each name is read before its page can change; closedir consumes it once.
    unsafe {
        let directory = fdopendir(&mut host, &mut fsp, std::ptr::null(), 0).cast::<Directory>();
        assert!(!directory.is_null());
        assert_eq!((*directory).owner, key);
        for (name, page_size) in [("one", 2), ("two", 2), ("three", 1)] {
            let entry = readdir(&mut host, &mut fsp, directory.cast());
            assert!(!entry.is_null());
            assert_eq!(
                CStr::from_ptr((*entry).d_name.as_ptr()).to_str().unwrap(),
                name
            );
            assert_eq!((*directory).entries.len(), page_size);
            if name == "one" {
                // Cached "two" must not escape after guest/forced reauthentication.
                authenticated.connection.force_user = true;
                assert!(readdir(&mut host, &mut fsp, directory.cast()).is_null());
                assert_eq!(*libc::__errno_location(), libc::EACCES);
                authenticated.connection.force_user = false;
                authenticated.info.account_name = c"different-account".as_ptr().cast_mut();
                assert!(readdir(&mut host, &mut fsp, directory.cast()).is_null());
                assert_eq!(*libc::__errno_location(), libc::EACCES);
                authenticated.info.account_name = c"alice".as_ptr().cast_mut();
            }
        }
        assert!(readdir(&mut host, &mut fsp, directory.cast()).is_null());
        rewind(&mut host, directory.cast());
        assert!((*directory).entries.is_empty());
        let entry = readdir(&mut host, &mut fsp, directory.cast());
        assert!(!entry.is_null());
        assert_eq!(
            CStr::from_ptr((*entry).d_name.as_ptr()).to_str().unwrap(),
            "fresh"
        );
        assert_eq!(closedir(&mut host, directory.cast()), 0);
        assert!(context_for_cleanup(&mut host).unwrap().files.is_empty());
        free_context(&mut host.data);
    }
    worker.join().unwrap();
    std::fs::remove_file(socket).unwrap();
}

#[test]
fn directory_cleanup_preserves_admission_failure_and_is_terminal_after_errors() {
    use smb_protocol::protocol::{FrameStream, PROTOCOL, REQUIRED_CAPABILITIES, Response};
    use std::{
        fs::File,
        io::Read,
        os::{fd::FromRawFd, unix::net::UnixListener},
    };
    for scenario in ["first_page", "later_page", "close_error", "revoked"] {
        let socket = std::env::temp_dir().join(format!(
            "zxs-vfs-directory-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let listener = UnixListener::bind(&socket).unwrap();
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
            assert!(matches!(
                wire.receive::<Request>().unwrap().0,
                Request::ReadDirectory {
                    handle: 7,
                    restart: false
                }
            ));
            let error = || Response::Error {
                code: storage_contract::ErrorCode::InternalError,
                message: "injected directory failure".into(),
                retryable: false,
                operation_id: None,
            };
            if scenario == "first_page" {
                wire.send(&error(), None).unwrap();
            } else {
                wire.send(
                    &Response::Ok {
                        result: json_codec::json!({
                            "entries": [], "more": false,
                        }),
                    },
                    None,
                )
                .unwrap();
            }
            if scenario == "later_page" {
                assert!(matches!(
                    wire.receive::<Request>().unwrap().0,
                    Request::ReadDirectory {
                        handle: 7,
                        restart: true
                    }
                ));
                wire.send(&error(), None).unwrap();
            }
            let request = wire.receive::<Request>().unwrap().0;
            if scenario == "revoked" {
                assert!(matches!(request, Request::Detach { handle: 7 }));
            } else {
                assert!(matches!(request, Request::Close { handle: 7 }));
            }
            wire.send(
                &if scenario == "close_error" {
                    error()
                } else {
                    Response::Ok {
                        result: json_codec::json!({"closed": true}),
                    }
                },
                None,
            )
            .unwrap();
        });
        let client = Client::connect(&socket, "alice", "repository").unwrap();
        let mut descriptors = [-1; 2];
        // SAFETY: pipe2 creates two local nonblocking descriptors in writable storage.
        let result =
            unsafe { libc::pipe2(descriptors.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) };
        assert_eq!(result, 0);
        // SAFETY: Successful pipe2 provides the sole-owned read endpoint.
        let mut observer = unsafe { File::from_raw_fd(descriptors[0]) };
        let mut fsp: sdk::files_struct = Default::default();
        let key = &mut fsp as *mut sdk::files_struct as usize;
        let opened = Open {
            info: Arc::new(OpenResult {
                entry_id: "directory".into(),
                kind: storage_contract::NamespaceEntryKind::Directory,
                path: "".into(),
                handle: 7,
                size: 0,
                max_file_bytes: 1,
                version_id: None,
                workspace_id: None,
            }),
            // SAFETY: Successful pipe2 returned this unique write endpoint.
            fd: unsafe { OwnedFd::from_raw_fd(descriptors[1]) },
            writable: false,
            io: Arc::new(Mutex::new(WriteSequence::new(false))),
        };
        let context = Box::new(Context {
            samba_username: "alice".into(),
            client,
            files: [(key, opened)].into_iter().collect(),
        });
        let mut authenticated = authentication::tests::AuthenticatedSession::new();
        let mut host = authenticated.handle();
        host.data = Box::into_raw(context).cast();
        // SAFETY: These are initialized real SDK objects with live authenticated
        // nested objects. Directory is consumed only once; fsp is an opaque key.
        unsafe {
            let directory = fdopendir(&mut host, &mut fsp, std::ptr::null(), 0);
            assert!(
                context_for_cleanup(&mut host)
                    .unwrap()
                    .files
                    .contains_key(&key)
            );
            if scenario == "first_page" {
                assert!(directory.is_null());
                assert_eq!(close(&mut host, &mut fsp), 0);
            } else {
                assert!(!directory.is_null());
                if scenario == "later_page" {
                    rewind(&mut host, directory);
                    assert!(readdir(&mut host, &mut fsp, directory).is_null());
                    assert_eq!(*libc::__errno_location(), libc::EIO);
                }
                if scenario == "revoked" {
                    authenticated.connection.force_user = true;
                }
                let result = closedir(&mut host, directory);
                assert_eq!(
                    result,
                    if matches!(scenario, "close_error" | "revoked") {
                        -1
                    } else {
                        0
                    }
                );
                if scenario == "revoked" {
                    assert_eq!(*libc::__errno_location(), libc::EACCES);
                }
            }
            assert!(context_for_cleanup(&mut host).unwrap().files.is_empty());
            free_context(&mut host.data);
        }
        assert_eq!(
            observer.read(&mut [0]).unwrap(),
            0,
            "directory descriptor leaked"
        );
        worker.join().unwrap();
        std::fs::remove_file(socket).unwrap();
    }
}

#[test]
fn revoked_close_detaches_without_publishing_and_releases_descriptor() {
    use smb_protocol::protocol::{FrameStream, PROTOCOL, REQUIRED_CAPABILITIES, Response};
    use std::{
        io::Read,
        os::unix::net::{UnixListener, UnixStream},
    };
    let socket = std::env::temp_dir().join(format!(
        "zxs-vfs-revoked-close-{}-{}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    let listener = UnixListener::bind(&socket).unwrap();
    let worker = std::thread::spawn(move || {
        let mut wire = FrameStream::new(listener.accept().unwrap().0).unwrap();
        assert!(matches!(
            wire.receive::<Request>().unwrap().0,
            Request::Hello { .. }
        ));
        wire.send(&Response::Ok { result: json_codec::json!({"protocol": PROTOCOL, "capabilities": REQUIRED_CAPABILITIES}) }, None).unwrap();
        assert!(matches!(
            wire.receive::<Request>().unwrap().0,
            Request::Detach { handle: 7 }
        ));
        wire.send(
            &Response::Ok {
                result: json_codec::json!({"detached": true}),
            },
            None,
        )
        .unwrap();
        assert_eq!(
            wire.receive::<Request>().unwrap_err().kind(),
            std::io::ErrorKind::UnexpectedEof
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
        }),
        fd: descriptor.into(),
        writable: true,
        io: Arc::new(Mutex::new(WriteSequence::new(false))),
    };
    let context = Box::new(Context {
        samba_username: "alice".into(),
        client,
        files: [(0, opened)].into_iter().collect(),
    });
    let mut authenticated = authentication::tests::AuthenticatedSession::new();
    let mut host = authenticated.handle();
    host.data = Box::into_raw(context).cast();
    authenticated.connection.force_user = true;
    // SAFETY: Real SDK structs and nested data remain live here; close uses fsp
    // only as an opaque lookup key, and free_context consumes the unique box.
    unsafe {
        assert_eq!(close(&mut host, std::ptr::null_mut()), -1);
        assert_eq!(*libc::__errno_location(), libc::EACCES);
        assert!(context_for_cleanup(&mut host).unwrap().files.is_empty());
        free_context(&mut host.data);
    }
    assert_eq!(peer.read(&mut [0u8; 1]).unwrap(), 0);
    worker.join().unwrap();
    std::fs::remove_file(socket).unwrap();
}

#[test]
fn removed_open_owns_descriptor_through_success_and_unwind() {
    use std::{io::Read, os::unix::net::UnixStream};
    for panic_during_finish in [false, true] {
        let (descriptor, mut peer) = UnixStream::pair().unwrap();
        peer.set_read_timeout(Some(std::time::Duration::from_secs(1)))
            .unwrap();
        let opened = Open {
            info: Arc::new(OpenResult {
                entry_id: "entry".into(),
                kind: NamespaceEntryKind::File,
                path: "file".into(),
                handle: 7,
                size: 0,
                max_file_bytes: 1,
                version_id: None,
                workspace_id: None,
            }),
            fd: descriptor.into(),
            writable: true,
            io: Arc::new(Mutex::new(WriteSequence::new(false))),
        };
        let finished = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            close_opened(opened, |request| {
                assert!(matches!(request, Request::Close { handle: 7 }));
                assert!(!panic_during_finish, "simulate finish unwinding");
                Ok(())
            })
        }));
        if panic_during_finish {
            assert!(finished.is_err());
        } else {
            assert_eq!(finished.unwrap().unwrap(), 0);
        }
        assert_eq!(peer.read(&mut [0]).unwrap(), 0);
    }
}

#[test]
fn context_cleanup_closes_owned_records_without_publishing_and_snapshot_outlives_it() {
    use smb_protocol::protocol::{FrameStream, PROTOCOL, REQUIRED_CAPABILITIES, Response};
    use std::{
        io::{ErrorKind, Read},
        os::unix::net::{UnixListener, UnixStream},
    };
    let socket = std::env::temp_dir().join(format!(
        "zxs-vfs-owned-context-{}-{}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    let listener = UnixListener::bind(&socket).unwrap();
    let worker = std::thread::spawn(move || {
        let mut wire = FrameStream::new(listener.accept().unwrap().0).unwrap();
        assert!(matches!(
            wire.receive::<Request>().unwrap().0,
            Request::Hello { .. }
        ));
        wire.send(
            &Response::Ok {
                result: json_codec::json!({
                    "protocol": PROTOCOL, "capabilities": REQUIRED_CAPABILITIES,
                }),
            },
            None,
        )
        .unwrap();
        assert_eq!(
            wire.receive::<Request>().unwrap_err().kind(),
            ErrorKind::UnexpectedEof,
            "terminal context cleanup must not publish or abort a remote handle"
        );
    });
    let client = Client::connect(&socket, "alice", "repository").unwrap();
    let mut files = BTreeMap::new();
    let mut peers = Vec::new();
    for key in 0..2 {
        let (descriptor, peer) = UnixStream::pair().unwrap();
        peer.set_nonblocking(true).unwrap();
        peers.push(peer);
        files.insert(
            key,
            Open {
                info: Arc::new(OpenResult {
                    entry_id: format!("entry-{key}"),
                    kind: NamespaceEntryKind::File,
                    path: format!("file-{key}"),
                    handle: key as u64,
                    size: 0,
                    max_file_bytes: 1,
                    version_id: None,
                    workspace_id: None,
                }),
                fd: descriptor.into(),
                writable: true,
                io: Arc::new(Mutex::new(WriteSequence::new(false))),
            },
        );
    }
    let context = Box::new(Context {
        samba_username: "alice".into(),
        client,
        files,
    });
    let mut authenticated = authentication::tests::AuthenticatedSession::new();
    let mut host = authenticated.handle();
    host.data = Box::into_raw(context).cast();
    // SAFETY: The real SDK handle and authenticated nested objects remain live;
    // null fsp is only the opaque map key 0, not dereferenced by snapshot.
    let held = unsafe { snapshot(&mut host, std::ptr::null_mut()) }.unwrap();
    // SAFETY: held owns the live descriptor; F_GETFD borrows it.
    let flags = unsafe { libc::fcntl(held.fd.as_raw_fd(), libc::F_GETFD) };
    assert!(flags >= 0);
    assert_ne!(flags & libc::FD_CLOEXEC, 0);
    // SAFETY: The data slot contains the unique Context installed above; after
    // its first consumption the same live slot is null and has nothing to free.
    unsafe {
        free_context(&mut host.data);
        assert!(host.data.is_null());
        free_context(&mut host.data);
    }
    assert_eq!(peers[1].read(&mut [0]).unwrap(), 0);
    assert_eq!(
        peers[0].read(&mut [0]).unwrap_err().kind(),
        ErrorKind::WouldBlock
    );
    drop(held);
    assert_eq!(peers[0].read(&mut [0]).unwrap(), 0);
    worker.join().unwrap();
    std::fs::remove_file(socket).unwrap();
}
