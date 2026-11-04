//! Real terminal owner and fresh-generation service transitions, source only.
use super::*;
use platform_io::fs_safety::EntryName;
use storage_addition::{DIRECTORY, Format, name, read_record};
use storage_contract::storage_directories::InspectRequest;
use storage_contract::storage_lifecycle::*;

struct Fixture {
    backend: Arc<ManagementBackend>,
    identity: Identity,
    token: String,
    auth: Arc<Mutex<AuthStore>>,
    original: Config,
    data: PathBuf,
    discovery: PathBuf,
    historical_operation: OperationId,
    _root: tempfile::TempDir,
}
fn fixture() -> Fixture {
    let root = crate::tests::private_tempdir();
    let data = root.path().join("system");
    let discovery = root.path().join("storage");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&discovery)
        .unwrap();
    let mut original =
        Config::development([discovery.join("old-voter"), discovery.join("retained-data")])
            .unwrap();
    original.disks[1].metadata_member = false;
    let mut engine = storage_engine::Engine::initialize(&data, original.clone()).unwrap();
    let historical_operation = OperationId::generate().unwrap();
    engine
        .create_repository(
            &storage_contract::RepositoryName::parse("retained-history").unwrap(),
            "P01",
            1,
            "admin",
            historical_operation,
        )
        .unwrap();
    let management = data.join("management");
    drop(AuthStore::initialize_management(&management, "admin", "resume-password").unwrap());
    let (backend, mut auth) = open_service(&data, &management).unwrap();
    let login = auth.login("admin", "resume-password").unwrap();
    let identity = auth.authenticate(&login.token).unwrap();
    let engine = Arc::new(EngineBackend::new(engine));
    engine.set_management_auth(auth.reader()).unwrap();
    let auth = Arc::new(Mutex::new(auth));
    *crate::lock(&backend.inner.auth) = Some(Arc::clone(&auth));
    backend.inner.set(Some(engine), StorageState::Ready);
    let request = ExitRequest {
        operation_id: OperationId::generate().unwrap(),
        slot_id: original.disks[0].slot_id.clone(),
        expected_disk_id: original.disks[0].disk_id.clone(),
        action: ExitAction::Deregister,
    };
    let preview: ExitPreview = json_codec::from_value(
        backend
            .storage_exit("disk_exit_preview", json!(request), &identity)
            .unwrap(),
    )
    .unwrap();
    let status: ExitStatus = json_codec::from_value(
        backend
            .storage_exit("disk_exit_commit", json!(ExitCommit { preview }), &identity)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(status.state, ExitState::Completed);
    assert!(matches!(
        crate::lock(&backend.inner.state).backend,
        BackendState::Terminal(_)
    ));
    Fixture {
        backend,
        identity,
        token: login.token,
        auth,
        original,
        data,
        discovery,
        historical_operation,
        _root: root,
    }
}
fn request(f: &Fixture) -> AdditionRequest {
    let root = f.discovery.join("fresh-voter");
    fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
    let listing = crate::storage_directories::browse_fresh_within(
        f.discovery.to_str().unwrap(),
        root.to_str().unwrap(),
        &f.original
            .disks
            .iter()
            .map(|disk| disk.root.clone())
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let selection = crate::storage_directories::inspect_fresh_within(
        f.discovery.to_str().unwrap(),
        &InspectRequest {
            mount_path: listing.mount_path,
            root_path: root.to_str().unwrap().into(),
        },
        &[f.data.clone(), f.backend.inner.management_root.clone()],
        &f.original
            .disks
            .iter()
            .map(|disk| disk.root.clone())
            .collect::<Vec<_>>(),
    )
    .unwrap();
    AdditionRequest {
        operation_id: OperationId::generate().unwrap(),
        additions: vec![DirectoryAddition {
            selection,
            slot_id: "N01".into(),
            level: 1,
            upstream_slot_id: None,
            metadata_member: true,
        }],
    }
}
fn command(
    f: &Fixture,
    action: &'static str,
    input: Value,
    identity: &Identity,
) -> Result<Value, ApiError> {
    f.backend.storage_addition(action, input, identity, || {
        Ok(f.discovery.to_str().unwrap().into())
    })
}
fn directory(f: &Fixture) -> PrivateDirectory {
    PrivateDirectory::open_existing(&f.backend.inner.management_root)
        .unwrap()
        .create_child(&EntryName::new(DIRECTORY).unwrap())
        .unwrap()
}
fn preview(f: &Fixture) -> AdditionPreview {
    json_codec::from_value(command(f, "disk_add_preview", json!(request(f)), &f.identity).unwrap())
        .unwrap()
}
fn cold_pending(f: &Fixture) -> StoredAddition {
    let (preview, target, held) = f
        .backend
        .prepare_addition(None, &f.identity, request(f), f.discovery.to_str().unwrap())
        .unwrap();
    let record = StoredAddition {
        source: AdditionSource::TerminalManagement {
            original: f.original.clone(),
        },
        format: Format::Addition,
        principal: f.identity.principal.clone(),
        target: target.clone(),
        archive: None,
        status: AdditionStatus {
            operation_id: preview.request.operation_id,
            preview,
            state: AdditionState::Adding,
            phase: AdditionPhase::Prepared,
            continuation: AdditionContinuation::Add,
            result: None,
            failure: None,
        },
    };
    let directory = directory(f);
    save(&directory, &record).unwrap();
    write(&directory, &latest_name(), &record.status.operation_id).unwrap();
    let owner = std::mem::replace(
        &mut crate::lock(&f.backend.inner.state).backend,
        BackendState::Unopened,
    );
    let BackendState::Terminal(owner) = owner else {
        panic!("expected terminal owner")
    };
    let terminal = Arc::try_unwrap(owner)
        .ok()
        .expect("exclusive terminal owner");
    let mut pending = terminal
        .prepare_metadata_resume(
            target,
            &f.identity.principal,
            record.status.operation_id,
            held,
        )
        .unwrap();
    // Real new-Q proof is not an ordinary successful operation. Lose the HTTP
    // archive binding while retaining the original immutable core proposal.
    assert!(pending.synchronize().unwrap().target_quorum_accepted());
    drop(pending);
    assert!(f.backend.inner.recover_terminal().unwrap());
    assert!(matches!(
        crate::lock(&f.backend.inner.state).backend,
        BackendState::ResumeRecovery
    ));
    record
}

#[test]
fn fresh_resume_reuses_add_api_preserves_pool_history_and_exact_receipt() {
    let f = fixture();
    let preview = preview(&f);
    assert!(preview.authority.is_terminal());
    assert_eq!(preview.original_metadata_members, 1);
    assert_eq!(preview.target_metadata_members, 1);
    let body = json!(AdditionCommit {
        preview: preview.clone()
    });
    let completed: AdditionStatus =
        json_codec::from_value(command(&f, "disk_add_commit", body.clone(), &f.identity).unwrap())
            .unwrap();
    assert_eq!(completed.state, AdditionState::Completed);
    assert_eq!(
        completed.result.as_ref().unwrap().kind,
        "terminal-metadata-resume"
    );
    let backend = f.backend.inner.backend().unwrap();
    assert_eq!(
        backend.addition_configuration(&f.identity).unwrap().pool_id,
        f.original.pool_id
    );
    assert!(
        backend
            .operation(&f.identity, f.historical_operation)
            .is_ok()
    );
    drop(backend);
    let repeated: AdditionStatus =
        json_codec::from_value(command(&f, "disk_add_commit", body, &f.identity).unwrap()).unwrap();
    assert_eq!(repeated.result, completed.result);
    assert!(f.original.disks[0].root.is_dir());
    let disks = f.backend.disk_status(&f.identity).unwrap();
    assert!(
        disks
            .iter()
            .any(|disk| disk.disk_id == f.original.disks[1].disk_id)
    );
    assert!(
        !disks
            .iter()
            .any(|disk| disk.disk_id == f.original.disks[0].disk_id)
    );
}

#[test]
fn cold_resume_uses_core_selector_not_http_phase_and_never_opens_stopped_root() {
    let f = fixture();
    let saved = cold_pending(&f);
    let stopped = &f.original.disks[0].root;
    fs::rename(stopped, stopped.with_extension("retained")).unwrap();
    fs::write(stopped, b"stopped path is not a directory").unwrap();
    let status: AdditionStatusResponse = json_codec::from_value(
        command(
            &f,
            "disk_add_status",
            json!({"operation_id":saved.status.operation_id}),
            &f.identity,
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        status.operation.unwrap().state,
        AdditionState::RecoveryRequired
    );
    assert!(f.backend.inner.backend().is_err());
    let completed: AdditionStatus = json_codec::from_value(
        command(
            &f,
            "disk_add_resume",
            json!(AdditionResume {
                operation_id: saved.status.operation_id,
                continuation: AdditionContinuation::Add
            }),
            &f.identity,
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(completed.state, AdditionState::Completed);
    assert_eq!(
        fs::read(stopped).unwrap(),
        b"stopped path is not a directory"
    );
    let record = read_record(&directory(&f), saved.status.operation_id)
        .unwrap()
        .unwrap();
    assert!(record.archive.is_some());
    assert_eq!(record.status.result, completed.result);
}

#[test]
fn pending_resume_cannot_be_cancelled_or_reauthorized_by_revoked_session() {
    let f = fixture();
    let saved = cold_pending(&f);
    let cancel = command(
        &f,
        "disk_add_cancel",
        json!(AdditionCancel {
            operation_id: saved.status.operation_id,
            preview_digest: saved.status.preview.preview_digest.clone(),
        }),
        &f.identity,
    )
    .unwrap_err();
    assert_eq!(cancel.code, ErrorCode::UnsupportedOperation);
    assert_eq!(
        read_record(&directory(&f), saved.status.operation_id)
            .unwrap()
            .unwrap()
            .status
            .continuation,
        AdditionContinuation::Add
    );
    crate::lock(&f.auth).logout(&f.token).unwrap();
    let body = json!(AdditionResume {
        operation_id: saved.status.operation_id,
        continuation: AdditionContinuation::Add
    });
    assert_eq!(
        command(&f, "disk_add_resume", body.clone(), &f.identity)
            .unwrap_err()
            .code,
        ErrorCode::Unauthorized
    );
    let login = crate::lock(&f.auth)
        .login("admin", "resume-password")
        .unwrap();
    let identity = crate::lock(&f.auth).authenticate(&login.token).unwrap();
    let completed: AdditionStatus =
        json_codec::from_value(command(&f, "disk_add_resume", body, &identity).unwrap()).unwrap();
    assert_eq!(completed.state, AdditionState::Completed);
    assert!(
        command(
            &f,
            "disk_add_status",
            json!({"operation_id":saved.status.operation_id}),
            &f.identity
        )
        .is_err()
    );
}

#[test]
fn completed_http_cache_alone_does_not_admit_a_pool_after_restart() {
    let f = fixture();
    let preview = preview(&f);
    let completed: AdditionStatus = json_codec::from_value(
        command(
            &f,
            "disk_add_commit",
            json!(AdditionCommit { preview }),
            &f.identity,
        )
        .unwrap(),
    )
    .unwrap();
    let directory = directory(&f);
    let mut record = read_record(&directory, completed.operation_id)
        .unwrap()
        .unwrap();
    record.status.result = None;
    record.status.state = AdditionState::RecoveryRequired;
    record.status.phase = AdditionPhase::Activating;
    save(&directory, &record).unwrap();
    {
        let mut state = crate::lock(&f.backend.inner.state);
        state.backend = BackendState::Unopened;
        state.admitted = false;
    }
    assert!(f.backend.inner.backend().is_err());
    f.backend.inner.recover_once().unwrap();
    let status: AdditionStatusResponse = json_codec::from_value(
        command(
            &f,
            "disk_add_status",
            json!({"operation_id":completed.operation_id}),
            &f.identity,
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(status.operation.unwrap().result, completed.result);
    assert!(f.backend.inner.backend().is_ok());
    assert!(
        directory
            .read_bounded(&name(completed.operation_id), 4 * 1024 * 1024)
            .is_ok()
    );
}

#[test]
fn terminal_preview_rejects_data_only_resume_and_never_discards_existing_history() {
    let f = fixture();
    let mut request = request(&f);
    request.additions[0].metadata_member = false;
    assert_eq!(
        command(&f, "disk_add_preview", json!(request), &f.identity)
            .unwrap_err()
            .code,
        ErrorCode::InvalidRequest
    );
    let terminal = f.backend.inner.terminal(&f.identity).unwrap().unwrap();
    assert!(terminal.operation(f.historical_operation).is_some());
    assert_eq!(terminal.registered_disks().len(), 1);
}

#[test]
fn managed_discovery_skips_stopped_paths_and_allows_fresh_same_filesystem_directory() {
    let f = fixture();
    let settings: storage_contract::storage_directories::DiscoveryRootSettings =
        json_codec::from_value(
            f.backend
                .setup_operation("discovery_root", json!({}), None, Some(&f.identity))
                .unwrap(),
        )
        .unwrap();
    f.backend
        .setup_operation(
            "set_discovery_root",
            json!(
                storage_contract::storage_directories::SetDiscoveryRootRequest {
                    expected_discovery_root: settings.discovery_root,
                    discovery_root: f.discovery.to_str().unwrap().into(),
                }
            ),
            None,
            Some(&f.identity),
        )
        .unwrap();
    let listing: storage_contract::storage_directories::DirectoryListing = json_codec::from_value(
        f.backend
            .setup_operation(
                "storage_directories",
                json!({"path":f.discovery.to_str().unwrap()}),
                None,
                Some(&f.identity),
            )
            .unwrap(),
    )
    .unwrap();
    assert!(listing.entries.is_empty());
    let selected = request(&f).additions.remove(0).selection;
    let admitted: storage_contract::storage_directories::CandidateSelection =
        json_codec::from_value(
            f.backend
                .setup_operation(
                    "inspect_storage_directory",
                    json!(InspectRequest {
                        mount_path: selected.mount_path,
                        root_path: selected.root_path
                    }),
                    None,
                    Some(&f.identity),
                )
                .unwrap(),
        )
        .unwrap();
    assert_eq!(admitted.root_identity.device, selected.root_identity.device);
    let stopped = &f.original.disks[0].root;
    assert_eq!(
        f.backend
            .setup_operation(
                "storage_directories",
                json!({"path":stopped.to_str().unwrap()}),
                None,
                Some(&f.identity)
            )
            .unwrap_err()
            .code,
        ErrorCode::ConfigConflict
    );
}

#[test]
fn historical_operation_id_cannot_reserve_a_new_resume_intent() {
    let f = fixture();
    let mut request = request(&f);
    request.operation_id = f.historical_operation;
    assert_eq!(
        command(&f, "disk_add_preview", json!(request), &f.identity)
            .unwrap_err()
            .code,
        ErrorCode::IdempotencyKeyReused
    );
    assert!(matches!(
        crate::lock(&f.backend.inner.state).backend,
        BackendState::Terminal(_)
    ));
}
