use super::{Error, MAX_ROOTS, MAX_SOURCE_CHAIN, inspect};
use storage_contract::working_tree::{
    DirectoryId, DiskRegistrationIdentity, LocalDirectoryBinding, LocalEntityIdentity,
    RootBinding, RootId, StorageLayout, ThreeRootBinding, ZxsLifecycleBindings,
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
        layout: StorageLayout::UnifiedWorkingTreeV1,
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
    let layout = a.layout;
    let shape = inspect(layout, [&c, &a, &b].into_iter()).unwrap();
    assert_eq!(shape.by_id.len(), 3);
    assert_eq!(shape.entities.len(), 18);
    assert_eq!(shape.area_ids.len(), 15);

    assert!(matches!(inspect(layout, [&a, &c].into_iter()), Err(Error::SourceChain)));
    let mut changed = c.clone();
    changed.source_chain[1].binding_generation += 1;
    assert!(matches!(inspect(layout, [&changed, &a, &b].into_iter()), Err(Error::SourceChain)));
    changed.source_chain = vec![b.registration.clone()];
    assert!(matches!(inspect(layout, [&a, &b, &changed].into_iter()), Err(Error::SourceChain)));
    changed.source_chain = vec![changed.registration.clone()];
    assert!(matches!(inspect(layout, [&changed].into_iter()), Err(Error::SourceChain)));
}

#[test]
fn physical_roots_include_all_nested_management_areas_in_the_same_identity_sets() {
    let a = disk(1, Vec::new());
    let mut b = disk(2, vec![a.registration.clone()]);
    let layout = a.layout;
    b.roots.zxs_lifecycles.deleted_data.directory_id = a.roots.common_data.directory_id;
    assert!(matches!(inspect(layout, [&b, &a].into_iter()), Err(Error::DuplicateIdentity)));

    // Distinct roots on the same filesystem are valid; aliases between them are not.
    let mut b = disk(2, vec![a.registration.clone()]);
    b.root_entity = LocalEntityIdentity::new(1, 10);
    for (offset, area) in [
        &mut b.roots.common_data, &mut b.roots.deleted_data, &mut b.roots.zxs_data,
        &mut b.roots.zxs_lifecycles.common_data, &mut b.roots.zxs_lifecycles.deleted_data,
    ].into_iter().enumerate() {
        area.entity = LocalEntityIdentity::new(1, 11 + offset as u64);
    }
    assert!(inspect(layout, [&a, &b].into_iter()).is_ok());
    b.roots.zxs_lifecycles.common_data.entity = a.root_entity;
    assert!(matches!(inspect(layout, [&b, &a].into_iter()), Err(Error::PhysicalIdentity)));
    b.roots.zxs_lifecycles.common_data.entity = LocalEntityIdentity::new(2, 99);
    assert!(matches!(inspect(layout, [&a, &b].into_iter()), Err(Error::PhysicalIdentity)));
}

#[test]
fn physical_roots_reject_incorrect_registration_and_bound_inspection() {
    let a = disk(1, Vec::new());
    let mut b = disk(2, vec![a.registration.clone()]);
    let layout = a.layout;
    b.registration.slot_id.clone_from(&a.registration.slot_id);
    assert!(matches!(inspect(layout, [&a, &b].into_iter()), Err(Error::DuplicateIdentity)));
    let mut b = disk(2, vec![a.registration.clone()]);
    b.registration.pool_id = PoolId::from_random([2; 16]).to_string();
    assert!(matches!(inspect(layout, [&a, &b].into_iter()), Err(Error::Registration)));
    b.source_chain = vec![a.registration.clone(); MAX_SOURCE_CHAIN + 1];
    assert!(matches!(inspect(layout, [&b].into_iter()), Err(Error::Budget)));
    assert!(matches!(inspect(layout, vec![&a; MAX_ROOTS + 1].into_iter()), Err(Error::Budget)));
}
