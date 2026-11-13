//! Selector evidence does not claim a completed pool or a selected shared draft.
use crate::authority_selection::{FreshBootstrapError, FreshSelection, SelectionGuard};
use crate::{OperationId, ValidatedConfig};
use platform_io::fs_safety::PrivateDirectory;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

struct Fixture {
    _temp: tempfile::TempDir,
    system: PrivateDirectory,
    disks: Vec<PrivateDirectory>,
    config: ValidatedConfig,
}
fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let system = temp.path().join("system");
    fs::create_dir(&system).unwrap();
    fs::set_permissions(&system, fs::Permissions::from_mode(0o700)).unwrap();
    let paths = (0..3).map(|index| temp.path().join(format!("disk-{index}")))
        .collect::<Vec<_>>();
    for path in &paths {
        fs::create_dir(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let mut config = crate::Config::development(paths.clone()).unwrap();
    for disk in &mut config.disks {
        disk.level = 1;
        disk.upstream_slot_id = None;
    }
    Fixture {
        system: PrivateDirectory::open_existing(&system).unwrap(),
        disks: paths.iter().map(|path| PrivateDirectory::open_existing(path).unwrap()).collect(),
        config: ValidatedConfig::try_from(config).unwrap(), _temp: temp,
    }
}
fn select(f: &Fixture, operation: OperationId) -> Result<SelectionGuard, FreshBootstrapError> {
    SelectionGuard::acquire_held(f.system.try_clone().unwrap()).unwrap()
        .select_fresh_held(&f.config, operation, &f.disks)
}
fn bytes_identity(path: &std::path::Path) -> (u64, u64, u32, Vec<u8>) {
    let stat = fs::symlink_metadata(path).unwrap();
    (stat.dev(), stat.ino(), stat.mode(), fs::read(path).unwrap())
}

#[test]
fn first_write_is_only_deny_selector_and_exact_read_does_not_rewrite_it() {
    let f = fixture();
    let operation = OperationId::generate().unwrap();
    let guard = select(&f, operation).unwrap();
    let selected = guard.fresh_selection().unwrap().clone();
    let path = f.system.path().join("initialized.json");
    let before = bytes_identity(&path);
    assert_eq!(fs::read_dir(f.system.path()).unwrap().count(), 1);
    for disk in &f.disks {
        assert_eq!(fs::read_dir(disk.path()).unwrap().count(), 0);
    }
    drop(guard);
    let reopened = select(&f, operation).unwrap();
    assert_eq!(reopened.fresh_selection(), Some(&selected));
    assert!(!reopened.allows_writer_creation());
    assert!(crate::authority_selection::require_legacy(f.system.path()).is_err());
    assert_eq!(bytes_identity(&path), before);
}

#[test]
fn old_unknown_and_partial_selectors_are_preserved_not_migrated() {
    for bytes in [
        b"".as_slice(), b"{}",
        b"{\"format\":\"fresh-directory-snapshot-system-v1\"}",
        b"{\"format\":\"fresh-manual-container-system-v1\"}",
    ] {
        let f = fixture();
        let path = f.system.path().join("initialized.json");
        fs::write(&path, bytes).unwrap();
        let before = bytes_identity(&path);
        assert!(FreshSelection::read(&f.system).is_err());
        assert!(select(&f, OperationId::generate().unwrap()).is_err());
        assert_eq!(bytes_identity(&path), before);
        assert_eq!(fs::read_dir(f.system.path()).unwrap().count(), 1);
    }
}

#[test]
fn occupied_disk_and_replaced_held_root_refuse_before_selector() {
    let f = fixture();
    let path = f.disks[0].path().join("unknown");
    fs::write(&path, b"original bytes").unwrap();
    let before = bytes_identity(&path);
    assert!(matches!(select(&f, OperationId::generate().unwrap()),
        Err(FreshBootstrapError::Occupied)));
    assert_eq!(bytes_identity(&path), before);
    assert_eq!(fs::read_dir(f.system.path()).unwrap().count(), 0);
    let f = fixture();
    let original = f.disks[0].path().to_owned();
    fs::rename(&original, original.with_extension("retained")).unwrap();
    fs::create_dir(&original).unwrap();
    fs::set_permissions(&original, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(select(&f, OperationId::generate().unwrap()).is_err());
    assert_eq!(fs::read_dir(f.system.path()).unwrap().count(), 0);
    assert_eq!(fs::read_dir(&original).unwrap().count(), 0);
}

#[test]
fn topology_and_changed_original_operation_never_gain_selection_authority() {
    let mut f = fixture();
    let paths = f.disks.iter().map(|disk| disk.path().to_owned()).collect::<Vec<_>>();
    let primary = f.config.clone();
    f.config = ValidatedConfig::try_from(crate::Config::development(paths).unwrap()).unwrap();
    assert!(matches!(select(&f, OperationId::generate().unwrap()),
        Err(FreshBootstrapError::Topology)));
    assert_eq!(fs::read_dir(f.system.path()).unwrap().count(), 0);
    f.config = primary;
    let operation = OperationId::generate().unwrap();
    let guard = select(&f, operation).unwrap();
    let selected = guard.fresh_selection().unwrap().clone();
    drop(guard);
    let before = bytes_identity(&f.system.path().join("initialized.json"));
    assert!(matches!(select(&f, OperationId::generate().unwrap()),
        Err(FreshBootstrapError::Changed)));
    assert_eq!(FreshSelection::read(&f.system).unwrap(), Some(selected));
    assert_eq!(bytes_identity(&f.system.path().join("initialized.json")), before);
}
