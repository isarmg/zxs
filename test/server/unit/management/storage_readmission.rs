//! Source regressions use real lifecycle publication, not invented Disabled rows.
use super::*;
use storage_contract::storage_lifecycle::{
    ExitAction, ExitCommit, ExitRequest, ExitState, ExitStatus,
};

struct Fixture {
    backend: Arc<ManagementBackend>,
    identity: Identity,
    token: String,
    auth: Arc<Mutex<AuthStore>>,
    original: Config,
    target: usize,
    _root: tempfile::TempDir,
}
fn fixture(metadata_member: bool) -> Fixture {
    let root = crate::tests::private_tempdir();
    let data = root.path().join("system");
    let mut original =
        Config::development((0..3).map(|index| root.path().join(format!("disk-{index}")))).unwrap();
    let target = if metadata_member { 0 } else { 2 };
    original.disks[target].metadata_member = metadata_member;
    let engine = storage_engine::Engine::initialize(&data, original.clone()).unwrap();
    let management = data.join("management");
    drop(
        AuthStore::initialize_management(&management, "admin", "readmission-test-password")
            .unwrap(),
    );
    let (backend, mut auth) = open_service(&data, &management).unwrap();
    let login = auth.login("admin", "readmission-test-password").unwrap();
    let identity = auth.authenticate(&login.token).unwrap();
    let engine = Arc::new(EngineBackend::new(engine));
    engine.set_management_auth(auth.reader()).unwrap();
    let auth = Arc::new(Mutex::new(auth));
    *crate::lock(&backend.inner.auth) = Some(auth.clone());
    backend.inner.set(Some(engine), StorageState::Ready);
    let f = Fixture {
        backend,
        identity,
        token: login.token,
        auth,
        original,
        target,
        _root: root,
    };
    disable(&f);
    f
}
fn disable(f: &Fixture) -> ExitStatus {
    let target = &f.original.disks[f.target];
    let request = ExitRequest {
        operation_id: OperationId::generate().unwrap(),
        slot_id: target.slot_id.clone(),
        expected_disk_id: target.disk_id.clone(),
        action: ExitAction::PlannedOffline,
    };
    let (preview, _) = f
        .backend
        .inner
        .addition_backend()
        .unwrap()
        .preview_storage_exit(&f.identity, request)
        .unwrap();
    let result: ExitStatus = json_codec::from_value(
        f.backend
            .storage_exit(
                "disk_exit_commit",
                json!(ExitCommit { preview }),
                &f.identity,
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(result.state, ExitState::Completed);
    result
}
fn request(f: &Fixture) -> ReadmissionRequest {
    ReadmissionRequest {
        operation_id: OperationId::generate().unwrap(),
        slot_id: f.original.disks[f.target].slot_id.clone(),
        expected_disk_id: f.original.disks[f.target].disk_id.clone(),
    }
}
fn command(f: &Fixture, action: &'static str, value: Value) -> Result<Value, ApiError> {
    f.backend.storage_readmission(action, value, &f.identity)
}
fn preview(f: &Fixture) -> ReadmissionPreview {
    json_codec::from_value(command(f, "disk_readmit_preview", json!(request(f))).unwrap()).unwrap()
}
fn mode(f: &Fixture) -> storage_engine::RegistrationMode {
    f.backend
        .inner
        .addition_backend()
        .unwrap()
        .test_with_engine_locked(|engine| {
            engine
                .registered_disk(&f.original.disks[f.target].disk_id)
                .unwrap()
                .registration_mode
        })
}

#[test]
fn metadata_and_data_only_readmit_preserve_registration_identity_and_exact_receipt() {
    for metadata in [true, false] {
        let f = fixture(metadata);
        let preview = preview(&f);
        assert_eq!(preview.metadata_member, metadata);
        assert_eq!(mode(&f), storage_engine::RegistrationMode::Disabled);
        let submitted = json!(ReadmissionCommit {
            preview: preview.clone()
        });
        let status: ReadmissionStatus =
            json_codec::from_value(command(&f, "disk_readmit_commit", submitted.clone()).unwrap())
                .unwrap();
        assert_eq!(status.state, ReadmissionState::Completed);
        assert_eq!(
            status.result.as_ref().unwrap().result["io_admission_epoch"],
            status.result.as_ref().unwrap().request_digest
        );
        assert_eq!(mode(&f), storage_engine::RegistrationMode::Enabled);
        f.backend
            .inner
            .addition_backend()
            .unwrap()
            .test_with_engine_locked(|engine| {
                let disk = engine.registered_disk(&preview.target.disk_id).unwrap();
                assert_eq!(disk.slot_id, preview.target.slot_id);
                assert_eq!(disk.binding_generation, preview.target.binding_generation);
                assert_eq!(disk.metadata_member, metadata);
                assert_eq!(engine.registered_disks().len(), f.original.disks.len());
            });
        assert_eq!(
            command(&f, "disk_readmit_commit", submitted).unwrap(),
            json!(status)
        );
        assert_eq!(
            command(&f, "disk_readmit_preview", json!(request(&f)))
                .unwrap_err()
                .code,
            ErrorCode::GenerationMismatch
        );
        assert_eq!(
            command(
                &f,
                "disk_readmit_status",
                json!({"operation_id":status.operation_id})
            )
            .unwrap(),
            json!(ReadmissionStatusResponse {
                operation: Some(status)
            })
        );
    }
}

#[test]
fn preview_and_registry_read_never_open_a_disabled_root_and_failed_commit_keeps_it_disabled() {
    let f = fixture(true);
    let root = &f.original.disks[f.target].root;
    let moved = root.with_extension("unavailable");
    fs::rename(root, &moved).unwrap();
    let preview = preview(&f);
    let snapshot: RegistrationSnapshot =
        json_codec::from_value(command(&f, "disk_registrations", json!({})).unwrap()).unwrap();
    let RegistrationSnapshot::ActiveMembership { registrations } = snapshot else {
        panic!("expected active registry");
    };
    assert_eq!(
        registrations
            .iter()
            .find(|entry| entry.disk_id == preview.target.disk_id)
            .unwrap()
            .mode,
        RegistrationMode::Disabled
    );
    assert_eq!(
        command(
            &f,
            "disk_readmit_commit",
            json!(ReadmissionCommit { preview })
        )
        .unwrap_err()
        .code,
        ErrorCode::DiskUnavailable
    );
    assert_eq!(mode(&f), storage_engine::RegistrationMode::Disabled);
    assert!(!root.exists());
}

#[test]
fn stale_registry_actor_identity_and_revoked_session_cannot_touch_the_disabled_target() {
    let f = fixture(true);
    let base = preview(&f);
    for change in 0..8 {
        let mut stale = base.clone();
        match change {
            0 => stale.registry_generation += 1,
            1 => stale.target.binding_generation += 1,
            2 => stale.principal = "different-actor".into(),
            3 => {
                stale.authority = ReadmissionAuthority::ActiveMembership {
                    membership_authority_hash: "0".repeat(64),
                }
            }
            4 => stale.target.disk_id = "disk_00000000000000000000000000000000".into(),
            5 => stale.source_sequence += 1,
            6 => stale.metadata_member = !stale.metadata_member,
            _ => stale.target.root_path = "/untrusted-replacement".into(),
        }
        assert_eq!(
            command(
                &f,
                "disk_readmit_commit",
                json!(ReadmissionCommit { preview: stale })
            )
            .unwrap_err()
            .code,
            ErrorCode::GenerationMismatch
        );
        assert!(open(&f.backend.inner.management_root).unwrap().is_some());
        assert_eq!(mode(&f), storage_engine::RegistrationMode::Disabled);
    }
    crate::lock(&f.auth).logout(&f.token).unwrap();
    assert_eq!(
        command(
            &f,
            "disk_readmit_commit",
            json!(ReadmissionCommit { preview: base })
        )
        .unwrap_err()
        .code,
        ErrorCode::Unauthorized
    );
}

#[test]
fn unpublished_readmission_retains_original_operation_after_engine_reopen() {
    let f = fixture(true);
    let (preview, original) = f
        .backend
        .inner
        .addition_backend()
        .unwrap()
        .preview_storage_readmission(&f.identity, request(&f))
        .unwrap();
    let directory = PrivateDirectory::open_existing(&f.backend.inner.management_root)
        .unwrap()
        .create_child(&EntryName::new(DIRECTORY).unwrap())
        .unwrap();
    let mut record = StoredReadmission {
        format: Format::Readmission,
        principal: f.identity.principal.clone(),
        original,
        archive: None,
        status: ReadmissionStatus {
            operation_id: preview.request.operation_id,
            state: ReadmissionState::Running,
            phase: ReadmissionPhase::Prepared,
            preview,
            failure: None,
            result: None,
        },
    };
    let intent = record.clone();
    let engine = f.backend.inner.addition_backend().unwrap();
    engine
        .run_storage_readmission(&f.identity, &intent, |phase, archive| {
            record.status.phase = phase;
            if let Some(archive) = archive {
                record.archive = Some(archive);
            }
            save(&directory, &record)?;
            write(&directory, &latest_name(), &record.status.operation_id)?;
            if phase == ReadmissionPhase::Archiving {
                Err(ApiError::conflict(
                    ErrorCode::RecoveryRequired,
                    "pause the original operation after fence",
                ))
            } else {
                Ok(())
            }
        })
        .unwrap_err();
    record.status.state = ReadmissionState::RecoveryRequired;
    save(&directory, &record).unwrap();
    assert_eq!(mode(&f), storage_engine::RegistrationMode::Disabled);
    assert_eq!(
        command(&f, "disk_readmit_preview", json!(request(&f)))
            .unwrap_err()
            .code,
        ErrorCode::RecoveryRequired
    );
    drop(engine);
    f.backend.inner.set(None, StorageState::RecoveryRequired);
    let status: ReadmissionStatus = json_codec::from_value(
        command(
            &f,
            "disk_readmit_resume",
            json!(ReadmissionResume {
                operation_id: record.status.operation_id,
            }),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(status.operation_id, record.status.operation_id);
    assert_eq!(status.state, ReadmissionState::Completed);
    assert_eq!(mode(&f), storage_engine::RegistrationMode::Enabled);
}

#[test]
fn common_lifecycle_gate_makes_registration_unknown_without_starting_a_second_operation() {
    let f = fixture(true);
    let preview = preview(&f);
    let _gate = crate::lock(&f.backend.inner.storage_addition);
    assert_eq!(
        command(&f, "disk_registrations", json!({})).unwrap(),
        json!(RegistrationSnapshot::Unadmitted)
    );
    assert_eq!(
        command(
            &f,
            "disk_readmit_commit",
            json!(ReadmissionCommit { preview })
        )
        .unwrap_err()
        .code,
        ErrorCode::StorageRetryRunning
    );
}

#[test]
fn an_older_readmission_receipt_survives_a_later_disable() {
    let f = fixture(true);
    let preview = preview(&f);
    let submitted = json!(ReadmissionCommit {
        preview: preview.clone()
    });
    let done: ReadmissionStatus =
        json_codec::from_value(command(&f, "disk_readmit_commit", submitted.clone()).unwrap())
            .unwrap();
    disable(&f);
    assert_eq!(mode(&f), storage_engine::RegistrationMode::Disabled);
    assert_eq!(
        command(&f, "disk_readmit_commit", submitted).unwrap(),
        json!(done)
    );
    assert_eq!(mode(&f), storage_engine::RegistrationMode::Disabled);
    let mut reused = preview;
    reused.target.binding_generation += 1;
    assert_eq!(
        command(
            &f,
            "disk_readmit_commit",
            json!(ReadmissionCommit { preview: reused })
        )
        .unwrap_err()
        .code,
        ErrorCode::IdempotencyKeyReused
    );
}

#[test]
fn crash_after_new_intent_does_not_resume_an_older_completed_recovery_plan() {
    let f = fixture(true);
    let first = preview(&f);
    command(
        &f,
        "disk_readmit_commit",
        json!(ReadmissionCommit { preview: first }),
    )
    .unwrap();
    disable(&f);
    let (preview, original) = f
        .backend
        .inner
        .addition_backend()
        .unwrap()
        .preview_storage_readmission(&f.identity, request(&f))
        .unwrap();
    let directory = PrivateDirectory::open_existing(&f.backend.inner.management_root)
        .unwrap()
        .create_child(&EntryName::new(DIRECTORY).unwrap())
        .unwrap();
    let record = StoredReadmission {
        format: Format::Readmission,
        principal: f.identity.principal.clone(),
        original,
        archive: None,
        status: ReadmissionStatus {
            operation_id: preview.request.operation_id,
            state: ReadmissionState::Running,
            phase: ReadmissionPhase::Fencing,
            preview,
            failure: None,
            result: None,
        },
    };
    // This is the exact production interval after durable intent and before the
    // core begin call has had any opportunity to retire an old recovery plan.
    save(&directory, &record).unwrap();
    write(&directory, &latest_name(), &record.status.operation_id).unwrap();
    f.backend.inner.set(None, StorageState::RecoveryRequired);
    let done: ReadmissionStatus = json_codec::from_value(
        command(
            &f,
            "disk_readmit_resume",
            json!(ReadmissionResume {
                operation_id: record.status.operation_id,
            }),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(done.operation_id, record.status.operation_id);
    assert_eq!(done.preview, record.status.preview);
    assert_eq!(done.state, ReadmissionState::Completed);
}

#[test]
fn detached_registration_rejects_new_and_stale_readmission_without_opening_target() {
    let f = fixture(true);
    let stale = preview(&f);
    let target = &f.original.disks[f.target];
    let exit_request = ExitRequest {
        operation_id: OperationId::generate().unwrap(),
        slot_id: target.slot_id.clone(),
        expected_disk_id: target.disk_id.clone(),
        action: ExitAction::Deregister,
    };
    let (exit_preview, _) = f
        .backend
        .inner
        .addition_backend()
        .unwrap()
        .preview_storage_exit(&f.identity, exit_request)
        .unwrap();
    f.backend
        .storage_exit(
            "disk_exit_commit",
            json!(ExitCommit {
                preview: exit_preview
            }),
            &f.identity,
        )
        .unwrap();
    let moved = target.root.with_extension("detached");
    fs::rename(&target.root, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &target.root).unwrap();
    for (action, input) in [
        ("disk_readmit_preview", json!(request(&f))),
        (
            "disk_readmit_commit",
            json!(ReadmissionCommit { preview: stale }),
        ),
    ] {
        assert_eq!(
            command(&f, action, input).unwrap_err().code,
            ErrorCode::GenerationMismatch
        );
    }
    let snapshot: RegistrationSnapshot =
        json_codec::from_value(command(&f, "disk_registrations", json!({})).unwrap()).unwrap();
    let RegistrationSnapshot::ActiveMembership { registrations } = snapshot else {
        panic!("expected active registry");
    };
    assert!(
        registrations
            .iter()
            .all(|entry| entry.disk_id != target.disk_id)
    );
    f.backend
        .inner
        .addition_backend()
        .unwrap()
        .test_with_engine_locked(|engine| {
            assert!(engine.registered_disk(&target.disk_id).is_none());
            assert!(
                engine
                    .config()
                    .disks
                    .iter()
                    .any(|disk| disk.disk_id == target.disk_id)
            );
        });
    assert!(target.root.symlink_metadata().unwrap().is_symlink());
    assert!(moved.join("disk.json").is_file());
}

#[test]
fn original_core_actor_is_checked_before_reopening_a_saved_readmission_target() {
    let f = fixture(true);
    let backend = f.backend.inner.addition_backend().unwrap();
    let (preview, original) = backend
        .preview_storage_readmission(&f.identity, request(&f))
        .unwrap();
    let record = StoredReadmission::prepared(preview, original);
    let directory = crate::engine_backend::storage_readmission::open_target(&record).unwrap();
    backend.test_with_engine_locked(|engine| {
        engine
            .begin_disk_readmission(
                &record.status.preview.target.disk_id,
                "different-original-actor",
                record.status.operation_id,
                directory,
            )
            .unwrap();
    });
    let target = &f.original.disks[f.target].root;
    let moved = target.with_extension("unavailable");
    fs::rename(target, &moved).unwrap();
    let mut progressed = false;
    let error = backend
        .run_storage_readmission(&f.identity, &record, |_, _| {
            progressed = true;
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::IdempotencyKeyReused);
    assert!(!progressed);
    assert!(!target.exists());
    assert_eq!(mode(&f), storage_engine::RegistrationMode::Disabled);
}

fn watch_readmission_roots(f: &Fixture, alternate: &Path) -> rustix::fd::OwnedFd {
    use rustix::fs::inotify;
    let watch =
        inotify::init(inotify::CreateFlags::CLOEXEC | inotify::CreateFlags::NONBLOCK).unwrap();
    for path in f
        .original
        .disks
        .iter()
        .map(|disk| disk.root.as_path())
        .chain([alternate])
    {
        inotify::add_watch(
            &watch,
            path,
            inotify::WatchFlags::OPEN | inotify::WatchFlags::ACCESS,
        )
        .unwrap();
    }
    watch
}

fn assert_no_readmission_root_open(watch: &rustix::fd::OwnedFd) {
    let mut bytes = [0u8; 4096];
    assert_eq!(
        rustix::io::read(watch, &mut bytes),
        Err(rustix::io::Errno::WOULDBLOCK)
    );
}

fn detach_readmission_fixture(f: &Fixture) {
    let target = &f.original.disks[f.target];
    let request = ExitRequest {
        operation_id: OperationId::generate().unwrap(),
        slot_id: target.slot_id.clone(),
        expected_disk_id: target.disk_id.clone(),
        action: ExitAction::Deregister,
    };
    let backend = f.backend.inner.addition_backend().unwrap();
    let (preview, _) = backend.preview_storage_exit(&f.identity, request).unwrap();
    f.backend
        .storage_exit(
            "disk_exit_commit",
            json!(ExitCommit { preview }),
            &f.identity,
        )
        .unwrap();
    backend.test_with_engine_locked(|engine| {
        assert!(engine.registered_disk(&target.disk_id).is_none());
    });
}

fn reject_rehashed_readmission_sources(
    f: &Fixture,
    directory: &PrivateDirectory,
    original: &StoredReadmission,
    alternate: &Path,
) {
    for changed in 0..8 {
        let mut forged = original.clone();
        let preview = &mut forged.status.preview;
        match changed {
            0 => {
                forged
                    .original
                    .disks
                    .iter_mut()
                    .find(|disk| disk.disk_id == preview.target.disk_id)
                    .unwrap()
                    .root = alternate.to_owned();
                preview.target.root_path = alternate.to_str().unwrap().to_owned();
            }
            1 => forged.original.max_file_bytes -= 1,
            2 => {
                preview.authority = ReadmissionAuthority::ActiveMembership {
                    membership_authority_hash: "0".repeat(64),
                }
            }
            3 => preview.registry_root_hash = "0".repeat(64),
            4 => preview.registry_generation += 1,
            5 => preview.source_sequence += 1,
            6 => preview.topology_generation += 1,
            _ => preview.snapshot_hash = "0".repeat(64),
        }
        preview.configuration_digest = configuration_digest(&forged.original);
        preview.preview_digest.clear();
        preview.preview_digest = digest(preview);
        save(directory, &forged).unwrap();
        // Every cache consistency check passes. Only the independently admitted
        // core source can reject these changes before any data-root syscall.
        assert!(
            read_record(directory, original.status.operation_id)
                .unwrap()
                .is_some()
        );
        let watch = watch_readmission_roots(f, alternate);
        assert_eq!(
            command(
                f,
                "disk_readmit_resume",
                json!(ReadmissionResume {
                    operation_id: original.status.operation_id,
                })
            )
            .unwrap_err()
            .code,
            ErrorCode::IdempotencyKeyReused,
            "source field {changed}"
        );
        assert_no_readmission_root_open(&watch);
    }
    save(directory, original).unwrap();
}

#[test]
fn rehashed_fenced_readmission_source_cannot_probe_unrelated_or_detached_roots() {
    let f = fixture(true);
    let detached = fixture(false);
    detach_readmission_fixture(&detached);
    let unrelated = crate::tests::private_tempdir();
    let backend = f.backend.inner.addition_backend().unwrap();
    let (preview, original) = backend
        .preview_storage_readmission(&f.identity, request(&f))
        .unwrap();
    let record = StoredReadmission::prepared(preview, original);
    let target = crate::engine_backend::storage_readmission::open_target(&record).unwrap();
    backend.test_with_engine_locked(|engine| {
        engine
            .begin_disk_readmission(
                &record.status.preview.target.disk_id,
                &f.identity.principal,
                record.status.operation_id,
                target,
            )
            .unwrap();
    });
    let directory = PrivateDirectory::open_existing(&f.backend.inner.management_root)
        .unwrap()
        .create_child(&EntryName::new(DIRECTORY).unwrap())
        .unwrap();
    save(&directory, &record).unwrap();
    write(&directory, &latest_name(), &record.status.operation_id).unwrap();
    assert!(record.archive.is_none());
    for alternate in [
        unrelated.path(),
        detached.original.disks[detached.target].root.as_path(),
    ] {
        reject_rehashed_readmission_sources(&f, &directory, &record, alternate);
    }
    let done: ReadmissionStatus = json_codec::from_value(
        command(
            &f,
            "disk_readmit_resume",
            json!(ReadmissionResume {
                operation_id: record.status.operation_id
            }),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(done.state, ReadmissionState::Completed);
}

#[test]
fn activating_and_cold_readmission_check_rehashed_source_before_opening_any_root() {
    for cold in [false, true] {
        let f = fixture(true);
        let detached = fixture(false);
        detach_readmission_fixture(&detached);
        let unrelated = crate::tests::private_tempdir();
        let preview = preview(&f);
        let operation = preview.request.operation_id;
        let backend = f.backend.inner.addition_backend().unwrap();
        backend.test_with_engine_locked(|engine| {
            engine.inject_fault_once(&if cold {
                "after_readmission_activation_plan".to_owned()
            } else {
                format!(
                    "after_readmission_marker_{}",
                    f.original.disks[f.target].disk_id
                )
            });
        });
        assert_eq!(
            command(
                &f,
                "disk_readmit_commit",
                json!(ReadmissionCommit { preview })
            )
            .unwrap_err()
            .code,
            ErrorCode::RecoveryRequired
        );
        let directory = open(&f.backend.inner.management_root).unwrap().unwrap();
        let record = read_record(&directory, operation).unwrap().unwrap();
        assert!(record.archive.is_some());
        if cold {
            drop(backend);
            f.backend.inner.set(None, StorageState::RecoveryRequired);
            let source =
                storage_engine::Engine::readmission_recovery_status(&f.backend.inner.data_root)
                    .unwrap()
                    .unwrap();
            for changed in 0..2 {
                let mut mismatched = source.clone();
                if changed == 0 {
                    mismatched.transition_hash = "0".repeat(64);
                } else {
                    mismatched.source_topology_generation += 1;
                }
                let watch = watch_readmission_roots(&f, unrelated.path());
                assert_eq!(
                    storage_engine::Engine::resume_disk_readmission_transition(
                        &f.backend.inner.data_root,
                        &mismatched,
                        None,
                    )
                    .err()
                    .unwrap()
                    .code(),
                    ErrorCode::IdempotencyKeyReused
                );
                assert_no_readmission_root_open(&watch);
            }
        } else {
            backend.test_with_engine_locked(|engine| {
                assert_eq!(
                    engine.disk_readmission_status().unwrap().unwrap().phase,
                    storage_engine::DiskReadmissionPhase::Activating
                );
                assert!(engine.config().disks[f.target].metadata_member);
            });
        }
        for alternate in [
            unrelated.path(),
            detached.original.disks[detached.target].root.as_path(),
        ] {
            reject_rehashed_readmission_sources(&f, &directory, &record, alternate);
        }
        let done: ReadmissionStatus = json_codec::from_value(
            command(
                &f,
                "disk_readmit_resume",
                json!(ReadmissionResume {
                    operation_id: operation
                }),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(done.state, ReadmissionState::Completed);
    }
}

#[test]
fn historical_readmission_replay_keeps_its_source_after_later_add_and_exit() {
    let f = fixture(true);
    let preview = preview(&f);
    let operation = preview.request.operation_id;
    command(
        &f,
        "disk_readmit_commit",
        json!(ReadmissionCommit { preview }),
    )
    .unwrap();
    let directory = open(&f.backend.inner.management_root).unwrap().unwrap();
    let record = read_record(&directory, operation).unwrap().unwrap();
    add_readmission_successor(&f);
    for exit in [false, true] {
        if exit {
            disable(&f);
        }
        let backend = f.backend.inner.addition_backend().unwrap();
        let target = crate::engine_backend::storage_readmission::open_target(&record).unwrap();
        backend.test_with_engine_locked(|engine| {
            let status = engine
                .begin_disk_readmission(
                    &record.status.preview.target.disk_id,
                    &f.identity.principal,
                    operation,
                    target,
                )
                .unwrap();
            assert_eq!(
                status.phase,
                storage_engine::DiskReadmissionPhase::Published
            );
            crate::engine_backend::storage_readmission::verify_status(&status, &record).unwrap();
            assert!(engine.state().topology_generation > status.source_topology_generation);
        });
    }
    assert_eq!(mode(&f), storage_engine::RegistrationMode::Disabled);
}

fn add_readmission_successor(f: &Fixture) {
    use std::os::unix::fs::DirBuilderExt;
    use storage_contract::storage_addition::{
        AdditionCommit, AdditionPreview, AdditionRequest, DirectoryAddition,
    };
    use storage_contract::storage_directories::InspectRequest;
    let discovery = f._root.path().to_str().unwrap();
    let path = f._root.path().join("later-data");
    fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
    let listing =
        crate::storage_directories::browse_within(discovery, path.to_str().unwrap()).unwrap();
    let selection = crate::storage_directories::inspect_within(
        discovery,
        &InspectRequest {
            mount_path: listing.mount_path,
            root_path: path.to_str().unwrap().into(),
        },
        std::slice::from_ref(&f.backend.inner.data_root),
    )
    .unwrap();
    let request = AdditionRequest {
        operation_id: OperationId::generate().unwrap(),
        additions: vec![DirectoryAddition {
            selection,
            slot_id: "LATER".into(),
            level: 1,
            upstream_slot_id: None,
            metadata_member: false,
        }],
    };
    let preview: AdditionPreview = json_codec::from_value(
        f.backend
            .storage_addition("disk_add_preview", json!(request), &f.identity, || {
                Ok(discovery.to_owned())
            })
            .unwrap(),
    )
    .unwrap();
    f.backend
        .storage_addition(
            "disk_add_commit",
            json!(AdditionCommit { preview }),
            &f.identity,
            || Ok(discovery.to_owned()),
        )
        .unwrap();
}
