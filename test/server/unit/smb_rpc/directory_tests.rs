//! Snapshot, ownership and live-authorization checks over the actual local RPC.
use super::binding_tests::{exchange, wire};
use super::*;
use smb_protocol::protocol::DirectoryPage;

fn page(wire: &mut FrameStream, handle: u64, restart: bool) -> DirectoryPage {
    let (reply, fd) = exchange(wire, Request::ReadDirectory { handle, restart });
    assert!(fd.is_none());
    let Response::Ok { result } = reply else {
        panic!("directory read failed: {reply:?}")
    };
    json_codec::from_value(result).unwrap()
}
fn root(wire: &mut FrameStream) -> u64 {
    let (reply, descriptor) = exchange(
        wire,
        Request::Open {
            path: String::new(),
            writable: false,
            create: false,
            truncate: false,
            exclusive: false,
            write_through: false,
        },
    );
    assert!(descriptor.is_some());
    let Response::Ok { result } = reply else {
        panic!("root open failed")
    };
    json_codec::from_value::<OpenResult>(result).unwrap().handle
}
#[test]
fn directory_cursor_freezes_its_commit_and_restart_captures_a_new_snapshot() {
    let (_temp, shared, _backend, branch) = fixture();
    let (mut client, worker, hello) = wire(shared.clone());
    assert!(matches!(hello, Response::Ok { .. }));
    let handle = root(&mut client);
    let first = page(&mut client, handle, false);
    assert_eq!(first.entries.len(), 1);
    assert_eq!(first.entries[0].entry.path, "report.txt");
    let mut writer = session(shared.clone(), &branch);
    writer
        .namespace(
            smb_protocol::protocol::NamespaceOperation::Mkdir,
            "new-directory".into(),
            None,
            false,
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    assert!(page(&mut client, handle, false).entries.is_empty());
    let restarted = page(&mut client, handle, true);
    assert_eq!(
        restarted
            .entries
            .iter()
            .map(|node| node.entry.path.as_str())
            .collect::<Vec<_>>(),
        vec!["new-directory", "report.txt"]
    );
    assert!(!restarted.more);
    assert!(matches!(
        exchange(
            &mut client,
            Request::Stat {
                path: "report.txt".into()
            }
        )
        .0,
        Response::Ok { .. }
    ));
    assert!(matches!(
        exchange(&mut client, Request::Close { handle }).0,
        Response::Ok { .. }
    ));
    drop(client);
    worker.join().unwrap().unwrap();
}
#[test]
fn directory_pages_recheck_current_binding_and_handle_ownership() {
    let (_temp, shared, _backend, _branch) = fixture();
    let (mut client, worker, hello) = wire(shared.clone());
    assert!(matches!(hello, Response::Ok { .. }));
    let handle = root(&mut client);
    assert_eq!(page(&mut client, handle, false).entries.len(), 1);
    let branch = shared
        .backend
        .repository_snapshot(&identity(&shared), &repository(&shared))
        .unwrap()
        .main_branch_id;
    binding_tests::rebind(&shared, &branch, false);
    assert!(
        matches!(exchange(&mut client,Request::ReadDirectory {handle,restart:false}).0,Response::Error {code,..} if code == storage_contract::ErrorCode::SmbRebindRequired)
    );
    assert!(
        matches!(exchange(&mut client,Request::ReadDirectory {handle:handle+100,restart:false}).0,Response::Error {code,..} if code == storage_contract::ErrorCode::NotFound)
    );
    assert!(matches!(
        exchange(&mut client, Request::Close { handle }).0,
        Response::Ok { .. }
    ));
    drop(client);
    worker.join().unwrap().unwrap();
}
#[test]
fn renamed_and_replaced_directory_does_not_retarget_the_open_handle() {
    let (_temp, shared, _backend, branch) = fixture();
    let mut session = session(shared, &branch);
    session
        .namespace(
            smb_protocol::protocol::NamespaceOperation::Mkdir,
            "original".into(),
            None,
            false,
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    let (opened, _) = session
        .open("original".into(), false, false, false, false, false)
        .unwrap();
    let opened: OpenResult = json_codec::from_value(opened).unwrap();
    session
        .namespace(
            smb_protocol::protocol::NamespaceOperation::Rename,
            "original".into(),
            Some("moved".into()),
            false,
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    session
        .namespace(
            smb_protocol::protocol::NamespaceOperation::Mkdir,
            "original".into(),
            None,
            false,
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    // The open descriptor keeps its snapshot, even before its first read.
    // Rewind cannot follow the replacement at the original alias path.
    let (old, _) = session
        .dispatch(Request::ReadDirectory {
            handle: opened.handle,
            restart: false,
        })
        .unwrap();
    assert!(
        json_codec::from_value::<DirectoryPage>(json_reply(old))
            .unwrap()
            .entries
            .is_empty()
    );
    assert_eq!(
        session
            .dispatch(Request::ReadDirectory {
                handle: opened.handle,
                restart: true,
            })
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::SmbRebindRequired
    );
}

fn cursor_state(session: &Session, handle: u64) -> (String, Option<String>) {
    let Handle::Directory(directory) = &session.handles[&handle] else {
        panic!("expected an open directory");
    };
    let cursor = directory.cursor.as_ref().unwrap();
    (cursor.snapshot.commit_id.clone(), cursor.after.clone())
}

#[test]
fn directory_read_failure_keeps_captured_page_and_restart_position() {
    let (temp, shared, _backend, branch) = fixture();
    let mut active = session(shared.clone(), &branch);
    let (opened, _descriptor) = active
        .open(String::new(), false, false, false, false, false)
        .unwrap();
    let opened: OpenResult = json_codec::from_value(opened).unwrap();
    let captured = cursor_state(&active, opened.handle);
    assert!(captured.1.is_none());
    active
        .namespace(
            smb_protocol::protocol::NamespaceOperation::Mkdir,
            "later".into(),
            None,
            false,
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();

    temp.auth
        .lock()
        .unwrap()
        .replace_authority(AuthStore::unavailable());
    for restart in [false, true] {
        assert_eq!(
            active
                .read_directory(opened.handle, restart)
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::RecoveryRequired
        );
        assert_eq!(cursor_state(&active, opened.handle), captured);
    }
    temp.auth
        .lock()
        .unwrap()
        .replace_authority(AuthStore::open(&temp.path().join("auth")).unwrap());
    let first: DirectoryPage =
        json_codec::from_value(active.read_directory(opened.handle, false).unwrap()).unwrap();
    assert_eq!(first.entries.len(), 1);
    assert_eq!(first.entries[0].entry.path, "report.txt");
    let exhausted = cursor_state(&active, opened.handle);
    assert_eq!(exhausted.0, captured.0);
    assert_eq!(exhausted.1.as_deref(), Some("report.txt"));
    for _ in 0..2 {
        let page: DirectoryPage =
            json_codec::from_value(active.read_directory(opened.handle, false).unwrap()).unwrap();
        assert!(page.entries.is_empty());
        assert!(!page.more);
        assert_eq!(cursor_state(&active, opened.handle), exhausted);
    }
    let restarted: DirectoryPage =
        json_codec::from_value(active.read_directory(opened.handle, true).unwrap()).unwrap();
    assert_eq!(
        restarted
            .entries
            .iter()
            .map(|entry| entry.entry.path.as_str())
            .collect::<Vec<_>>(),
        vec!["later", "report.txt"]
    );
    assert_ne!(cursor_state(&active, opened.handle).0, captured.0);
    active.close(opened.handle, false).unwrap();
}
