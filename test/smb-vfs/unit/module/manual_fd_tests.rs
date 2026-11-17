use super::*;
use smb_protocol::protocol::{FrameStream, PROTOCOL, REQUIRED_CAPABILITIES, Response};
use std::{
    fs::File,
    os::unix::{fs::FileExt, net::UnixListener},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

fn ok(wire: &mut FrameStream, result: json_codec::Value) {
    wire.send(&Response::Ok { result }, None).unwrap();
}

fn admitted_read(wire: &mut FrameStream) {
    assert!(matches!(
        wire.receive::<Request>().unwrap().0,
        Request::AuthorizeRead { handle: 7 }
    ));
    ok(wire, json_codec::json!({"mtime":{"state":"unpublished"}}));
}

fn denied_read(wire: &mut FrameStream, code: storage_contract::ErrorCode) {
    assert!(matches!(
        wire.receive::<Request>().unwrap().0,
        Request::AuthorizeRead { handle: 7 }
    ));
    wire.send(&Response::Error {
        code,
        message: "manual read authority revoked".into(),
        retryable: false,
        operation_id: None,
    }, None).unwrap();
}

fn reply_to_manual_open(wire: &mut FrameStream, source: &File) {
    assert!(matches!(
        wire.receive::<Request>().unwrap().0,
        Request::Hello { .. }
    ));
    ok(wire, json_codec::json!({
        "protocol":PROTOCOL,"capabilities":REQUIRED_CAPABILITIES,
    }));
    let Request::Open {
        path, writable, create, truncate, exclusive, write_through,
    } = wire.receive::<Request>().unwrap().0 else {
        panic!("expected the actual VFS Open request");
    };
    assert_eq!(path, "manual");
    assert!(writable);
    assert!(!create && !truncate && !exclusive && !write_through);
    let opened = OpenResult {
        data_io: smb_protocol::protocol::OpenIo::Descriptor,
        entry_id: "entry".into(),
        kind: NamespaceEntryKind::File,
        path,
        handle: 7,
        size: 6,
        max_file_bytes: 64,
        version_id: None,
        workspace_id: None,
        mtime: FileMtime::Unpublished {},
    };
    wire.send(&Response::Ok {
        result: json_codec::to_value(opened).unwrap(),
    }, Some(source.as_fd())).unwrap();
}

fn prepare(wire: &mut FrameStream, expected_end: u64, expected_bytes: u64) {
    let Request::PrepareWrite { handle, end, bytes } = wire.receive::<Request>().unwrap().0 else {
        panic!("expected the original per-mutation admission before FD I/O");
    };
    assert_eq!((handle, end, bytes), (7, expected_end, expected_bytes));
    ok(wire, json_codec::json!({"ready":true}));
}

/// # Safety
/// The real generated SDK objects, owned buffers and Context remain live until
/// the worker completes. No host pointer enters the async worker.
unsafe fn complete(request: *mut sdk::tevent_req, event: *mut sdk::tevent_context) -> isize {
    assert!(!request.is_null());
    // SAFETY: request/event are the matching live tevent objects from this test.
    unsafe {
        while sdk::tevent_req_is_in_progress(request) {
            assert_eq!(sdk::_tevent_loop_once(event, c"zxs manual FD test".as_ptr()), 0);
        }
        let mut aio: sdk::vfs_aio_state = Default::default();
        let result = io_recv(request, &mut aio);
        assert_eq!(aio.error, 0);
        sdk::_talloc_free(request.cast(), c"zxs manual FD test".as_ptr());
        result
    }
}

#[test]
fn manual_open_reads_its_real_fd_after_write_truncate_and_extend_without_workspace_identity() {
    for asynchronous_io in [false, true] {
        let stem = format!(
            "zxs-vfs-manual-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos(),
        );
        let socket = std::env::temp_dir().join(format!("{stem}.sock"));
        let path = std::env::temp_dir().join(format!("{stem}.work"));
        let source = std::fs::OpenOptions::new().create_new(true).read(true).write(true)
            .open(&path).unwrap();
        source.write_all_at(b"abcdef", 0).unwrap();
        let sent_source = source.try_clone().unwrap();
        let listener = UnixListener::bind(&socket).unwrap();
        let worker = std::thread::spawn(move || {
            let mut wire = FrameStream::with_timeout(
                listener.accept().unwrap().0, Duration::from_secs(5),
            ).unwrap();
            reply_to_manual_open(&mut wire, &sent_source);
            prepare(&mut wire, 10, 4);
            assert!(matches!(
                wire.receive::<Request>().unwrap().0,
                Request::Dirty { handle: 7 }
            ));
            ok(&mut wire, json_codec::json!({"dirty":true}));
            for _ in 0..3 {
                admitted_read(&mut wire);
            }
            prepare(&mut wire, 2, 0);
            prepare(&mut wire, 7, 0);
            for _ in 0..3 {
                admitted_read(&mut wire);
            }
            denied_read(&mut wire, storage_contract::ErrorCode::Forbidden);
            denied_read(&mut wire, storage_contract::ErrorCode::SmbRebindRequired);
            assert!(matches!(
                wire.receive::<Request>().unwrap().0,
                Request::Detach { handle: 7 }
            ));
            ok(&mut wire, json_codec::json!({"detached":true}));
            assert_eq!(
                wire.receive::<Request>().unwrap_err().kind(),
                std::io::ErrorKind::UnexpectedEof,
            );
        });
        let client = Client::connect(&socket, "alice", "repository").unwrap();
        let owner = Box::new(Context {
            samba_username: "alice".into(), client, files: BTreeMap::new(),
        });
        let mut authenticated = authentication::tests::AuthenticatedSession::new();
        let mut host = authenticated.handle();
        host.data = Box::into_raw(owner).cast();
        let mut fsp = sdk::files_struct::default();
        let filename = sdk::smb_filename {
            base_name: c"manual".as_ptr().cast_mut(), ..Default::default()
        };
        let how = sdk::vfs_open_how { flags: libc::O_RDWR, ..Default::default() };
        // SAFETY: real generated SDK objects, authentication and buffers stay
        // live on this event thread. Each async request completes before their
        // storage is reused; close consumes the only Open record exactly once.
        unsafe {
            assert!(openat(&mut host, std::ptr::null(), &filename, &mut fsp, &how) >= 0);
            let opened = &context(&mut host).unwrap().files[&(&mut fsp as *mut _ as usize)];
            assert!(opened.info.workspace_id.is_none());
            assert!(opened.info.version_id.is_none());
            assert_eq!(DescriptorRole::for_open(&opened.info, opened.writable),
                DescriptorRole::PrivateWork);
            let event = asynchronous_io.then(|| asynchronous::tests::event_context());
            if let Some(event) = event {
                assert!(!event.is_null());
                let request = pwrite_send(
                    &mut host, std::ptr::null_mut(), event, &mut fsp,
                    b"wxyz".as_ptr().cast(), 4, 6,
                );
                assert_eq!(complete(request, event), 4);
            } else {
                assert_eq!(pwrite(&mut host, &mut fsp, b"wxyz".as_ptr().cast(), 4, 6), 4);
            }
            let mut bytes = [0u8; 12];
            let count = if let Some(event) = event {
                let request = pread_send(
                    &mut host, std::ptr::null_mut(), event, &mut fsp,
                    bytes.as_mut_ptr().cast(), bytes.len(), 0,
                );
                complete(request, event)
            } else {
                pread(&mut host, &mut fsp, bytes.as_mut_ptr().cast(), bytes.len(), 0)
            };
            assert_eq!(count, 10);
            assert_eq!(&bytes[..10], b"abcdefwxyz");
            let mut stat = sdk::SMB_STRUCT_STAT::default();
            assert_eq!(fstat(&mut host, &mut fsp, &mut stat), 0);
            assert_eq!(stat.st_ex_size, 10);
            assert_eq!(lseek(&mut host, &mut fsp, 0, libc::SEEK_END), 10);
            assert_eq!(truncate(&mut host, &mut fsp, 2), 0);
            assert_eq!(truncate(&mut host, &mut fsp, 7), 0);
            bytes.fill(0xff);
            let count = if let Some(event) = event {
                let request = pread_send(
                    &mut host, std::ptr::null_mut(), event, &mut fsp,
                    bytes.as_mut_ptr().cast(), bytes.len(), 0,
                );
                complete(request, event)
            } else {
                pread(&mut host, &mut fsp, bytes.as_mut_ptr().cast(), bytes.len(), 0)
            };
            assert_eq!(count, 7);
            assert_eq!(&bytes[..7], b"ab\0\0\0\0\0");
            assert_eq!(fstat(&mut host, &mut fsp, &mut stat), 0);
            assert_eq!(stat.st_ex_size, 7);
            assert_eq!(lseek(&mut host, &mut fsp, 0, libc::SEEK_END), 7);
            bytes.fill(0xcc);
            assert_eq!(pread(&mut host, &mut fsp, bytes.as_mut_ptr().cast(), 1, 0), -1);
            assert_eq!(*libc::__errno_location(), libc::EACCES);
            assert_eq!(bytes, [0xcc; 12]);
            stat.st_ex_size = 99;
            assert_eq!(fstat(&mut host, &mut fsp, &mut stat), -1);
            assert_eq!(*libc::__errno_location(), libc::ESTALE);
            assert_eq!(stat.st_ex_size, 99);
            authenticated.connection.force_user = true;
            assert_eq!(pwrite(&mut host, &mut fsp, b"x".as_ptr().cast(), 1, 0), -1);
            assert_eq!(*libc::__errno_location(), libc::EACCES);
            assert_eq!(close(&mut host, &mut fsp), -1);
            assert_eq!(*libc::__errno_location(), libc::EACCES);
            assert!(context_for_cleanup(&mut host).unwrap().files.is_empty());
            free_context(&mut host.data);
            if let Some(event) = event {
                sdk::_talloc_free(event.cast(), c"zxs manual FD test".as_ptr());
            }
        }
        worker.join().unwrap();
        let mut retained = [0xff; 7];
        assert_eq!(source.read_at(&mut retained, 0).unwrap(), 7);
        assert_eq!(&retained, b"ab\0\0\0\0\0");
        drop(source);
        std::fs::remove_file(socket).unwrap();
        std::fs::remove_file(path).unwrap();
    }
}


#[test]
fn native_working_mode_uses_ordered_rpc_not_the_placeholder_fd_for_sync_and_async_io() {
    use smb_protocol::protocol::{OpenIo, WorkingPosition};
    for asynchronous_io in [false, true] {
        let socket = std::env::temp_dir().join(format!(
            "zxs-vfs-native-{}-{}.sock", std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos(),
        ));
        let listener = UnixListener::bind(&socket).unwrap();
        let placeholder = File::open(std::env::temp_dir()).unwrap();
        let held = placeholder.try_clone().unwrap();
        let worker = std::thread::spawn(move || {
            let mut wire = FrameStream::with_timeout(
                listener.accept().unwrap().0, Duration::from_secs(5),
            ).unwrap();
            assert!(matches!(wire.receive::<Request>().unwrap().0, Request::Hello { .. }));
            ok(&mut wire, json_codec::json!({
                "protocol": PROTOCOL, "capabilities": REQUIRED_CAPABILITIES,
            }));
            assert!(matches!(wire.receive::<Request>().unwrap().0,
                Request::Open { writable: true, .. }));
            let info = OpenResult {
                data_io: OpenIo::NativeWorking, entry_id: "entry".into(),
                kind: NamespaceEntryKind::File, path: "native".into(), handle: 7,
                size: 0, max_file_bytes: 256 * 1024,
                version_id: None, workspace_id: None, mtime: FileMtime::Unpublished {},
            };
            wire.send(&Response::Ok { result: json_codec::to_value(info).unwrap() },
                Some(held.as_fd())).unwrap();
            let mut bytes = Vec::<u8>::new();
            let mut frames = Vec::new();
            loop {
                match wire.receive::<Request>().unwrap().0 {
                    Request::WorkingWrite { handle: 7, position, bytes: input } => {
                        let offset = match position {
                            WorkingPosition::At { offset } => offset as usize,
                            WorkingPosition::Append {} => bytes.len(),
                        };
                        assert!(input.len() <= smb_protocol::protocol::MAX_READ_BYTES);
                        frames.push((offset, input.len()));
                        bytes.resize(bytes.len().max(offset + input.len()), 0);
                        bytes[offset..offset + input.len()].copy_from_slice(&input);
                        ok(&mut wire, json_codec::json!({"written": input.len()}));
                    }
                    Request::WorkingResize { handle: 7, length } => {
                        bytes.resize(length as usize, 0);
                        ok(&mut wire, json_codec::json!({"resized": true}));
                    }
                    Request::Read { handle: 7, offset, length } => {
                        let from = (offset as usize).min(bytes.len());
                        let to = (from + length as usize).min(bytes.len());
                        ok(&mut wire, json_codec::json!({"bytes": &bytes[from..to]}));
                    }
                    Request::WorkingStat { handle: 7 } => {
                        ok(&mut wire, json_codec::json!({"size": bytes.len().to_string()}));
                    }
                    Request::AuthorizeRead { handle: 7 } => {
                        ok(&mut wire, json_codec::json!({"mtime": {"state":"unpublished"}}));
                    }
                    Request::Flush { handle: 7 } => {
                        assert_eq!(bytes, b"\0\0\0gg\0\0\0\0");
                        ok(&mut wire, json_codec::json!({"saved":true}));
                    }
                    Request::Close { handle: 7 } => {
                        ok(&mut wire, json_codec::json!({"closed":true}));
                        break;
                    }
                    unexpected => panic!("native mode must not use FD/Dirty: {unexpected:?}"),
                }
            }
            let block = smb_protocol::protocol::MAX_READ_BYTES;
            assert_eq!(frames, [(3, block), (3 + block, 3), (6 + block, 2)]);
            assert!(held.metadata().unwrap().is_dir());
        });
        let client = Client::connect(&socket, "alice", "repository").unwrap();
        let owner = Box::new(Context {
            samba_username: "alice".into(), client, files: BTreeMap::new(),
        });
        let mut authenticated = authentication::tests::AuthenticatedSession::new();
        let mut host = authenticated.handle();
        host.data = Box::into_raw(owner).cast();
        let mut fsp = sdk::files_struct::default();
        let filename = sdk::smb_filename {
            base_name: c"native".as_ptr().cast_mut(), ..Default::default()
        };
        let how = sdk::vfs_open_how { flags: libc::O_RDWR, ..Default::default() };
        // SAFETY: the real generated SDK host and buffers remain live on this
        // event thread. Async work completes before any host storage is reused.
        unsafe {
            assert!(openat(&mut host, std::ptr::null(), &filename, &mut fsp, &how) >= 0);
            let opened = &context(&mut host).unwrap().files[&(&mut fsp as *mut _ as usize)];
            assert_eq!(opened.info.data_io, OpenIo::NativeWorking);
            assert_eq!(DescriptorRole::for_open(&opened.info, opened.writable),
                DescriptorRole::NativeWorking);
            let event = asynchronous_io.then(|| asynchronous::tests::event_context());
            let input = vec![b'g'; smb_protocol::protocol::MAX_READ_BYTES + 3];
            if let Some(event) = event {
                assert!(!event.is_null());
                let request = pwrite_send(
                    &mut host, std::ptr::null_mut(), event, &mut fsp,
                    input.as_ptr().cast(), input.len(), 3,
                );
                assert_eq!(complete(request, event), input.len() as isize);
            } else {
                assert_eq!(pwrite(&mut host, &mut fsp,
                    input.as_ptr().cast(), input.len(), 3), input.len() as isize);
            }
            assert_eq!(pwrite(&mut host, &mut fsp, b"++".as_ptr().cast(), 2, -1), 2);
            assert_eq!(truncate(&mut host, &mut fsp, 5), 0);
            assert_eq!(truncate(&mut host, &mut fsp, 9), 0);
            let mut output = [0xff; 9];
            let count = if let Some(event) = event {
                let request = pread_send(
                    &mut host, std::ptr::null_mut(), event, &mut fsp,
                    output.as_mut_ptr().cast(), output.len(), 0,
                );
                complete(request, event)
            } else {
                pread(&mut host, &mut fsp, output.as_mut_ptr().cast(), output.len(), 0)
            };
            assert_eq!(count, 9);
            assert_eq!(&output, b"\0\0\0gg\0\0\0\0");
            let mut stat = sdk::SMB_STRUCT_STAT::default();
            assert_eq!(fstat(&mut host, &mut fsp, &mut stat), 0);
            assert_eq!(stat.st_ex_size, 9);
            assert_eq!(lseek(&mut host, &mut fsp, 0, libc::SEEK_END), -1);
            assert_eq!(*libc::__errno_location(), libc::ESPIPE);
            let flush_event = match event {
                Some(event) => event,
                None => asynchronous::tests::event_context(),
            };
            assert!(!flush_event.is_null());
            let flushing = fsync_send(
                &mut host, std::ptr::null_mut(), flush_event, &mut fsp,
            );
            assert_eq!(complete(flushing, flush_event), 0);
            if event.is_none() {
                sdk::_talloc_free(flush_event.cast(), c"zxs native RPC test".as_ptr());
            }
            assert_eq!(close(&mut host, &mut fsp), 0);
            free_context(&mut host.data);
            if let Some(event) = event {
                sdk::_talloc_free(event.cast(), c"zxs native RPC test".as_ptr());
            }
        }
        worker.join().unwrap();
        assert!(placeholder.metadata().unwrap().is_dir());
        std::fs::remove_file(socket).unwrap();
    }
}
