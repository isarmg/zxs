//! Default share resolution and live default-branch reads use the real engine/RPC.
use super::*;
use std::{os::unix::net::UnixStream, thread::JoinHandle};

fn empty_fixture(two_primaries: bool) -> (Fixture, Arc<Shared>, Arc<LostCommitReply>) {
    let temp = tempfile::tempdir().unwrap();
    let mut config = Config::development([temp.path().join("p"), temp.path().join("b")]).unwrap();
    if two_primaries {
        config.disks[1].level = 1;
        config.disks[1].slot_id = "P02".into();
        config.disks[1].upstream_slot_id = None;
    }
    let engine = Engine::initialize(&temp.path().join("state"), config).unwrap();
    fixture_backend(temp, engine)
}

fn wire(
    shared: Arc<Shared>,
    username: &str,
) -> (FrameStream, JoinHandle<io::Result<()>>, Response) {
    let (client, server) = UnixStream::pair().unwrap();
    let worker =
        std::thread::spawn(move || run_connection(FrameStream::new(server).unwrap(), shared));
    let mut stream = FrameStream::new(client).unwrap();
    stream
        .send(
            &Request::<String>::HelloDefault {
                protocol: PROTOCOL.into(),
                capabilities: REQUIRED_CAPABILITIES
                    .iter()
                    .map(|value| (*value).into())
                    .collect(),
                username: username.into(),
            },
            None,
        )
        .unwrap();
    let (response, fd) = stream.receive::<Response>().unwrap();
    assert!(fd.is_none());
    (stream, worker, response)
}

fn close_wire(stream: FrameStream, worker: JoinHandle<io::Result<()>>) {
    drop(stream);
    worker.join().unwrap().unwrap();
}

fn okay(stream: &mut FrameStream, request: Request) -> (Value, Option<OwnedFd>) {
    let (reply, descriptor) = binding_tests::exchange(stream, request);
    let Response::Ok { result } = reply else {
        panic!("request failed: {reply:?}")
    };
    (result, descriptor)
}

fn open(stream: &mut FrameStream, path: &str, writable: bool, create: bool) -> (OpenResult, File) {
    let (value, fd) = okay(
        stream,
        Request::Open {
            path: path.into(),
            writable,
            create,
            truncate: false,
            exclusive: create,
            write_through: false,
        },
    );
    (
        json_codec::from_value(value).unwrap(),
        File::from(fd.unwrap()),
    )
}

fn save(stream: &mut FrameStream, opened: &OpenResult, file: &File, bytes: &[u8]) {
    okay(
        stream,
        Request::PrepareWrite {
            handle: opened.handle,
            end: bytes.len() as u64,
            bytes: bytes.len() as u64,
        },
    );
    file.write_all_at(bytes, 0).unwrap();
    okay(
        stream,
        Request::Dirty {
            handle: opened.handle,
        },
    );
    okay(
        stream,
        Request::Flush {
            handle: opened.handle,
        },
    );
    okay(
        stream,
        Request::Close {
            handle: opened.handle,
        },
    );
}

fn directory(stream: &mut FrameStream, path: &str) -> Vec<String> {
    let (opened, _fd) = open(stream, path, false, false);
    let mut paths = Vec::new();
    loop {
        let (page, _) = okay(
            stream,
            Request::ReadDirectory {
                handle: opened.handle,
                restart: false,
            },
        );
        let page: smb_protocol::protocol::DirectoryPage = json_codec::from_value(page).unwrap();
        paths.extend(
            page.entries
                .iter()
                .map(|entry| entry.entry.path.as_str().to_owned()),
        );
        if !page.more {
            break;
        }
    }
    okay(
        stream,
        Request::Close {
            handle: opened.handle,
        },
    );
    paths
}

#[test]
fn default_share_creates_one_internal_root_on_write_and_reuses_it_after_real_rpc_save() {
    let (_temp, shared, _backend) = empty_fixture(false);
    let (mut stream, worker, reply) = wire(shared.clone(), "alice");
    assert!(matches!(reply, Response::Ok { .. }), "{reply:?}");
    assert_eq!(directory(&mut stream, ""), vec!["P01"]);
    assert!(directory(&mut stream, "P01").is_empty());
    assert!(
        shared
            .backend
            .repositories_snapshot(&identity(&shared))
            .unwrap()
            .is_empty()
    );
    let (opened, fd) = open(&mut stream, "P01/default.txt", true, true);
    save(&mut stream, &opened, &fd, b"hello");
    let initial = shared
        .backend
        .repositories_snapshot(&identity(&shared))
        .unwrap();
    assert_eq!(initial.len(), 1);
    assert!(initial[0].file_manager_root && initial[0].smb_enabled);
    assert_eq!(initial[0].primary_slot_id, "P01");
    assert_eq!(initial[0].target_copies, 1);
    assert_eq!(directory(&mut stream, "P01"), vec!["default.txt"]);
    let (reader, _fd) = open(&mut stream, "P01/default.txt", false, false);
    assert_eq!(read_rpc(&mut stream, reader.handle, 0, 5), b"hello");
    okay(
        &mut stream,
        Request::Close {
            handle: reader.handle,
        },
    );
    close_wire(stream, worker);
    let (mut stream, worker, reply) = wire(shared.clone(), "alice");
    assert!(matches!(reply, Response::Ok { .. }), "{reply:?}");
    assert_eq!(
        shared
            .backend
            .repositories_snapshot(&identity(&shared))
            .unwrap(),
        initial
    );
    let (reader, _fd) = open(&mut stream, "P01/default.txt", false, false);
    assert_eq!(read_rpc(&mut stream, reader.handle, 0, 5), b"hello");
    let (reply, _) = binding_tests::exchange(&mut stream, Request::Stat { path: "B01".into() });
    assert!(matches!(
        reply,
        Response::Error {
            code: storage_contract::ErrorCode::NotFound,
            ..
        }
    ));
    close_wire(stream, worker);
}

#[test]
fn default_share_missing_stat_before_first_create_keeps_the_namespace_unchanged() {
    for named_repository in [false, true] {
        let (_temp, shared, _backend) = if named_repository {
            let (temp, shared, backend, _branch) = fixture();
            (temp, shared, backend)
        } else {
            empty_fixture(false)
        };
        let before = shared
            .backend
            .repositories_snapshot(&identity(&shared))
            .unwrap();
        let (mut stream, worker, _) = wire(shared.clone(), "alice");
        // Samba probes the final component before issuing a create. The
        // missing disk-root namespace must be an ordinary not-found result,
        // not an I/O failure and not a reason to allocate metadata on lookup.
        let (reply, descriptor) = binding_tests::exchange(
            &mut stream,
            Request::Stat {
                path: "P01/default.txt".into(),
            },
        );
        assert!(
            matches!(
                reply,
                Response::Error {
                    code: storage_contract::ErrorCode::DirectoryNotFound,
                    ..
                }
            ),
            "{reply:?}"
        );
        assert!(descriptor.is_none());
        assert_eq!(
            shared
                .backend
                .repositories_snapshot(&identity(&shared))
                .unwrap(),
            before
        );
        let (opened, file) = open(&mut stream, "P01/default.txt", true, true);
        save(&mut stream, &opened, &file, b"created");
        let (reader, _file) = open(&mut stream, "P01/default.txt", false, false);
        assert_eq!(read_rpc(&mut stream, reader.handle, 0, 7), b"created");
        close_wire(stream, worker);
    }
}

#[test]
fn default_share_rejects_wrong_account_before_creating_any_namespace() {
    let (_temp, shared, _backend) = empty_fixture(false);
    let (stream, worker, reply) = wire(shared.clone(), "another-user");
    assert!(matches!(
        reply,
        Response::Error {
            code: storage_contract::ErrorCode::Forbidden,
            ..
        }
    ));
    close_wire(stream, worker);
    assert!(
        shared
            .backend
            .repositories_snapshot(&identity(&shared))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn default_share_keeps_primary_paths_separate_and_rejects_cross_namespace_rename() {
    let (_temp, shared, _backend) = empty_fixture(true);
    let (mut stream, worker, reply) = wire(shared.clone(), "alice");
    assert!(matches!(reply, Response::Ok { .. }), "{reply:?}");
    assert_eq!(directory(&mut stream, ""), vec!["P01", "P02"]);
    for (path, bytes) in [("P01/same.txt", b"one"), ("P02/same.txt", b"two")] {
        let (opened, fd) = open(&mut stream, path, true, true);
        save(&mut stream, &opened, &fd, bytes);
    }
    let (one, _fd) = open(&mut stream, "P01/same.txt", false, false);
    let (two, _fd) = open(&mut stream, "P02/same.txt", false, false);
    assert_ne!(one.handle, two.handle);
    assert_eq!(read_rpc(&mut stream, one.handle, 0, 3), b"one");
    assert_eq!(read_rpc(&mut stream, two.handle, 0, 3), b"two");
    let operation_id = new_id("op").unwrap();
    let (reply, _) = binding_tests::exchange(
        &mut stream,
        Request::Namespace {
            kind: smb_protocol::protocol::NamespaceOperation::Rename,
            path: "P01/same.txt".into(),
            destination: Some("P02/moved.txt".into()),
            replace: false,
            operation_id: operation_id.clone(),
        },
    );
    assert!(
        matches!(reply, Response::Error { code: storage_contract::ErrorCode::UnsupportedOperation, operation_id: Some(ref returned), .. } if returned == &operation_id),
        "{reply:?}"
    );
    assert_eq!(directory(&mut stream, "P01"), vec!["same.txt"]);
    assert_eq!(directory(&mut stream, "P02"), vec!["same.txt"]);
    close_wire(stream, worker);
}

#[test]
fn default_share_preserves_named_repository_beside_new_disk_root() {
    let (_temp, shared, _backend, _branch) = fixture();
    let before = shared
        .backend
        .repositories_snapshot(&identity(&shared))
        .unwrap();
    let (mut stream, worker, reply) = wire(shared.clone(), "alice");
    assert!(matches!(reply, Response::Ok { .. }), "{reply:?}");
    assert_eq!(directory(&mut stream, "P01"), vec!["资料"]);
    assert_eq!(directory(&mut stream, "P01/资料"), vec!["report.txt"]);
    let (old, _fd) = open(&mut stream, "P01/资料/report.txt", false, false);
    assert_eq!(read_rpc(&mut stream, old.handle, 0, 3), b"old");
    let (created, fd) = open(&mut stream, "P01/new.txt", true, true);
    save(&mut stream, &created, &fd, b"new");
    assert_eq!(directory(&mut stream, "P01"), vec!["资料", "new.txt"]);
    let after = shared
        .backend
        .repositories_snapshot(&identity(&shared))
        .unwrap();
    assert_eq!(after.len(), before.len() + 1);
    assert!(after.contains(&before[0]));
    let (reply, _) = binding_tests::exchange(
        &mut stream,
        Request::Namespace {
            kind: smb_protocol::protocol::NamespaceOperation::Rename,
            path: "P01/new.txt".into(),
            destination: Some("P01/资料/moved.txt".into()),
            replace: false,
            operation_id: new_id("op").unwrap(),
        },
    );
    assert!(
        matches!(
            reply,
            Response::Error {
                code: storage_contract::ErrorCode::UnsupportedOperation,
                ..
            }
        ),
        "{reply:?}"
    );
    assert_eq!(read_rpc(&mut stream, old.handle, 0, 3), b"old");
    close_wire(stream, worker);
}

#[test]
fn live_smb_connection_follows_directory_default_without_rebinding_and_keeps_old_reader() {
    let (_temp, shared, _backend, _branch) = fixture();
    let (repository, main, draft) = directory_fixture(&shared);
    let before = shared
        .backend
        .repository_snapshot(&identity(&shared), &repository)
        .unwrap();
    let (mut stream, worker, reply) = wire(shared.clone(), "alice");
    assert!(matches!(reply, Response::Ok { .. }));
    let (old_reader, _fd) = open(&mut stream, "P01/资料/report.txt", false, false);
    select_directory(&shared, &repository, &draft);
    let (writer, fd) = open(&mut stream, "P01/资料/report.txt", true, false);
    save(&mut stream, &writer, &fd, b"new");
    let (reader, _fd) = open(&mut stream, "P01/资料/report.txt", false, false);
    assert_eq!(read_rpc(&mut stream, reader.handle, 0, 3), b"new");
    okay(
        &mut stream,
        Request::Close {
            handle: reader.handle,
        },
    );
    assert_eq!(read_rpc(&mut stream, old_reader.handle, 0, 3), b"old");
    select_directory(&shared, &repository, &main);
    let (reader, _fd) = open(&mut stream, "P01/资料/report.txt", false, false);
    assert_eq!(read_rpc(&mut stream, reader.handle, 0, 3), b"old");
    let after = shared
        .backend
        .repository_snapshot(&identity(&shared), &repository)
        .unwrap();
    assert_eq!(before.main_branch_id, after.main_branch_id);
    assert_eq!(before.smb_binding_generation, after.smb_binding_generation);
    close_wire(stream, worker);
}

#[test]
fn default_share_fences_old_writer_when_directory_default_changes() {
    let (_temp, shared, backend, _branch) = fixture();
    let (repository, _main, draft) = directory_fixture(&shared);
    let (mut stream, worker, reply) = wire(shared.clone(), "alice");
    assert!(matches!(reply, Response::Ok { .. }));
    let (old, fd) = open(&mut stream, "P01/资料/report.txt", true, false);
    okay(
        &mut stream,
        Request::PrepareWrite {
            handle: old.handle,
            end: 3,
            bytes: 3,
        },
    );
    fd.write_all_at(b"bad", 0).unwrap();
    select_directory(&shared, &repository, &draft);
    let (reply, _) = binding_tests::exchange(&mut stream, Request::Dirty { handle: old.handle });
    assert!(
        matches!(
            reply,
            Response::Error {
                code: storage_contract::ErrorCode::DirectoryBranchConflict,
                ..
            }
        ),
        "{reply:?}"
    );
    assert_eq!(backend.uploads.load(Ordering::SeqCst), 0);
    assert!(fs::read_dir(&shared.spool).unwrap().any(|entry| {
        entry
            .unwrap()
            .path()
            .extension()
            .is_some_and(|ext| ext == "json")
    }));
    okay(&mut stream, Request::Abort { handle: old.handle });
    let (reader, _fd) = open(&mut stream, "P01/资料/report.txt", false, false);
    assert_eq!(read_rpc(&mut stream, reader.handle, 0, 3), b"old");
    close_wire(stream, worker);
}

#[test]
fn default_share_does_not_revive_closed_namespace_after_disable_and_reenable() {
    let (_temp, shared, _backend, branch) = fixture();
    let (mut stream, worker, _) = wire(shared.clone(), "alice");
    let (reader, _fd) = open(&mut stream, "P01/资料/report.txt", false, false);
    okay(
        &mut stream,
        Request::Close {
            handle: reader.handle,
        },
    );
    binding_tests::rebind(&shared, &branch, false);
    binding_tests::rebind(&shared, &branch, true);
    let (reply, _) = binding_tests::exchange(
        &mut stream,
        Request::Stat {
            path: "P01/资料/report.txt".into(),
        },
    );
    assert!(
        matches!(
            reply,
            Response::Error {
                code: storage_contract::ErrorCode::SmbRebindRequired,
                ..
            }
        ),
        "{reply:?}"
    );
    close_wire(stream, worker);
    let (mut stream, worker, _) = wire(shared.clone(), "alice");
    let (reader, _fd) = open(&mut stream, "P01/资料/report.txt", false, false);
    assert_eq!(read_rpc(&mut stream, reader.handle, 0, 3), b"old");
    close_wire(stream, worker);
}

#[test]
fn default_share_keeps_canonical_existing_path_and_validates_missing_create_before_root() {
    let (_temp, shared, _backend) = empty_fixture(false);
    let (mut stream, worker, _) = wire(shared.clone(), "alice");
    let nfd = "P01/cafe\u{301}.txt";
    let request = |path: &str| Request::Open {
        path: path.into(),
        writable: true,
        create: true,
        truncate: false,
        exclusive: false,
        write_through: false,
    };
    let (reply, _) = binding_tests::exchange(&mut stream, request(nfd));
    assert!(matches!(reply, Response::Error { .. }));
    assert!(
        shared
            .backend
            .repositories_snapshot(&identity(&shared))
            .unwrap()
            .is_empty()
    );
    let (created, fd) = open(&mut stream, "P01/café.txt", true, true);
    save(&mut stream, &created, &fd, b"yes");
    let (value, fd) = okay(&mut stream, request(nfd));
    let opened: OpenResult = json_codec::from_value(value).unwrap();
    assert_eq!(opened.path, "P01/café.txt");
    drop(fd);
    okay(
        &mut stream,
        Request::Abort {
            handle: opened.handle,
        },
    );
    close_wire(stream, worker);
}

#[test]
fn default_disk_directory_rechecks_binding_before_returning_its_captured_page() {
    let (_temp, shared, _backend) = empty_fixture(false);
    let (mut stream, worker, _) = wire(shared.clone(), "alice");
    let (created, fd) = open(&mut stream, "P01/file.txt", true, true);
    save(&mut stream, &created, &fd, b"x");
    let (directory, _fd) = open(&mut stream, "P01", false, false);
    let repository = shared
        .backend
        .repositories_snapshot(&identity(&shared))
        .unwrap()
        .remove(0);
    binding_tests::rebind(&shared, &repository.main_branch_id, false);
    for request in [
        Request::AuthorizeRead {
            handle: directory.handle,
        },
        Request::ReadDirectory {
            handle: directory.handle,
            restart: false,
        },
    ] {
        let (reply, _) = binding_tests::exchange(&mut stream, request);
        assert!(
            matches!(
                reply,
                Response::Error {
                    code: storage_contract::ErrorCode::SmbRebindRequired,
                    ..
                }
            ),
            "{reply:?}"
        );
    }
    okay(
        &mut stream,
        Request::Close {
            handle: directory.handle,
        },
    );
    binding_tests::rebind(&shared, &repository.main_branch_id, true);
    let (reply, _) = binding_tests::exchange(
        &mut stream,
        Request::Stat {
            path: "P01/file.txt".into(),
        },
    );
    assert!(
        matches!(
            reply,
            Response::Error {
                code: storage_contract::ErrorCode::SmbRebindRequired,
                ..
            }
        ),
        "{reply:?}"
    );
    close_wire(stream, worker);
}

#[test]
fn default_namespace_changes_use_the_real_root_and_do_not_create_cross_disk_targets() {
    let (_temp, shared, _backend) = empty_fixture(true);
    let (mut stream, worker, _) = wire(shared.clone(), "alice");
    let namespace = |kind, path: &str, destination: Option<&str>| Request::Namespace {
        kind,
        path: path.into(),
        destination: destination.map(str::to_owned),
        replace: false,
        operation_id: new_id("op").unwrap(),
    };
    use smb_protocol::protocol::NamespaceOperation as Kind;
    okay(&mut stream, namespace(Kind::Mkdir, "P01/folder", None));
    let (created, fd) = open(&mut stream, "P01/folder/a.txt", true, true);
    save(&mut stream, &created, &fd, b"x");
    let (reply, _) = binding_tests::exchange(
        &mut stream,
        namespace(Kind::Rename, "P01/folder/a.txt", Some("P02/b.txt")),
    );
    assert!(
        matches!(
            reply,
            Response::Error {
                code: storage_contract::ErrorCode::UnsupportedOperation,
                ..
            }
        ),
        "{reply:?}"
    );
    assert_eq!(
        shared
            .backend
            .repositories_snapshot(&identity(&shared))
            .unwrap()
            .len(),
        1
    );
    okay(
        &mut stream,
        namespace(Kind::Rename, "P01/folder/a.txt", Some("P01/folder/b.txt")),
    );
    assert_eq!(directory(&mut stream, "P01/folder"), vec!["folder/b.txt"]);
    okay(
        &mut stream,
        namespace(Kind::Unlink, "P01/folder/b.txt", None),
    );
    okay(&mut stream, namespace(Kind::Rmdir, "P01/folder", None));
    assert!(directory(&mut stream, "P01").is_empty());
    close_wire(stream, worker);
}

#[test]
fn default_disk_pages_are_bounded_and_do_not_mix_changed_snapshots() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([temp.path().join("p"), temp.path().join("b")]).unwrap();
    let mut engine = Engine::initialize(&temp.path().join("state"), config).unwrap();
    let repo = engine
        .create_disk_root(
            "P01",
            "alice",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "alice")
        .unwrap();
    for index in 0..260 {
        engine
            .stage_bytes(
                &workspace.workspace_id,
                &format!("file-{index:03}.txt"),
                b"x",
            )
            .unwrap();
    }
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate().unwrap(),
            principal: "alice".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "pagination fixture".into(),
            request_digest: None,
        })
        .unwrap();
    let (_fixture, shared, _backend) = fixture_backend(temp, engine);
    let (mut stream, worker, _) = wire(shared.clone(), "alice");
    let (opened, _fd) = open(&mut stream, "P01", false, false);
    let (first, _) = okay(
        &mut stream,
        Request::ReadDirectory {
            handle: opened.handle,
            restart: false,
        },
    );
    assert!(
        json_codec::to_vec(&first).unwrap().len()
            < smb_protocol::protocol::MAX_DIRECTORY_PAGE_BYTES
    );
    let page: smb_protocol::protocol::DirectoryPage = json_codec::from_value(first).unwrap();
    assert_eq!(
        page.entries.len(),
        smb_protocol::protocol::MAX_DIRECTORY_PAGE_ENTRIES
    );
    assert!(page.more);
    let (created, fd) = open(&mut stream, "P01/new.txt", true, true);
    save(&mut stream, &created, &fd, b"n");
    let (reply, _) = binding_tests::exchange(
        &mut stream,
        Request::ReadDirectory {
            handle: opened.handle,
            restart: false,
        },
    );
    assert!(
        matches!(
            reply,
            Response::Error {
                code: storage_contract::ErrorCode::ListingChanged,
                ..
            }
        ),
        "{reply:?}"
    );
    okay(
        &mut stream,
        Request::ReadDirectory {
            handle: opened.handle,
            restart: true,
        },
    );
    let (last, _) = okay(
        &mut stream,
        Request::ReadDirectory {
            handle: opened.handle,
            restart: false,
        },
    );
    let last: smb_protocol::protocol::DirectoryPage = json_codec::from_value(last).unwrap();
    assert_eq!(last.entries.len(), 5);
    assert!(!last.more);
    assert_eq!(directory(&mut stream, "P01").len(), 261);
    close_wire(stream, worker);
}

#[test]
fn default_namespace_resolution_failure_preserves_identity_without_replay_permission() {
    let (temp, shared, _backend) = empty_fixture(false);
    let (mut stream, worker, _) = wire(shared.clone(), "alice");
    crate::lock(&temp.auth).replace_authority(AuthStore::unavailable());
    let operation_id = new_id("op").unwrap();
    let (reply, _) = binding_tests::exchange(
        &mut stream,
        Request::Namespace {
            kind: smb_protocol::protocol::NamespaceOperation::Mkdir,
            path: "P01/folder".into(),
            destination: None,
            replace: false,
            operation_id: operation_id.clone(),
        },
    );
    assert!(
        matches!(reply, Response::Error { code: storage_contract::ErrorCode::RecoveryRequired,
        operation_id: Some(ref returned), retryable: false, .. } if returned == &operation_id),
        "{reply:?}"
    );
    close_wire(stream, worker);
}

#[test]
fn default_disk_open_pages_have_a_separate_bounded_budget_released_on_read_or_close() {
    let (_temp, shared, _backend) = empty_fixture(false);
    let (mut stream, worker, _) = wire(shared.clone(), "alice");
    let mut handles = Vec::new();
    for _ in 0..8 {
        handles.push(open(&mut stream, "P01", false, false).0.handle);
    }
    let (reply, _) = binding_tests::exchange(
        &mut stream,
        Request::Open {
            path: "P01".into(),
            writable: false,
            create: false,
            truncate: false,
            exclusive: false,
            write_through: false,
        },
    );
    assert!(
        matches!(
            reply,
            Response::Error {
                code: storage_contract::ErrorCode::ResourceLimitExceeded,
                ..
            }
        ),
        "{reply:?}"
    );
    okay(
        &mut stream,
        Request::ReadDirectory {
            handle: handles[0],
            restart: false,
        },
    );
    handles.push(open(&mut stream, "P01", false, false).0.handle);
    for handle in handles {
        okay(&mut stream, Request::Close { handle });
    }
    let (last, _fd) = open(&mut stream, "P01", false, false);
    okay(
        &mut stream,
        Request::Close {
            handle: last.handle,
        },
    );
    close_wire(stream, worker);
}

#[test]
fn default_named_directory_identity_and_canonical_paths_match_enumeration_stat_and_open() {
    let (_temp, shared, _backend, _branch) = fixture();
    let (mut stream, worker, _) = wire(shared.clone(), "alice");
    let (disk, _fd) = open(&mut stream, "p01", false, false);
    assert_eq!(disk.path, "P01");
    let (stat, _) = okay(&mut stream, Request::Stat { path: "p01".into() });
    assert_eq!(stat["path"], "P01");
    assert_eq!(stat["entry_id"], disk.entry_id);
    let (root, _) = okay(&mut stream, Request::Stat { path: ".".into() });
    assert_eq!(root["path"], "");
    let (page, _) = okay(
        &mut stream,
        Request::ReadDirectory {
            handle: disk.handle,
            restart: false,
        },
    );
    let page: smb_protocol::protocol::DirectoryPage = json_codec::from_value(page).unwrap();
    let listed = page
        .entries
        .iter()
        .find(|entry| entry.entry.path.as_str() == "资料")
        .unwrap();
    let (stat, _) = okay(
        &mut stream,
        Request::Stat {
            path: "p01/资料".into(),
        },
    );
    let (named, _fd) = open(&mut stream, "p01/资料", false, false);
    assert_eq!(stat["path"], "P01/资料");
    assert_eq!(stat["entry_id"], listed.entry.entry_id);
    assert_eq!(named.entry_id, listed.entry.entry_id);
    assert_eq!(named.path, "P01/资料");
    let (contents, _) = okay(
        &mut stream,
        Request::ReadDirectory {
            handle: named.handle,
            restart: false,
        },
    );
    let contents: smb_protocol::protocol::DirectoryPage = json_codec::from_value(contents).unwrap();
    assert_eq!(contents.entries[0].entry.path.as_str(), "report.txt");
    let (reader, _fd) = open(&mut stream, "p01/资料/report.txt", false, false);
    assert_eq!(reader.path, "P01/资料/report.txt");
    let (stat, _) = okay(
        &mut stream,
        Request::Stat {
            path: "p01/资料/report.txt".into(),
        },
    );
    assert_eq!(stat["path"], reader.path);
    assert_eq!(stat["entry_id"], reader.entry_id);
    close_wire(stream, worker);
}

#[test]
fn default_mkdir_replay_admission_precedes_any_internal_root_creation() {
    for cause in ["published", "record", "active"] {
        let (_temp, shared, _backend) = empty_fixture(true);
        let operation_id = storage_contract::OperationId::generate().unwrap();
        if cause == "published" {
            shared
                .backend
                .create_repository(
                    &identity(&shared),
                    crate::CreateRepository {
                        name: storage_contract::RepositoryName::parse("Existing").unwrap(),
                        primary_slot_id: "P02".into(),
                        target_copies: 1,
                        operation_id,
                    },
                )
                .unwrap();
            // The final engine-lock admission also rejects a reused operation,
            // even if a caller's prior protected read has raced with another API.
            let error = shared
                .backend
                .resolve_smb_path(
                    &identity(&shared),
                    crate::ResolveSmbPath {
                        slot_id: "P01".into(),
                        path: name_policy::LookupPath::parse("new").unwrap(),
                        intent: crate::SmbPathIntent::CreateDirectory(operation_id),
                    },
                )
                .err()
                .unwrap();
            assert_eq!(
                error.code,
                storage_contract::ErrorCode::IdempotencyKeyReused
            );
        } else if cause == "record" {
            fs::write(
                shared.spool.join(format!("{operation_id}.namespace.json")),
                b"preserve existing evidence",
            )
            .unwrap();
        } else {
            shared
                .namespace_operations
                .lock()
                .unwrap()
                .insert(operation_id);
        }
        let before = shared
            .backend
            .repositories_snapshot(&identity(&shared))
            .unwrap();
        let (mut stream, worker, _) = wire(shared.clone(), "alice");
        let (reply, _) = binding_tests::exchange(
            &mut stream,
            Request::Namespace {
                kind: smb_protocol::protocol::NamespaceOperation::Mkdir,
                path: "P01/new".into(),
                destination: None,
                replace: false,
                operation_id: operation_id.to_string(),
            },
        );
        let expected = if cause == "published" {
            storage_contract::ErrorCode::OperationIdConflict
        } else {
            storage_contract::ErrorCode::RecoveryRequired
        };
        assert!(
            matches!(reply, Response::Error { code, retryable: false, operation_id: Some(ref returned), .. }
            if code == expected && returned == &operation_id.to_string()),
            "{cause}: {reply:?}"
        );
        assert_eq!(
            shared
                .backend
                .repositories_snapshot(&identity(&shared))
                .unwrap(),
            before,
            "{cause}"
        );
        if cause == "record" {
            assert_eq!(
                fs::read(shared.spool.join(format!("{operation_id}.namespace.json"))).unwrap(),
                b"preserve existing evidence"
            );
        }
        if cause == "active" {
            shared
                .namespace_operations
                .lock()
                .unwrap()
                .remove(&operation_id);
        }
        close_wire(stream, worker);
    }
}

#[test]
fn invalid_read_only_create_or_truncate_never_creates_a_default_root() {
    let (_temp, shared, _backend) = empty_fixture(false);
    let (mut stream, worker, _) = wire(shared.clone(), "alice");
    for (create, truncate) in [(true, false), (false, true), (true, true)] {
        let (reply, _) = binding_tests::exchange(
            &mut stream,
            Request::Open {
                path: "P01/file.txt".into(),
                writable: false,
                create,
                truncate,
                exclusive: false,
                write_through: false,
            },
        );
        assert!(
            matches!(
                reply,
                Response::Error {
                    code: storage_contract::ErrorCode::InvalidRequest,
                    ..
                }
            ),
            "{reply:?}"
        );
        assert!(
            shared
                .backend
                .repositories_snapshot(&identity(&shared))
                .unwrap()
                .is_empty()
        );
    }
    close_wire(stream, worker);
}

#[test]
fn default_mkdir_exhausted_budget_never_creates_a_root_and_releases_operation() {
    let (_temp, shared, _backend) = empty_fixture(false);
    let identity = identity(&shared);
    // Fill the fixture's 16 MiB account budget, including its 64 KiB record
    // overhead, without allocating a physical file of that size.
    let extent = 16 * 1024 * 1024 - 64 * 1024;
    let held = shared
        .budget
        .reserve(
            &identity.principal,
            &shared.spool.join("occupied"),
            extent,
            extent,
        )
        .unwrap();
    let operation_id = OperationId::generate().unwrap();
    let request = || Request::Namespace {
        kind: smb_protocol::protocol::NamespaceOperation::Mkdir,
        path: "P01/new".into(),
        destination: None,
        replace: false,
        operation_id: operation_id.to_string(),
    };
    let (mut stream, worker, _) = wire(shared.clone(), "alice");
    let (reply, _) = binding_tests::exchange(&mut stream, request());
    assert!(
        matches!(reply, Response::Error {
            code: storage_contract::ErrorCode::ResourceLimitExceeded,
            retryable: false,
            operation_id: Some(ref returned),
            ..
        } if returned == &operation_id.to_string()),
        "{reply:?}"
    );
    assert!(
        shared
            .backend
            .repositories_snapshot(&identity)
            .unwrap()
            .is_empty()
    );
    assert!(shared.namespace_operations.lock().unwrap().is_empty());
    for extension in ["budget.json", "namespace.json"] {
        assert!(
            !shared
                .spool
                .join(format!("{operation_id}.{extension}"))
                .try_exists()
                .unwrap()
        );
    }
    drop(held);
    okay(&mut stream, request());
    assert_eq!(directory(&mut stream, "P01"), vec!["new"]);
    close_wire(stream, worker);
}
