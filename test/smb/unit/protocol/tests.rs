use super::*;
use std::{
    fs::File,
    io::{Read, Seek, Write},
    os::fd::AsFd,
};

#[test]
fn hello_requires_an_explicit_capability_list() {
    let request: Request = Request::Hello {
        protocol: PROTOCOL.into(),
        capabilities: REQUIRED_CAPABILITIES
            .iter()
            .map(|value| (*value).into())
            .collect(),
        username: "alice".into(),
        repository_id: "repository".into(),
    };
    let mut value = json_codec::to_value(&request).unwrap();
    assert!(json_codec::from_value::<Request>(value.clone()).is_ok());
    value.as_object_mut().unwrap().remove("capabilities");
    assert!(json_codec::from_value::<Request>(value).is_err());
}

#[test]
fn namespace_requests_require_a_preallocated_identity_on_the_wire() {
    for kind in [
        NamespaceOperation::Mkdir,
        NamespaceOperation::Rename,
        NamespaceOperation::Unlink,
        NamespaceOperation::Rmdir,
    ] {
        let request = namespace_request(kind, "entry".into(), None, false);
        let value = json_codec::to_value(&request).unwrap();
        storage_contract::validate_id(value["operation_id"].as_str().unwrap(), "op").unwrap();
        let mut missing = value;
        missing.as_object_mut().unwrap().remove("operation_id");
        assert!(json_codec::from_value::<Request>(missing).is_err());
    }
}

#[test]
fn unlinkat_flags_preserve_target_kind_and_reject_unknown_flags() {
    for (flags, expected) in [
        (0, NamespaceOperation::Unlink),
        (libc::AT_REMOVEDIR, NamespaceOperation::Rmdir),
    ] {
        let request = remove_request("entry".into(), flags).unwrap();
        let Request::Namespace {
            operation_id,
            kind,
            path,
            destination,
            replace,
        } = request
        else {
            panic!("expected namespace request");
        };
        assert_eq!(operation_id.to_string().len(), 35);
        assert_eq!(kind, expected);
        assert_eq!(path, "entry");
        assert!(destination.is_none());
        assert!(!replace);
    }
    for flags in [-1, libc::AT_SYMLINK_NOFOLLOW, libc::AT_REMOVEDIR | 1] {
        assert_eq!(
            remove_request("entry".into(), flags)
                .unwrap_err()
                .raw_os_error(),
            Some(libc::EINVAL)
        );
    }
}

#[test]
fn framing_transfers_an_owned_cloexec_descriptor_without_a_path() {
    let (one, two) = UnixStream::pair().unwrap();
    let mut sender = FrameStream::new(one).unwrap();
    let mut receiver = FrameStream::new(two).unwrap();
    let source = File::open("/etc/hostname").unwrap();
    sender
        .send(
            &Response::Ok {
                result: json_codec::json!({"handle":"18446744073709551615"}),
            },
            Some(source.as_fd()),
        )
        .unwrap();
    let (result, fd) = receiver.receive::<Response>().unwrap();
    assert!(matches!(result, Response::Ok { .. }));
    let fd = fd.unwrap();
    // SAFETY: F_GETFD only reads the flag word of this live owned descriptor.
    assert_ne!(
        unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
        0
    );
    let mut transferred = File::from(fd);
    transferred.rewind().unwrap();
    assert!(transferred.metadata().unwrap().is_file());
}

#[test]
fn serialized_frames_reject_invalid_sizes_before_writing_and_preserve_descriptor_delivery() {
    let (one, two) = UnixStream::pair().unwrap();
    let mut sender = FrameStream::new(one).unwrap();
    let mut receiver = FrameStream::new(two).unwrap();
    let source = File::open("/dev/null").unwrap();
    for payload in [Vec::new(), vec![b'x'; MAX_FRAME_BYTES + 1]] {
        assert_eq!(
            sender
                .send_serialized(&payload, Some(source.as_fd()))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        receiver.0.set_nonblocking(true).unwrap();
        assert_eq!(
            receiver.0.read(&mut [0u8; 1]).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        receiver.0.set_nonblocking(false).unwrap();
    }
    let payload = json_codec::to_vec(&Response::Ok {
        result: json_codec::json!({"alive":true}),
    })
    .unwrap();
    sender
        .send_serialized(&payload, Some(source.as_fd()))
        .unwrap();
    let (response, fd) = receiver.receive::<Response>().unwrap();
    assert!(matches!(response, Response::Ok { result } if result["alive"] == true));
    let fd = fd.unwrap();
    // SAFETY: F_GETFD only reads the flags of this live owned descriptor.
    assert_ne!(
        unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
        0
    );
}

#[test]
fn oversized_length_is_rejected_before_allocating_payload() {
    let (mut one, two) = UnixStream::pair().unwrap();
    one.write_all(&u32::MAX.to_be_bytes()).unwrap();
    let error = FrameStream::new(two)
        .unwrap()
        .receive::<Request>()
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
}

#[test]
fn established_idle_wait_is_cancellable_and_does_not_consume_frame_deadline() {
    let (one, two) = UnixStream::pair().unwrap();
    let cancel = two.try_clone().unwrap();
    let receiver = std::thread::spawn(move || {
        FrameStream::with_timeout(two, Duration::from_millis(30))
            .unwrap()
            .receive_with_mode::<Request>(ReadMode::EstablishedSession)
    });
    std::thread::sleep(Duration::from_millis(100));
    let mut sender = FrameStream::new(one).unwrap();
    sender
        .send(
            &Request::<String>::Stat {
                path: "file".into(),
            },
            None,
        )
        .unwrap();
    assert!(matches!(
        receiver.join().unwrap().unwrap().0,
        Request::Stat { .. }
    ));
    let receiver = std::thread::spawn(move || {
        sender.receive_with_mode::<Request>(ReadMode::EstablishedSession)
    });
    cancel.shutdown(std::net::Shutdown::Both).unwrap();
    assert!(receiver.join().unwrap().is_err());
}

#[test]
fn handshake_and_partial_frames_have_deadlines() {
    for (mode, prefix) in [
        (ReadMode::Deadline, Vec::new()),
        (ReadMode::EstablishedSession, vec![0]),
        (ReadMode::EstablishedSession, 100u32.to_be_bytes().to_vec()),
    ] {
        let (mut one, two) = UnixStream::pair().unwrap();
        one.write_all(&prefix).unwrap();
        let started = Instant::now();
        let error = FrameStream::with_timeout(two, Duration::from_millis(30))
            .unwrap()
            .receive_with_mode::<Request>(mode)
            .unwrap_err();
        assert!(matches!(
            error.kind(),
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
        ));
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}

#[test]
fn partial_payload_progress_does_not_restart_the_frame_deadline() {
    let (mut one, two) = UnixStream::pair().unwrap();
    one.write_all(&100u32.to_be_bytes()).unwrap();
    let reader = std::thread::spawn(move || {
        FrameStream::with_timeout(two, Duration::from_millis(100))
            .unwrap()
            .receive_with_mode::<Request>(ReadMode::EstablishedSession)
    });
    // Progress every 20 ms would keep a per-read timeout alive, but must not
    // extend the one deadline covering this incomplete payload.
    let mut sent = 0;
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(20));
        if one.write_all(b" ").is_err() {
            break;
        }
        sent += 1;
    }
    let error = reader.join().unwrap().unwrap_err();
    assert!(matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    ));
    assert!(sent < 20, "progress must not keep this frame alive");
}

#[test]
fn directory_reads_use_only_session_handles_and_do_not_accept_client_snapshots() {
    let request: Request = Request::ReadDirectory {
        handle: u64::MAX,
        restart: false,
    };
    let value = json_codec::to_value(&request).unwrap();
    assert_eq!(value["handle"], "18446744073709551615");
    assert!(json_codec::from_value::<Request>(value.clone()).is_ok());
    for field in ["commit_id", "path", "after"] {
        let mut forged = value.clone();
        forged
            .as_object_mut()
            .unwrap()
            .insert(field.into(), "forged".into());
        assert!(json_codec::from_value::<Request>(forged).is_err());
    }
    assert!(json_codec::from_value::<Request>(json_codec::json!({"op":"list","path":""})).is_err());
}

#[test]
fn partial_write_progress_does_not_restart_the_frame_deadline() {
    let (one, mut two) = UnixStream::pair().unwrap();
    let size: libc::c_int = 4096;
    // SAFETY: setsockopt reads the live integer through its correctly sized
    // pointer and only reduces this test socket's send buffer.
    assert_eq!(
        unsafe {
            libc::setsockopt(
                one.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_SNDBUF,
                (&size as *const libc::c_int).cast(),
                mem::size_of_val(&size) as libc::socklen_t,
            )
        },
        0
    );
    let sender = std::thread::spawn(move || {
        FrameStream::with_timeout(one, Duration::from_millis(100))
            .unwrap()
            .send(&"x".repeat(512 * 1024), None)
    });
    // Small but continuous progress would keep a per-write timeout alive until
    // the whole payload was sent. A whole-frame deadline must stop it instead.
    let mut buffer = [0u8; 8192];
    let mut received = 0;
    loop {
        std::thread::sleep(Duration::from_millis(20));
        match two.read(&mut buffer).unwrap() {
            0 => break,
            count => received += count,
        }
    }
    let error = sender.join().unwrap().unwrap_err();
    assert!(matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    ));
    assert!(received < 512 * 1024);
}

#[test]
fn blocked_send_is_cancelled_by_socket_shutdown() {
    let (one, _two) = UnixStream::pair().unwrap();
    let cancel = one.try_clone().unwrap();
    let sender = std::thread::spawn(move || {
        FrameStream::with_timeout(one, Duration::from_secs(30))
            .unwrap()
            .send(&"x".repeat(4 * 1024 * 1024), None)
    });
    std::thread::sleep(Duration::from_millis(50));
    cancel.shutdown(std::net::Shutdown::Both).unwrap();
    let started = Instant::now();
    assert!(sender.join().unwrap().is_err());
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn misplaced_extra_and_truncated_rights_are_rejected_without_leaking() {
    for (on_payload, descriptor_count) in [(true, 1), (false, 2), (false, 80)] {
        let (mut one, two) = UnixStream::pair().unwrap();
        let (descriptor, mut peer) = UnixStream::pair().unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let payload = json_codec::to_vec(&Response::Ok {
            result: json_codec::json!({}),
        })
        .unwrap();
        let header = (payload.len() as u32).to_be_bytes();
        if on_payload {
            one.write_all(&header).unwrap();
        }
        let bytes = if on_payload {
            payload.as_slice()
        } else {
            &header
        };
        let mut vector = libc::iovec {
            iov_base: bytes.as_ptr().cast_mut().cast(),
            iov_len: bytes.len(),
        };
        let mut control = [0usize; 64];
        // SAFETY: zero is a valid empty msghdr, populated with live buffers below.
        let mut message: libc::msghdr = unsafe { mem::zeroed() };
        message.msg_iov = &mut vector;
        message.msg_iovlen = 1;
        message.msg_control = control.as_mut_ptr().cast();
        // SAFETY: 80 ints plus its cmsghdr fit the aligned 512-byte buffer.
        unsafe {
            message.msg_controllen =
                libc::CMSG_SPACE((descriptor_count * mem::size_of::<i32>()) as u32) as usize;
            let cmsg = libc::CMSG_FIRSTHDR(&message);
            (*cmsg).cmsg_level = libc::SOL_SOCKET;
            (*cmsg).cmsg_type = libc::SCM_RIGHTS;
            (*cmsg).cmsg_len =
                libc::CMSG_LEN((descriptor_count * mem::size_of::<i32>()) as u32) as usize;
            for index in 0..descriptor_count {
                std::ptr::write_unaligned(
                    libc::CMSG_DATA(cmsg).cast::<i32>().add(index),
                    descriptor.as_raw_fd(),
                );
            }
            assert_eq!(
                libc::sendmsg(one.as_raw_fd(), &message, libc::MSG_NOSIGNAL),
                bytes.len() as isize
            );
        }
        if !on_payload {
            one.write_all(&payload).unwrap();
        }
        drop(descriptor);
        let error = FrameStream::new(two)
            .unwrap()
            .receive::<Response>()
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            peer.read(&mut [0u8; 1]).unwrap(),
            0,
            "rejected rights leaked"
        );
    }
}

#[test]
fn immutable_reads_hide_appended_bytes_while_workspace_reads_follow_live_length() {
    use std::os::unix::fs::FileExt;
    // SAFETY: the static name is NUL-terminated; memfd_create returns a fresh
    // descriptor or -1, checked before adoption. This test touches no disk files.
    let raw = unsafe { libc::memfd_create(c"zxs-read-extent-test".as_ptr(), libc::MFD_CLOEXEC) };
    assert!(raw >= 0);
    // SAFETY: successful memfd_create transfers this fresh descriptor exactly once.
    let file = unsafe { File::from_raw_fd(raw) };
    file.write_all_at(b"good", 0).unwrap();
    let info = OpenResult {
        entry_id: "entry".into(),
        kind: storage_contract::NamespaceEntryKind::File,
        path: "file".into(),
        handle: 1,
        size: 4,
        max_file_bytes: 1024,
        version_id: Some("version".into()),
        workspace_id: None,
        mtime: FileMtime::Unpublished {},
    };
    // Model alteration of the same inode after the server verified/opened it.
    file.write_all_at(b"SECRET", 4).unwrap();
    for (offset, requested, expected) in [
        (0, 16, b"good".as_slice()),
        (2, 16, b"od".as_slice()),
        (4, 16, b"".as_slice()),
        (5, 16, b"".as_slice()),
        (0, 2, b"go".as_slice()),
        (0, 0, b"".as_slice()),
    ] {
        let mut bytes = [0xa5; 16];
        let count = info.read_count(false, offset, requested);
        let received = file.read_at(&mut bytes[..count], offset).unwrap();
        assert_eq!(&bytes[..received], expected);
        assert!(bytes[received..].iter().all(|byte| *byte == 0xa5));
    }
    assert_eq!(info.read_count(false, u64::MAX, usize::MAX), 0);
    assert_eq!(info.read_count(true, 4, 16), 16);
    let mut bytes = [0; 16];
    let count = info.read_count(true, 4, bytes.len());
    let received = file.read_at(&mut bytes[..count], 4).unwrap();
    assert_eq!(&bytes[..received], b"SECRET");
    file.set_len(2).unwrap();
    let count = info.read_count(true, 0, bytes.len());
    let received = file.read_at(&mut bytes[..count], 0).unwrap();
    assert_eq!(&bytes[..received], b"go");
}

#[test]
fn samba_username_is_one_exact_name_not_a_mapping() {
    for name in ["alice", "SambaOwner", "local-owner", "dom\\owner"] {
        assert!(validate_username(name).is_ok());
    }
    for name in ["", "alice=bob", " alice", "alice ", "a\0b", "a\nb"] {
        assert!(validate_username(name).is_err());
    }
    assert!(validate_username(&"a".repeat(257)).is_err());
}

#[test]
fn namespace_and_open_kind_reject_unknown_wire_states() {
    let request = namespace_request(NamespaceOperation::Mkdir, "entry".into(), None, false);
    let mut value = json_codec::to_value(request).unwrap();
    value["kind"] = "future_operation".into();
    assert!(json_codec::from_value::<Request>(value).is_err());
    let opened = json_codec::json!({
        "entry_id":"entry", "kind":"future_kind", "path":"entry", "handle":"1",
        "size":"0", "max_file_bytes":"1024", "version_id":null, "workspace_id":null
    });
    assert!(json_codec::from_value::<OpenResult>(opened).is_err());
}

#[test]
fn protocol_error_codes_require_known_string_identities() {
    for code in [
        json_codec::json!("FUTURE_CODE"),
        json_codec::json!({"NOT_FOUND":null}),
    ] {
        let value = json_codec::json!({"status":"error", "code":code, "message":"failure", "retryable":false, "operation_id":null});
        assert!(json_codec::from_value::<Response>(value).is_err());
    }
    let mut request = json_codec::to_value(namespace_request(
        NamespaceOperation::Mkdir,
        "entry".into(),
        None,
        false,
    ))
    .unwrap();
    request["kind"] = json_codec::json!({"mkdir":null});
    assert!(json_codec::from_value::<Request>(request).is_err());
}

#[test]
fn unrepresentable_external_timeout_is_an_error_not_a_panic() {
    let (one, _peer) = UnixStream::pair().unwrap();
    let mut stream = FrameStream(one, Duration::MAX);
    assert_eq!(
        stream.send_serialized(b"{}", None).unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
    assert_eq!(
        stream.receive::<Response>().unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
}

#[test]
fn raw_and_domain_namespace_requests_preserve_identical_wire_bytes() {
    let bytes = br#"{"op":"namespace","operation_id":"op_00000000000000000000000000000000","kind":"mkdir","path":"entry","destination":null,"replace":false}"#;
    let raw: Request<String> = json_codec::from_slice(bytes).unwrap();
    let domain = raw.parse_operation_id().unwrap();
    assert_eq!(json_codec::to_vec(&domain).unwrap(), bytes);
    for id in ["", "op_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", "../outside"] {
        let raw: Request<String> = json_codec::from_value(json_codec::json!({
            "op":"namespace","operation_id":id,"kind":"mkdir","path":"entry","destination":null,"replace":false
        })).unwrap();
        assert!(raw.parse_operation_id().is_err());
    }
}

#[path = "mtime_tests.rs"]
mod mtime_tests;
