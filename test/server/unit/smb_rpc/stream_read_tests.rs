use super::*;

fn corrupt(path: &Path, offset: u64, bytes: &[u8]) {
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .write_all_at(bytes, offset)
        .unwrap();
}

#[test]
fn read_open_does_not_scan_the_body_and_never_transfers_a_payload_descriptor() {
    let (temp, shared, _, branch) = fixture();
    for path in immutable_fixture_payloads(temp.path()) {
        corrupt(&path, 0, b"bad");
    }
    let mut active = session(shared, &branch);
    let (opened, descriptor) = open_file(&mut active, false);
    assert_eq!(opened.size, 3);
    assert_eq!(
        descriptor.read_at(&mut [0; 3], 0).unwrap_err().raw_os_error(),
        Some(libc::EISDIR)
    );
    let error = active.read(opened.handle, 0, 3).unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::ContentUnavailable);
    assert!(std::error::Error::source(&error).is_some());
    active.close(opened.handle, true).unwrap();
    assert!(active.handles.is_empty());
    assert!(active.read_cache.is_none());
}

#[test]
fn read_cache_serves_the_same_verified_bytes_but_still_checks_live_authorization() {
    let (temp, shared, backend, branch) = fixture();
    let paths = immutable_fixture_payloads(temp.path());
    let mut active = session(shared, &branch);
    let (opened, _descriptor) = open_file(&mut active, false);
    assert_eq!(read_session(&mut active, opened.handle, 0, 1), b"o");
    for path in paths {
        corrupt(&path, 0, b"bad");
    }
    assert_eq!(read_session(&mut active, opened.handle, 1, 2), b"ld");
    backend.revoked.store(true, Ordering::SeqCst);
    for (offset, length) in [(0, 1), (3, 1), (0, 0)] {
        assert_eq!(
            active.read(opened.handle, offset, length).unwrap_err().code,
            storage_contract::ErrorCode::Forbidden
        );
    }
    backend.revoked.store(false, Ordering::SeqCst);
    assert!(read_session(&mut active, opened.handle, 3, 1).is_empty());
    assert!(read_session(&mut active, opened.handle, u64::MAX, 1).is_empty());
    assert!(
        active.read(opened.handle, 0, (smb_protocol::protocol::MAX_READ_BYTES + 1) as u32).is_err()
    );
    active.close(opened.handle, true).unwrap();
    assert!(active.read_cache.is_none());
    assert_eq!(
        active.read(opened.handle, 0, 1).unwrap_err().code,
        storage_contract::ErrorCode::NotFound
    );
}

#[test]
fn read_uses_a_verified_same_version_fallback() {
    let (temp, shared, _, branch) = fixture();
    let primary = immutable_fixture_payloads(temp.path())
        .into_iter()
        .find(|path| path.starts_with(temp.path().join("p")))
        .unwrap();
    corrupt(&primary, 0, b"bad");
    let mut active = session(shared, &branch);
    let (opened, _) = open_file(&mut active, false);
    assert_eq!(read_session(&mut active, opened.handle, 0, 3), b"old");
    active.close(opened.handle, false).unwrap();
}

#[test]
fn cached_rpc_read_rejects_a_rebound_session() {
    let (_temp, shared, _, branch) = fixture();
    let (mut wire, worker, hello) = binding_tests::wire(shared.clone());
    assert!(matches!(hello, Response::Ok { .. }));
    let (Response::Ok { result }, descriptor) = binding_tests::exchange(
        &mut wire,
        Request::Open {
            path: "report.txt".into(),
            writable: false,
            create: false,
            truncate: false,
            exclusive: false,
            write_through: false,
        },
    ) else {
        panic!("open failed")
    };
    let opened: OpenResult = json_codec::from_value(result).unwrap();
    assert_eq!(read_rpc(&mut wire, opened.handle, 0, 1), b"o");
    binding_tests::rebind(&shared, &branch, false);
    let (reply, fd) = binding_tests::exchange(
        &mut wire,
        Request::Read { handle: opened.handle, offset: 1, length: 2 },
    );
    assert!(fd.is_none());
    assert!(matches!(
        reply,
        Response::Error { code: storage_contract::ErrorCode::SmbRebindRequired, .. }
    ));
    assert!(matches!(
        binding_tests::exchange(&mut wire, Request::Detach { handle: opened.handle }).0,
        Response::Ok { .. }
    ));
    drop(descriptor);
    drop(wire);
    worker.join().unwrap().unwrap();
}

fn large_fixture() -> (Fixture, Arc<Shared>, String, usize, Vec<PathBuf>) {
    let (temp, shared, backend, branch) = fixture();
    let width = backend.inner.test_with_engine_locked(|engine| {
        let width = engine.state().versions.values().next().unwrap().checksum_chunk_bytes as usize;
        let workspace = engine.create_workspace(&branch, "alice").unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, "report.txt", &vec![42; width * 3 + 17])
            .unwrap();
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: OperationId::generate(),
                principal: "alice".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "large read fixture".into(),
                request_digest: None,
            })
            .unwrap();
        width
    });
    fn visit(root: &Path, size: u64, found: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(root).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(&path, size, found);
            } else if path.components().any(|part| part.as_os_str() == "file-containers")
                && path.metadata().unwrap().len() == size
            {
                found.push(path);
            }
        }
    }
    let mut paths = Vec::new();
    visit(temp.path(), (width * 3 + 17) as u64, &mut paths);
    assert_eq!(paths.len(), 3);
    (temp, shared, branch, width, paths)
}

#[test]
fn first_chunk_succeeds_without_verifying_an_unrequested_corrupt_tail() {
    let (_temp, shared, branch, width, paths) = large_fixture();
    for path in &paths {
        corrupt(path, (width * 2) as u64, &[0]);
    }
    let mut active = session(shared, &branch);
    let (opened, _) = open_file(&mut active, false);
    assert_eq!(read_session(&mut active, opened.handle, 0, 1), [42]);
    assert_eq!(
        read_session(&mut active, opened.handle, (width - 1) as u64, 2),
        [42]
    );
    assert_eq!(read_session(&mut active, opened.handle, width as u64, 1), [42]);
    assert_eq!(
        active.read(opened.handle, (width * 2) as u64, 1).unwrap_err().code,
        storage_contract::ErrorCode::ContentUnavailable
    );
    assert!(active.read_cache.is_none());
}

#[test]
fn held_payload_identity_survives_path_replacement_and_late_truncation_is_an_error() {
    let (_temp, shared, branch, width, paths) = large_fixture();
    let mut active = session(shared, &branch);
    let (opened, _) = open_file(&mut active, false);
    let held: Vec<_> = paths
        .iter()
        .map(|path| {
            let original = path.with_extension("held");
            fs::rename(path, &original).unwrap();
            fs::write(path, vec![0; width * 3 + 17]).unwrap();
            original
        })
        .collect();
    assert_eq!(read_session(&mut active, opened.handle, 0, 1), [42]);
    for path in held {
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_len(width as u64)
            .unwrap();
    }
    let error = active.read(opened.handle, width as u64, 1).unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::ContentUnavailable);
    assert!(format!("{error:?}").contains("UnexpectedEof"));
}
