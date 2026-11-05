//! Real terminal owner and fresh-generation service transitions, source only.
use super::*;
use platform_io::fs_safety::EntryName;
use storage_addition::{DIRECTORY, Format, name, read_record};
use storage_contract::storage_directories::InspectRequest;
use storage_contract::storage_lifecycle::*;

#[path = "terminal_resume_mixed.rs"]
mod mixed;

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
    let (backend, mut auth) = open_service_with_options(
        &data,
        &management,
        ManagementOptions {
            discovery_root: discovery.to_str().unwrap().parse().unwrap(),
            ..Default::default()
        },
    )
    .unwrap();
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
    let listing = crate::storage_directories::browse(root.to_str().unwrap()).unwrap();
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
    f.backend.storage_addition(action, input, identity)
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
    cold_pending_request(f, request(f))
}
#[derive(Clone, Copy)]
enum PendingPreparation {
    Reserved,
    QuorumAccepted,
    GenerationSelected,
}
fn cold_pending_request(f: &Fixture, request: AdditionRequest) -> StoredAddition {
    cold_pending_at(f, request, PendingPreparation::QuorumAccepted)
}
fn cold_pending_at(
    f: &Fixture,
    request: AdditionRequest,
    preparation: PendingPreparation,
) -> StoredAddition {
    let (preview, target, held) = f
        .backend
        .prepare_addition(None, &f.identity, request, f.discovery.to_str().unwrap())
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
    if !matches!(preparation, PendingPreparation::Reserved) {
        let status = pending.synchronize().unwrap();
        assert!(status.target_quorum_accepted());
        if matches!(preparation, PendingPreparation::GenerationSelected) {
            let voter = record
                .target
                .disks
                .iter()
                .find(|disk| disk.metadata_member)
                .unwrap();
            let certificate = fs::read(
                voter
                    .root
                    .join("metadata/membership-pending")
                    .join(&status.transition_hash)
                    .join("resume-certificate.json"),
            )
            .unwrap();
            // This fixture starts from genesis, so its actual ResumeCore is the
            // portable authority without a predecessor-chain envelope. Copy the
            // real certificate at the selector crash boundary; invent no votes.
            let AdditionAuthority::TerminalManagement {
                terminal_operation_id,
                ..
            } = &record.status.preview.authority
            else {
                unreachable!()
            };
            fs::write(
                f.data
                    .join("terminal-management")
                    .join(terminal_operation_id.to_string())
                    .join("resume-generation.json"),
                certificate,
            )
            .unwrap();
            assert_eq!(
                TerminalManagement::metadata_resume_recovery(&f.data)
                    .unwrap()
                    .unwrap()
                    .phase,
                storage_engine::TerminalMetadataResumePhase::CompleteGeneration
            );
        }
    }
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
    let observed = status.operation.unwrap();
    assert_eq!(observed.state, AdditionState::RecoveryRequired);
    assert_eq!(observed.phase, AdditionPhase::ResumeCancellationUnknown);
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
fn decided_resume_cannot_be_cancelled_or_reauthorized_by_revoked_session() {
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
    assert_eq!(cancel.code, ErrorCode::RecoveryRequired);
    assert_eq!(
        f.backend
            .inner
            .terminal_resume_cancellation_phase(&saved)
            .unwrap(),
        AdditionPhase::ResumeQuorumAccepted
    );
    assert_eq!(
        read_record(&directory(&f), saved.status.operation_id)
            .unwrap()
            .unwrap()
            .status
            .phase,
        AdditionPhase::ResumeQuorumAccepted
    );
    assert_eq!(
        read_record(&directory(&f), saved.status.operation_id)
            .unwrap()
            .unwrap()
            .status
            .continuation,
        AdditionContinuation::Cancel
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
        ErrorCode::UnsafePath
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

#[test]
fn undecided_terminal_resume_cancellation_survives_http_loss_and_releases_management() {
    let f = fixture();
    let saved = cold_pending_at(&f, request(&f), PendingPreparation::Reserved);
    let body = json!(AdditionCancel {
        operation_id: saved.status.operation_id,
        preview_digest: saved.status.preview.preview_digest.clone(),
    });
    let cancelled: AdditionStatus =
        json_codec::from_value(command(&f, "disk_add_cancel", body.clone(), &f.identity).unwrap())
            .unwrap();
    assert_eq!(cancelled.state, AdditionState::Cancelled);
    assert_eq!(
        cancelled.result.as_ref().unwrap().kind,
        "terminal-metadata-resume-cancelled"
    );
    assert!(matches!(
        crate::lock(&f.backend.inner.state).backend,
        BackendState::Terminal(_)
    ));
    assert!(f.backend.inner.backend().is_err());
    let root = &saved
        .target
        .disks
        .iter()
        .find(|disk| disk.slot_id == "N01")
        .unwrap()
        .root;
    fs::rename(root, root.with_extension("cancelled-retained")).unwrap();
    fs::write(
        root,
        b"a cancelled directory is not probed by receipt queries",
    )
    .unwrap();
    let directory = directory(&f);
    let mut uncertain = read_record(&directory, saved.status.operation_id)
        .unwrap()
        .unwrap();
    uncertain.status.result = None;
    uncertain.status.state = AdditionState::RecoveryRequired;
    uncertain.status.phase = AdditionPhase::CertifyingAbort;
    save(&directory, &uncertain).unwrap();
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
    assert_eq!(status.operation.unwrap().result, cancelled.result);
    let replay: AdditionStatus =
        json_codec::from_value(command(&f, "disk_add_cancel", body, &f.identity).unwrap()).unwrap();
    assert_eq!(replay.result, cancelled.result);
    assert_eq!(
        fs::read(root).unwrap(),
        b"a cancelled directory is not probed by receipt queries"
    );
    assert!(
        f.backend
            .inner
            .terminal(&f.identity)
            .unwrap()
            .unwrap()
            .operation(f.historical_operation)
            .is_some()
    );
}

#[test]
fn terminal_cancellation_is_permanent_across_a_later_fresh_resume() {
    let f = fixture();
    let saved = cold_pending_at(&f, request(&f), PendingPreparation::Reserved);
    let cancelled: AdditionStatus = json_codec::from_value(
        command(
            &f,
            "disk_add_cancel",
            json!(AdditionCancel {
                operation_id: saved.status.operation_id,
                preview_digest: saved.status.preview.preview_digest.clone(),
            }),
            &f.identity,
        )
        .unwrap(),
    )
    .unwrap();
    let old_root = f.discovery.join("fresh-voter");
    let retained = f.discovery.join("cancelled-reservation");
    fs::rename(&old_root, &retained).unwrap();
    let next = preview(&f);
    assert_ne!(next.request.operation_id, saved.status.operation_id);
    let completed: AdditionStatus = json_codec::from_value(
        command(
            &f,
            "disk_add_commit",
            json!(AdditionCommit { preview: next }),
            &f.identity,
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(completed.state, AdditionState::Completed);
    assert!(
        retained.is_dir(),
        "cancellation must never erase prepared storage"
    );
    let replay: AdditionStatusResponse = json_codec::from_value(
        command(
            &f,
            "disk_add_status",
            json!({"operation_id":saved.status.operation_id}),
            &f.identity,
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(replay.operation.unwrap().result, cancelled.result);
}

#[test]
fn readable_cancel_certificate_does_not_publish_http_until_owner_handover() {
    let f = fixture();
    let mut saved = cold_pending_at(&f, request(&f), PendingPreparation::Reserved);
    let held = f.backend.inner.open_resume_directories(&saved).unwrap();
    let mut pending = TerminalManagement::reopen_metadata_resume(&f.data, held).unwrap();
    let status = pending.status().unwrap();
    let archive = pending.control_archive();
    saved.archive = Some(ArchiveBinding {
        archive_path: archive.archive_path,
        root_hash: archive.root_hash,
        transition_hash: status.transition_hash.clone(),
    });
    saved.status.continuation = AdditionContinuation::Cancel;
    saved.status.state = AdditionState::RecoveryRequired;
    saved.status.phase = AdditionPhase::CertifyingAbort;
    save(&directory(&f), &saved).unwrap();
    assert!(matches!(
        pending
            .cancel(
                &f.identity.principal,
                saved.status.operation_id,
                &status.transition_hash
            )
            .unwrap(),
        storage_engine::TerminalMetadataResumeCancelOutcome::Cancelled(_)
    ));
    // Even an authenticated, readable certificate is not a service handover.
    // The owner must finish the durability barriers before HTTP can finish.
    f.backend.inner.restore_resume(pending);
    let observed: AdditionStatusResponse = json_codec::from_value(
        command(
            &f,
            "disk_add_status",
            json!({"operation_id":saved.status.operation_id}),
            &f.identity,
        )
        .unwrap(),
    )
    .unwrap();
    let observed = observed.operation.unwrap();
    assert_eq!(observed.state, AdditionState::RecoveryRequired);
    assert_eq!(observed.phase, AdditionPhase::Cancelling);
    assert!(observed.result.is_none());
    assert!(matches!(
        crate::lock(&f.backend.inner.state).backend,
        BackendState::MetadataResume(_)
    ));
    let completed: AdditionStatus = json_codec::from_value(
        command(
            &f,
            "disk_add_cancel",
            json!(AdditionCancel {
                operation_id: saved.status.operation_id,
                preview_digest: saved.status.preview.preview_digest.clone(),
            }),
            &f.identity,
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(completed.state, AdditionState::Cancelled);
    assert!(matches!(
        crate::lock(&f.backend.inner.state).backend,
        BackendState::Terminal(_)
    ));
}

#[test]
fn fresh_live_owner_checks_complete_target_before_take() {
    let f = fixture();
    let saved = cold_pending_at(&f, request(&f), PendingPreparation::Reserved);
    let held = f.backend.inner.open_resume_directories(&saved).unwrap();
    let pending = TerminalManagement::reopen_metadata_resume(&f.data, held).unwrap();
    let mut changed = saved.clone();
    changed
        .target
        .disks
        .iter_mut()
        .find(|disk| disk.slot_id == "N01")
        .unwrap()
        .level = 2;
    f.backend.inner.restore_resume(pending);
    assert_eq!(
        f.backend
            .inner
            .terminal_resume_cancellation_phase(&saved)
            .unwrap(),
        AdditionPhase::ResumeCancellable
    );
    let failure =
        f.backend
            .inner
            .acquire_terminal_resume(&f.identity, &mut changed, None, &directory(&f));
    match failure {
        Err(error) => assert_eq!(error.code, ErrorCode::IdempotencyKeyReused),
        Ok(_) => panic!("different target must not take the held fresh resume owner"),
    }
    assert!(matches!(
        crate::lock(&f.backend.inner.state).backend,
        BackendState::MetadataResume(_)
    ));
}

#[test]
fn cold_selected_generation_cancel_never_implicitly_activates_original_resume() {
    let f = fixture();
    let saved = cold_pending_at(&f, request(&f), PendingPreparation::GenerationSelected);
    let AdditionAuthority::TerminalManagement {
        terminal_operation_id,
        ..
    } = &saved.status.preview.authority
    else {
        unreachable!()
    };
    let attempt = f
        .data
        .join("terminal-management")
        .join(terminal_operation_id.to_string());
    let selected = fs::read(attempt.join("resume-generation.json")).unwrap();
    let error = command(
        &f,
        "disk_add_cancel",
        json!(AdditionCancel {
            operation_id: saved.status.operation_id,
            preview_digest: saved.status.preview.preview_digest.clone(),
        }),
        &f.identity,
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::RecoveryRequired);
    assert!(f.backend.inner.backend().is_err());
    assert!(matches!(
        crate::lock(&f.backend.inner.state).backend,
        BackendState::ResumeRecovery
    ));
    assert_eq!(
        fs::read(attempt.join("resume-generation.json")).unwrap(),
        selected
    );
    for name in [
        "resume-complete.json",
        "resume-bootstrap.json",
        "resume-cancel-intent.json",
        "resume-cancel.json",
    ] {
        assert!(
            !attempt.join(name).exists(),
            "cancel must not create {name}"
        );
    }
    assert_eq!(
        TerminalManagement::metadata_resume_recovery(&f.data)
            .unwrap()
            .unwrap()
            .phase,
        storage_engine::TerminalMetadataResumePhase::CompleteGeneration
    );
    let completed: AdditionStatus = json_codec::from_value(
        command(
            &f,
            "disk_add_resume",
            json!(AdditionResume {
                operation_id: saved.status.operation_id,
                continuation: AdditionContinuation::Add,
            }),
            &f.identity,
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(completed.state, AdditionState::Completed);
    assert_eq!(completed.result.unwrap().kind, "terminal-metadata-resume");
}
