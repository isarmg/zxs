use super::super::{
    Arc, Client, Context, FileMtime, Mutex, NamespaceEntryKind, Open, OpenResult, Request,
    WriteSequence, authentication,
};
use super::*;
use smb_protocol::protocol::{FrameStream, PROTOCOL, REQUIRED_CAPABILITIES, Response};
use std::{
    fs::File,
    os::unix::net::UnixListener,
    time::{SystemTime, UNIX_EPOCH},
};

#[link(name = "smbd-base-private-samba")]
#[link(name = "samba-passdb")]
unsafe extern "C" {}

fn access(sd: *const sdk::security_descriptor, sid: sdk::dom_sid, mask: u32) -> u32 {
    let mut sid = sid;
    let token = sdk::security_token {
        num_sids: 1,
        sids: &mut sid,
        ..Default::default()
    };
    let mut granted = 0;
    // SAFETY: The descriptor, token and single SID remain live throughout the
    // actual SDK access check; all output storage is exclusive.
    unsafe { sdk::se_file_access_check(sd, &token, false, mask, &mut granted).v }
}

#[test]
fn logical_acl_preserves_owner_only_modes_and_requested_fields() {
    // This loads defaults from an empty file, not a passdb or credentials store.
    // SID conversion is deliberately reserved for the isolated real-Samba gate.
    // SAFETY: The static path is NUL terminated; the SDK reads only this file.
    assert!(unsafe { sdk::lp_load_initial_only(c"/dev/null".as_ptr()) });
    // SAFETY: Immutable SDK SID constants are copied into this test's storage.
    let (owner, group, other) = unsafe {
        (
            sdk::global_sid_System,
            sdk::global_sid_Builtin_Guests,
            sdk::global_sid_Anonymous,
        )
    };
    for (mode, allowed) in [
        (libc::S_IFDIR | 0o700, sdk::SEC_DIR_ADD_FILE),
        (libc::S_IFREG | 0o600, sdk::SEC_FILE_WRITE_DATA),
    ] {
        // SAFETY: Constructors copy live SID storage into their owned output;
        // null is a supported top-level talloc allocation context.
        let sd = unsafe {
            descriptor(
                ptr::null_mut(),
                SUPPORTED_SECURITY_INFO,
                mode,
                [&owner, &group],
                -1,
            )
        }
        .unwrap();
        assert_eq!(access(sd, owner, allowed), 0);
        assert_ne!(access(sd, other, allowed), 0);
        assert_ne!(access(sd, group, allowed), 0);
        if mode & libc::S_IFMT == libc::S_IFREG {
            assert_ne!(access(sd, owner, sdk::SEC_FILE_EXECUTE), 0);
        }
        // SAFETY: The returned descriptor and owned SID copies are live until
        // their single release below. Compare exact SDK SID bytes via fields.
        unsafe {
            assert_eq!((*(*sd).owner_sid).sid_rev_num, owner.sid_rev_num);
            assert_eq!((*(*sd).owner_sid).num_auths, owner.num_auths);
            assert_eq!((*(*sd).owner_sid).id_auth, owner.id_auth);
            assert_eq!((*(*sd).owner_sid).sub_auths, owner.sub_auths);
            assert_eq!((*(*sd).group_sid).sub_auths, group.sub_auths);
            assert_ne!((*sd).type_ & sdk::SEC_DESC_DACL_PROTECTED as u16, 0);
            sdk::_talloc_free(sd.cast(), c"logical ACL test".as_ptr());
        }
    }
    for mask in [0, sdk::SECINFO_OWNER, sdk::SECINFO_GROUP, sdk::SECINFO_DACL] {
        // SAFETY: Same allocation and input lifetimes as above.
        unsafe {
            let sd = descriptor(
                ptr::null_mut(),
                mask,
                libc::S_IFDIR | 0o700,
                [&owner, &group],
                -1,
            )
            .unwrap();
            assert_eq!(!(*sd).owner_sid.is_null(), mask & sdk::SECINFO_OWNER != 0);
            assert_eq!(!(*sd).group_sid.is_null(), mask & sdk::SECINFO_GROUP != 0);
            assert_eq!(!(*sd).dacl.is_null(), mask & sdk::SECINFO_DACL != 0);
            assert_eq!(
                (*sd).type_ & sdk::SEC_DESC_DACL_PRESENT as u16 != 0,
                mask & sdk::SECINFO_DACL != 0
            );
            sdk::_talloc_free(sd.cast(), c"logical ACL field test".as_ptr());
        }
    }
}

#[test]
fn registered_acl_rejects_unknown_fields_and_revoked_or_forced_identity() {
    // SAFETY: The SDK registration stub records our actual callback table.
    unsafe { assert_eq!(super::super::samba_init_module().v, 0) };
    let get = super::super::FUNCTIONS
        .get()
        .unwrap()
        .fget_nt_acl_fn
        .unwrap();
    // SAFETY: A null output is explicitly rejected before any host dereference.
    unsafe {
        assert_eq!(
            get(
                ptr::null_mut(),
                ptr::null_mut(),
                0,
                ptr::null_mut(),
                ptr::null_mut()
            )
            .v,
            0xc000000d,
        );
    }
    let mut output = ptr::dangling_mut();
    // SAFETY: Invalid optional inputs are rejected; output is live pointer storage.
    unsafe {
        assert_eq!(
            get(
                ptr::null_mut(),
                ptr::null_mut(),
                0x8000_0000,
                ptr::null_mut(),
                &mut output
            )
            .v,
            0xc000000d
        );
    }
    assert!(output.is_null());
    for (failure, remote_error) in [
        None,
        Some(storage_contract::ErrorCode::Forbidden),
        Some(storage_contract::ErrorCode::SmbRebindRequired),
        None,
    ]
    .into_iter()
    .enumerate()
    {
        let socket = std::env::temp_dir().join(format!(
            "zxs-acl-{}-{}.sock",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
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
            wire.send(&Response::Ok { result: json_codec::json!({"protocol":PROTOCOL,"capabilities":REQUIRED_CAPABILITIES}) }, None).unwrap();
            if let Some(code) = remote_error {
                assert!(matches!(
                    wire.receive::<Request>().unwrap().0,
                    Request::AuthorizeRead { handle: 7 }
                ));
                wire.send(
                    &Response::Error {
                        code,
                        message: "ACL authority revoked".into(),
                        retryable: false,
                        operation_id: None,
                    },
                    None,
                )
                .unwrap();
            }
        });
        let client = Client::connect(&socket, "alice", "repository").unwrap();
        let mut fsp = sdk::files_struct::default();
        let opened = Open {
            info: Arc::new(OpenResult {
                entry_id: "directory".into(),
                kind: NamespaceEntryKind::Directory,
                path: String::new(),
                handle: 7,
                size: 0,
                max_file_bytes: 4096,
                version_id: None,
                workspace_id: None,
                mtime: FileMtime::NotFile {},
            }),
            fd: File::open(std::env::temp_dir()).unwrap().into(),
            writable: false,
            io: Arc::new(Mutex::new(WriteSequence::new(false))),
        };
        if failure == 3 {
            assert!(
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let _guard = opened.io.lock().unwrap();
                    panic!("poison logical ACL handle for FFI isolation test");
                }))
                .is_err()
            );
        }
        let mut context = Box::new(Context {
            samba_username: "alice".into(),
            client,
            files: [(&mut fsp as *mut _ as usize, opened)]
                .into_iter()
                .collect(),
        });
        let mut session = authentication::tests::AuthenticatedSession::new();
        session.connection.force_user = failure == 0;
        let mut host = session.handle();
        host.data = (&mut *context as *mut Context).cast();
        output = ptr::dangling_mut();
        // SAFETY: Host/session/open storage stays live, and rejection must occur
        // before SID mapping or allocation. The Context box remains test-owned.
        unsafe {
            let expected = if failure == 3 {
                sdk::map_nt_error_from_unix(libc::EIO).v
            } else if remote_error == Some(storage_contract::ErrorCode::SmbRebindRequired) {
                sdk::map_nt_error_from_unix(libc::ESTALE).v
            } else {
                0xc0000022
            };
            assert_eq!(
                get(
                    &mut host,
                    &mut fsp,
                    SUPPORTED_SECURITY_INFO,
                    ptr::null_mut(),
                    &mut output
                )
                .v,
                expected
            );
        }
        assert!(output.is_null());
        drop(context);
        worker.join().unwrap();
        std::fs::remove_file(socket).unwrap();
    }
}
