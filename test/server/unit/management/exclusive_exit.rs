//! Production native-owner scenarios; source only, not executed in this change.
use super::*;
use crate::auth::AuthStore;
use std::fs;
use std::os::unix::fs::PermissionsExt;

struct Fixture {
    auth: AuthStore,
    config: Config,
    control: tempfile::TempDir,
    system: PathBuf,
    management: PathBuf,
    _temp: tempfile::TempDir,
}
impl Fixture {
    fn new() -> Self {
        let temp = crate::tests::private_tempdir();
        let system = temp.path().join("system");
        let management = temp.path().join("management");
        let mut config =
            Config::development((0..3).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
        for (index, disk) in config.disks.iter_mut().enumerate() {
            disk.metadata_member = index == 0;
            disk.level = 1;
            disk.upstream_slot_id = None;
        }
        for path in std::iter::once(&system).chain(config.disks.iter().map(|disk| &disk.root)) {
            fs::create_dir(path).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let mut engine = Engine::initialize_storage(
            &system, config.clone(), OperationId::generate().unwrap(),
        ).unwrap();
        engine
            .create_repository(
                &storage_contract::RepositoryName::parse("terminal-native").unwrap(),
                "P01",
                1,
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        drop(engine);
        let auth =
            AuthStore::initialize_management(&management, "admin", "native-exit-fixture-password")
                .unwrap();
        Self {
            auth,
            config,
            control: crate::tests::private_tempdir(),
            system,
            management,
            _temp: temp,
        }
    }
    fn owner(&self) -> Result<ExclusiveExit, ApiError> {
        let identity = self.auth.sole_identity()?;
        assert!(identity.admission_session.is_none());
        ExclusiveExit::open(
            PrivateDirectory::open_existing(self.control.path()).unwrap(),
            PrivateDirectory::open_existing(&self.system).unwrap(),
            PrivateDirectory::open_existing(&self.management).unwrap(),
            self.auth.reader(),
            identity,
        )
    }
    fn request(&self) -> ExitRequest {
        ExitRequest {
            operation_id: OperationId::generate().unwrap(),
            slot_id: self.config.disks[0].slot_id.clone(),
            expected_disk_id: self.config.disks[0].disk_id.clone(),
            action: ExitAction::Deregister,
        }
    }
    fn replace_disk_paths(&self) {
        for disk in &self.config.disks {
            fs::rename(&disk.root, disk.root.with_extension("held")).unwrap();
            fs::write(&disk.root, b"not a directory; terminal must not probe").unwrap();
        }
    }
}

#[test]
fn unique_native_final_voter_handover_and_cold_receipt_do_not_reopen_targets() {
    let fixture = Fixture::new();
    let mut owner = fixture.owner().unwrap();
    let preview = owner.preview(fixture.request()).unwrap();
    assert_eq!(preview.authority, ExitAuthority::TerminalManagement);
    let status = owner
        .commit(ExitCommit {
            preview: preview.clone(),
        })
        .unwrap();
    assert_eq!(status.state, ExitState::Completed);
    assert_eq!(
        status.result.as_ref().unwrap().result["authority_kind"],
        "terminal-management"
    );
    assert_eq!(
        status.result.as_ref().unwrap().result["target_io"],
        "closed"
    );
    assert!(matches!(owner.source, Source::Terminal(_)));
    drop(owner);
    fixture.replace_disk_paths();
    let mut cold = fixture.owner().unwrap();
    let boundary = cold.terminal_boundary().unwrap().unwrap();
    assert_eq!(boundary.operation_id, status.operation_id);
    assert!(!boundary.authority_hash.is_empty());
    assert!(!boundary.registry.root_hash().is_empty());
    assert_eq!(boundary.sequence, status.result.as_ref().unwrap().sequence);
    assert_eq!(cold.status(status.operation_id).unwrap(), status);
    assert_eq!(cold.commit(ExitCommit { preview }).unwrap(), status);
    let RegistrationSnapshot::TerminalManagement { registrations } = cold.registrations().unwrap()
    else {
        panic!("expected authenticated terminal registry");
    };
    assert_eq!(registrations.len(), 2);
    assert!(registrations.iter().all(|entry| !entry.metadata_member));
    assert_eq!(
        cold.preview(fixture.request()).unwrap_err().code,
        ErrorCode::RecoveryRequired
    );
    for disk in &fixture.config.disks {
        assert_eq!(
            fs::read(&disk.root).unwrap(),
            b"not a directory; terminal must not probe"
        );
    }
}

#[test]
fn original_terminal_interruption_resumes_same_receipt_from_system_only() {
    for phase in [
        "after_terminal_stop",
        "after_terminal_io_closed",
        "after_terminal_publication",
    ] {
        let fixture = Fixture::new();
        let mut owner = fixture.owner().unwrap();
        let preview = owner.preview(fixture.request()).unwrap();
        let Source::Active(backend) = &owner.source else {
            panic!("expected active source")
        };
        backend.test_with_engine_locked(|engine| engine.inject_fault_once(phase));
        assert_eq!(
            owner
                .commit(ExitCommit {
                    preview: preview.clone()
                })
                .unwrap_err()
                .code,
            ErrorCode::RecoveryRequired
        );
        drop(owner);
        fixture.replace_disk_paths();
        let mut cold = fixture.owner().unwrap();
        let status = cold.status(preview.request.operation_id).unwrap();
        assert_eq!(status.state, ExitState::Completed, "{phase}");
        assert_eq!(status.preview, preview);
        assert_eq!(cold.commit(ExitCommit { preview }).unwrap(), status);
    }
}

#[test]
fn corrupted_terminal_publication_is_not_an_empty_or_active_pool() {
    let fixture = Fixture::new();
    let mut owner = fixture.owner().unwrap();
    let preview = owner.preview(fixture.request()).unwrap();
    let operation = preview.request.operation_id;
    owner.commit(ExitCommit { preview }).unwrap();
    drop(owner);
    fixture.replace_disk_paths();
    let publication = fixture
        .system
        .join("terminal-management")
        .join(operation.to_string())
        .join("complete.json");
    fs::write(&publication, b"corrupt terminal publication").unwrap();
    assert!(fixture.owner().is_err());
    assert_eq!(
        fs::read(publication).unwrap(),
        b"corrupt terminal publication"
    );
}

#[test]
fn terminal_receipt_still_requires_live_primary_credentials() {
    let fixture = Fixture::new();
    let mut owner = fixture.owner().unwrap();
    let preview = owner.preview(fixture.request()).unwrap();
    let operation = preview.request.operation_id;
    owner.commit(ExitCommit { preview }).unwrap();
    drop(fixture.auth);
    assert_eq!(
        owner.status(operation).unwrap_err().code,
        ErrorCode::Unauthorized
    );
}

#[test]
fn exclusive_owner_does_not_start_planned_offline_or_stale_final_voter_intent() {
    let fixture = Fixture::new();
    let mut owner = fixture.owner().unwrap();
    let mut offline = fixture.request();
    offline.action = ExitAction::PlannedOffline;
    assert!(owner.preview(offline).is_err());
    let mut preview = owner.preview(fixture.request()).unwrap();
    let operation = preview.request.operation_id;
    preview.snapshot_hash = "0".repeat(64);
    assert_eq!(
        owner.commit(ExitCommit { preview }).unwrap_err().code,
        ErrorCode::GenerationMismatch
    );
    assert_eq!(
        owner.status(operation).unwrap_err().code,
        ErrorCode::OperationNotFound
    );
}
