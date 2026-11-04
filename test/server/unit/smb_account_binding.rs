use super::*;
use std::io::Read;

fn configuration(root: &std::path::Path) -> SmbConfig {
    SmbConfig {
        socket: root.join("rpc.sock"),
        spool_dir: root.join("smb-spool"),
        allowed_uid: 0,
        username: "alice".into(),
        additional_users: vec!["bob".into()],
        spool_limits: crate::SpoolLimits::default(),
    }
}
#[test]
fn selection_is_durable_and_old_generations_never_revive() {
    let root = crate::tests::private_tempdir();
    let config = configuration(root.path());
    let authority = Authority::open(&config).unwrap();
    assert_eq!(authority.read().unwrap().selection().username, "alice");
    let before = authority.committed_selection().unwrap();
    let selected = authority.select(1, "bob", || Ok(())).unwrap();
    assert_eq!(before.username, "alice");
    assert_eq!(*authority.committed_selection().unwrap(), selected);
    assert_eq!(selected.generation, 2);
    assert!(authority.select(1, "alice", || Ok(())).is_err());
    drop(authority);
    let authority = Authority::open(&config).unwrap();
    assert_eq!(authority.read().unwrap().selection(), &selected);
    assert_eq!(*authority.committed_selection().unwrap(), selected);
    let selected = authority.select(2, "alice", || Ok(())).unwrap();
    assert_eq!(selected.generation, 3);
    assert!(authority.read().unwrap().require_generation(1).is_err());
}
#[test]
fn denied_or_stale_save_leaves_selection_unchanged() {
    let root = crate::tests::private_tempdir();
    let authority = Authority::open(&configuration(root.path())).unwrap();
    assert!(authority.select(1, "outside", || Ok(())).is_err());
    assert!(
        authority
            .select(1, "bob", || Err::<(), _>(ApiError::unauthorized()))
            .is_err()
    );
    assert_eq!(
        authority.select(1, "alice", || Ok(())).unwrap().generation,
        1
    );
    assert_eq!(authority.read().unwrap().selection().username, "alice");
}
#[test]
fn missing_state_removed_candidate_and_unknown_schema_fail_closed() {
    let root = crate::tests::private_tempdir();
    let mut config = configuration(root.path());
    let authority = Authority::open(&config).unwrap();
    authority.select(1, "bob", || Ok(())).unwrap();
    drop(authority);
    config.additional_users.clear();
    assert!(Authority::open(&config).is_err());
    config.additional_users.push("bob".into());
    let path = root.path().join("smb-account/selection.json");
    let bytes = std::fs::read(&path).unwrap();
    std::fs::write(
        &path,
        b"{\"version\":2,\"username\":\"bob\",\"generation\":\"2\"}",
    )
    .unwrap();
    assert!(Authority::open(&config).is_err());
    assert_ne!(std::fs::read(&path).unwrap(), bytes);
    std::fs::remove_file(path).unwrap();
    assert!(Authority::open(&config).is_err());
}
#[test]
fn switch_waits_for_admitted_operation_and_closes_idle_connections() {
    let root = crate::tests::private_tempdir();
    let authority = Authority::open(&configuration(root.path())).unwrap();
    let (mut peer, server) = UnixStream::pair().unwrap();
    peer.set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .unwrap();
    let connections = Arc::new(Mutex::new(BTreeMap::from([(1, server)])));
    authority.attach_connections(&connections);
    let admitted = authority.read().unwrap();
    let worker = authority.clone();
    let (done, result) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || done.send(worker.select(1, "bob", || Ok(()))).unwrap());
    assert!(
        result
            .recv_timeout(std::time::Duration::from_millis(50))
            .is_err()
    );
    drop(admitted);
    assert_eq!(
        result
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap()
            .unwrap()
            .username,
        "bob"
    );
    thread.join().unwrap();
    assert_eq!(peer.read(&mut [0]).unwrap(), 0);
}
#[test]
fn candidates_are_bounded_distinct_and_never_root() {
    let root = crate::tests::private_tempdir();
    let mut config = configuration(root.path());
    config.additional_users = vec!["ALICE".into()];
    assert!(candidates(&config).is_err());
    config.additional_users = vec!["root".into()];
    assert!(candidates(&config).is_err());
    config.additional_users = (0..16).map(|i| format!("user{i}")).collect();
    assert!(candidates(&config).is_err());
}

#[test]
fn replacement_directory_and_duplicate_record_are_never_admitted() {
    let root = crate::tests::private_tempdir();
    let config = configuration(root.path());
    let authority = Authority::open(&config).unwrap();
    let original = root.path().join("smb-account");
    let moved = root.path().join("retained-original");
    std::fs::rename(&original, &moved).unwrap();
    let replacement = PrivateDirectory::create_trusted(&original).unwrap();
    assert!(authority.select(1, "bob", || Ok(())).is_err());
    assert_eq!(authority.read().unwrap().selection().username, "alice");
    assert!(
        std::fs::read_dir(replacement.path())
            .unwrap()
            .next()
            .is_none()
    );
    drop(authority);
    drop(replacement);
    std::fs::remove_dir(&original).unwrap();
    std::fs::rename(moved, &original).unwrap();
    let path = original.join("selection.json");
    std::fs::write(
        &path,
        br#"{"version":1,"username":"alice","username":"bob","generation":"1"}"#,
    )
    .unwrap();
    assert!(Authority::open(&config).is_err());
}

#[test]
fn selection_lock_and_generation_overflow_fail_without_reset() {
    let root = crate::tests::private_tempdir();
    let config = configuration(root.path());
    let authority = Authority::open(&config).unwrap();
    assert!(Authority::open(&config).is_err());
    drop(authority);
    let path = root.path().join("smb-account/selection.json");
    let bytes = format!(
        r#"{{"version":1,"username":"alice","generation":"{}"}}"#,
        u64::MAX
    );
    std::fs::write(&path, &bytes).unwrap();
    let authority = Authority::open(&config).unwrap();
    assert!(authority.select(u64::MAX, "bob", || Ok(())).is_err());
    assert_eq!(std::fs::read_to_string(path).unwrap(), bytes);
    assert_eq!(authority.read().unwrap().selection().username, "alice");
}
