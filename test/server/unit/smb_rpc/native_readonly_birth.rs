use super::*;
use name_policy::Name;
use storage_contract::ErrorCode;

#[test]
fn native_readonly_creator_and_existing_open_if_do_not_own_the_writer_slot() {
    let root = tempfile::tempdir().unwrap();
    let system = root.path().join("state");
    let config = Config::development([(Name::parse("p").unwrap(), root.path().join("primary"))])
        .unwrap();
    let slot = config.disks[0].slot_id.clone();
    let engine = Engine::initialize(&system, config).unwrap();
    let auth = AuthStore::initialize(&system, "alice", "long-test-password").unwrap();
    let spool = root.path().join("spool");
    fs::create_dir(&spool).unwrap();
    fs::set_permissions(&spool, fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir(spool.join("empty-directory")).unwrap();
    let mut direct = Direct::start(root.path(), engine, auth);
    let mut peer = direct.peer();
    let open_if = |writable, exclusive| Request::Open {
        path: "p/FIRST.TXT".into(), writable, create: true, truncate: false,
        exclusive, write_through: false,
    };
    let (born, _reader_fd) = direct.open("p/first.txt", false, true);
    let target = direct.listed(0);
    assert_eq!(born.data_io, OpenIo::NativeWorking);
    assert_eq!(born.entry_id, target.entry.to_string());
    let receipts = |direct: &Direct| direct.backend.with_disk_registration(
        &direct._auth.sole_identity().unwrap(), &slot, |_, engine| {
            no_file_history(engine);
            assert_eq!(engine.state().file_containers.len(), 1);
            Ok(engine.state().operations.len())
        },
    ).unwrap();
    let before = receipts(&direct);
    let Reply::Open { metadata: writer, descriptor: _writer_fd } = peer.dispatch(open_if(true, false))
        .unwrap() else { panic!("independent writer reaches the same native file"); };
    assert_eq!(writer.entry_id, born.entry_id);
    // Even while a genuine writer owns the slot, OPEN_IF may read a
    // verified existing occurrence; it may neither create nor become writable.
    let Reply::Open { metadata: other, descriptor: _other_fd } = direct.call(open_if(false, false))
        else { panic!("read-only existing OPEN_IF"); };
    assert_eq!(other.entry_id, born.entry_id);
    assert!(direct.read(other.handle).is_empty());
    assert_eq!(direct.share.dispatch(open_if(false, true)).unwrap_err().code, ErrorCode::NameConflict);
    assert_eq!(direct.share.dispatch(open_if(true, false)).unwrap_err().code, ErrorCode::FileWriteBusy);
    assert_eq!(direct.share.dispatch(Request::WorkingResize {
        handle: born.handle, length: 1,
    }).unwrap_err().code, ErrorCode::Forbidden);
    assert!(direct.share.dispatch(Request::Open {
        path: "p/first.txt/child".into(), writable: false, create: true, truncate: false,
        exclusive: false, write_through: false,
    }).is_err());
    for handle in [born.handle, other.handle] {
        assert_eq!(direct.flush(handle)["changed"], false);
        direct.call(Request::Close { handle });
    }
    peer.dispatch(Request::Close { handle: writer.handle }).unwrap();
    assert_eq!(direct.listed(0), target);
    assert_eq!(receipts(&direct), before);
}
