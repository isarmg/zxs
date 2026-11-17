use super::*;
use crate::disk::DiskRoot;
use name_policy::Name;
use storage_contract::file_container::retained::RetainedName;

fn pool() -> PoolId {
    PoolId::from_random([1; 16])
}

fn disk(value: u8) -> DiskId {
    DiskId::from_random([value; 16])
}

fn operation(value: u8) -> OperationId {
    OperationId::from_random([value; 16])
}

#[test]
fn original_pool_registration_survives_rebuild_and_does_not_follow_worktree_or_slot() {
    let mut registry = RetainedDiskRegistry::new(pool());
    let first = registry.registration(disk(2), operation(10)).unwrap();
    assert_eq!(first.number.value(), 0);
    registry.observe(operation(10), &first).unwrap();
    // The first native decision reserves its number even before a worktree
    // completion. A later instance on that same disk repeats the original fact.
    let same_disk = registry.registration(disk(2), operation(11)).unwrap();
    assert_eq!(same_disk, first);
    registry.observe(operation(11), &same_disk).unwrap();
    let second = registry.registration(disk(3), operation(12)).unwrap();
    assert_eq!(second.number.value(), 1);
    registry.observe(operation(12), &second).unwrap();
    let mut reopened = RetainedDiskRegistry::new(pool());
    for (original, record) in [
        (operation(10), &first),
        (operation(11), &same_disk),
        (operation(12), &second),
    ] {
        reopened.observe(original, record).unwrap();
    }
    assert_eq!(
        reopened.registration(disk(2), operation(13)).unwrap(),
        first
    );
    assert_eq!(
        reopened.registration(disk(3), operation(14)).unwrap(),
        second
    );
    let forged = RetainedDiskRegistration {
        operation: operation(20),
        ..first.clone()
    };
    assert!(reopened.observe(operation(20), &forged).is_err());
    let missing_original = RetainedDiskRegistry::new(pool());
    assert_ne!(
        missing_original
            .registration(disk(3), operation(12))
            .unwrap(),
        second
    );
}

#[test]
fn actual_held_parent_name_allocation_keeps_full_name_and_full_utc_record() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let parent = DiskRoot::open(directory.path()).unwrap();
    let mut registry = RetainedDiskRegistry::new(pool());
    let original = Name::parse("same.txt").unwrap();
    let first = allocate_file(
        &registry,
        &parent,
        disk(2),
        operation(10),
        &original,
        std::iter::empty(),
        16,
    )
    .unwrap();
    registry
        .observe(operation(10), &first.registration)
        .unwrap();
    registry
        .require_allocation(&first, &original, RetainedKind::File)
        .unwrap();
    let second = allocate_file(
        &registry,
        &parent,
        disk(2),
        operation(11),
        &original,
        [&first.name],
        16,
    )
    .unwrap();
    assert_eq!(second.registration, first.registration);
    assert_ne!(second.name, first.name);
    assert_eq!(second.name.original_name(), original.as_str());
    registry
        .require_allocation(&second, &original, RetainedKind::File)
        .unwrap();
    let bytes = storage_contract::canonical_json(&first).unwrap();
    let reloaded = storage_contract::decode_strict::<RetainedAllocation>(&bytes).unwrap();
    registry
        .require_allocation(&reloaded, &original, RetainedKind::File)
        .unwrap();
    assert_eq!(storage_contract::canonical_json(&reloaded).unwrap(), bytes);
    let allowed =
        crate::name_budget::NameBudget::from_maximum(parent.component_name_maximum().unwrap())
            .unwrap()
            .allowed();
    let long = Name::parse(&"x".repeat(allowed.min(240))).unwrap();
    let allocated = allocate_file(
        &registry,
        &parent,
        disk(2),
        operation(12),
        &long,
        std::iter::empty(),
        16,
    )
    .unwrap();
    assert_eq!(allocated.name.as_str().len(), long.as_str().len() + 16);
    assert_eq!(
        RetainedName::parse(allocated.name.as_str()).unwrap(),
        allocated.name
    );
    let different_time = RetainedAllocation {
        created_at: Timestamp::from_unix_seconds(0),
        ..allocated
    };
    assert!(
        registry
            .require_allocation(&different_time, &long, RetainedKind::File)
            .is_err()
    );
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}
