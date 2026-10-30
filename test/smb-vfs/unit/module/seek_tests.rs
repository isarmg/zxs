use super::*;
use smb_protocol::protocol::{FrameStream, PROTOCOL, REQUIRED_CAPABILITIES, Response};
use std::{
    fs::File,
    os::unix::{fs::FileExt, net::UnixListener},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

fn with_seek_host(
    writable: bool,
    authorization: Vec<Option<storage_contract::ErrorCode>>,
    witness: impl FnOnce(
        &mut sdk::vfs_handle_struct,
        &mut sdk::files_struct,
        &mut authentication::tests::AuthenticatedSession,
        &File,
    ),
) {
    let stem = format!(
        "zxs-vfs-seek-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
    );
    let socket = std::env::temp_dir().join(format!("{stem}.sock"));
    let work = std::env::temp_dir().join(format!("{stem}.work"));
    let listener = UnixListener::bind(&socket).unwrap();
    let worker = std::thread::spawn(move || {
        let mut stream = FrameStream::with_timeout(
            listener.accept().unwrap().0,
            Duration::from_secs(5),
        ).unwrap();
        assert!(matches!(
            stream.receive::<Request>().unwrap().0,
            Request::Hello { .. }
        ));
        stream
            .send(
                &Response::Ok {
                    result: json_codec::json!({"protocol":PROTOCOL,"capabilities":REQUIRED_CAPABILITIES}),
                },
                None,
            )
            .unwrap();
        for error in authorization {
            assert!(matches!(
                stream.receive::<Request>().unwrap().0,
                Request::AuthorizeRead { handle: 7 }
            ));
            let response = match error {
                Some(code) => Response::Error {
                    code,
                    message: "seek admission revoked".into(),
                    retryable: false,
                    operation_id: None,
                },
                None => Response::Ok {
                    result: json_codec::json!({"authorized":true}),
                },
            };
            stream.send(&response, None).unwrap();
        }
        assert!(matches!(
            stream.receive::<Request>().unwrap().0,
            Request::Close { handle: 7 }
        ));
        stream
            .send(
                &Response::Ok { result: json_codec::json!({"closed":true}) },
                None,
            )
            .unwrap();
    });
    let client = Client::connect(&socket, "alice", "repository").unwrap();
    let file = if writable {
        let file = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&work)
            .unwrap();
        file.write_all_at(b"abcdef", 0).unwrap();
        file
    } else {
        File::open(std::env::temp_dir()).unwrap()
    };
    let opened = Open {
        info: Arc::new(OpenResult {
            entry_id: "file".into(),
            kind: NamespaceEntryKind::File,
            path: "file".into(),
            handle: 7,
            size: if writable { 6 } else { 1 << 34 },
            max_file_bytes: 1 << 35,
            version_id: Some("version".into()),
            workspace_id: writable.then(|| "workspace".into()),
        }),
        fd: file.try_clone().unwrap().into(),
        writable,
        io: Arc::new(Mutex::new(WriteSequence::new(false))),
    };
    let mut fsp: sdk::files_struct = Default::default();
    let context = Box::new(Context {
        samba_username: "alice".into(),
        client,
        files: [(&mut fsp as *mut _ as usize, opened)].into_iter().collect(),
    });
    let mut authenticated = authentication::tests::AuthenticatedSession::new();
    let mut host = authenticated.handle();
    host.data = Box::into_raw(context).cast();
    // SAFETY: the configured SDK registration stub accepts this real generated
    // callback table; host/session/fsp storage stays live on this event thread.
    unsafe {
        assert_eq!(samba_init_module().v, 0);
    }
    assert!(FUNCTIONS.get().unwrap().lseek_fn.is_some());
    witness(&mut host, &mut fsp, &mut authenticated, &file);
    // SAFETY: close consumes this one Open record; the context allocation is
    // reclaimed exactly once afterward. The fsp remains a live opaque key.
    unsafe {
        assert_eq!(close(&mut host, &mut fsp), 0);
        let context = &*host.data.cast::<Context>();
        assert!(context.files.is_empty());
        let seek = FUNCTIONS.get().unwrap().lseek_fn.unwrap();
        assert_eq!(seek(&mut host, &mut fsp, 0, libc::SEEK_END), -1);
        assert_eq!(*libc::__errno_location(), libc::EINVAL);
        drop(Box::from_raw(host.data.cast::<Context>()));
        host.data = std::ptr::null_mut();
    }
    worker.join().unwrap();
    drop(file);
    std::fs::remove_file(socket).unwrap();
    if writable {
        std::fs::remove_file(work).unwrap();
    }
}

#[test]
fn registered_readonly_seek_rejects_placeholder_geometry_and_rechecks_admission() {
    let mut authorization = vec![None; 5];
    authorization.push(Some(storage_contract::ErrorCode::SmbRebindRequired));
    with_seek_host(false, authorization, |host, fsp, authenticated, _| {
        let seek = FUNCTIONS.get().unwrap().lseek_fn.unwrap();
        // SAFETY: the helper retains the authenticated real SDK objects and
        // directory placeholder until this registered callback returns.
        unsafe {
            for whence in [
                libc::SEEK_SET,
                libc::SEEK_CUR,
                libc::SEEK_END,
                libc::SEEK_DATA,
                libc::SEEK_HOLE,
            ] {
                assert_eq!(seek(host, fsp, 0, whence), -1);
                assert_eq!(*libc::__errno_location(), libc::ENOTSUP);
            }
            authenticated.connection.force_user = true;
            assert_eq!(seek(host, fsp, 0, libc::SEEK_END), -1);
            assert_eq!(*libc::__errno_location(), libc::EACCES);
            authenticated.connection.force_user = false;
            assert_eq!(seek(host, fsp, 0, libc::SEEK_END), -1);
            assert_eq!(*libc::__errno_location(), libc::ESTALE);
        }
    });
}

#[test]
fn registered_writable_seek_preserves_work_fd_position_length_and_kernel_errors() {
    let mut authorization = vec![None; 8];
    authorization.push(Some(storage_contract::ErrorCode::Forbidden));
    with_seek_host(true, authorization, |host, fsp, _, file| {
        let seek = FUNCTIONS.get().unwrap().lseek_fn.unwrap();
        // SAFETY: the helper owns the work FD and live real SDK objects; each
        // call is serialized on the same event thread and awaits live admission.
        unsafe {
            assert_eq!(seek(host, fsp, 0, libc::SEEK_END), 6);
            file.set_len(11).unwrap();
            assert_eq!(seek(host, fsp, 0, libc::SEEK_END), 11);
            assert_eq!(seek(host, fsp, 2, libc::SEEK_SET), 2);
            assert_eq!(seek(host, fsp, 3, libc::SEEK_CUR), 5);
            assert_eq!(seek(host, fsp, -1, libc::SEEK_END), 10);
            assert_eq!(seek(host, fsp, -1, libc::SEEK_SET), -1);
            assert_eq!(*libc::__errno_location(), libc::EINVAL);
            for whence in [libc::SEEK_DATA, libc::SEEK_HOLE] {
                let expected = libc::lseek(file.as_raw_fd(), 0, whence);
                let error = if expected < 0 {
                    *libc::__errno_location()
                } else {
                    0
                };
                assert_eq!(seek(host, fsp, 0, whence), expected);
                if expected < 0 {
                    assert_eq!(*libc::__errno_location(), error);
                }
            }
            let position = libc::lseek(file.as_raw_fd(), 0, libc::SEEK_CUR);
            assert!(position >= 0);
            assert_eq!(seek(host, fsp, 99, libc::SEEK_SET), -1);
            assert_eq!(*libc::__errno_location(), libc::EACCES);
            assert_eq!(libc::lseek(file.as_raw_fd(), 0, libc::SEEK_CUR), position);
        }
    });
}
