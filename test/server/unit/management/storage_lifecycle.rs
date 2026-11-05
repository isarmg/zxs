//! Source scenarios use the actual engine coordinator; no tests were run here.
use super::*;
use crate::management::storage_configuration::admits_configuration;

struct Fixture {
    backend: Arc<ManagementBackend>,
    identity: Identity,
    token: String,
    auth: Arc<Mutex<AuthStore>>,
    original: Config,
    data: PathBuf,
    branch: String,
    _temp: tempfile::TempDir,
}
fn fixture() -> Fixture {
    fixture_with_metadata_members(3)
}
fn fixture_with_metadata_members(metadata_members: usize) -> Fixture {
    let temp = crate::tests::private_tempdir();
    let data = temp.path().join("system");
    let mut original =
        Config::development((0..3).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
    for (index, disk) in original.disks.iter_mut().enumerate() {
        disk.metadata_member = index < metadata_members;
    }
    let mut engine = storage_engine::Engine::initialize(&data, original.clone()).unwrap();
    let repository = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("exit-source").unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let branch = repository.main_branch_id;
    let workspace = engine.create_workspace(&branch, "admin").unwrap();
    engine
        .stage_bytes(
            &workspace.workspace_id,
            "exclusive.txt",
            b"keep these physical bytes",
        )
        .unwrap();
    engine
        .commit_workspace(storage_engine::CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: String::new(),
            request_digest: None,
        })
        .unwrap();
    let management = data.join("management");
    drop(
        AuthStore::initialize_management(&management, "admin", "lifecycle-test-password").unwrap(),
    );
    let (backend, mut auth) = open_service_with_options(
        &data,
        &management,
        ManagementOptions {
            discovery_root: temp.path().to_str().unwrap().parse().unwrap(),
            ..Default::default()
        },
    )
    .unwrap();
    let login = auth.login("admin", "lifecycle-test-password").unwrap();
    let identity = auth.authenticate(&login.token).unwrap();
    let engine = Arc::new(EngineBackend::new(engine));
    engine.set_management_auth(auth.reader()).unwrap();
    let auth = Arc::new(Mutex::new(auth));
    *crate::lock(&backend.inner.auth) = Some(auth.clone());
    backend.inner.set(Some(engine), StorageState::Ready);
    Fixture {
        backend,
        identity,
        token: login.token,
        auth,
        original,
        data,
        branch,
        _temp: temp,
    }
}
fn request(f: &Fixture, action: ExitAction) -> ExitRequest {
    ExitRequest {
        operation_id: OperationId::generate().unwrap(),
        slot_id: f.original.disks[0].slot_id.clone(),
        expected_disk_id: f.original.disks[0].disk_id.clone(),
        action,
    }
}
fn command(f: &Fixture, action: &'static str, value: Value) -> Result<Value, ApiError> {
    f.backend.storage_exit(action, value, &f.identity)
}
fn pending(f: &Fixture, action: ExitAction) -> StoredExit {
    let backend = f.backend.inner.addition_backend().unwrap();
    let (preview, original) = backend
        .preview_storage_exit(&f.identity, request(f, action))
        .unwrap();
    let directory = PrivateDirectory::open_existing(&f.backend.inner.management_root)
        .unwrap()
        .create_child(&EntryName::new(DIRECTORY).unwrap())
        .unwrap();
    let mut record = StoredExit {
        format: Format::Exit,
        principal: f.identity.principal.clone(),
        original,
        archive: None,
        status: ExitStatus {
            operation_id: preview.request.operation_id,
            state: ExitState::Running,
            phase: ExitPhase::Prepared,
            preview,
            failure: None,
            result: None,
        },
    };
    save(&directory, &record).unwrap();
    write(&directory, &latest_name(), &record.status.operation_id).unwrap();
    let intent = record.clone();
    backend
        .run_storage_exit(&f.identity, &intent, |phase, archive| {
            record.status.phase = phase;
            if let Some(archive) = archive {
                record.archive = Some(archive);
            }
            save(&directory, &record)?;
            if phase == ExitPhase::Activating {
                Err(ApiError::conflict(
                    ErrorCode::RecoveryRequired,
                    "test pauses before actual exit activation",
                ))
            } else {
                Ok(())
            }
        })
        .unwrap_err();
    record.status.state = ExitState::RecoveryRequired;
    save(&directory, &record).unwrap();
    record
}
fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut found = BTreeMap::new();
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(files(&path));
        } else {
            found.insert(path.clone(), fs::read(path).unwrap());
        }
    }
    found
}
fn resume(f: &Fixture, record: &StoredExit) -> Result<Value, ApiError> {
    command(
        f,
        "disk_exit_resume",
        json!(ExitResume {
            operation_id: record.status.operation_id
        }),
    )
}

#[test]
fn actual_exit_preserves_target_bytes_and_replays_exact_offline_or_deregister_result() {
    for action in [ExitAction::PlannedOffline, ExitAction::Deregister] {
        let f = fixture();
        let record = pending(&f, action);
        let before = files(&f.original.disks[0].root);
        let result: ExitStatus = json_codec::from_value(resume(&f, &record).unwrap()).unwrap();
        assert_eq!(result.state, ExitState::Completed);
        assert_eq!(result.result.as_ref().unwrap().kind, action.result_kind());
        assert_eq!(
            result.result.as_ref().unwrap().request_digest,
            record.archive.as_ref().unwrap().transition_hash
        );
        assert_eq!(files(&f.original.disks[0].root), before);
        f.backend
            .inner
            .addition_backend()
            .unwrap()
            .test_with_engine_locked(|engine| {
                assert_eq!(
                    engine.disk_exit_status().unwrap().unwrap().phase,
                    storage_engine::DiskExitPhase::Published
                );
                match action {
                    ExitAction::PlannedOffline => assert_eq!(
                        engine
                            .registered_disk(&f.original.disks[0].disk_id)
                            .unwrap()
                            .registration_mode,
                        storage_engine::RegistrationMode::Disabled
                    ),
                    ExitAction::Deregister => assert!(
                        engine
                            .registered_disk(&f.original.disks[0].disk_id)
                            .is_none()
                    ),
                }
                assert_eq!(
                    engine.list_tree(&f.branch).unwrap().len(),
                    usize::from(action == ExitAction::PlannedOffline)
                );
            });
        assert_eq!(resume(&f, &record).unwrap(), json!(result));
        assert_eq!(
            command(
                &f,
                "disk_exit_commit",
                json!(ExitCommit {
                    preview: record.status.preview.clone()
                })
            )
            .unwrap(),
            json!(result)
        );
        let current = f
            .backend
            .inner
            .addition_backend()
            .unwrap()
            .addition_configuration(&f.identity)
            .unwrap();
        assert!(
            admits_configuration(&f.backend.inner.management_root, &current, &f.original).unwrap()
        );
        let mut wrong = current.clone();
        wrong.disks[1].binding_generation += 1;
        assert!(
            !admits_configuration(&f.backend.inner.management_root, &wrong, &f.original).unwrap()
        );
    }
}

#[test]
fn published_exit_status_recovers_from_missing_management_completion_projection() {
    let f = fixture();
    let mut record = pending(&f, ExitAction::Deregister);
    let directory = open(&f.backend.inner.management_root).unwrap().unwrap();
    let intent = record.clone();
    let result = f
        .backend
        .inner
        .addition_backend()
        .unwrap()
        .run_storage_exit(&f.identity, &intent, |phase, archive| {
            record.status.phase = phase;
            if let Some(archive) = archive {
                record.archive = Some(archive);
            }
            save(&directory, &record)
        })
        .unwrap();
    assert!(
        read_record(&directory, record.status.operation_id)
            .unwrap()
            .unwrap()
            .status
            .result
            .is_none()
    );
    crate::lock(&f.backend.inner.state).backend = BackendState::Unopened;
    let engine = storage_engine::Engine::open(&f.data).unwrap();
    let backend = Arc::new(EngineBackend::new(engine));
    backend
        .set_management_auth(crate::lock(&f.auth).reader())
        .unwrap();
    f.backend.inner.set(Some(backend), StorageState::Ready);
    let observed = f
        .backend
        .exit_status(&f.identity, Some(record.status.operation_id))
        .unwrap()
        .operation
        .unwrap();
    assert_eq!(observed.result, Some(result));
    assert_eq!(observed.state, ExitState::Completed);
    assert_eq!(resume(&f, &record).unwrap(), json!(observed));
}

#[test]
fn exit_preview_and_resume_revalidate_generation_actor_and_durable_lifecycle_gate() {
    let f = fixture();
    let preview: ExitPreview = json_codec::from_value(
        command(
            &f,
            "disk_exit_preview",
            json!(request(&f, ExitAction::Deregister)),
        )
        .unwrap(),
    )
    .unwrap();
    f.backend
        .inner
        .addition_backend()
        .unwrap()
        .test_with_engine_locked(|engine| {
            engine
                .create_repository(
                    &storage_contract::RepositoryName::parse("concurrent-change").unwrap(),
                    "P01",
                    1,
                    "admin",
                    OperationId::generate().unwrap(),
                )
                .unwrap();
        });
    assert_eq!(
        command(&f, "disk_exit_commit", json!(ExitCommit { preview }))
            .unwrap_err()
            .code,
        ErrorCode::GenerationMismatch
    );
    assert!(
        f.backend.require_settled_exit().is_ok(),
        "an unfenced stale preview cannot block new plans"
    );
    let record = pending(&f, ExitAction::Deregister);
    assert_eq!(
        f.backend.require_settled_exit().unwrap_err().code,
        ErrorCode::RecoveryRequired
    );
    let mut changed = record.status.preview.clone();
    changed.request.action = ExitAction::PlannedOffline;
    assert_eq!(
        command(
            &f,
            "disk_exit_commit",
            json!(ExitCommit { preview: changed })
        )
        .unwrap_err()
        .code,
        ErrorCode::IdempotencyKeyReused
    );
    crate::lock(&f.auth).logout(&f.token).unwrap();
    assert_eq!(
        resume(&f, &record).unwrap_err().code,
        ErrorCode::Unauthorized
    );
    assert!(
        read_record(
            &open(&f.backend.inner.management_root).unwrap().unwrap(),
            record.status.operation_id
        )
        .unwrap()
        .unwrap()
        .status
        .result
        .is_none()
    );
}

#[test]
fn a_prepared_exit_is_never_completed_by_control_phase_or_fabricated_receipt() {
    let f = fixture();
    let mut record = pending(&f, ExitAction::Deregister);
    let directory = open(&f.backend.inner.management_root).unwrap().unwrap();
    record.status.state = ExitState::Completed;
    record.status.phase = ExitPhase::Completed;
    save(&directory, &record).unwrap();
    match read_record(&directory, record.status.operation_id) {
        Err(error) => assert_eq!(error.code, ErrorCode::ObjectCorrupt),
        Ok(_) => panic!("completion without an engine receipt was admitted"),
    }
}

fn complete(f: &Fixture, request: ExitRequest) -> ExitStatus {
    let preview: ExitPreview =
        json_codec::from_value(command(f, "disk_exit_preview", json!(request)).unwrap()).unwrap();
    json_codec::from_value(command(f, "disk_exit_commit", json!(ExitCommit { preview })).unwrap())
        .unwrap()
}

#[test]
fn successive_http_exits_replay_old_receipts_and_recover_the_setup_configuration_chain() {
    let f = fixture();
    let offline = complete(&f, request(&f, ExitAction::PlannedOffline));
    let moved = f._temp.path().join("stopped-directory-kept");
    fs::rename(&f.original.disks[0].root, &moved).unwrap();
    let retained = files(&moved);
    let detached = complete(&f, request(&f, ExitAction::Deregister));
    assert_eq!(detached.state, ExitState::Completed);
    assert_eq!(files(&moved), retained);
    assert!(!f.original.disks[0].root.exists());

    // A different already-enabled directory can start another lifecycle after
    // Published. The old status is not mistaken for this new operation.
    let second = complete(
        &f,
        ExitRequest {
            operation_id: OperationId::generate().unwrap(),
            slot_id: f.original.disks[1].slot_id.clone(),
            expected_disk_id: f.original.disks[1].disk_id.clone(),
            action: ExitAction::Deregister,
        },
    );
    assert_eq!(second.state, ExitState::Completed);

    // The same setup ancestry also crosses back from Exit to a real Add.
    use storage_contract::storage_addition::{
        AdditionCommit, AdditionPreview, AdditionRequest, AdditionState, AdditionStatus,
        DirectoryAddition,
    };
    let discovery = f._temp.path().to_str().unwrap();
    let added = f._temp.path().join("later-added-directory");
    fs::DirBuilder::new().mode(0o700).create(&added).unwrap();
    let browsed = crate::storage_directories::browse(added.to_str().unwrap()).unwrap();
    let selected = crate::storage_directories::inspect_within(
        discovery,
        &storage_contract::storage_directories::InspectRequest {
            mount_path: browsed.mount_path,
            root_path: added.to_str().unwrap().into(),
        },
        std::slice::from_ref(&f.data),
    )
    .unwrap();
    let addition = AdditionRequest {
        operation_id: OperationId::generate().unwrap(),
        additions: vec![DirectoryAddition {
            selection: selected,
            slot_id: "P04".into(),
            level: 1,
            upstream_slot_id: None,
            metadata_member: true,
        }],
    };
    let preview: AdditionPreview = json_codec::from_value(
        f.backend
            .storage_addition("disk_add_preview", json!(addition), &f.identity)
            .unwrap(),
    )
    .unwrap();
    let added: AdditionStatus = json_codec::from_value(
        f.backend
            .storage_addition(
                "disk_add_commit",
                json!(AdditionCommit { preview }),
                &f.identity,
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(added.state, AdditionState::Completed);
    let backend = f.backend.inner.addition_backend().unwrap();
    let current = backend.addition_configuration(&f.identity).unwrap();
    assert!(admits_configuration(&f.backend.inner.management_root, &current, &f.original).unwrap());
    let mut edited = current.clone();
    edited.disks[2].binding_generation += 1;
    assert!(!admits_configuration(&f.backend.inner.management_root, &edited, &f.original).unwrap());

    // Losing only the old HTTP completion projection must not lose the permanent
    // engine receipt after the head moved to a different Exit.
    let directory = open(&f.backend.inner.management_root).unwrap().unwrap();
    let mut record = read_record(&directory, offline.operation_id)
        .unwrap()
        .unwrap();
    record.status.state = ExitState::RecoveryRequired;
    record.status.phase = ExitPhase::Activating;
    record.status.result = None;
    save(&directory, &record).unwrap();
    let observed = f
        .backend
        .exit_status(&f.identity, Some(offline.operation_id))
        .unwrap()
        .operation
        .unwrap();
    assert_eq!(observed.result, offline.result);
    assert_eq!(observed.state, ExitState::Completed);
    assert_eq!(resume(&f, &record).unwrap(), json!(observed));

    // A structurally valid management copy cannot rebind that real receipt to
    // another historical disk, even after the current head became an Add.
    let restored = read_record(&directory, offline.operation_id)
        .unwrap()
        .unwrap();
    let mut rebound = restored.clone();
    let other = &f.original.disks[1];
    rebound.status.preview.target.disk_id = other.disk_id.clone();
    rebound.status.preview.target.slot_id = other.slot_id.clone();
    rebound.status.preview.target.binding_generation = other.binding_generation;
    rebound.status.preview.target.root_path = other.root.to_str().unwrap().into();
    rebound.status.preview.request.expected_disk_id = other.disk_id.clone();
    rebound.status.preview.request.slot_id = other.slot_id.clone();
    rebound.status.preview.preview_digest.clear();
    rebound.status.preview.preview_digest = digest(&rebound.status.preview);
    save(&directory, &rebound).unwrap();
    assert!(
        read_record(&directory, offline.operation_id)
            .unwrap()
            .is_some()
    );
    assert_eq!(
        f.backend
            .exit_status(&f.identity, Some(offline.operation_id))
            .unwrap_err()
            .code,
        ErrorCode::RecoveryRequired
    );
    assert_eq!(
        resume(&f, &rebound).unwrap_err().code,
        ErrorCode::RecoveryRequired
    );
    save(&directory, &restored).unwrap();

    // Every metadata-role change must remain linked to the original config.
    // A later valid receipt cannot substitute for the missing first edge.
    let saved = directory.path().join(name(offline.operation_id).as_path());
    let held = f._temp.path().join("held-first-intent.json");
    fs::rename(&saved, &held).unwrap();
    assert!(
        !admits_configuration(&f.backend.inner.management_root, &current, &f.original).unwrap()
    );
    fs::rename(held, saved).unwrap();
    assert!(admits_configuration(&f.backend.inner.management_root, &current, &f.original).unwrap());
    assert_eq!(files(&moved), retained);
    drop(backend);
    crate::lock(&f.backend.inner.state).backend = BackendState::Unopened;
    let engine = storage_engine::Engine::open(&f.data).unwrap();
    assert!(engine.list_tree(&f.branch).unwrap().is_empty());
    assert!(engine.operation_record(offline.operation_id).is_ok());
    assert!(engine.operation_record(second.operation_id).is_ok());
    assert!(engine.operation_record(added.operation_id).is_ok());
    assert!(!f.original.disks[0].root.exists());
}

#[test]
fn exit_http_receipt_waits_for_final_publication_admission_after_persist() {
    let f = fixture();
    let record = pending(&f, ExitAction::Deregister);
    let backend = f.backend.inner.addition_backend().unwrap();
    backend.test_with_engine_locked(|engine| {
        engine.inject_fault_once("before_disk_exit_final_admission");
    });
    assert_eq!(
        resume(&f, &record).unwrap_err().code,
        ErrorCode::RecoveryRequired
    );
    backend.test_with_engine_locked(|engine| {
        assert!(
            engine
                .state()
                .operations
                .contains_key(&record.status.operation_id)
        );
        assert_eq!(
            engine.disk_exit_status().unwrap().unwrap().phase,
            storage_engine::DiskExitPhase::Activating
        );
    });
    assert!(
        backend
            .storage_exit_result(&f.identity, &record)
            .unwrap()
            .is_none()
    );
    let status = f
        .backend
        .exit_status(&f.identity, Some(record.status.operation_id))
        .unwrap()
        .operation
        .unwrap();
    assert_eq!(status.state, ExitState::RecoveryRequired);
    assert!(status.result.is_none());
    let result: ExitStatus = json_codec::from_value(resume(&f, &record).unwrap()).unwrap();
    assert_eq!(result.state, ExitState::Completed);
    assert_eq!(
        result.result.unwrap().operation_id,
        record.status.operation_id
    );
}

fn terminal_commit(f: &Fixture, action: ExitAction) -> ExitStatus {
    let preview: ExitPreview =
        json_codec::from_value(command(f, "disk_exit_preview", json!(request(f, action))).unwrap())
            .unwrap();
    assert_eq!(preview.authority, ExitAuthority::TerminalManagement);
    assert_eq!(
        (
            preview.original_metadata_members,
            preview.target_metadata_members
        ),
        (1, 0)
    );
    json_codec::from_value(command(f, "disk_exit_commit", json!(ExitCommit { preview })).unwrap())
        .unwrap()
}

fn prepare_terminal_cold_recovery(f: &Fixture, failure: Option<&str>) -> (OperationId, PathBuf) {
    let backend = f.backend.inner.addition_backend().unwrap();
    let operation = OperationId::generate().unwrap();
    backend.test_with_engine_locked(|engine| {
        engine
            .begin_disk_exit(
                &f.original.disks[0].disk_id,
                storage_engine::MembershipExitAction::Deregister,
                "admin",
                operation,
            )
            .unwrap();
        let mut exit = engine.prepare_terminal_disk_exit().unwrap();
        if let Some(failure) = failure {
            engine.inject_fault_once(failure);
            assert!(exit.complete(engine).is_err());
        }
    });
    drop(backend);
    let mut state = crate::lock(&f.backend.inner.state);
    state.backend = BackendState::Unopened;
    state.admitted = false;
    state.status = StorageState::RecoveryRequired;
    (
        operation,
        f.data
            .join("terminal-management")
            .join(operation.to_string()),
    )
}

#[test]
fn prepared_terminal_cold_recovery_finishes_without_opening_the_stopped_root() {
    for failure in [
        None,
        Some("after_terminal_stop"),
        Some("after_terminal_io_closed"),
    ] {
        let f = fixture_with_metadata_members(1);
        let (operation, directory) = prepare_terminal_cold_recovery(&f, failure);
        assert!(!directory.join("complete.json").exists());
        let root = &f.original.disks[0].root;
        let moved = root.with_extension("preserved");
        fs::rename(root, &moved).unwrap();
        fs::write(root, b"terminal startup cannot open this as a directory").unwrap();
        let retained = files(&moved);
        assert!(f.backend.inner.recover_terminal().unwrap(), "{failure:?}");
        let terminal = crate::lock(&f.backend.inner.state)
            .backend
            .terminal()
            .cloned()
            .unwrap();
        assert_eq!(terminal.result().operation_id, operation);
        assert_eq!(terminal.result().result["target_io"], "closed");
        assert_eq!(
            f.backend.management_status().state(),
            StorageState::TerminalManagement
        );
        assert!(directory.join("complete.json").is_file());
        assert!(f.backend.inner.backend().is_err());
        assert_eq!(files(&moved), retained);
        assert_eq!(
            fs::read(root).unwrap(),
            b"terminal startup cannot open this as a directory"
        );
    }
}

#[test]
fn prepared_terminal_cold_recovery_preserves_and_rejects_bad_publication() {
    let f = fixture_with_metadata_members(1);
    let (_, directory) = prepare_terminal_cold_recovery(&f, None);
    let complete = directory.join("complete.json");
    fs::write(&complete, b"damaged terminal publication").unwrap();
    assert!(f.backend.inner.recover_terminal().is_err());
    assert_eq!(fs::read(complete).unwrap(), b"damaged terminal publication");
    assert!(matches!(
        crate::lock(&f.backend.inner.state).backend,
        BackendState::Unopened
    ));
    assert!(f.backend.inner.backend().is_err());
}

#[test]
fn terminal_cold_recovery_does_not_treat_orphan_or_malformed_resume_as_absent() {
    for marker in [
        "resume-generation.json",
        "resume-bootstrap.json",
        "resume-complete.json",
        "resume.json",
    ] {
        let f = fixture_with_metadata_members(1);
        let completed = terminal_commit(&f, ExitAction::Deregister);
        crate::lock(&f.backend.inner.state).backend = BackendState::Unopened;
        let directory = f
            .data
            .join("terminal-management")
            .join(completed.operation_id.to_string());
        let publication = fs::read(directory.join("complete.json")).unwrap();
        fs::write(directory.join(marker), b"{}").unwrap();
        assert!(f.backend.inner.recover_terminal().is_err(), "{marker}");
        assert_eq!(fs::read(directory.join(marker)).unwrap(), b"{}");
        assert_eq!(
            fs::read(directory.join("complete.json")).unwrap(),
            publication
        );
        assert!(matches!(
            crate::lock(&f.backend.inner.state).backend,
            BackendState::Unopened
        ));
        assert!(f.backend.inner.backend().is_err());
    }
}

#[test]
fn final_voter_deregistration_enters_real_management_and_keeps_data_only_registrations() {
    let f = fixture_with_metadata_members(1);
    let status = terminal_commit(&f, ExitAction::Deregister);
    assert_eq!(status.state, ExitState::Completed);
    assert_eq!(
        status.result.as_ref().unwrap().result["authority_kind"],
        "terminal-management"
    );
    assert_eq!(
        f.backend.management_status().state(),
        StorageState::TerminalManagement
    );
    assert!(f.backend.inner.backend().is_err());
    let registrations = f.backend.disk_status(&f.identity).unwrap();
    assert_eq!(registrations.len(), 2);
    assert!(
        registrations
            .iter()
            .all(|disk| !disk.online && !disk.metadata_member)
    );
    assert!(
        registrations
            .iter()
            .all(|disk| disk.disk_id != f.original.disks[0].disk_id)
    );
    let observed = f
        .backend
        .inner
        .terminal_storage_status(&f.identity)
        .unwrap()
        .unwrap();
    let terminal = observed.terminal.unwrap();
    assert_eq!(
        terminal.registrations,
        storage_contract::storage_lifecycle::TerminalRegistrations::RetainedRegistrations
    );
    assert_eq!(terminal.registered_directories, 2);
    assert_eq!(
        (
            terminal.original_metadata_members,
            terminal.original_metadata_quorum
        ),
        (1, 1)
    );
    let repeated: ExitStatus = json_codec::from_value(
        command(
            &f,
            "disk_exit_commit",
            json!(ExitCommit {
                preview: status.preview.clone()
            }),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(repeated.result, status.result);
    assert!(
        f.backend
            .setup_operation("storage_activate", json!({}), None, Some(&f.identity))
            .is_err()
    );
}

#[test]
fn final_voter_waits_for_real_backend_owners_and_keeps_the_original_operation() {
    let f = fixture_with_metadata_members(1);
    let held = f.backend.inner.addition_backend().unwrap();
    let preview: ExitPreview = json_codec::from_value(
        command(
            &f,
            "disk_exit_preview",
            json!(request(&f, ExitAction::PlannedOffline)),
        )
        .unwrap(),
    )
    .unwrap();
    let operation = preview.request.operation_id;
    assert_eq!(
        command(
            &f,
            "disk_exit_commit",
            json!(ExitCommit {
                preview: preview.clone()
            })
        )
        .unwrap_err()
        .code,
        ErrorCode::RecoveryRequired
    );
    assert!(matches!(
        crate::lock(&f.backend.inner.state).backend,
        BackendState::Draining(_)
    ));
    assert!(f.backend.inner.backend().is_err());
    let saved = read_record(
        &open(&f.backend.inner.management_root).unwrap().unwrap(),
        operation,
    )
    .unwrap()
    .unwrap();
    assert_eq!(saved.status.operation_id, operation);
    assert!(saved.status.result.is_none());
    drop(held);
    let completed: ExitStatus = json_codec::from_value(resume(&f, &saved).unwrap()).unwrap();
    assert_eq!(completed.state, ExitState::Completed);
    assert_eq!(completed.operation_id, operation);
    assert_eq!(f.backend.disk_status(&f.identity).unwrap().len(), 3);
}

#[test]
fn final_voter_resume_revalidates_revocation_and_cold_open_never_probes_stopped_root() {
    let f = fixture_with_metadata_members(1);
    let held = f.backend.inner.addition_backend().unwrap();
    let preview: ExitPreview = json_codec::from_value(
        command(
            &f,
            "disk_exit_preview",
            json!(request(&f, ExitAction::Deregister)),
        )
        .unwrap(),
    )
    .unwrap();
    command(
        &f,
        "disk_exit_commit",
        json!(ExitCommit {
            preview: preview.clone()
        }),
    )
    .unwrap_err();
    let directory = open(&f.backend.inner.management_root).unwrap().unwrap();
    let mut saved = read_record(&directory, preview.request.operation_id)
        .unwrap()
        .unwrap();
    crate::lock(&f.auth).logout(&f.token).unwrap();
    drop(held);
    assert_eq!(
        resume(&f, &saved).unwrap_err().code,
        ErrorCode::Unauthorized
    );
    let login = crate::lock(&f.auth)
        .login("admin", "lifecycle-test-password")
        .unwrap();
    let identity = crate::lock(&f.auth).authenticate(&login.token).unwrap();
    let completed: ExitStatus = json_codec::from_value(
        f.backend
            .storage_exit(
                "disk_exit_resume",
                json!(ExitResume {
                    operation_id: preview.request.operation_id
                }),
                &identity,
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(completed.state, ExitState::Completed);
    // Model a crash before the HTTP archive binding/receipt update became durable.
    saved.archive = None;
    save(&directory, &saved).unwrap();
    crate::lock(&f.backend.inner.state).backend = BackendState::Unopened;
    let root = &f.original.disks[0].root;
    fs::rename(root, root.with_extension("retained")).unwrap();
    fs::write(
        root,
        b"not a storage directory; terminal startup must not inspect it",
    )
    .unwrap();
    assert!(f.backend.inner.recover_terminal().unwrap());
    let status: ExitStatusResponse = json_codec::from_value(
        f.backend
            .storage_exit(
                "disk_exit_status",
                json!({"operation_id":saved.status.operation_id}),
                &identity,
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(status.operation.unwrap().state, ExitState::Completed);
    assert_eq!(
        fs::read(root).unwrap(),
        b"not a storage directory; terminal startup must not inspect it"
    );
    assert_eq!(
        f.backend.disk_status(&f.identity).unwrap_err().code,
        ErrorCode::Unauthorized
    );
}
