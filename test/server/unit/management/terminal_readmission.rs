//! Source regressions cross the real final-voter transition and retained root.
use super::*;
use platform_io::fs_safety::{
    EntryName,
    linux::{MountPolicy, OpenAt2Root},
};
use storage_contract::{OperationId, storage_lifecycle::*};
use storage_engine::PendingMemberDirectory;

struct Fixture {
    backend: Arc<ManagementBackend>,
    identity: Identity,
    token: String,
    auth: Arc<Mutex<AuthStore>>,
    original: Config,
    historical_operation: OperationId,
    _root: tempfile::TempDir,
}

fn fixture() -> Fixture {
    let root = crate::tests::private_tempdir();
    let data = root.path().join("system");
    let mut original =
        Config::development((0..3).map(|index| root.path().join(format!("disk-{index}")))).unwrap();
    original.disks[2].metadata_member = false;
    let mut engine = storage_engine::Engine::initialize(&data, original.clone()).unwrap();
    let historical_operation = OperationId::generate().unwrap();
    engine
        .create_repository(
            &storage_contract::RepositoryName::parse("retained-history").unwrap(),
            &original.disks[0].slot_id,
            1,
            "admin",
            historical_operation,
        )
        .unwrap();
    let management = data.join("management");
    drop(
        AuthStore::initialize_management(&management, "admin", "terminal-readmit-password")
            .unwrap(),
    );
    let (backend, mut auth) = open_service(&data, &management).unwrap();
    let login = auth.login("admin", "terminal-readmit-password").unwrap();
    let identity = auth.authenticate(&login.token).unwrap();
    let engine = Arc::new(EngineBackend::new(engine));
    engine.set_management_auth(auth.reader()).unwrap();
    let auth = Arc::new(Mutex::new(auth));
    *crate::lock(&backend.inner.auth) = Some(Arc::clone(&auth));
    backend.inner.set(Some(engine), StorageState::Ready);
    let fixture = Fixture {
        backend,
        identity,
        token: login.token,
        auth,
        original,
        historical_operation,
        _root: root,
    };
    // The first voter is already metadata_member=false in the final historical
    // configuration. Its original role must come from authenticated stop history.
    for index in [0, 2, 1] {
        disable(&fixture, index);
    }
    assert!(matches!(
        crate::lock(&fixture.backend.inner.state).backend,
        BackendState::Terminal(_)
    ));
    fixture
}

fn disable(f: &Fixture, index: usize) {
    let disk = &f.original.disks[index];
    let request = ExitRequest {
        operation_id: OperationId::generate().unwrap(),
        slot_id: disk.slot_id.clone(),
        expected_disk_id: disk.disk_id.clone(),
        action: ExitAction::PlannedOffline,
    };
    let preview: ExitPreview = json_codec::from_value(
        f.backend
            .storage_exit("disk_exit_preview", json!(request), &f.identity)
            .unwrap(),
    )
    .unwrap();
    let status: ExitStatus = json_codec::from_value(
        f.backend
            .storage_exit(
                "disk_exit_commit",
                json!(ExitCommit { preview }),
                &f.identity,
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(status.state, ExitState::Completed);
}

fn request(f: &Fixture, index: usize) -> ReadmissionRequest {
    ReadmissionRequest {
        operation_id: OperationId::generate().unwrap(),
        slot_id: f.original.disks[index].slot_id.clone(),
        expected_disk_id: f.original.disks[index].disk_id.clone(),
    }
}
fn command(f: &Fixture, action: &'static str, value: Value) -> Result<Value, ApiError> {
    f.backend.storage_readmission(action, value, &f.identity)
}
fn read_preview(f: &Fixture) -> ReadmissionPreview {
    json_codec::from_value(command(f, "disk_readmit_preview", json!(request(f, 0))).unwrap())
        .unwrap()
}
fn record(f: &Fixture) -> StoredReadmission {
    let terminal = f.backend.inner.terminal(&f.identity).unwrap().unwrap();
    let (preview, original) =
        super::preview(&terminal, &f.identity.principal, request(f, 0)).unwrap();
    StoredReadmission::prepared(preview, original)
}
fn directory(f: &Fixture) -> PrivateDirectory {
    PrivateDirectory::open_existing(&f.backend.inner.management_root)
        .unwrap()
        .create_child(&EntryName::new("storage-readmissions").unwrap())
        .unwrap()
}
fn take_terminal(f: &Fixture) -> TerminalManagement {
    let previous = std::mem::replace(
        &mut crate::lock(&f.backend.inner.state).backend,
        BackendState::ChangingAuthority,
    );
    let BackendState::Terminal(owner) = previous else {
        panic!("expected terminal owner")
    };
    match Arc::try_unwrap(owner) {
        Ok(terminal) => terminal,
        Err(_) => panic!("unexpected retained management reader"),
    }
}
fn hold_pending(f: &Fixture, intent: &mut StoredReadmission) {
    let held = crate::engine_backend::storage_readmission::open_target(intent).unwrap();
    let pending = take_terminal(f)
        .prepare_disk_readmission(
            &intent.status.preview.target.disk_id,
            &f.identity.principal,
            intent.status.operation_id,
            held,
        )
        .unwrap();
    let binding = pending_binding(&pending, intent).unwrap();
    bind_archive(intent, binding).unwrap();
    let directory = directory(f);
    save(&directory, intent).unwrap();
    write(&directory, &latest_name(), &intent.status.operation_id).unwrap();
    f.backend.inner.restore_readmission(pending);
}
fn resume(f: &Fixture, operation_id: OperationId) -> ReadmissionStatus {
    json_codec::from_value(
        command(
            f,
            "disk_readmit_resume",
            json!(ReadmissionResume { operation_id }),
        )
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn terminal_restores_the_earlier_disabled_role_without_opening_other_stopped_roots() {
    let f = fixture();
    let intent = record(&f);
    assert!(!intent.original.disks[0].metadata_member);
    assert!(intent.status.preview.metadata_member);
    assert!(intent.status.preview.authority.is_terminal());
    let snapshot: RegistrationSnapshot =
        json_codec::from_value(command(&f, "disk_registrations", json!({})).unwrap()).unwrap();
    let RegistrationSnapshot::TerminalManagement { registrations } = snapshot else {
        panic!("expected terminal registry")
    };
    assert!(!registrations[0].metadata_member);
    assert_eq!(
        registrations
            .iter()
            .find(|row| row.disk_id == intent.status.preview.target.disk_id)
            .unwrap()
            .readmission,
        Some(ReadmissionRole::MetadataMember)
    );
    for index in [1, 2] {
        let root = &f.original.disks[index].root;
        fs::rename(root, root.with_extension("stays-closed")).unwrap();
    }
    let submitted = json!(ReadmissionCommit {
        preview: intent.status.preview.clone()
    });
    let completed: ReadmissionStatus =
        json_codec::from_value(command(&f, "disk_readmit_commit", submitted.clone()).unwrap())
            .unwrap();
    assert_eq!(completed.state, ReadmissionState::Completed);
    let receipt = completed.result.as_ref().unwrap();
    assert_eq!(receipt.kind, "disk-readmit");
    assert_eq!(receipt.result["authority_kind"], "terminal-metadata-resume");
    assert_eq!(
        receipt.result["terminal_authority"],
        completed.preview.authority.hash()
    );
    assert_eq!(receipt.result["io_admission_epoch"], receipt.request_digest);
    f.backend
        .inner
        .addition_backend()
        .unwrap()
        .test_with_engine_locked(|engine| {
            let disk = engine
                .registered_disk(&completed.preview.target.disk_id)
                .unwrap();
            assert_eq!(
                disk.registration_mode,
                storage_engine::RegistrationMode::Enabled
            );
            assert_eq!(
                disk.identity().binding_generation,
                completed.preview.target.binding_generation
            );
            assert_eq!(disk.slot_id, completed.preview.target.slot_id);
            assert!(disk.metadata_member);
            assert!(
                engine
                    .state()
                    .operations
                    .contains_key(&f.historical_operation)
            );
            assert_eq!(engine.registered_disks().len(), 3);
            for index in [1, 2] {
                assert_eq!(
                    engine
                        .registered_disk(&f.original.disks[index].disk_id)
                        .unwrap()
                        .registration_mode,
                    storage_engine::RegistrationMode::Disabled
                );
            }
        });
    assert_eq!(
        command(&f, "disk_readmit_commit", submitted).unwrap(),
        json!(completed)
    );
}

#[test]
fn terminal_preview_status_and_registry_do_not_probe_an_unavailable_target() {
    let f = fixture();
    let root = &f.original.disks[0].root;
    let moved = root.with_extension("unavailable");
    fs::rename(root, &moved).unwrap();
    let preview = read_preview(&f);
    command(&f, "disk_registrations", json!({})).unwrap();
    assert_eq!(
        command(&f, "disk_readmit_status", json!({"operation_id":null})).unwrap(),
        json!(ReadmissionStatusResponse { operation: None })
    );
    assert_eq!(
        command(
            &f,
            "disk_readmit_commit",
            json!(ReadmissionCommit {
                preview: preview.clone()
            })
        )
        .unwrap_err()
        .code,
        ErrorCode::RecoveryRequired
    );
    let status: ReadmissionStatusResponse = json_codec::from_value(
        command(
            &f,
            "disk_readmit_status",
            json!({"operation_id":preview.request.operation_id}),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        status.operation.unwrap().state,
        ReadmissionState::RecoveryRequired
    );
    assert!(!root.exists());
    fs::rename(moved, root).unwrap();
    f.backend.inner.set(None, StorageState::RecoveryRequired);
    f.backend.inner.recover_once().unwrap();
    assert_eq!(
        resume(&f, preview.request.operation_id).state,
        ReadmissionState::Completed
    );
}

#[test]
fn every_terminal_preview_binding_and_revocation_are_checked_before_target_io() {
    let f = fixture();
    let original = read_preview(&f);
    let root = &f.original.disks[0].root;
    fs::rename(root, root.with_extension("must-not-open")).unwrap();
    for index in 0..9 {
        let mut stale = original.clone();
        match index {
            0 => {
                stale.authority = ReadmissionAuthority::ActiveMembership {
                    membership_authority_hash: stale.authority.hash().into(),
                }
            }
            1 => {
                if let ReadmissionAuthority::TerminalManagement {
                    terminal_authority_hash,
                    ..
                } = &mut stale.authority
                {
                    *terminal_authority_hash = "f".repeat(64);
                }
            }
            2 => stale.registry_generation += 1,
            3 => stale.registry_root_hash = "0".repeat(64),
            4 => stale.target.binding_generation += 1,
            5 => stale.principal = "another-actor".into(),
            6 => stale.source_sequence += 1,
            7 => stale.snapshot_hash = "0".repeat(64),
            _ => stale.configuration_digest = "0".repeat(64),
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
        assert!(matches!(
            crate::lock(&f.backend.inner.state).backend,
            BackendState::Terminal(_)
        ));
    }
    crate::lock(&f.auth).logout(&f.token).unwrap();
    assert_eq!(
        command(
            &f,
            "disk_readmit_commit",
            json!(ReadmissionCommit { preview: original })
        )
        .unwrap_err()
        .code,
        ErrorCode::Unauthorized
    );
    assert!(!root.exists());
}

#[test]
fn pending_actor_and_operation_mismatch_preserve_the_live_system_owner() {
    let f = fixture();
    let mut intent = record(&f);
    hold_pending(&f, &mut intent);
    for actor in [false, true] {
        let mut wrong = intent.clone();
        if actor {
            wrong.principal = "another-actor".into();
        } else {
            wrong.status.operation_id = OperationId::generate().unwrap();
        }
        assert_eq!(
            f.backend
                .inner
                .verify_terminal_readmission_owner(&f.identity, &wrong)
                .unwrap_err()
                .code,
            ErrorCode::IdempotencyKeyReused
        );
        let state = crate::lock(&f.backend.inner.state);
        let BackendState::MetadataResume(pending) = &state.backend else {
            panic!("pending owner was lost")
        };
        assert_eq!(pending.operation_id(), intent.status.operation_id);
        assert_eq!(pending.principal(), f.identity.principal);
    }
    for field in 0..4 {
        let mut wrong = intent.clone();
        match field {
            0 => wrong.status.preview.registry_generation += 1,
            1 => wrong.status.preview.source_sequence += 1,
            2 => wrong.status.preview.snapshot_hash = "0".repeat(64),
            _ => wrong.status.preview.target.root_path.push_str("-replaced"),
        }
        assert_eq!(
            f.backend
                .inner
                .verify_terminal_readmission_owner(&f.identity, &wrong)
                .unwrap_err()
                .code,
            ErrorCode::GenerationMismatch
        );
        assert!(matches!(
            crate::lock(&f.backend.inner.state).backend,
            BackendState::MetadataResume(_)
        ));
    }
    // This must consume the held pending owner, not open a second SYSTEM writer.
    assert_eq!(
        resume(&f, intent.status.operation_id).state,
        ReadmissionState::Completed
    );
}

#[test]
fn cold_retained_resume_routes_from_system_proof_and_preserves_the_original_operation() {
    let f = fixture();
    let mut intent = record(&f);
    hold_pending(&f, &mut intent);
    // A crash drops the live writer. Inner::set deliberately cannot displace
    // that owner when an ordinary recovery worker races with the handover.
    let pending = {
        let mut state = crate::lock(&f.backend.inner.state);
        state.status = StorageState::RecoveryRequired;
        state.admitted = false;
        std::mem::replace(&mut state.backend, BackendState::Unopened)
    };
    assert!(matches!(&pending, BackendState::MetadataResume(_)));
    drop(pending);
    f.backend.inner.recover_once().unwrap();
    assert!(matches!(
        crate::lock(&f.backend.inner.state).backend,
        BackendState::ResumeRecovery
    ));
    let proof = TerminalManagement::metadata_resume_recovery(&f.backend.inner.data_root)
        .unwrap()
        .unwrap();
    for field in 0..4 {
        let mut wrong = intent.clone();
        match field {
            0 => wrong.status.preview.registry_generation += 1,
            1 => wrong.status.preview.source_sequence += 1,
            2 => wrong.status.preview.snapshot_hash = "0".repeat(64),
            _ => wrong.status.preview.target.root_path.push_str("-replaced"),
        }
        assert_eq!(
            recovery_binding(&proof, &wrong).unwrap_err().code,
            ErrorCode::IdempotencyKeyReused
        );
    }
    let status: ReadmissionStatusResponse = json_codec::from_value(
        command(
            &f,
            "disk_readmit_status",
            json!({"operation_id":intent.status.operation_id}),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        status.operation.unwrap().state,
        ReadmissionState::RecoveryRequired
    );
    let completed = resume(&f, intent.status.operation_id);
    assert_eq!(completed.state, ReadmissionState::Completed);
    assert_eq!(completed.preview, intent.status.preview);
}

#[test]
fn old_terminal_readmission_receipt_survives_a_later_final_voter_stop() {
    let f = fixture();
    let preview = read_preview(&f);
    let submitted = json!(ReadmissionCommit { preview });
    let completed: ReadmissionStatus =
        json_codec::from_value(command(&f, "disk_readmit_commit", submitted.clone()).unwrap())
            .unwrap();
    disable(&f, 0);
    let root = &f.original.disks[0].root;
    fs::rename(root, root.with_extension("offline-again")).unwrap();
    assert_eq!(
        command(&f, "disk_readmit_commit", submitted).unwrap(),
        json!(completed)
    );
    assert_eq!(
        command(
            &f,
            "disk_readmit_status",
            json!({"operation_id":completed.operation_id})
        )
        .unwrap(),
        json!(ReadmissionStatusResponse {
            operation: Some(completed)
        })
    );
    assert!(matches!(
        crate::lock(&f.backend.inner.state).backend,
        BackendState::Terminal(_)
    ));
    assert!(!root.exists());
}

#[test]
fn data_only_terminal_registration_is_not_silently_promoted_to_a_metadata_voter() {
    let f = fixture();
    assert_eq!(
        command(&f, "disk_readmit_preview", json!(request(&f, 2)))
            .unwrap_err()
            .code,
        ErrorCode::ConfigConflict
    );
    let snapshot: RegistrationSnapshot =
        json_codec::from_value(command(&f, "disk_registrations", json!({})).unwrap()).unwrap();
    let RegistrationSnapshot::TerminalManagement { registrations } = snapshot else {
        panic!("expected terminal registry")
    };
    let data = registrations
        .iter()
        .find(|row| row.disk_id == f.original.disks[2].disk_id)
        .unwrap();
    assert_eq!(data.mode, RegistrationMode::Disabled);
    assert_eq!(data.readmission, None);
    assert!(!data.metadata_member);
}

#[test]
fn fresh_add_pending_cannot_be_consumed_as_a_retained_readmission() {
    let f = fixture();
    let intent = record(&f);
    let root = f._root.path().join("fresh-metadata");
    fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
    let mut fresh = Config::development([root.clone()]).unwrap().disks.remove(0);
    fresh.slot_id = "N01".into();
    let mut target = intent.original.clone();
    for disk in &mut target.disks {
        disk.metadata_member = false;
    }
    target.disks.push(fresh.clone());
    let pending = take_terminal(&f)
        .prepare_metadata_resume(
            target,
            &f.identity.principal,
            intent.status.operation_id,
            vec![PendingMemberDirectory {
                disk_id: fresh.disk_id,
                directory: OpenAt2Root::open(&root, MountPolicy::SameMount)
                    .unwrap()
                    .into_directory(),
            }],
        )
        .unwrap();
    f.backend.inner.restore_readmission(pending);
    assert_eq!(
        f.backend
            .inner
            .verify_terminal_readmission_owner(&f.identity, &intent)
            .unwrap_err()
            .code,
        ErrorCode::IdempotencyKeyReused
    );
    let state = crate::lock(&f.backend.inner.state);
    let BackendState::MetadataResume(pending) = &state.backend else {
        panic!("fresh owner was consumed")
    };
    assert_eq!(
        pending.admission(),
        TerminalMetadataResumeAdmission::FreshDirectories
    );
}
