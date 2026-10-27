//! Real core and Unix RPC regression for live SMB binding admission.
use super::*;
use std::{os::unix::net::UnixStream, thread::JoinHandle};

pub(super) fn wire(shared: Arc<Shared>) -> (FrameStream, JoinHandle<io::Result<()>>, Response) {
    let repository = repository(&shared);
    let (client, server) = UnixStream::pair().unwrap();
    let worker =
        std::thread::spawn(move || run_connection(FrameStream::new(server).unwrap(), shared));
    let mut stream = FrameStream::new(client).unwrap();
    stream
        .send(
            &Request::<String>::Hello {
                protocol: PROTOCOL.into(),
                capabilities: REQUIRED_CAPABILITIES
                    .iter()
                    .map(|value| (*value).into())
                    .collect(),
                username: "alice".into(),
                repository_id: repository,
            },
            None,
        )
        .unwrap();
    let (response, descriptor) = stream.receive::<Response>().unwrap();
    assert!(descriptor.is_none());
    (stream, worker, response)
}
pub(super) fn exchange(wire: &mut FrameStream, request: Request) -> (Response, Option<OwnedFd>) {
    wire.send(&request, None).unwrap();
    wire.receive().unwrap()
}
pub(super) fn rebind(shared: &Shared, branch: &str, enabled: bool) {
    let administrator = identity(shared);
    let repository = repository(shared);
    let binding = shared
        .backend
        .smb_binding_status(&administrator, &repository)
        .unwrap();
    shared
        .backend
        .update_smb_binding(
            &administrator,
            crate::UpdateSmbBinding {
                repository: &repository,
                branch,
                enabled,
                expected_history_generation: binding.history_generation,
                operation: storage_contract::OperationId::generate(),
            },
        )
        .unwrap();
}
fn refused(wire: &mut FrameStream, request: Request) {
    let (response, fd) = exchange(wire, request);
    assert!(fd.is_none());
    assert!(
        matches!(response, Response::Error { code, .. } if code == storage_contract::ErrorCode::SmbRebindRequired)
    );
}
fn create(path: &str) -> Request {
    Request::Open {
        path: path.into(),
        writable: true,
        create: true,
        truncate: false,
        exclusive: true,
        write_through: false,
    }
}
// Every check below inspects the entire dispatch log, so a fallback Tree or an
// extra workspace-selection check cannot hide behind a successful response.
#[derive(Clone, Copy)]
enum ReadAuthorization {
    Version,
    CurrentBranch,
}
fn open_authorization_handles(
    wire: &mut FrameStream,
) -> [(OpenResult, File, ReadAuthorization); 3] {
    [
        ("report.txt", false, ReadAuthorization::Version),
        ("report.txt", true, ReadAuthorization::CurrentBranch),
        ("", false, ReadAuthorization::CurrentBranch),
    ]
    .map(|(path, writable, action)| {
        let (reply, descriptor) = exchange(
            wire,
            Request::Open {
                path: path.into(),
                writable,
                create: false,
                truncate: false,
                exclusive: false,
                write_through: false,
            },
        );
        let Response::Ok { result } = reply else {
            panic!("authorization fixture open failed: {reply:?}")
        };
        (
            json_codec::from_value(result).unwrap(),
            File::from(descriptor.unwrap()),
            action,
        )
    })
}
fn check_read_authorization(
    wire: &mut FrameStream,
    backend: &LostCommitReply,
    branch: &str,
    handles: &[(OpenResult, File, ReadAuthorization)],
    error: Option<&str>,
) {
    for (opened, _, action) in handles {
        backend.take_authorizations();
        backend.take_version_authorizations();
        let (reply, descriptor) = exchange(
            wire,
            Request::AuthorizeRead {
                handle: opened.handle,
            },
        );
        assert!(descriptor.is_none());
        match (reply, error) {
            (Response::Ok { result }, None) => assert_eq!(result, json!({"authorized":true})),
            (Response::Error { code, .. }, Some(expected)) => assert_eq!(code.as_str(), expected),
            (reply, expected) => {
                panic!("unexpected authorization reply: {reply:?}, error={expected:?}")
            }
        }
        match action {
            ReadAuthorization::Version => {
                assert_eq!(
                    backend.take_version_authorizations(),
                    vec![opened.version_id.as_ref().unwrap().clone()]
                );
                assert!(backend.take_authorizations().is_empty());
            }
            ReadAuthorization::CurrentBranch => {
                assert!(backend.take_version_authorizations().is_empty());
                assert_eq!(backend.take_authorizations(), vec![branch.to_owned()]);
            }
        }
    }
}
fn missing_read_handle(wire: &mut FrameStream, backend: &LostCommitReply, handle: u64) {
    backend.take_authorizations();
    backend.take_version_authorizations();
    let (reply, descriptor) = exchange(wire, Request::AuthorizeRead { handle });
    assert!(descriptor.is_none());
    assert!(matches!(
        reply,
        Response::Error { code, message, retryable: false, operation_id: None }
            if code == storage_contract::ErrorCode::NotFound && message == "SMB handle missing"
    ));
    assert!(backend.take_authorizations().is_empty());
    assert!(backend.take_version_authorizations().is_empty());
}
fn close_authorization_handles(
    wire: &mut FrameStream,
    handles: &[(OpenResult, File, ReadAuthorization)],
) {
    for (opened, _, _) in handles {
        let (reply, descriptor) = exchange(
            wire,
            Request::Close {
                handle: opened.handle,
            },
        );
        assert!(descriptor.is_none());
        assert!(matches!(reply, Response::Ok { .. }));
    }
}

#[test]
fn authorize_read_dispatches_narrow_checks_without_tree_and_preserves_missing_handles() {
    let (_temp, shared, backend, branch) = fixture();
    let (mut client, worker, hello) = wire(shared);
    assert!(matches!(hello, Response::Ok { .. }));
    let handles = open_authorization_handles(&mut client);
    check_read_authorization(&mut client, &backend, &branch, &handles, None);
    missing_read_handle(&mut client, &backend, u64::MAX);
    close_authorization_handles(&mut client, &handles);
    for (opened, _, _) in &handles {
        missing_read_handle(&mut client, &backend, opened.handle);
    }
    drop(client);
    worker.join().unwrap().unwrap();
}

#[test]
fn authorize_read_rechecks_current_owner_and_account_for_every_open_handle_kind() {
    for owner_changed in [true, false] {
        let (temp, shared, backend, branch) = fixture();
        let (mut client, worker, hello) = wire(shared.clone());
        assert!(matches!(hello, Response::Ok { .. }));
        let handles = open_authorization_handles(&mut client);
        check_read_authorization(&mut client, &backend, &branch, &handles, None);
        let (replacement, error) = if owner_changed {
            (
                AuthStore::initialize(
                    &temp.path().join("replacement-auth"),
                    "replacement-owner",
                    "long-test-password",
                )
                .unwrap(),
                "UNAUTHORIZED",
            )
        } else {
            (
                AuthStore::unavailable(&temp.path().join("recovering-auth")),
                "RECOVERY_REQUIRED",
            )
        };
        temp.auth.lock().unwrap().replace_authority(replacement);
        check_read_authorization(&mut client, &backend, &branch, &handles, Some(error));
        // A missing handle still fails locally, even while admission is denied.
        missing_read_handle(&mut client, &backend, u64::MAX);
        let original = AuthStore::open(&temp.path().join("auth")).unwrap();
        temp.auth.lock().unwrap().replace_authority(original);
        // Failed authorization did not consume or retarget any existing handle.
        check_read_authorization(&mut client, &backend, &branch, &handles, None);
        close_authorization_handles(&mut client, &handles);
        drop(client);
        worker.join().unwrap().unwrap();
    }
}

#[test]
fn authorize_read_keeps_read_semantics_after_normal_writes_and_default_selection() {
    let (_temp, shared, backend, branch) = fixture();
    let (repository, _main, draft) = directory_fixture(&shared);
    let (mut client, worker, hello) = wire(shared.clone());
    assert!(matches!(hello, Response::Ok { .. }));
    let handles = open_authorization_handles(&mut client);
    let (writer, file, _) = &handles[1];
    file.write_all_at(b"new", 0).unwrap();
    for request in [
        Request::Dirty {
            handle: writer.handle,
        },
        Request::Flush {
            handle: writer.handle,
        },
    ] {
        assert!(matches!(
            exchange(&mut client, request).0,
            Response::Ok { .. }
        ));
    }
    check_read_authorization(&mut client, &backend, &branch, &handles, None);
    let mut bytes = [0; 3];
    handles[0].1.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"old", "immutable reader keeps its opened version");
    select_directory(&shared, &repository, &draft);
    // A changed write destination must not prevent reading an existing FD.
    check_read_authorization(&mut client, &backend, &branch, &handles, None);
    handles[0].1.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"old");
    file.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(
        &bytes, b"new",
        "writable read still uses its private descriptor"
    );
    assert!(matches!(
        exchange(&mut client, Request::PrepareWrite {
            handle: writer.handle, end: 3, bytes: 3,
        }).0,
        Response::Error { code, .. } if code == storage_contract::ErrorCode::DirectoryBranchConflict
    ));
    close_authorization_handles(&mut client, &handles);
    drop(client);
    worker.join().unwrap().unwrap();
}

#[test]
fn authorize_read_rechecks_binding_after_disable_and_reenable_for_every_handle_kind() {
    let (_temp, shared, backend, branch) = fixture();
    let (mut client, worker, hello) = wire(shared.clone());
    assert!(matches!(hello, Response::Ok { .. }));
    let handles = open_authorization_handles(&mut client);
    let (writer, file, _) = &handles[1];
    // Publication retires the active workspace, permitting administrative rebind
    // while the writable descriptor remains open for subsequent read requests.
    file.write_all_at(b"new", 0).unwrap();
    for request in [
        Request::Dirty {
            handle: writer.handle,
        },
        Request::Flush {
            handle: writer.handle,
        },
    ] {
        assert!(matches!(
            exchange(&mut client, request).0,
            Response::Ok { .. }
        ));
    }
    check_read_authorization(&mut client, &backend, &branch, &handles, None);
    for enabled in [false, true] {
        rebind(&shared, &branch, enabled);
        check_read_authorization(
            &mut client,
            &backend,
            &branch,
            &handles,
            Some("SMB_REBIND_REQUIRED"),
        );
        missing_read_handle(&mut client, &backend, u64::MAX);
    }
    close_authorization_handles(&mut client, &handles);
    drop(client);
    worker.join().unwrap().unwrap();
}

#[test]
fn disabled_and_reenabled_binding_cannot_revive_an_idle_authenticated_connection() {
    let (_temp, shared, _backend, branch) = fixture();
    let (mut old, worker, hello) = wire(shared.clone());
    assert!(matches!(hello, Response::Ok { .. }));
    rebind(&shared, &branch, false);
    refused(&mut old, create("after-disable.txt"));
    refused(
        &mut old,
        Request::Stat {
            path: String::new(),
        },
    );
    refused(
        &mut old,
        Request::Namespace {
            operation_id: new_id("op"),
            kind: smb_protocol::protocol::NamespaceOperation::Mkdir,
            path: "disabled".into(),
            destination: None,
            replace: false,
        },
    );
    let (new, rejected, hello) = wire(shared.clone());
    assert!(
        matches!(hello, Response::Error { code, .. } if code == storage_contract::ErrorCode::SmbRebindRequired)
    );
    drop(new);
    rejected.join().unwrap().unwrap();
    rebind(&shared, &branch, true);
    refused(&mut old, create("after-reenable.txt"));
    let (mut new, admitted, hello) = wire(shared.clone());
    assert!(matches!(hello, Response::Ok { .. }));
    assert!(matches!(
        exchange(
            &mut new,
            Request::Stat {
                path: "report.txt".into()
            }
        )
        .0,
        Response::Ok { .. }
    ));
    let (opened, fd) = exchange(&mut new, create("fresh.txt"));
    let Response::Ok { result } = opened else {
        panic!("fresh binding must admit write")
    };
    let opened: OpenResult = json_codec::from_value(result).unwrap();
    assert!(fd.is_some());
    assert!(matches!(
        exchange(
            &mut new,
            Request::Close {
                handle: opened.handle
            }
        )
        .0,
        Response::Ok { .. }
    ));
    drop(fd);
    drop(new);
    admitted.join().unwrap().unwrap();
    drop(old);
    worker.join().unwrap().unwrap();
}

#[test]
fn existing_reader_keeps_its_version_on_normal_writes_but_stops_after_binding_change() {
    let (_temp, shared, _backend, branch) = fixture();
    let (mut wire, worker, hello) = wire(shared.clone());
    assert!(matches!(hello, Response::Ok { .. }));
    let (opened, fd) = exchange(
        &mut wire,
        Request::Open {
            path: "report.txt".into(),
            writable: false,
            create: false,
            truncate: false,
            exclusive: false,
            write_through: false,
        },
    );
    let Response::Ok { result } = opened else {
        panic!("read open failed")
    };
    let opened: OpenResult = json_codec::from_value(result).unwrap();
    let read = File::from(fd.unwrap());
    let mut writer = session(shared.clone(), &branch);
    let (write, bytes) = open_file(&mut writer, true);
    bytes.write_all_at(b"new", 0).unwrap();
    writer.dirty(write.handle).unwrap();
    writer.close(write.handle, false).unwrap();
    assert!(matches!(
        exchange(
            &mut wire,
            Request::AuthorizeRead {
                handle: opened.handle
            }
        )
        .0,
        Response::Ok { .. }
    ));
    let mut old = [0; 3];
    read.read_exact_at(&mut old, 0).unwrap();
    assert_eq!(&old, b"old");
    rebind(&shared, &branch, true);
    refused(
        &mut wire,
        Request::AuthorizeRead {
            handle: opened.handle,
        },
    );
    refused(&mut wire, create("after-rebind.txt"));
    assert!(matches!(
        exchange(
            &mut wire,
            Request::Close {
                handle: opened.handle
            }
        )
        .0,
        Response::Ok { .. }
    ));
    drop(read);
    drop(wire);
    worker.join().unwrap().unwrap();
}

#[test]
fn live_writer_cannot_prepare_or_publish_after_disable_and_failed_close_retains_bytes() {
    let (_temp, shared, backend, branch) = fixture();
    let (mut wire, worker, hello) = wire(shared.clone());
    assert!(matches!(hello, Response::Ok { .. }));
    let (opened, fd) = exchange(
        &mut wire,
        Request::Open {
            path: "report.txt".into(),
            writable: true,
            create: false,
            truncate: false,
            exclusive: false,
            write_through: false,
        },
    );
    let Response::Ok { result } = opened else {
        panic!("write open failed")
    };
    let opened: OpenResult = json_codec::from_value(result).unwrap();
    let file = File::from(fd.unwrap());
    assert!(matches!(
        exchange(
            &mut wire,
            Request::PrepareWrite {
                handle: opened.handle,
                end: 3,
                bytes: 3
            }
        )
        .0,
        Response::Ok { .. }
    ));
    file.write_all_at(b"new", 0).unwrap();
    assert!(matches!(
        exchange(
            &mut wire,
            Request::Dirty {
                handle: opened.handle
            }
        )
        .0,
        Response::Ok { .. }
    ));
    // Active workspaces correctly block administrative rebind. A published
    // writer remains open without an active workspace, so disable is allowed.
    let administrator = identity(&shared);
    let repository = repository(&shared);
    let binding = shared
        .backend
        .smb_binding_status(&administrator, &repository)
        .unwrap();
    assert_eq!(
        shared
            .backend
            .update_smb_binding(
                &administrator,
                crate::UpdateSmbBinding {
                    repository: &repository,
                    branch: &branch,
                    enabled: false,
                    expected_history_generation: binding.history_generation,
                    operation: storage_contract::OperationId::generate()
                }
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::WorkspaceBusy
    );
    assert!(matches!(
        exchange(
            &mut wire,
            Request::Flush {
                handle: opened.handle
            }
        )
        .0,
        Response::Ok { .. }
    ));
    rebind(&shared, &branch, false);
    // Model bytes from a previously admitted/in-flight FD write. Revocation
    // does not forcibly close a descriptor held by the trusted host process.
    file.write_all_at(b"bad", 0).unwrap();
    for request in [
        Request::PrepareWrite {
            handle: opened.handle,
            end: 4,
            bytes: 1,
        },
        Request::Dirty {
            handle: opened.handle,
        },
    ] {
        refused(&mut wire, request);
    }
    for request in [
        Request::Flush {
            handle: opened.handle,
        },
        Request::Close {
            handle: opened.handle,
        },
    ] {
        let (reply, fd) = exchange(&mut wire, request);
        assert!(fd.is_none());
        assert!(
            matches!(reply, Response::Error { code, .. } if code == storage_contract::ErrorCode::WorkspaceRebaseRequired)
        );
    }
    assert_eq!(backend.uploads.load(Ordering::SeqCst), 1);
    assert!(shared.file_writers.lock().unwrap().is_empty());
    assert!(
        matches!(exchange(&mut wire, Request::AuthorizeRead { handle: opened.handle }).0, Response::Error { code, .. } if code == storage_contract::ErrorCode::NotFound)
    );
    let records: Vec<_> = fs::read_dir(&shared.spool)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.to_string_lossy().ends_with(".work.json"))
        .collect();
    assert_eq!(records.len(), 1);
    let record: Value = json_codec::from_slice(&fs::read(&records[0]).unwrap()).unwrap();
    assert_eq!(record["handle_state"], "detached");
    assert_eq!(record["detach_error"]["code"], "WORKSPACE_REBASE_REQUIRED");
    assert_eq!(
        fs::read(record["source_path"].as_str().unwrap()).unwrap(),
        b"bad"
    );
    drop(file);
    drop(wire);
    worker.join().unwrap().unwrap();
}

#[test]
fn hello_captures_typed_repository_but_each_synthetic_root_access_rechecks_binding() {
    let (_temp, shared, backend, branch) = fixture();
    let id = repository(&shared);
    let (mut client, worker, hello) = wire(shared.clone());
    assert!(matches!(hello, Response::Ok { .. }));
    assert_eq!(
        *backend.repository_snapshots.lock().unwrap(),
        vec![id.clone()]
    );
    assert!(backend.repository_authorizations.lock().unwrap().is_empty());
    let (reply, fd) = exchange(
        &mut client,
        Request::Stat {
            path: String::new(),
        },
    );
    assert!(matches!(reply, Response::Ok { .. }));
    assert!(fd.is_none());
    assert_eq!(
        *backend.repository_authorizations.lock().unwrap(),
        vec![id.clone()]
    );
    rebind(&shared, &branch, false);
    refused(&mut client, Request::Stat { path: ".".into() });
    assert_eq!(
        *backend.repository_authorizations.lock().unwrap(),
        vec![id.clone(), id.clone()]
    );
    assert_eq!(*backend.repository_snapshots.lock().unwrap(), vec![id]);
    drop(client);
    worker.join().unwrap().unwrap();
}
