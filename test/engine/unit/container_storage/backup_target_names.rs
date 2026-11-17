//! Prospective native Backup geometry only. The original source is not renamed
//! and proving an introduced name fits does not grant an installation.
use super::*;
use crate::disk_names::configuration::{ConfiguredDiskNameError, ConfiguredDiskNames};
use crate::{Config, Engine, NamespaceOperation, NamespaceOperationKind};
use name_policy::Name;
use std::path::PathBuf;
use storage_contract::OperationId;

fn chain(a: Name, b: Name) -> Config {
    Config::development([
        (a, PathBuf::from("/unopened-a")), (b, PathBuf::from("/unopened-b")),
        (Name::parse("C").unwrap(), PathBuf::from("/unopened-c")),
    ]).unwrap()
}

#[test]
fn native_backup_names_charge_every_source_uuid_on_the_actual_fanout_target() {
    let a = Name::parse(&"界".repeat(69)).unwrap();
    let mut config = chain(a.clone(), Name::parse("B").unwrap());
    let mut sibling = config.disks[2].clone();
    sibling.disk_id = DiskId::generate().unwrap().to_string();
    sibling.slot_id = "C02".into();
    sibling.name = Name::parse("Sibling").unwrap();
    config.disks.push(sibling.clone());
    let names = ConfiguredDiskNames::new(&config).unwrap();
    let a_id = config.disks[0].disk_id.parse::<DiskId>().unwrap();
    let b_id = config.disks[1].disk_id.parse::<DiskId>().unwrap();
    let c = config.disks[2].disk_id.parse::<DiskId>().unwrap();
    let other = sibling.disk_id.parse::<DiskId>().unwrap();
    let left = names.source_path(c).unwrap();
    let right = names.source_path(other).unwrap();
    assert_eq!(left.source_ids().collect::<Vec<_>>(), [b_id, a_id]);
    assert_eq!(right.source_ids().collect::<Vec<_>>(), [b_id, a_id]);
    let stop = AtomicBool::new(false);
    let state = State::default();
    let exact = u64::try_from(a.as_str().len() + 34 + 16).unwrap();
    require_new_names_at(exact, &left, &state, &stop).unwrap();
    let failure = require_new_names_at(exact - 1, &right, &state, &stop).unwrap_err();
    assert!(matches!(failure, crate::Error::ConfiguredDiskName(
        ConfiguredDiskNameError::ComponentBudget { target, disk_id, .. }
    ) if target == other && disk_id == a_id));

    // The entry point obtains the budget from the supplied real held root.
    let temporary = tempfile::Builder::new()
        .permissions(std::os::unix::fs::PermissionsExt::from_mode(0o700)).tempdir().unwrap();
    let root = DiskRoot::open(temporary.path()).unwrap();
    let actual = root.component_name_maximum().unwrap();
    assert_eq!(BackupTargetLayout::require_new_names(&root, &right, &state, &stop)
        .map_err(|e| e.code()), require_new_names_at(actual, &right, &state, &stop)
        .map_err(|e| e.code()));
    assert!(root.entries(root.path()).unwrap().next().is_none());
}

#[test]
fn native_backup_new_empty_directory_needs_user_reserve_even_without_version_leaves() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temp.path().join("system"),
        Config::development(crate::test_disks([temp.path().join("primary")])).unwrap(),
    ).unwrap();
    let op = || OperationId::generate().unwrap();
    let repository = engine.create_disk_root("P01", "admin", op()).unwrap();
    let leaf = "界".repeat(30);
    engine.file_namespace(
        &repository.repository_id,
        NamespaceOperation::parse(NamespaceOperationKind::Mkdir, &leaf, None).unwrap(),
        "admin", op(),
    ).unwrap();
    assert!(engine.state().versions.is_empty());
    assert!(engine.state().namespace_directories.values().any(|record| record.name.as_str() == leaf));
    let config = chain(Name::parse("A").unwrap(), Name::parse("B").unwrap());
    let names = ConfiguredDiskNames::new(&config).unwrap();
    let path = names.source_path(config.disks[2].disk_id.parse().unwrap()).unwrap();
    let stop = AtomicBool::new(false);
    let exact = u64::try_from(leaf.len() + 16).unwrap();
    require_new_names_at(exact, &path, engine.state(), &stop).unwrap();
    assert!(matches!(require_new_names_at(exact - 1, &path, engine.state(), &stop),
        Err(crate::Error::ConversionNameBudget(crate::name_budget::NameBudgetError::ComponentTooLong {
            bytes, allowed, ..
        })) if bytes == leaf.len() && allowed + 1 == bytes));
}
