use super::*;
use std::time::Duration;

// Keep worker-only test setup concise while exercising the same production
// preparation/queue boundary as the VFS callbacks.
unsafe fn send(
    mem_ctx: *mut sdk::TALLOC_CTX,
    event: *mut sdk::tevent_context,
    destination: *mut c_void,
    capacity: usize,
    job: impl FnOnce() -> io::Result<Completion> + Send + 'static,
) -> *mut sdk::tevent_req {
    // SAFETY: The test caller retains the production send ownership contract.
    unsafe {
        super::send(
            mem_ctx,
            event,
            destination,
            capacity,
            || Ok(()),
            move |()| job(),
        )
    }
}

// tevent.c initializes a process-global backend list without synchronization.
// Samba initializes it on the host event loop; Rust's parallel test threads
// must not race the first backend registration. Only context creation is
// serialized: independent request execution and cancellation stay concurrent.
static EVENT_INITIALIZATION: Mutex<()> = Mutex::new(());

pub(crate) fn event_context() -> *mut sdk::tevent_context {
    let _initialization = EVENT_INITIALIZATION.lock().unwrap();
    // SAFETY: a null talloc parent requests a new independently owned event
    // context; the global backend initialization is serialized above.
    unsafe { sdk::tevent_context_init(std::ptr::null_mut()) }
}
#[link(name = "tevent-private-samba")]
#[link(name = "talloc-private-samba")]
#[link(name = "smbconf")]
#[link(name = "samba-security-private-samba")]
unsafe extern "C" {}

// Registration is the only host stub. Event and allocation functions come
// from the actual libraries built with the configured target SDK.
#[unsafe(export_name = "smb_register_vfs")]
unsafe extern "C" fn register(
    _version: i32,
    _name: *const libc::c_char,
    _table: *const sdk::vfs_fn_pointers,
) -> sdk::NTSTATUS {
    super::super::nt_status(0)
}
#[test]
fn worker_wait_keeps_host_event_loop_pending_and_read_is_copied_on_completion() {
    let (started, ready) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let mut bytes = [0_u8; 4];
    // SAFETY: This test uses real SDK allocations on one thread and live local output buffers.
    // Requests are completed or freed before output/event storage is released; each allocation
    // is freed once.
    unsafe {
        let event = event_context();
        assert!(!event.is_null());
        let request = send(
            std::ptr::null_mut(),
            event,
            bytes.as_mut_ptr().cast(),
            bytes.len(),
            move || {
                started.send(()).unwrap();
                wait.recv_timeout(Duration::from_secs(5))
                    .map_err(io::Error::other)?;
                Ok(Completion {
                    result: 4,
                    bytes: vec![1, 2, 3, 4],
                })
            },
        );
        assert!(!request.is_null());
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(sdk::tevent_req_is_in_progress(request));
        assert_eq!(bytes, [0; 4]);
        release.send(()).unwrap();
        while sdk::tevent_req_is_in_progress(request) {
            assert_eq!(sdk::_tevent_loop_once(event, c"zxs SDK test".as_ptr()), 0);
        }
        let mut aio: sdk::vfs_aio_state = std::mem::zeroed();
        assert_eq!(recv(request, &mut aio), 4);
        assert_eq!(aio.error, 0);
        assert_eq!(bytes, [1, 2, 3, 4]);
        sdk::_talloc_free(request.cast(), c"zxs SDK test".as_ptr());
        sdk::_talloc_free(event.cast(), c"zxs SDK test".as_ptr());
    }
}
#[test]
fn oversized_completion_is_rejected_without_writing_host_buffer() {
    let mut destination = [7_u8; 2];
    // SAFETY: Real SDK allocations and the local output buffer stay live on
    // this thread until completion; request and event are each freed once.
    unsafe {
        let event = event_context();
        assert!(!event.is_null());
        let request = send(
            std::ptr::null_mut(),
            event,
            destination.as_mut_ptr().cast(),
            destination.len(),
            || {
                Ok(Completion {
                    result: 3,
                    bytes: vec![9; 3],
                })
            },
        );
        assert!(!request.is_null());
        while sdk::tevent_req_is_in_progress(request) {
            assert_eq!(
                sdk::_tevent_loop_once(event, c"zxs SDK bounds test".as_ptr()),
                0
            );
        }
        let mut aio: sdk::vfs_aio_state = Default::default();
        assert_eq!(recv(request, &mut aio), -1);
        assert_eq!(aio.error, libc::EINVAL);
        assert_eq!(destination, [7; 2]);
        sdk::_talloc_free(request.cast(), c"zxs SDK bounds test".as_ptr());
        sdk::_talloc_free(event.cast(), c"zxs SDK bounds test".as_ptr());
    }
}

#[test]
fn cancelling_request_before_worker_finishes_does_not_touch_released_host_state() {
    let (started, ready) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let (finished, done) = mpsc::channel();
    let mut destination = [0_u8; 4];
    // SAFETY: This test uses real SDK allocations on one thread and live local output buffers.
    // Requests are completed or freed before output/event storage is released; each allocation
    // is freed once.
    unsafe {
        let event = event_context();
        assert!(!event.is_null());
        let request = send(
            std::ptr::null_mut(),
            event,
            destination.as_mut_ptr().cast(),
            4,
            move || {
                started.send(()).unwrap();
                wait.recv_timeout(Duration::from_secs(5))
                    .map_err(io::Error::other)?;
                finished.send(()).unwrap();
                Ok(Completion {
                    result: 4,
                    bytes: vec![9; 4],
                })
            },
        );
        assert!(!request.is_null());
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        sdk::_talloc_free(request.cast(), c"zxs SDK cancellation test".as_ptr());
        sdk::_talloc_free(event.cast(), c"zxs SDK cancellation test".as_ptr());
        release.send(()).unwrap();
        done.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(destination, [0; 4]);
    }
}

#[test]
fn sync_and_async_reads_bound_immutable_extents_but_allow_workspace_growth() {
    use super::super as vfs;
    use smb_protocol::protocol::{
        FrameStream, OpenResult, PROTOCOL, REQUIRED_CAPABILITIES, Request, Response,
    };
    use std::{
        fs::File,
        os::unix::{fs::FileExt, net::UnixListener},
        time::{SystemTime, UNIX_EPOCH},
    };
    let socket = std::env::temp_dir().join(format!(
        "zxs-read-bound-{}-{}.sock",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let listener = UnixListener::bind(&socket).unwrap();
    let worker = std::thread::spawn(move || {
        let mut stream = FrameStream::new(listener.accept().unwrap().0).unwrap();
        while let Ok((request, _)) = stream.receive::<Request>() {
            let result = match request {
                Request::Hello { .. } => {
                    json_codec::json!({"protocol": PROTOCOL, "capabilities": REQUIRED_CAPABILITIES})
                }
                Request::AuthorizeRead { handle: 7 } => json_codec::json!({"authorized":true}),
                _ => panic!("unexpected request"),
            };
            stream.send(&Response::Ok { result }, None).unwrap();
        }
    });
    let client = smb_protocol::client::Client::connect(&socket, "alice", "repository").unwrap();
    // SAFETY: static NUL-terminated name; the fresh descriptor is checked before adoption.
    let raw = unsafe { libc::memfd_create(c"zxs-vfs-read-bound".as_ptr(), libc::MFD_CLOEXEC) };
    assert!(raw >= 0);
    // SAFETY: memfd_create returned a fresh descriptor adopted exactly once.
    let file = unsafe { File::from_raw_fd(raw) };
    file.write_all_at(b"good", 0).unwrap();
    let opened = vfs::Open {
        info: Arc::new(OpenResult {
            entry_id: "entry".into(),
            kind: storage_contract::NamespaceEntryKind::File,
            path: "file".into(),
            handle: 7,
            size: 4,
            max_file_bytes: 1024,
            version_id: Some("version".into()),
            workspace_id: None,
        }),
        fd: file.try_clone().unwrap().into(),
        writable: false,
        io: Arc::new(Mutex::new(vfs::WriteSequence::new(false))),
    };
    let context = Box::new(vfs::Context {
        samba_username: "alice".into(),
        client,
        files: [(0, opened)].into_iter().collect(),
    });
    let mut authenticated = vfs::authentication::tests::AuthenticatedSession::new();
    let mut handle = sdk::vfs_handle_struct {
        conn: &mut *authenticated.connection,
        data: Box::into_raw(context).cast(),
        ..Default::default()
    };
    // Extend the same inode after OpenResult captured its verified logical size.
    file.write_all_at(b"SECRET", 4).unwrap();
    // SAFETY: All host objects and output buffers are live local storage; fsp is
    // only an opaque map key. The real SDK event/request allocations are freed
    // on this thread after completion. free_context owns and closes its FD.
    unsafe {
        let snapshot = vfs::snapshot(&mut handle, std::ptr::null_mut()).unwrap();
        let stored = &vfs::context(&mut handle)
            .unwrap()
            .files
            .get(&0)
            .unwrap()
            .info;
        assert!(Arc::ptr_eq(stored, &snapshot.info));
        assert_eq!(Arc::strong_count(stored), 2);
        drop(snapshot);
        assert_eq!(Arc::strong_count(stored), 1);
        let event = event_context();
        assert!(!event.is_null());
        for writable in [false, true] {
            vfs::context(&mut handle)
                .unwrap()
                .files
                .get_mut(&0)
                .unwrap()
                .writable = writable;
            let mut metadata = sdk::SMB_STRUCT_STAT::default();
            assert_eq!(
                vfs::fstat(&mut handle, std::ptr::null_mut(), &mut metadata),
                0
            );
            assert_eq!(metadata.st_ex_size, if writable { 10 } else { 4 });
            for offset in [0, 2, 4, 5] {
                let expected: &[u8] = if writable {
                    &b"goodSECRET"[offset..]
                } else {
                    &b"good"[offset.min(4)..]
                };
                let mut bytes = [0xa5u8; 16];
                assert_eq!(
                    vfs::pread(
                        &mut handle,
                        std::ptr::null_mut(),
                        bytes.as_mut_ptr().cast(),
                        bytes.len(),
                        offset as _
                    ),
                    expected.len() as isize
                );
                assert_eq!(&bytes[..expected.len()], expected);
                assert!(bytes[expected.len()..].iter().all(|byte| *byte == 0xa5));
                bytes.fill(0xa5);
                let request = vfs::pread_send(
                    &mut handle,
                    std::ptr::null_mut(),
                    event,
                    std::ptr::null_mut(),
                    bytes.as_mut_ptr().cast(),
                    bytes.len(),
                    offset as _,
                );
                assert!(!request.is_null());
                while sdk::tevent_req_is_in_progress(request) {
                    assert_eq!(sdk::_tevent_loop_once(event, c"zxs read bound".as_ptr()), 0);
                }
                let mut aio: sdk::vfs_aio_state = Default::default();
                assert_eq!(vfs::io_recv(request, &mut aio), expected.len() as isize);
                assert_eq!(aio.error, 0);
                assert_eq!(&bytes[..expected.len()], expected);
                assert!(bytes[expected.len()..].iter().all(|byte| *byte == 0xa5));
                sdk::_talloc_free(request.cast(), c"zxs read bound".as_ptr());
            }
            let mut stat: sdk::SMB_STRUCT_STAT = Default::default();
            assert_eq!(vfs::fstat(&mut handle, std::ptr::null_mut(), &mut stat), 0);
            assert_eq!(stat.st_ex_size, if writable { 10 } else { 4 });
        }
        sdk::_talloc_free(event.cast(), c"zxs read bound".as_ptr());
        vfs::free_context(&mut handle.data);
    }
    worker.join().unwrap();
    std::fs::remove_file(socket).unwrap();
}

#[test]
fn worker_panic_completes_as_io_error_without_unwinding_through_host() {
    // SAFETY: Real SDK objects remain on this owning thread until received/freed.
    unsafe {
        let event = event_context();
        let request = send(std::ptr::null_mut(), event, std::ptr::null_mut(), 0, || {
            panic!("worker request panic")
        });
        assert!(!request.is_null());
        while sdk::tevent_req_is_in_progress(request) {
            assert_eq!(
                sdk::_tevent_loop_once(event, c"panic boundary test".as_ptr()),
                0
            );
        }
        let mut aio: sdk::vfs_aio_state = Default::default();
        assert_eq!(recv(request, &mut aio), -1);
        assert_eq!(aio.error, libc::EIO);
        sdk::_talloc_free(request.cast(), c"panic boundary test".as_ptr());
        sdk::_talloc_free(event.cast(), c"panic boundary test".as_ptr());
    }
}

#[test]
fn concurrent_pool_callers_share_one_successful_sender() {
    let callers = (0..8)
        .map(|_| std::thread::spawn(|| pool().unwrap() as *const _ as usize))
        .collect::<Vec<_>>();
    let expected = pool().unwrap() as *const _ as usize;
    for caller in callers {
        assert_eq!(caller.join().unwrap(), expected);
    }
}

#[test]
fn worker_reporting_failure_still_completes() {
    const CHILD: &str = "ZXS_TEST_SMB_STDERR_FULL";
    if std::env::var_os(CHILD).is_none() {
        let full = std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/full")
            .unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "module::asynchronous::tests::worker_reporting_failure_still_completes",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .stderr(full)
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success());
                return;
            }
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("stderr failure left the host request pending");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    // SAFETY: The child owns the real SDK event/request until completion and frees each once.
    unsafe {
        let event = event_context();
        let request = send(std::ptr::null_mut(), event, std::ptr::null_mut(), 0, || {
            Err(io::Error::from_raw_os_error(libc::ENOSPC))
        });
        assert!(!request.is_null());
        while sdk::tevent_req_is_in_progress(request) {
            assert_eq!(
                sdk::_tevent_loop_once(event, c"stderr failure test".as_ptr()),
                0
            );
        }
        let mut aio: sdk::vfs_aio_state = Default::default();
        assert_eq!(recv(request, &mut aio), -1);
        assert_eq!(aio.error, libc::EIO);
        sdk::_talloc_free(request.cast(), c"stderr failure test".as_ptr());
        sdk::_talloc_free(event.cast(), c"stderr failure test".as_ptr());
    }
}

#[test]
fn preparation_failure_completes_on_host_and_releases_owned_resources() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct OwnedPreparation(Arc<AtomicBool>);
    impl Drop for OwnedPreparation {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    for panic in [false, true] {
        let dropped = Arc::new(AtomicBool::new(false));
        let preparation_resource = OwnedPreparation(Arc::clone(&dropped));
        let host_thread = std::thread::current().id();
        let mut destination = [7_u8; 2];
        // SAFETY: Real SDK objects and the output buffer stay live on this
        // thread until received/freed; the failing preparation never queues I/O.
        unsafe {
            let event = event_context();
            assert!(!event.is_null());
            let request = super::send(
                std::ptr::null_mut(),
                event,
                destination.as_mut_ptr().cast(),
                destination.len(),
                move || -> io::Result<()> {
                    let _resource = preparation_resource;
                    assert_eq!(std::thread::current().id(), host_thread);
                    if panic {
                        panic!("host preparation panic");
                    }
                    Err(io::Error::from_raw_os_error(libc::ENOMEM))
                },
                |()| panic!("failed preparation must not reach the worker"),
            );
            assert!(!request.is_null());
            assert!(dropped.load(Ordering::SeqCst));
            assert!(!sdk::tevent_req_is_in_progress(request));
            let mut aio: sdk::vfs_aio_state = Default::default();
            assert_eq!(recv(request, &mut aio), -1);
            assert_eq!(aio.error, if panic { libc::EIO } else { libc::ENOMEM });
            assert_eq!(destination, [7; 2]);
            sdk::_talloc_free(request.cast(), c"preparation failure test".as_ptr());
            sdk::_talloc_free(event.cast(), c"preparation failure test".as_ptr());
        }
    }
}

#[test]
fn invalid_host_preparation_is_reported_by_all_send_callbacks() {
    use super::super as vfs;
    // SAFETY: Each null handle is rejected by preparation before dereference.
    // The real SDK event/request remains live and is received/freed on this thread.
    unsafe {
        let event = event_context();
        assert!(!event.is_null());
        let requests = [
            vfs::pread_send(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                event,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
                0,
            ),
            vfs::pwrite_send(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                event,
                std::ptr::null_mut(),
                std::ptr::null(),
                0,
                0,
            ),
            vfs::fsync_send(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                event,
                std::ptr::null_mut(),
            ),
        ];
        for request in requests {
            assert!(!request.is_null());
            assert!(!sdk::tevent_req_is_in_progress(request));
            let mut aio: sdk::vfs_aio_state = Default::default();
            assert_eq!(recv(request, &mut aio), -1);
            assert_eq!(aio.error, libc::EACCES);
            sdk::_talloc_free(request.cast(), c"invalid preparation test".as_ptr());
        }
        // A host may cancel without recv; a failed preparation has no Extra box.
        let request = super::send(
            std::ptr::null_mut(),
            event,
            std::ptr::null_mut(),
            0,
            || -> io::Result<()> { panic!("cancel failed preparation") },
            |()| panic!("must not queue"),
        );
        assert!(!request.is_null());
        sdk::_talloc_free(request.cast(), c"cancel failed preparation".as_ptr());
        sdk::_talloc_free(event.cast(), c"invalid preparation test".as_ptr());
    }
}
