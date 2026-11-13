use super::*;
use storage_contract::container_edit::CommitMode;
use storage_contract::Generation;

fn fresh() -> (Fixture, Arc<Shared>, Arc<LostCommitReply>, String) {
    let temp = tempfile::tempdir().unwrap();
    for name in ["state", "p"] {
        let path = temp.path().join(name);
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let mut engine = Engine::initialize_manual_container_storage(
        &temp.path().join("state"),
        Config::development([temp.path().join("p")]).unwrap(),
        OperationId::generate().unwrap(),
    ).unwrap();
    let repo = engine.create_repository(
        &storage_contract::RepositoryName::parse("manual recovery").unwrap(),
        "P01", 1, "alice", OperationId::generate().unwrap(),
    ).unwrap();
    let workspace = engine.create_workspace(&repo.main_branch_id, "alice").unwrap();
    engine.stage_bytes(&workspace.workspace_id, "report.txt", b"old").unwrap();
    engine.commit_workspace(CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: OperationId::generate().unwrap(),
        principal: "alice".into(), expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token, message: "initial".into(), request_digest: None,
    }).unwrap();
    let (fixture, shared, backend) = fixture_backend(temp, engine);
    let entry = session(shared.clone(), &repo.main_branch_id).entries().unwrap().remove(0);
    shared.backend.set_container_commit_mode(&identity(&shared), crate::SetContainerCommitMode {
        context: crate::ReadContainerEdit {
            file: entry.file_id.parse().unwrap(), repository: repo.repository_id.parse().unwrap(),
            path: entry.path,
        },
        expected_generation: Generation::new(0), mode: CommitMode::Manual,
        operation_id: OperationId::generate().unwrap(),
    }).unwrap();
    (fixture, shared, backend, repo.main_branch_id)
}

fn reopen(fixture: &Fixture) -> Arc<Shared> {
    let backend = Arc::new(EngineBackend::new(
        Engine::open(&fixture.path().join("state")).unwrap(),
    ));
    backend.set_management_auth(crate::lock(&fixture.auth).reader()).unwrap();
    let spool = fixture.path().join("spool");
    Arc::new(Shared {
        budget: budget::Budget::recover(&spool, crate::SpoolLimits {
            bytes_per_account: 16 * 1024 * 1024,
            bytes_total: 64 * 1024 * 1024, recovery_reserve_bytes: 1,
        }).unwrap(),
        max_file_bytes: 4096,
        authenticated_sessions: tokio::sync::Semaphore::new(MAX_AUTHENTICATED_SESSIONS),
        backend, account: account_authority(&spool),
        auth: crate::lock(&fixture.auth).reader(), spool,
        file_writers: Mutex::new(BTreeSet::new()),
        namespace_operations: Mutex::new(BTreeSet::new()),
    })
}

fn recover(client: &mut Session, operation: OperationId, resume: bool) -> Value {
    match client.dispatch_admitted(Request::RecoverManualDraft {
        operation_id: operation, resume,
    }).unwrap() {
        Reply::Json(value) => value,
        _ => panic!("manual recovery returns a JSON protocol result"),
    }
}

fn wire_recover(shared: Arc<Shared>, operation: OperationId, resume: bool) -> Value {
    let (client, server) = std::os::unix::net::UnixStream::pair().unwrap();
    let worker = std::thread::spawn(move || {
        run_connection(FrameStream::new(server).unwrap(), shared).unwrap();
    });
    let mut wire = FrameStream::new(client).unwrap();
    wire.send(&Request::<String>::HelloDefault {
        protocol: PROTOCOL.into(),
        capabilities: REQUIRED_CAPABILITIES.iter().map(|value| (*value).into()).collect(),
        username: "alice".into(),
    }, None).unwrap();
    let (hello, descriptor) = wire.receive::<Response>().unwrap();
    assert!(matches!(hello, Response::Ok { .. }));
    assert!(descriptor.is_none());
    wire.send(&Request::RecoverManualDraft {
        operation_id: operation.to_string(), resume,
    }, None).unwrap();
    let (response, descriptor) = wire.receive::<Response>().unwrap();
    assert!(descriptor.is_none());
    let Response::Ok { result } = response else {
        panic!("authenticated recovery RPC failed: {response:?}");
    };
    drop(wire);
    worker.join().unwrap();
    result
}

#[test]
fn frozen_unsent_request_survives_service_restart_and_later_live_descriptor_changes() {
    let (root, shared, backend, branch) = fresh();
    let head = shared.backend.branch_snapshot(&identity(&shared), &branch).unwrap().head_commit_id;
    let original = session(shared.clone(), &branch).entries().unwrap().remove(0);
    let mut client = session(shared.clone(), &branch);
    let (opened, descriptor) = client.open("report.txt".into(), true, false, false, false, false)
        .unwrap();
    let file = File::from(descriptor);
    client.prepare_write(opened.handle, 5, 5).unwrap();
    file.write_all_at(b"first", 0).unwrap();
    client.dirty(opened.handle).unwrap();
    let writer = match client.handles.get(&opened.handle).unwrap() {
        Handle::Write(writer, _) => writer,
        _ => panic!("writable open owns its original source"),
    };
    let revision = manual_recovery::freeze(&shared, &identity(&shared), writer, 4096).unwrap();
    file.write_all_at(b"later", 0).unwrap();
    file.sync_all().unwrap();
    drop(file);
    drop(client);
    drop(backend);
    drop(shared);
    let shared = reopen(&root);
    let mut client = session(shared.clone(), &branch);
    let query = wire_recover(shared.clone(), revision.operation, false);
    assert_eq!(query["draft_persisted"], false);
    assert_eq!(query["live_bytes_changed"], true);
    let saved = wire_recover(shared.clone(), revision.operation, true);
    assert_eq!(saved["draft_persisted"], true);
    assert_eq!(saved["live_bytes_changed"], true);
    assert_eq!(saved["original_receipt"]["commit_id"], Value::Null);
    let (opened, descriptor) = client.open("report.txt".into(), false, false, false, false, false)
        .unwrap();
    drop(descriptor);
    assert_eq!(client.read(opened.handle, 0, 5).unwrap().as_ref(), b"first");
    assert_eq!(client.entries().unwrap()[0].version_id, original.version_id);
    assert_eq!(shared.backend.branch_snapshot(&identity(&shared), &branch).unwrap().head_commit_id, head);
    assert_eq!(recover(&mut client, revision.operation, true)["original_receipt"],
        saved["original_receipt"]);
}

#[test]
fn lost_reply_preserves_original_revision_until_reconciled_then_dirty_op_advances() {
    let (_root, shared, backend, branch) = fresh();
    let mut client = session(shared.clone(), &branch);
    let head = shared.backend.branch_snapshot(&identity(&shared), &branch).unwrap().head_commit_id;
    let (opened, descriptor) = client.open("report.txt".into(), true, false, false, false, false)
        .unwrap();
    let file = File::from(descriptor);
    client.prepare_write(opened.handle, 5, 5).unwrap();
    file.write_all_at(b"first", 0).unwrap();
    client.dirty(opened.handle).unwrap();
    backend.fail.store(true, Ordering::SeqCst);
    assert!(client.flush(opened.handle).is_err());
    let original = match client.handles.get(&opened.handle).unwrap() {
        Handle::Write(writer, _) => writer.operation_id,
        _ => panic!("writer remains after lost response"),
    };
    file.write_all_at(b"later", 0).unwrap();
    assert_eq!(client.flush(opened.handle).unwrap_err().code,
        storage_contract::ErrorCode::WorkspaceRebaseRequired);
    let saved = recover(&mut client, original, false);
    assert_eq!(saved["draft_persisted"], true);
    assert_eq!(saved["live_bytes_changed"], true);
    client.dirty(opened.handle).unwrap();
    let next = client.flush(opened.handle).unwrap();
    assert_ne!(next["operation_id"], json!(original));
    assert_eq!(recover(&mut client, original, true)["original_receipt"], saved["original_receipt"]);
    assert_eq!(shared.backend.branch_snapshot(&identity(&shared), &branch).unwrap().head_commit_id, head);
}

#[test]
fn original_result_remains_queryable_after_discard_and_mode_change_without_new_save() {
    let (_root, shared, backend, branch) = fresh();
    let mut client = session(shared.clone(), &branch);
    let (opened, descriptor) = client.open("report.txt".into(), true, false, false, false, false)
        .unwrap();
    let file = File::from(descriptor);
    client.prepare_write(opened.handle, 5, 5).unwrap();
    file.write_all_at(b"first", 0).unwrap();
    client.dirty(opened.handle).unwrap();
    let saved = client.flush(opened.handle).unwrap();
    let operation: OperationId = saved["operation_id"].as_str().unwrap().parse().unwrap();
    let file_id = client.entries().unwrap()[0].file_id.parse().unwrap();
    backend.inner.test_with_engine_locked(|engine| {
        let principal = storage_contract::Principal::new("alice".into()).unwrap();
        let edit = engine.container_edit(file_id).unwrap();
        engine.discard_container_draft(file_id, edit.generation(), operation,
            &principal, OperationId::generate().unwrap()).unwrap();
        let edit = engine.container_edit(file_id).unwrap();
        engine.set_container_commit_mode(&storage_contract::container_edit::SetCommitMode {
            file: file_id, expected_generation: edit.generation(), mode: CommitMode::Automatic,
        }, &principal, OperationId::generate().unwrap()).unwrap();
    });
    let before = backend.inner.test_with_engine_locked(|engine| engine.state().clone());
    assert_eq!(recover(&mut client, operation, true)["draft_persisted"], true);
    backend.inner.test_with_engine_locked(|engine| assert_eq!(engine.state(), &before));
}

#[test]
fn unresolved_request_cannot_resume_after_mode_change_and_legacy_bytes_are_retained() {
    let (_root, shared, backend, branch) = fresh();
    let mut client = session(shared.clone(), &branch);
    let (opened, descriptor) = client.open("report.txt".into(), true, false, false, false, false)
        .unwrap();
    let file = File::from(descriptor);
    client.prepare_write(opened.handle, 5, 5).unwrap();
    file.write_all_at(b"first", 0).unwrap();
    client.dirty(opened.handle).unwrap();
    let writer = match client.handles.get(&opened.handle).unwrap() {
        Handle::Write(writer, _) => writer,
        _ => panic!("writable open owns original bytes"),
    };
    let revision = manual_recovery::freeze(&shared, &identity(&shared), writer, 4096).unwrap();
    let file_id = writer.container.as_ref().unwrap().context.file;
    let record = shared.spool.join(format!("manual-{}.work.json", revision.operation));
    let frozen = shared.spool.join(format!("manual-{}", revision.operation));
    let record_bytes = fs::read(&record).unwrap();
    backend.inner.test_with_engine_locked(|engine| {
        let principal = storage_contract::Principal::new("alice".into()).unwrap();
        engine.set_container_commit_mode(&storage_contract::container_edit::SetCommitMode {
            file: file_id, expected_generation: Generation::new(1), mode: CommitMode::Automatic,
        }, &principal, OperationId::generate().unwrap()).unwrap();
    });
    let before = backend.inner.test_with_engine_locked(|engine| engine.state().clone());
    assert_eq!(recover(&mut client, revision.operation, false)["draft_persisted"], false);
    assert!(client.dispatch_admitted(Request::RecoverManualDraft {
        operation_id: revision.operation, resume: true,
    }).is_err());
    backend.inner.test_with_engine_locked(|engine| assert_eq!(engine.state(), &before));
    assert_eq!(fs::read(&record).unwrap(), record_bytes);
    assert_eq!(fs::read(&frozen).unwrap(), b"first");
    let legacy = br#"{"format":"smb-protocol-work","published":false}"#;
    fs::write(&record, legacy).unwrap();
    assert!(client.dispatch_admitted(Request::RecoverManualDraft {
        operation_id: revision.operation, resume: true,
    }).is_err());
    assert_eq!(fs::read(&record).unwrap(), legacy);
    assert_eq!(fs::read(&frozen).unwrap(), b"first");
    backend.inner.test_with_engine_locked(|engine| assert_eq!(engine.state(), &before));
}
