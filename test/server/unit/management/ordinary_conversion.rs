use super::*;
use std::io::Read;
use storage_contract::storage_directories::{
    CandidateSelection, DirectorySource, DiscoveredDiskState,
};

struct Fixture {
    backend: Arc<ManagementBackend>,
    identity: Identity,
    token: String,
    discovery: PathBuf,
    data: PathBuf,
    _root: tempfile::TempDir,
}
fn fixture() -> Fixture {
    let root = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let data = root.path().join("system");
    let discovery = root.path().join("disks");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&discovery)
        .unwrap();
    let engine = storage_engine::Engine::initialize(
        &data,
        storage_contract::Config::development([discovery.join("known")]).unwrap(),
    )
    .unwrap();
    let management = data.join("management");
    drop(
        AuthStore::initialize_management(&management, "admin", "conversion-test-password").unwrap(),
    );
    let (backend, mut auth) = open_service_with_options(
        &data,
        &management,
        ManagementOptions {
            discovery_root: discovery.to_str().unwrap().parse().unwrap(),
            ..Default::default()
        },
    )
    .unwrap();
    let token = auth
        .login("admin", "conversion-test-password")
        .unwrap()
        .token;
    let identity = auth.authenticate(&token).unwrap();
    let engine = Arc::new(EngineBackend::new(engine));
    engine.set_management_auth(auth.reader()).unwrap();
    *crate::lock(&backend.inner.auth) = Some(Arc::new(Mutex::new(auth)));
    backend.inner.set(Some(engine), StorageState::Ready);
    for name in ["ordinary", "new-primary"] {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(discovery.join(name))
            .unwrap();
    }
    fs::create_dir(discovery.join("ordinary/empty")).unwrap();
    fs::write(
        discovery.join("ordinary/document.txt"),
        b"keep original content",
    )
    .unwrap();
    Fixture {
        backend,
        identity,
        token,
        discovery,
        data,
        _root: root,
    }
}
fn selection(f: &Fixture) -> (DirectorySource, CandidateSelection) {
    let disks = f.backend.discovered_disks(&f.identity).unwrap();
    let source = disks
        .candidates
        .iter()
        .find(|entry| entry.name == "ordinary")
        .unwrap();
    let target = disks
        .candidates
        .iter()
        .find(|entry| entry.name == "new-primary")
        .unwrap();
    let DiscoveredDiskState::Unsupported {
        source: Some(source),
        ..
    } = &source.state
    else {
        panic!("ordinary source must not be treated as admitted storage")
    };
    let DiscoveredDiskState::Empty { selection } = &target.state else {
        panic!("target must be empty")
    };
    (source.clone(), selection.clone())
}
fn preview(f: &Fixture) -> OrdinaryDiskConversionPreview {
    let (source, target) = selection(f);
    f.backend
        .preview_ordinary_conversion(
            &f.identity,
            OrdinaryDiskConversionRequest {
                operation_id: OperationId::generate().unwrap(),
                source,
                target,
                slot_id: "D02".into(),
                metadata_member: false,
            },
        )
        .unwrap()
}
fn commit(
    f: &Fixture,
    preview: &OrdinaryDiskConversionPreview,
    confirm: bool,
) -> Result<Value, ApiError> {
    f.backend.ordinary_conversion(
        "ordinary_conversion_commit",
        json_codec::to_value(OrdinaryDiskConversionCommit {
            preview: preview.clone(),
            acknowledge_contents_only: confirm,
        })
        .unwrap(),
        &f.identity,
    )
}
fn target_empty(f: &Fixture) {
    assert_eq!(
        fs::read_dir(f.discovery.join("new-primary"))
            .unwrap()
            .count(),
        0
    );
}

fn reopen_engine(f: &Fixture) {
    crate::lock(&f.backend.inner.state).backend = BackendState::Unopened;
    let engine = Arc::new(EngineBackend::new(
        storage_engine::Engine::open(&f.data).unwrap(),
    ));
    let auth = crate::lock(&f.backend.inner.auth).clone().unwrap();
    engine
        .set_management_auth(crate::lock(&auth).reader())
        .unwrap();
    f.backend.inner.set(Some(engine), StorageState::Ready);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ImportInterruption {
    RegistrationPublished,
    RootIntent,
    RootCreated,
    WorkspaceIntent,
    WorkspaceCreated,
    CommitIntent,
}

fn pending_record(f: &Fixture, preview: &OrdinaryDiskConversionPreview) -> Record {
    Record {
        format: Format::Ordinary,
        principal: f.identity.principal.clone(),
        status: OrdinaryDiskConversionStatus {
            preview: preview.clone(),
            phase: OrdinaryDiskConversionPhase::Registering,
            registration: None,
            workspace_id: None,
            root_operation_id: None,
            result: None,
            stop_operation_id: None,
            stop_result: None,
            failure: None,
        },
        commit_request: None,
        import_attempted: false,
    }
}

fn interrupted_conversion(
    boundary: ImportInterruption,
) -> (Fixture, OrdinaryDiskConversionPreview) {
    let f = fixture();
    let p = preview(&f);
    let source = f
        .backend
        .inspect_conversion_source(
            &f.identity,
            &p.source,
            Some(&p.target.request.additions[0].selection.root_path),
        )
        .unwrap();
    let registration = f
        .backend
        .commit_storage_addition(
            &f.identity,
            AdditionCommit {
                preview: p.target.clone(),
            },
        )
        .unwrap();
    let directory = PrivateDirectory::open_existing(f.data.join("management"))
        .unwrap()
        .create_child(&EntryName::new(DIRECTORY).unwrap())
        .unwrap();
    let mut record = pending_record(&f, &p);
    record.status.phase = OrdinaryDiskConversionPhase::Importing;
    record.status.registration = Some(registration);
    record.import_attempted = true;
    save(&directory, &record).unwrap();
    write(
        &directory,
        &storage_addition::latest_name(),
        &p.operation_id,
    )
    .unwrap();
    if boundary == ImportInterruption::RegistrationPublished {
        record.status.registration = None;
        record.status.phase = OrdinaryDiskConversionPhase::Registering;
        record.import_attempted = false;
        save(&directory, &record).unwrap();
        drop(source);
        // Registration already published but its conversion projection did not.
        // Stopping must not require the original source to remain unchanged.
        fs::write(f.discovery.join("ordinary/newer.txt"), b"external edit").unwrap();
        reopen_engine(&f);
        return (f, p);
    }
    let PreparedOrdinaryConversion { backend, source } = source;
    let mut workspace_callbacks = 0;
    let result = backend.run_ordinary_conversion(
        &f.identity,
        &p.target,
        source,
        p.import_operation_id,
        |progress| {
            let interrupted = || {
                storage_engine::Error::from(std::io::Error::other(
                    "test interruption at durable boundary",
                ))
            };
            if progress.workspace_id().is_some() {
                workspace_callbacks += 1;
            }
            if boundary == ImportInterruption::RootCreated && workspace_callbacks == 1 {
                // The root really published; the next intent never synchronized.
                return Err(interrupted());
            }
            record.status.root_operation_id = progress.root_operation_id();
            record.status.workspace_id = progress.workspace_id().map(str::to_owned);
            record.commit_request = progress.request().cloned();
            save(&directory, &record).unwrap();
            let stop = match boundary {
                ImportInterruption::RootIntent => progress.root_operation_id().is_some(),
                ImportInterruption::WorkspaceIntent => workspace_callbacks == 1,
                ImportInterruption::WorkspaceCreated => workspace_callbacks == 2,
                ImportInterruption::CommitIntent => progress.request().is_some(),
                ImportInterruption::RootCreated | ImportInterruption::RegistrationPublished => {
                    false
                }
            };
            if stop {
                return Err(interrupted());
            }
            Ok(())
        },
    );
    assert!(result.is_err(), "{boundary:?}");
    drop(backend);
    reopen_engine(&f);
    (f, p)
}

fn stop(f: &Fixture, p: &OrdinaryDiskConversionPreview, confirm: bool) -> Result<Value, ApiError> {
    f.backend.ordinary_conversion(
        "ordinary_conversion_stop",
        json_codec::json!(OrdinaryDiskConversionStop {
            operation_id: p.operation_id,
            acknowledge_retained_state: confirm,
        }),
        &f.identity,
    )
}

fn assert_busy_status_verifies_terminal(f: &Fixture, p: &OrdinaryDiskConversionPreview) {
    let directory =
        PrivateDirectory::open_existing(f.data.join("management").join(DIRECTORY)).unwrap();
    let mut record = owned_record(&directory, p.operation_id, &f.identity).unwrap();
    // A successful read/replay may have reconciled a lost projection only in
    // memory. Install that verified projection before testing cache corruption.
    record.status = f
        .backend
        .ordinary_conversion_status(&f.identity, Some(p.operation_id))
        .unwrap()
        .operation
        .unwrap();
    let original = record.status.clone();
    let cached = match record.status.phase {
        OrdinaryDiskConversionPhase::Completed => record.status.result.as_mut().unwrap(),
        OrdinaryDiskConversionPhase::Stopped => record.status.stop_result.as_mut().unwrap(),
        phase => panic!("test requires a terminal status, got {phase:?}"),
    };
    cached.result["tampered_projection"] = json!(true);
    save(&directory, &record).unwrap();
    let gate = crate::lock(&f.backend.inner.ordinary_conversion);
    assert_eq!(
        f.backend
            .ordinary_conversion_status(&f.identity, Some(p.operation_id))
            .unwrap_err()
            .code,
        ErrorCode::ObjectCorrupt
    );
    record.status = original.clone();
    save(&directory, &record).unwrap();
    assert_eq!(
        f.backend
            .ordinary_conversion_status(&f.identity, Some(p.operation_id))
            .unwrap()
            .operation
            .unwrap(),
        original
    );
    if record.status.phase == OrdinaryDiskConversionPhase::Completed {
        let request = record.commit_request.clone();
        record
            .commit_request
            .as_mut()
            .unwrap()
            .message
            .push_str(" changed");
        save(&directory, &record).unwrap();
        assert_eq!(
            f.backend
                .ordinary_conversion_status(&f.identity, Some(p.operation_id))
                .unwrap_err()
                .code,
            ErrorCode::ObjectCorrupt
        );
        record.commit_request = request;
        save(&directory, &record).unwrap();
    }
    drop(gate);
}

#[test]
fn cold_import_boundaries_have_a_durable_stop_that_retains_source_and_staging() {
    for boundary in [
        ImportInterruption::RegistrationPublished,
        ImportInterruption::RootIntent,
        ImportInterruption::RootCreated,
        ImportInterruption::WorkspaceIntent,
        ImportInterruption::WorkspaceCreated,
        ImportInterruption::CommitIntent,
    ] {
        let (f, p) = interrupted_conversion(boundary);
        assert!(stop(&f, &p, false).is_err());
        let stopped: OrdinaryDiskConversionStatus =
            json_codec::from_value(stop(&f, &p, true).unwrap()).unwrap();
        assert_eq!(
            stopped.phase,
            OrdinaryDiskConversionPhase::Stopped,
            "{boundary:?}"
        );
        assert!(stopped.result.is_none());
        assert_eq!(
            stopped.stop_result.as_ref().unwrap().kind,
            "directory.import.stop"
        );
        assert_eq!(
            Some(stopped.stop_result.as_ref().unwrap().operation_id),
            stopped.stop_operation_id
        );
        assert_eq!(
            fs::read(f.discovery.join("ordinary/document.txt")).unwrap(),
            b"keep original content"
        );
        let sequence = f
            .backend
            .inner
            .backend()
            .unwrap()
            .test_with_engine_locked(|engine| {
                assert!(
                    engine
                        .config()
                        .disks
                        .iter()
                        .any(|disk| disk.slot_id == "D02")
                );
                if let Some(workspace) = &stopped.workspace_id {
                    match engine.workspace(workspace) {
                        Ok(workspace) => {
                            assert_eq!(workspace.state, storage_contract::WorkspaceState::Aborted);
                            if boundary == ImportInterruption::CommitIntent {
                                assert_eq!(workspace.files.len(), 1);
                            }
                        }
                        Err(error) => {
                            assert_eq!(boundary, ImportInterruption::WorkspaceIntent);
                            assert_eq!(error.code(), ErrorCode::WorkspaceNotFound);
                        }
                    }
                }
                engine.state().sequence
            });
        let directory =
            PrivateDirectory::open_existing(f.data.join("management").join(DIRECTORY)).unwrap();
        let mut record = owned_record(&directory, p.operation_id, &f.identity).unwrap();
        // Native stop published, but its final management projection was lost.
        record.status.phase = OrdinaryDiskConversionPhase::Stopping;
        record.status.stop_result = None;
        save(&directory, &record).unwrap();
        reopen_engine(&f);
        let observed = f
            .backend
            .ordinary_conversion_status(&f.identity, Some(p.operation_id))
            .unwrap()
            .operation
            .unwrap();
        assert_eq!(observed.phase, OrdinaryDiskConversionPhase::Stopped);
        assert_eq!(observed.stop_result, stopped.stop_result);
        let replay: OrdinaryDiskConversionStatus =
            json_codec::from_value(stop(&f, &p, true).unwrap()).unwrap();
        assert_eq!(replay.stop_result, stopped.stop_result);
        assert_busy_status_verifies_terminal(&f, &p);
        f.backend
            .ensure_prior_conversion_settled(&directory, &f.identity)
            .unwrap();
        assert_eq!(
            f.backend
                .inner
                .backend()
                .unwrap()
                .test_with_engine_locked(|engine| engine.state().sequence),
            sequence
        );
        if boundary == ImportInterruption::CommitIntent {
            // This fixture drives lifecycle operations without the production
            // periodic observer. Refresh through its real sampling boundary
            // before starting a new operation after the cold-recovery checks.
            f.backend.inner.observe_storage_once();
            fs::DirBuilder::new()
                .mode(0o700)
                .create(f.discovery.join("next-primary"))
                .unwrap();
            let observed = f.backend.discovered_disks(&f.identity).unwrap();
            let source = observed
                .candidates
                .iter()
                .find_map(|candidate| match &candidate.state {
                    DiscoveredDiskState::Unsupported {
                        source: Some(source),
                        ..
                    } if candidate.name == "ordinary" => Some(source.clone()),
                    _ => None,
                })
                .unwrap();
            let target = observed
                .candidates
                .iter()
                .find_map(|candidate| match &candidate.state {
                    DiscoveredDiskState::Empty { selection }
                        if candidate.name == "next-primary" =>
                    {
                        Some(selection.clone())
                    }
                    _ => None,
                })
                .unwrap();
            let next = f
                .backend
                .preview_ordinary_conversion(
                    &f.identity,
                    OrdinaryDiskConversionRequest {
                        operation_id: OperationId::generate().unwrap(),
                        source,
                        target,
                        slot_id: "D03".into(),
                        metadata_member: false,
                    },
                )
                .unwrap();
            let next: OrdinaryDiskConversionStatus = json_codec::from_value(
                commit(&f, &next, true).unwrap_or_else(|error| {
                    let retained = read_record(&directory, next.operation_id)
                        .map(|record| record.status.phase);
                    let state = crate::lock(&f.backend.inner.state);
                    panic!(
                        "{error:?}; storage={:?}, admitted={}, observation_age={:?}, retained_phase={retained:?}",
                        state.status,
                        state.admitted,
                        state.observed_at.elapsed(),
                    );
                }),
            )
            .unwrap();
            assert_eq!(next.phase, OrdinaryDiskConversionPhase::Completed);
            assert_eq!(
                f.backend
                    .ordinary_conversion_status(&f.identity, Some(p.operation_id))
                    .unwrap()
                    .operation
                    .unwrap()
                    .phase,
                OrdinaryDiskConversionPhase::Stopped
            );
        }
    }
}

#[derive(Clone, Copy)]
enum QueuedConversionChange {
    HealthExpires,
    SessionRevoked,
    ServiceStopping,
}

fn queued_conversion(change: QueuedConversionChange) {
    let f = fixture();
    let preview = preview(&f);
    let engine = Arc::downgrade(&f.backend.inner.backend().unwrap());
    let original_owners = engine.strong_count();
    let source = f
        .backend
        .inspect_conversion_source(
            &f.identity,
            &preview.source,
            Some(&preview.target.request.additions[0].selection.root_path),
        )
        .unwrap();
    assert_eq!(engine.strong_count(), original_owners + 1);
    let directory = PrivateDirectory::open_existing(f.data.join("management"))
        .unwrap()
        .create_child(&EntryName::new(DIRECTORY).unwrap())
        .unwrap();
    let mut record = pending_record(&f, &preview);
    // Model an arbitrarily long completed source scan without sleeping. New
    // dispatch remains blocked; only this typed preparation keeps its original
    // owner through registration and the separately authorized import.
    crate::lock(&f.backend.inner.state).observed_at =
        std::time::Instant::now() - std::time::Duration::from_secs(16);
    assert_eq!(
        f.backend.inner.backend().err().unwrap().code,
        ErrorCode::StorageUnavailable
    );
    match change {
        QueuedConversionChange::HealthExpires => (),
        QueuedConversionChange::SessionRevoked => {
            let auth = crate::lock(&f.backend.inner.auth).clone().unwrap();
            crate::lock(&auth).logout(&f.token).unwrap();
        }
        QueuedConversionChange::ServiceStopping => {
            f.backend.inner.stopping.store(true, Ordering::SeqCst);
        }
    }
    let outcome =
        f.backend
            .execute_ordinary_conversion(&f.identity, &directory, &mut record, source, false);
    match change {
        QueuedConversionChange::HealthExpires => {
            outcome.unwrap();
            assert_eq!(record.status.phase, OrdinaryDiskConversionPhase::Completed);
        }
        QueuedConversionChange::SessionRevoked => {
            assert_eq!(outcome.unwrap_err().status, http::StatusCode::UNAUTHORIZED);
            target_empty(&f);
        }
        QueuedConversionChange::ServiceStopping => {
            assert_eq!(outcome.unwrap_err().code, ErrorCode::ServiceStopping);
            target_empty(&f);
        }
    }
    assert_eq!(engine.strong_count(), original_owners);
    assert_eq!(
        fs::read(f.discovery.join("ordinary/document.txt")).unwrap(),
        b"keep original content"
    );
}

#[test]
fn admitted_conversion_retains_its_engine_when_the_health_sample_expires_after_source_scan() {
    queued_conversion(QueuedConversionChange::HealthExpires);
}

#[test]
fn an_admitted_conversion_still_rejects_a_queued_session_revocation() {
    queued_conversion(QueuedConversionChange::SessionRevoked);
}

#[test]
fn an_admitted_conversion_still_rejects_queued_service_shutdown() {
    queued_conversion(QueuedConversionChange::ServiceStopping);
}

#[test]
fn ordinary_conversion_preview_and_rejected_confirmation_never_touch_source_or_target() {
    let f = fixture();
    let p = preview(&f);
    assert_eq!(p.source_preview.files, 1);
    assert_eq!(p.source_preview.directories, 1);
    assert_eq!(p.target.request.additions[0].level, 1);
    assert!(p.target.request.additions[0].upstream_slot_id.is_none());
    target_empty(&f);
    assert!(!f.data.join("management").join(DIRECTORY).exists());
    assert!(commit(&f, &p, false).is_err());
    assert!(!f.data.join("management").join(DIRECTORY).exists());
    target_empty(&f);
    assert_eq!(
        fs::read(f.discovery.join("ordinary/document.txt")).unwrap(),
        b"keep original content"
    );
    assert_eq!(
        f.backend
            .inner
            .backend()
            .unwrap()
            .addition_configuration(&f.identity)
            .unwrap()
            .disks
            .len(),
        1
    );
}

#[test]
fn ordinary_conversion_rejects_overlapping_sources_before_admitting_empty_target() {
    let f = fixture();
    let (source, target) = selection(&f);
    let mut same_identity = source.clone();
    same_identity.identity = target.root_identity.clone();
    let mut same_path = source.clone();
    same_path.path = target.root_path.clone();
    let mut nested = source;
    nested.path = format!("{}/child", target.root_path);
    for source in [same_identity, same_path, nested] {
        assert!(
            f.backend
                .preview_ordinary_conversion(
                    &f.identity,
                    OrdinaryDiskConversionRequest {
                        operation_id: OperationId::generate().unwrap(),
                        source,
                        target: target.clone(),
                        slot_id: "D02".into(),
                        metadata_member: false,
                    },
                )
                .is_err()
        );
    }
    target_empty(&f);
    assert!(!f.data.join("management").join(DIRECTORY).exists());
}

#[test]
fn ordinary_conversion_cold_reopen_recovers_only_exact_publication_receipt() {
    let f = fixture();
    let p = preview(&f);
    let status: OrdinaryDiskConversionStatus =
        json_codec::from_value(commit(&f, &p, true).unwrap()).unwrap();
    assert_busy_status_verifies_terminal(&f, &p);
    let directory =
        PrivateDirectory::open_existing(f.data.join("management").join(DIRECTORY)).unwrap();
    // Simulate a lost final control-record update after the original import
    // published: the saved request was already durable before that commit.
    let mut record = owned_record(&directory, p.operation_id, &f.identity).unwrap();
    record.status.phase = OrdinaryDiskConversionPhase::Importing;
    record.status.result = None;
    save(&directory, &record).unwrap();
    crate::lock(&f.backend.inner.state).backend = BackendState::Unopened;
    let reopened = storage_engine::Engine::open(&f.data).unwrap();
    let sequence = reopened.state().sequence;
    let engine = Arc::new(EngineBackend::new(reopened));
    let auth = crate::lock(&f.backend.inner.auth).clone().unwrap();
    engine
        .set_management_auth(crate::lock(&auth).reader())
        .unwrap();
    f.backend
        .inner
        .set(Some(engine.clone()), StorageState::Ready);
    let observed = f
        .backend
        .ordinary_conversion_status(&f.identity, Some(p.operation_id))
        .unwrap()
        .operation
        .unwrap();
    assert_eq!(observed.phase, OrdinaryDiskConversionPhase::Completed);
    assert_eq!(observed.result, status.result);
    assert_eq!(
        engine.test_with_engine_locked(|engine| engine.state().sequence),
        sequence
    );
    // The same operation id with another request is not success evidence.
    record
        .commit_request
        .as_mut()
        .unwrap()
        .message
        .push_str(" changed");
    save(&directory, &record).unwrap();
    let observed = f
        .backend
        .ordinary_conversion_status(&f.identity, Some(p.operation_id))
        .unwrap()
        .operation
        .unwrap();
    assert_eq!(
        observed.phase,
        OrdinaryDiskConversionPhase::RecoveryRequired
    );
    assert!(observed.result.is_none());
    assert_eq!(
        commit(&f, &p, true).unwrap_err().code,
        ErrorCode::RecoveryRequired
    );
    assert_eq!(
        engine.test_with_engine_locked(|engine| engine.state().sequence),
        sequence
    );
}

#[test]
fn ordinary_conversion_status_distinguishes_no_work_from_unknown_operation() {
    let f = fixture();
    assert!(
        f.backend
            .ordinary_conversion_status(&f.identity, None)
            .unwrap()
            .operation
            .is_none()
    );
    assert_eq!(
        f.backend
            .ordinary_conversion_status(&f.identity, Some(OperationId::generate().unwrap()))
            .unwrap_err()
            .code,
        ErrorCode::OperationNotFound
    );
    assert!(!f.data.join("management").join(DIRECTORY).exists());
}

#[test]
fn revoked_session_cannot_start_or_query_conversion() {
    let f = fixture();
    let p = preview(&f);
    let auth = crate::lock(&f.backend.inner.auth).clone().unwrap();
    crate::lock(&auth).logout(&f.token).unwrap();
    assert!(commit(&f, &p, true).is_err());
    assert!(
        f.backend
            .ordinary_conversion(
                "ordinary_conversion_status",
                json_codec::json!({}),
                &f.identity,
            )
            .is_err()
    );
    target_empty(&f);
    assert!(!f.data.join("management").join(DIRECTORY).exists());
}

#[test]
fn interrupted_before_first_import_write_resumes_only_the_original_review() {
    let f = fixture();
    let p = preview(&f);
    let registration = f
        .backend
        .commit_storage_addition(
            &f.identity,
            AdditionCommit {
                preview: p.target.clone(),
            },
        )
        .unwrap();
    let directory = PrivateDirectory::open_existing(f.data.join("management"))
        .unwrap()
        .create_child(&EntryName::new(DIRECTORY).unwrap())
        .unwrap();
    save(
        &directory,
        &Record {
            format: Format::Ordinary,
            principal: f.identity.principal.clone(),
            status: OrdinaryDiskConversionStatus {
                preview: p.clone(),
                phase: OrdinaryDiskConversionPhase::Importing,
                registration: Some(registration),
                workspace_id: None,
                root_operation_id: None,
                result: None,
                stop_operation_id: None,
                stop_result: None,
                failure: None,
            },
            commit_request: None,
            import_attempted: true,
        },
    )
    .unwrap();
    write(
        &directory,
        &storage_addition::latest_name(),
        &p.operation_id,
    )
    .unwrap();
    assert_eq!(
        commit(&f, &p, true).unwrap_err().code,
        ErrorCode::RecoveryRequired
    );
    let resumed: OrdinaryDiskConversionStatus = json_codec::from_value(
        f.backend
            .ordinary_conversion(
                "ordinary_conversion_resume",
                json_codec::json!({"operation_id":p.operation_id}),
                &f.identity,
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(resumed.phase, OrdinaryDiskConversionPhase::Completed);
    assert_eq!(resumed.result.unwrap().operation_id, p.import_operation_id);
    assert_eq!(resumed.preview, p);
    assert_eq!(
        fs::read(f.discovery.join("ordinary/document.txt")).unwrap(),
        b"keep original content"
    );
}

#[test]
fn completed_prior_owner_does_not_block_new_work_or_disclose_old_workflow() {
    let mut f = fixture();
    let p = preview(&f);
    commit(&f, &p, true).unwrap();
    // Exercise a separately recovered sole-administrator identity, not an
    // account-creation API. The original immutable operation owner stays admin.
    let auth = crate::lock(&f.backend.inner.auth).take().unwrap();
    let mut snapshot = crate::lock(&auth).snapshot().unwrap();
    drop(auth);
    snapshot["administrator"]["username"] = json_codec::json!("recovered-admin");
    snapshot["sessions"] = json_codec::json!({});
    let mut auth = AuthStore::restore_management(&f.data.join("management"), snapshot).unwrap();
    f.token = auth
        .login("recovered-admin", "conversion-test-password")
        .unwrap()
        .token;
    f.identity = auth.authenticate(&f.token).unwrap();
    f.backend
        .inner
        .backend()
        .unwrap()
        .set_management_auth(auth.reader())
        .unwrap();
    *crate::lock(&f.backend.inner.auth) = Some(Arc::new(Mutex::new(auth)));
    let directory =
        PrivateDirectory::open_existing(f.data.join("management").join(DIRECTORY)).unwrap();
    f.backend
        .ensure_prior_conversion_settled(&directory, &f.identity)
        .unwrap();
    assert!(
        f.backend
            .ordinary_conversion_status(&f.identity, None)
            .unwrap()
            .operation
            .is_none()
    );
    assert_eq!(
        f.backend
            .ordinary_conversion_status(&f.identity, Some(p.operation_id))
            .unwrap_err()
            .status,
        http::StatusCode::FORBIDDEN
    );
    assert_eq!(
        commit(&f, &p, true).unwrap_err().status,
        http::StatusCode::FORBIDDEN
    );
    let mut prior = read_record(&directory, p.operation_id).unwrap();
    prior
        .commit_request
        .as_mut()
        .unwrap()
        .message
        .push_str(" mismatched");
    save(&directory, &prior).unwrap();
    assert_eq!(
        f.backend
            .ensure_prior_conversion_settled(&directory, &f.identity)
            .unwrap_err()
            .code,
        ErrorCode::RecoveryRequired
    );
}

#[test]
fn conversion_status_observes_writer_without_reclassifying_its_stale_record() {
    let f = fixture();
    let p = preview(&f);
    commit(&f, &p, true).unwrap();
    let directory =
        PrivateDirectory::open_existing(f.data.join("management").join(DIRECTORY)).unwrap();
    let completed = read_record(&directory, p.operation_id).unwrap();
    // A caller that observes the writer's gate must not turn an earlier durable
    // snapshot into idle recovery when that writer releases the gate meanwhile.
    for _ in 0..24 {
        let mut earlier = read_record(&directory, p.operation_id).unwrap();
        earlier.status.phase = OrdinaryDiskConversionPhase::Registering;
        earlier.status.result = None;
        earlier.status.workspace_id = None;
        earlier.status.root_operation_id = None;
        earlier.commit_request = None;
        earlier.import_attempted = false;
        save(&directory, &earlier).unwrap();
        std::thread::scope(|scope| {
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let worker_barrier = barrier.clone();
            let backend = &f.backend;
            let completed = &completed;
            let directory = &directory;
            let writer = scope.spawn(move || {
                let _guard = crate::lock(&backend.inner.ordinary_conversion);
                worker_barrier.wait();
                save(directory, completed).unwrap();
            });
            barrier.wait();
            let status = f
                .backend
                .ordinary_conversion_status(&f.identity, Some(p.operation_id))
                .unwrap()
                .operation
                .unwrap();
            assert_ne!(status.phase, OrdinaryDiskConversionPhase::RecoveryRequired);
            writer.join().unwrap();
        });
    }
}

#[test]
fn source_change_and_forged_higher_level_are_rejected_before_registration() {
    let f = fixture();
    let p = preview(&f);
    let mut invalid = p.clone();
    invalid.target.request.additions[0].level = 2;
    invalid.target.request.additions[0].upstream_slot_id = Some("P01".into());
    invalid.preview_digest = digest(&invalid);
    assert!(commit(&f, &invalid, true).is_err());
    target_empty(&f);
    fs::write(f.discovery.join("ordinary/document.txt"), b"source changed").unwrap();
    assert_eq!(commit(&f, &p, true).unwrap_err().code, ErrorCode::PlanStale);
    target_empty(&f);
    assert_eq!(
        f.backend
            .inner
            .backend()
            .unwrap()
            .addition_configuration(&f.identity)
            .unwrap()
            .disks
            .len(),
        1
    );
}

#[test]
fn ordinary_conversion_registers_new_primary_then_publishes_exact_root_contents_once() {
    let f = fixture();
    let p = preview(&f);
    let value = commit(&f, &p, true).unwrap();
    let status: OrdinaryDiskConversionStatus = json_codec::from_value(value).unwrap();
    assert_eq!(status.phase, OrdinaryDiskConversionPhase::Completed);
    assert!(
        status
            .registration
            .as_ref()
            .is_some_and(|registration| registration.state == AdditionState::Completed)
    );
    assert_eq!(
        status.result.as_ref().unwrap().operation_id,
        p.import_operation_id
    );
    assert!(status.workspace_id.is_some());
    let backend = f.backend.inner.backend().unwrap();
    let sequence = backend.test_with_engine_locked(|engine| {
        let disk = engine
            .config()
            .disks
            .iter()
            .find(|disk| disk.slot_id == "D02")
            .unwrap();
        assert_eq!(disk.level, 1);
        assert!(disk.upstream_slot_id.is_none());
        let repository = engine
            .state()
            .repositories
            .values()
            .find(|repo| repo.file_manager_root && repo.primary_slot_id == "D02")
            .unwrap();
        let entries = engine.list_tree(&repository.main_branch_id).unwrap();
        assert!(entries.iter().any(|entry| entry.path() == "empty"));
        let file = entries
            .iter()
            .find(|entry| entry.path() == "document.txt")
            .unwrap();
        let mut content = engine.open_content(&file.version_id).unwrap();
        let mut bytes = Vec::new();
        content.file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"keep original content");
        engine.state().sequence
    });
    assert_eq!(
        fs::read(f.discovery.join("ordinary/document.txt")).unwrap(),
        b"keep original content"
    );
    let replay: OrdinaryDiskConversionStatus =
        json_codec::from_value(commit(&f, &p, true).unwrap()).unwrap();
    assert_eq!(replay.phase, OrdinaryDiskConversionPhase::Completed);
    assert_eq!(replay.result, status.result);
    assert_eq!(
        backend.test_with_engine_locked(|engine| engine.state().sequence),
        sequence
    );
    let queried = f
        .backend
        .ordinary_conversion_status(&f.identity, Some(p.operation_id))
        .unwrap()
        .operation
        .unwrap();
    assert_eq!(queried.result, status.result);
}

#[test]
fn uncertain_import_never_starts_a_replacement_workspace() {
    let f = fixture();
    let p = preview(&f);
    let directory = PrivateDirectory::open_existing(f.data.join("management"))
        .unwrap()
        .create_child(&EntryName::new(DIRECTORY).unwrap())
        .unwrap();
    let record = Record {
        format: Format::Ordinary,
        principal: f.identity.principal.clone(),
        status: OrdinaryDiskConversionStatus {
            preview: p.clone(),
            phase: OrdinaryDiskConversionPhase::Importing,
            registration: None,
            workspace_id: Some("workspace_unresolved".into()),
            root_operation_id: None,
            result: None,
            stop_operation_id: None,
            stop_result: None,
            failure: None,
        },
        commit_request: None,
        import_attempted: true,
    };
    save(&directory, &record).unwrap();
    assert_eq!(
        f.backend
            .ordinary_conversion(
                "ordinary_conversion_resume",
                json_codec::to_value(OrdinaryDiskConversionResume {
                    operation_id: p.operation_id
                })
                .unwrap(),
                &f.identity
            )
            .unwrap_err()
            .code,
        ErrorCode::RecoveryRequired
    );
    target_empty(&f);
    assert_eq!(
        f.backend
            .inner
            .backend()
            .unwrap()
            .addition_configuration(&f.identity)
            .unwrap()
            .disks
            .len(),
        1
    );
}

fn registered_conversion_input(
    f: &Fixture,
    preview: &OrdinaryDiskConversionPreview,
) -> PreparedOrdinaryConversion {
    let prepared = f
        .backend
        .inspect_conversion_source(
            &f.identity,
            &preview.source,
            Some(&preview.target.request.additions[0].selection.root_path),
        )
        .unwrap();
    let registration = f
        .backend
        .commit_storage_addition(
            &f.identity,
            AdditionCommit {
                preview: preview.target.clone(),
            },
        )
        .unwrap();
    assert_eq!(registration.state, AdditionState::Completed);
    assert!(registration.result.is_some());
    prepared
}

fn conversion_import_snapshot(backend: &EngineBackend) -> (Vec<u8>, BTreeMap<PathBuf, Vec<u8>>) {
    backend.test_with_engine_locked(|engine| {
        let mut pending = engine
            .config()
            .disks
            .iter()
            .filter(|disk| disk.metadata_member)
            .map(|disk| disk.root.clone())
            .collect::<Vec<_>>();
        let mut journals = BTreeMap::new();
        let mut inspected = 0;
        // Registration can select a generation-specific journal. Capture both
        // that active log and retained logs, rather than assuming the old path.
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(directory).unwrap() {
                inspected += 1;
                assert!(
                    inspected <= 100_000,
                    "conversion fixture inventory exceeded its bound"
                );
                let entry = entry.unwrap();
                let kind = entry.file_type().unwrap();
                assert!(
                    kind.is_dir() || kind.is_file(),
                    "fixture contains a link or special file"
                );
                if kind.is_dir() {
                    pending.push(entry.path());
                } else if entry.file_name() == "journal.bin" {
                    journals.insert(entry.path(), fs::read(entry.path()).unwrap());
                }
            }
        }
        assert!(!journals.is_empty());
        (json_codec::to_vec(engine.state()).unwrap(), journals)
    })
}

#[test]
fn registered_conversion_import_revalidates_revocation_after_registration() {
    let f = fixture();
    let preview = preview(&f);
    let PreparedOrdinaryConversion { backend, source } = registered_conversion_input(&f, &preview);
    // Registration really completed under the original session. The retained
    // Engine lease must not authorize the next import after that session ends.
    let auth = crate::lock(&f.backend.inner.auth).clone().unwrap();
    crate::lock(&auth).logout(&f.token).unwrap();
    let before = conversion_import_snapshot(&backend);
    let mut progressed = false;
    let failure = backend
        .run_ordinary_conversion(
            &f.identity,
            &preview.target,
            source,
            preview.import_operation_id,
            |_| {
                progressed = true;
                Ok(())
            },
        )
        .unwrap_err();
    assert!(failure.before_import());
    assert_eq!(failure.into_api().status, http::StatusCode::UNAUTHORIZED);
    assert!(!progressed);
    assert_eq!(conversion_import_snapshot(&backend), before);
    assert_eq!(
        fs::read(f.discovery.join("ordinary/document.txt")).unwrap(),
        b"keep original content"
    );
}

#[test]
fn registered_conversion_import_rejects_a_later_membership_fence() {
    let f = fixture();
    let preview = preview(&f);
    let PreparedOrdinaryConversion { backend, source } = registered_conversion_input(&f, &preview);
    let fence_operation = OperationId::generate().unwrap();
    backend.test_with_engine_locked(|engine| {
        let mut target = engine.config().clone();
        let previous = target.disks.len();
        let generated = storage_contract::Config::development(
            (0..=previous).map(|index| f.discovery.join(format!("later-fence-{index}"))),
        )
        .unwrap();
        let mut addition = generated.disks.into_iter().nth(previous).unwrap();
        addition.slot_id = "D03".into();
        addition.level = 1;
        addition.upstream_slot_id = None;
        addition.metadata_member = false;
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&addition.root)
            .unwrap();
        target.disks.push(addition);
        engine
            .begin_membership_handoff(target, "admin", fence_operation)
            .unwrap();
        assert_eq!(
            engine.membership_fence_status().unwrap().operation_id,
            fence_operation
        );
    });
    // A real later membership fence wins even though this exact Engine and
    // source were admitted before it. No import root or workspace may appear.
    let before = conversion_import_snapshot(&backend);
    let mut progressed = false;
    let failure = backend
        .run_ordinary_conversion(
            &f.identity,
            &preview.target,
            source,
            preview.import_operation_id,
            |_| {
                progressed = true;
                Ok(())
            },
        )
        .unwrap_err();
    assert!(failure.before_import());
    assert_eq!(failure.into_api().code, ErrorCode::RecoveryRequired);
    assert!(!progressed);
    assert_eq!(conversion_import_snapshot(&backend), before);
    assert_eq!(
        fs::read(f.discovery.join("ordinary/document.txt")).unwrap(),
        b"keep original content"
    );
}

#[test]
fn target_name_budget_rejects_preview_and_user_rename_requires_a_new_review() {
    let f = fixture();
    let parent = f.discovery.join("ordinary/empty");
    let accepted = format!("{}ab", "界".repeat(79));
    fs::write(parent.join(&accepted), b"retain these bytes").unwrap();
    let original = preview(&f);
    let overlong = "界".repeat(80);
    fs::rename(parent.join(&accepted), parent.join(&overlong)).unwrap();
    let (source, target) = selection(&f);
    let failure = f.backend.preview_ordinary_conversion(
        &f.identity,
        OrdinaryDiskConversionRequest {
            operation_id: OperationId::generate().unwrap(),
            source,
            target,
            slot_id: "D02".into(),
            metadata_member: false,
        },
    ).unwrap_err();
    assert_eq!(failure.code, ErrorCode::PlanInvalid);
    assert_eq!(fs::read(parent.join(&overlong)).unwrap(), b"retain these bytes");
    target_empty(&f);
    assert!(!f.data.join("management").join(DIRECTORY).exists());

    let renamed = "界".repeat(79);
    fs::rename(parent.join(&overlong), parent.join(&renamed)).unwrap();
    let revised = preview(&f);
    assert_ne!(revised.source_preview.digest, original.source_preview.digest);
    assert_eq!(commit(&f, &original, true).unwrap_err().code, ErrorCode::PlanStale);
    target_empty(&f);
    let status: OrdinaryDiskConversionStatus =
        json_codec::from_value(commit(&f, &revised, true).unwrap()).unwrap();
    assert_eq!(status.phase, OrdinaryDiskConversionPhase::Completed);
    assert_eq!(fs::read(parent.join(renamed)).unwrap(), b"retain these bytes");
}

#[test]
fn registered_legacy_review_rejects_over_budget_import_and_retains_registration() {
    let f = fixture();
    let mut p = preview(&f);
    let long = "界".repeat(80);
    fs::write(f.discovery.join("ordinary/empty").join(&long), b"keep original").unwrap();
    // Reconstruct a valid review issued before target budgeting existed using
    // the unchanged source-only inventory and unchanged request digest format.
    let prepared = f.backend.inspect_conversion_source(
        &f.identity, &p.source, Some(&p.target.request.additions[0].selection.root_path),
    ).unwrap();
    p.source_preview = source_preview(&prepared);
    p.preview_digest = digest(&p);
    let registration = f.backend.commit_storage_addition(
        &f.identity, AdditionCommit { preview: p.target.clone() },
    ).unwrap();
    assert_eq!(registration.state, AdditionState::Completed);
    let directory = PrivateDirectory::open_existing(f.data.join("management"))
        .unwrap()
        .create_child(&EntryName::new(DIRECTORY).unwrap())
        .unwrap();
    let mut record = pending_record(&f, &p);
    record.status.registration = Some(registration.clone());
    save(&directory, &record).unwrap();
    let backend = f.backend.inner.backend().unwrap();
    let before = conversion_import_snapshot(&backend);
    let failure = f.backend.ordinary_conversion(
        "ordinary_conversion_resume",
        json_codec::json!({"operation_id": p.operation_id}),
        &f.identity,
    ).unwrap_err();
    assert_eq!(failure.code, ErrorCode::PlanInvalid);
    assert_eq!(conversion_import_snapshot(&backend), before);
    assert_eq!(
        fs::read(f.discovery.join("ordinary/empty").join(long)).unwrap(),
        b"keep original"
    );
    let retained = read_record(&directory, p.operation_id).unwrap();
    assert_eq!(retained.status.phase, OrdinaryDiskConversionPhase::RecoveryRequired);
    assert_eq!(retained.status.registration, Some(registration));
    assert!(retained.status.workspace_id.is_none());
    assert!(retained.status.root_operation_id.is_none());
    assert!(!retained.import_attempted);
}

#[test]
fn completed_conversion_result_remains_queryable_after_source_exceeds_budget() {
    let f = fixture();
    let p = preview(&f);
    let completed: OrdinaryDiskConversionStatus =
        json_codec::from_value(commit(&f, &p, true).unwrap()).unwrap();
    let long = "界".repeat(80);
    fs::write(f.discovery.join("ordinary/empty").join(long), b"later source data").unwrap();
    let backend = f.backend.inner.backend().unwrap();
    let before = conversion_import_snapshot(&backend);
    let replay: OrdinaryDiskConversionStatus =
        json_codec::from_value(commit(&f, &p, true).unwrap()).unwrap();
    assert_eq!(replay.result, completed.result);
    let resumed: OrdinaryDiskConversionStatus = json_codec::from_value(
        f.backend.ordinary_conversion(
            "ordinary_conversion_resume",
            json_codec::json!({"operation_id": p.operation_id}),
            &f.identity,
        ).unwrap(),
    ).unwrap();
    assert_eq!(resumed.result, completed.result);
    assert_eq!(conversion_import_snapshot(&backend), before);
}
