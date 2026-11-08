use super::*;
use std::{
    fs,
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
};

struct Fixture {
    temp: tempfile::TempDir,
    system: PathBuf,
    management: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let system = temp.path().join("system");
        let management = temp.path().join("management");
        for path in [&system, &management] {
            fs::DirBuilder::new().mode(0o700).create(path).unwrap();
        }
        Self {
            temp,
            system,
            management,
        }
    }
    fn open(&self) -> Result<PreparedService, Error> {
        PreparedService::open(&self.system, &self.management)
    }
    fn configuration_evidence(&self) {
        // This is deliberately invalid Engine configuration. The prerequisite
        // gate must preserve normal fault diagnosis, never claim it is healthy.
        write_private(&self.system.join("config.json"), b"damaged configuration");
    }
}
fn write_private(path: &Path, bytes: &[u8]) {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
}
fn snapshot(path: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    for entry in fs::read_dir(path).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            files.extend(snapshot(&entry.path()));
        } else {
            files.insert(entry.path(), fs::read(entry.path()).unwrap());
        }
    }
    files
}

#[test]
fn absent_roots_do_not_become_a_fresh_installation() {
    let temp = tempfile::tempdir().unwrap();
    let system = temp.path().join("system");
    let management = temp.path().join("management");
    assert!(matches!(
        PreparedService::open(&system, &management),
        Err(Error::Directory { .. })
    ));
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
}
#[test]
fn empty_private_roots_do_not_create_config_or_account() {
    let fixture = Fixture::new();
    assert!(matches!(fixture.open(), Err(Error::StorageNotPrepared)));
    assert!(snapshot(fixture.temp.path()).is_empty());
}
#[test]
fn management_only_is_not_storage_preparation() {
    let fixture = Fixture::new();
    write_private(
        &fixture.system.join(INITIALIZATION),
        &crate::setup_recovery::management_initialization_record(),
    );
    write_private(
        &fixture.system.join(AUTHORITY),
        crate::management::MANAGEMENT_AUTHORITY_BYTES,
    );
    let before = snapshot(fixture.temp.path());
    assert!(matches!(fixture.open(), Err(Error::StorageNotPrepared)));
    assert_eq!(snapshot(fixture.temp.path()), before);
}
#[test]
fn established_configuration_fault_keeps_existing_diagnostic_owner() {
    let fixture = Fixture::new();
    fixture.configuration_evidence();
    let before = snapshot(fixture.temp.path());
    let prepared = fixture.open().unwrap();
    prepared.recheck().unwrap();
    assert!(matches!(
        prepared.require_current_authority(),
        Err(Error::InitialAuthority)
    ));
    assert_eq!(snapshot(fixture.temp.path()), before);
}
#[test]
fn current_initial_management_requires_its_exact_authority() {
    for authority in [None, Some(b"unexpected".as_slice())] {
        let fixture = Fixture::new();
        fixture.configuration_evidence();
        write_private(
            &fixture.system.join(INITIALIZATION),
            &crate::setup_recovery::management_initialization_record(),
        );
        if let Some(bytes) = authority {
            write_private(&fixture.system.join(AUTHORITY), bytes);
        }
        let before = snapshot(fixture.temp.path());
        assert!(matches!(fixture.open(), Err(Error::InitialAuthority)));
        assert_eq!(snapshot(fixture.temp.path()), before);
    }
}
#[test]
fn malformed_initial_intent_is_not_an_established_pool() {
    let fixture = Fixture::new();
    fixture.configuration_evidence();
    write_private(&fixture.system.join(INITIALIZATION), b"{}");
    write_private(
        &fixture.system.join(AUTHORITY),
        crate::management::MANAGEMENT_AUTHORITY_BYTES,
    );
    assert!(matches!(fixture.open(), Err(Error::InitialAuthority)));
}
#[test]
fn configuration_link_and_unsafe_private_mode_are_not_repaired() {
    let fixture = Fixture::new();
    std::os::unix::fs::symlink("missing", fixture.system.join("config.json")).unwrap();
    assert!(matches!(
        fixture.open(),
        Err(Error::Evidence {
            name: "config.json",
            ..
        })
    ));
    assert!(
        fs::symlink_metadata(fixture.system.join("config.json"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    fs::remove_file(fixture.system.join("config.json")).unwrap();
    fixture.configuration_evidence();
    fs::set_permissions(&fixture.management, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(fixture.open(), Err(Error::Directory { .. })));
    assert_eq!(
        fs::metadata(&fixture.management)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
}
#[test]
fn captured_roots_and_authority_cannot_be_silently_replaced() {
    let fixture = Fixture::new();
    fixture.configuration_evidence();
    write_private(
        &fixture.system.join(AUTHORITY),
        crate::management::MANAGEMENT_AUTHORITY_BYTES,
    );
    let prepared = fixture.open().unwrap();
    let held = prepared.system_directory().unwrap();
    require_existing_authority(&held).unwrap();
    fs::remove_file(fixture.system.join(AUTHORITY)).unwrap();
    assert!(matches!(
        require_existing_authority(&held),
        Err(Error::InitialAuthority)
    ));
    assert!(!fixture.system.join(AUTHORITY).exists());
    fs::rename(
        &fixture.management,
        fixture.temp.path().join("moved-management"),
    )
    .unwrap();
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&fixture.management)
        .unwrap();
    assert!(matches!(prepared.recheck(), Err(Error::Directory { .. })));
    assert!(prepared.credential_directory().is_err());
    assert_eq!(fs::read_dir(&fixture.management).unwrap().count(), 0);
}

fn activate_first(fixture: &Fixture, finish: bool) -> Result<(), ApiError> {
    use crate::setup_recovery::{CompleteSetupRequest, StorageActivationRequest};
    let discovery = fixture.temp.path().join("disks");
    let disk = discovery.join("one");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&discovery)
        .unwrap();
    fs::DirBuilder::new().mode(0o700).create(&disk).unwrap();
    let setup = SetupRecovery::new(
        fixture.system.clone(),
        fixture.management.clone(),
        BTreeMap::new(),
    )
    .unwrap()
    .with_discovery_root(discovery.to_str().unwrap().parse().unwrap())
    .allow_development_mode();
    setup
        .initialize_management(
            CompleteSetupRequest {
                username: "admin".into(),
                password: "source-test-password-only".into(),
            },
            |username, password| {
                crate::auth::AuthStore::initialize_management(
                    &fixture.management,
                    username,
                    password,
                )?;
                crate::management::claim_authority(&fixture.system)
            },
        )
        .unwrap();
    let location = crate::storage_directories::browse(disk.to_str().unwrap()).unwrap();
    let selection = crate::storage_directories::inspect_within(
        discovery.to_str().unwrap(),
        &storage_contract::storage_directories::InspectRequest {
            mount_path: location.mount_path,
            root_path: disk.to_str().unwrap().into(),
        },
        &[fixture.system.clone(), fixture.management.clone()],
    )
    .unwrap();
    let identity = crate::auth::Identity {
        admission_session: None,
        smb_binding: None,
        principal: "admin".into(),
    };
    setup.activate(
        &identity,
        StorageActivationRequest {
            selections: vec![selection],
            mode: storage_contract::Mode::Development,
        },
        |_| Ok(()),
        |engine| {
            drop(engine);
            if finish {
                Ok(())
            } else {
                Err(ApiError::invalid("fixture install interrupted"))
            }
        },
    )
}
#[test]
fn durable_initial_activation_admits_without_writing_or_probing_data_disks() {
    let fixture = Fixture::new();
    activate_first(&fixture, true).unwrap();
    let disk = fixture.temp.path().join("disks/one");
    fs::rename(&disk, fixture.temp.path().join("unavailable-disk")).unwrap();
    let before = snapshot(fixture.temp.path());
    fixture.open().unwrap().recheck().unwrap();
    assert_eq!(snapshot(fixture.temp.path()), before);
    assert!(
        !disk.exists(),
        "gate must leave real disk failure for Engine, not recreate a root"
    );
}
#[test]
fn interrupted_initial_activation_is_not_bootstrapped_by_normal_entry() {
    let fixture = Fixture::new();
    assert!(activate_first(&fixture, false).is_err());
    let before = snapshot(fixture.temp.path());
    assert!(matches!(fixture.open(), Err(Error::Activation { .. })));
    assert_eq!(snapshot(fixture.temp.path()), before);
    assert!(
        !fixture
            .system
            .join("storage-activation-complete.json")
            .exists()
    );
}
