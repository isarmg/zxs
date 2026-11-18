use super::{Error, MAX_ROOTS, MAX_SOURCE_CHAIN, inspect};
use storage_contract::namespace_root::{
    DirectoryId, DiskRegistrationIdentity, LocalDirectoryBinding, LocalEntityIdentity,
    RootBinding, RootId, ThreeRootBinding, ZxsLifecycleBindings,
};
use storage_contract::{DiskId, Generation, OperationId, PoolId};

fn disk(index: u8, source_chain: Vec<DiskRegistrationIdentity>) -> RootBinding {
    let area = |role: u8| {
        let mut id = [index; 16];
        id[15] = role;
        LocalDirectoryBinding {
            directory_id: DirectoryId::from_random(id),
            entity: LocalEntityIdentity::new(u64::from(index), u64::from(role) + 2),
        }
    };
    RootBinding {
        root_id: RootId::from_random([index; 16]),
        registration: DiskRegistrationIdentity {
            pool_id: PoolId::from_random([0; 16]).to_string(),
            disk_id: DiskId::from_random([index; 16]).to_string(),
            slot_id: format!("D{index}"),
            binding_generation: u64::from(index),
        },
        source_chain,
        root_entity: LocalEntityIdentity::new(u64::from(index), 1),
        roots: ThreeRootBinding {
            common_data: area(0), deleted_data: area(1), zxs_data: area(2),
            zxs_lifecycles: ZxsLifecycleBindings { common_data: area(3), deleted_data: area(4) },
        },
        operation_id: OperationId::from_random([1; 16]),
        generation: Generation::new(1),
    }
}

#[test]
fn physical_roots_close_recursive_registrations_in_any_input_order() {
    let a = disk(1, Vec::new());
    let b = disk(2, vec![a.registration.clone()]);
    let c = disk(3, vec![b.registration.clone(), a.registration.clone()]);
    let shape = inspect([&c, &a, &b].into_iter()).unwrap();
    assert_eq!(shape.by_id.len(), 3);
    assert_eq!(shape.entities.len(), 18);
    assert_eq!(shape.area_ids.len(), 15);

    assert!(matches!(inspect([&a, &c].into_iter()), Err(Error::SourceChain)));
    let mut changed = c.clone();
    changed.source_chain[1].binding_generation += 1;
    assert!(matches!(inspect([&changed, &a, &b].into_iter()), Err(Error::SourceChain)));
    changed.source_chain = vec![b.registration.clone()];
    assert!(matches!(inspect([&a, &b, &changed].into_iter()), Err(Error::SourceChain)));
    changed.source_chain = vec![changed.registration.clone()];
    assert!(matches!(inspect([&changed].into_iter()), Err(Error::SourceChain)));
}

#[test]
fn physical_roots_include_all_nested_management_areas_in_the_same_identity_sets() {
    let a = disk(1, Vec::new());
    let mut b = disk(2, vec![a.registration.clone()]);
    b.roots.zxs_lifecycles.deleted_data.directory_id = a.roots.common_data.directory_id;
    assert!(matches!(inspect([&b, &a].into_iter()), Err(Error::DuplicateIdentity)));

    // Distinct roots on the same filesystem are valid; aliases between them are not.
    let mut b = disk(2, vec![a.registration.clone()]);
    b.root_entity = LocalEntityIdentity::new(1, 10);
    for (offset, area) in [
        &mut b.roots.common_data, &mut b.roots.deleted_data, &mut b.roots.zxs_data,
        &mut b.roots.zxs_lifecycles.common_data, &mut b.roots.zxs_lifecycles.deleted_data,
    ].into_iter().enumerate() {
        area.entity = LocalEntityIdentity::new(1, 11 + offset as u64);
    }
    assert!(inspect([&a, &b].into_iter()).is_ok());
    b.roots.zxs_lifecycles.common_data.entity = a.root_entity;
    assert!(matches!(inspect([&b, &a].into_iter()), Err(Error::PhysicalIdentity)));
    b.roots.zxs_lifecycles.common_data.entity = LocalEntityIdentity::new(2, 99);
    assert!(matches!(inspect([&a, &b].into_iter()), Err(Error::PhysicalIdentity)));
}

#[test]
fn physical_roots_reject_incorrect_registration_and_bound_inspection() {
    let a = disk(1, Vec::new());
    let mut b = disk(2, vec![a.registration.clone()]);
    b.registration.slot_id.clone_from(&a.registration.slot_id);
    assert!(matches!(inspect([&a, &b].into_iter()), Err(Error::DuplicateIdentity)));
    let mut b = disk(2, vec![a.registration.clone()]);
    b.registration.pool_id = PoolId::from_random([2; 16]).to_string();
    assert!(matches!(inspect([&a, &b].into_iter()), Err(Error::Registration)));
    b.source_chain = vec![a.registration.clone(); MAX_SOURCE_CHAIN + 1];
    assert!(matches!(inspect([&b].into_iter()), Err(Error::Budget)));
    assert!(matches!(inspect(vec![&a; MAX_ROOTS + 1].into_iter()), Err(Error::Budget)));
}

#[test]
fn standalone_root_requires_real_named_areas_and_rejects_previous_shape() {
    let bound = disk(1, Vec::new());
    let value = json_codec::to_value(&bound).unwrap();
    assert!(value.get("layout").is_none() && value.get("roots").is_none());
    assert!(value.get("areas").unwrap().is_object());
    let restored: RootBinding = json_codec::from_value(value.clone()).unwrap();
    assert_eq!(bound, restored);
    let mut missing = value.clone();
    let areas = missing.as_object_mut().unwrap().remove("areas").unwrap();
    assert!(json_codec::from_value::<RootBinding>(missing.clone()).is_err());
    missing["roots"] = areas;
    assert!(json_codec::from_value::<RootBinding>(missing.clone()).is_err());
    missing["layout"] = "unified_working_tree_v1".into();
    assert!(json_codec::from_value::<RootBinding>(missing).is_err());
    let mut mixed = value;
    mixed["layout"] = "file-containers".into();
    assert!(json_codec::from_value::<RootBinding>(mixed).is_err());
}

#[test]
fn fresh_initialization_checks_every_selected_root_before_creating_any_directory() {
    use std::fs;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use name_policy::Name;
    use crate::{Config, Engine, ErrorCode, Mode};

    let temp = tempfile::tempdir().unwrap();
    let system = temp.path().join("new-system");
    let missing = temp.path().join("new-disk");
    let occupied = temp.path().join("existing");
    fs::create_dir(&occupied).unwrap();
    fs::set_permissions(&occupied, fs::Permissions::from_mode(0o700)).unwrap();
    let original = occupied.join("photo.zxs");
    fs::write(&original, b"ordinary user bytes").unwrap();
    let identity = fs::metadata(&original).unwrap().ino();
    let config = Config::independent_roots(Mode::Development, [
        (Name::parse("First").unwrap(), missing.clone()),
        (Name::parse("Second").unwrap(), occupied.clone()),
    ]).unwrap();
    let error = Engine::initialize(&system, config).err().expect("must refuse nonempty selection");
    assert_eq!(error.code(), ErrorCode::DiskAlreadyEnrolled);
    assert!(!system.exists());
    assert!(!missing.exists());
    assert_eq!(fs::read_dir(&occupied).unwrap().count(), 1);
    assert_eq!(fs::metadata(&original).unwrap().ino(), identity);
    assert_eq!(fs::read(&original).unwrap(), b"ordinary user bytes");
}

#[test]
fn fresh_initialization_rejects_overlapping_missing_system_and_storage_before_creation() {
    use crate::{Config, Engine, ErrorCode, Mode};
    use name_policy::Name;

    let temp = tempfile::tempdir().unwrap();
    let system = temp.path().join("absent");
    let config = Config::independent_roots(Mode::Development,
        [(Name::parse("Primary").unwrap(), system.join("nested"))]).unwrap();
    let error = Engine::initialize(&system, config).err().expect("must refuse overlap");
    assert_eq!(error.code(), ErrorCode::ConfigInvalid);
    assert!(!system.exists());
}

#[test]
fn dangling_system_configuration_is_occupied_before_init_or_explicit_recovery() {
    use crate::{Config, Engine, ErrorCode, Mode};
    use name_policy::Name;
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};

    let temp = tempfile::tempdir().unwrap();
    let system = temp.path().join("system");
    let missing = temp.path().join("missing-disk");
    fs::create_dir(&system).unwrap();
    fs::set_permissions(&system, fs::Permissions::from_mode(0o700)).unwrap();
    let config_path = system.join("config.json");
    let absent = temp.path().join("not-a-config");
    symlink(&absent, &config_path).unwrap();
    let config = Config::independent_roots(Mode::Development,
        [(Name::parse("Primary").unwrap(), missing.clone())]).unwrap();
    let error = Engine::initialize(&system, config).err().expect("occupied SYSTEM");
    assert_eq!(error.code(), ErrorCode::AlreadyInitialized);
    let error = Engine::restore_from_disks(&system, std::slice::from_ref(&missing))
        .err().expect("explicit restoration must not follow a dangling configuration");
    assert_eq!(error.code(), ErrorCode::AlreadyInitialized);
    assert_eq!(fs::read_link(&config_path).unwrap(), absent);
    assert_eq!(fs::read_dir(&system).unwrap().count(), 1);
    assert!(!missing.exists());
}
