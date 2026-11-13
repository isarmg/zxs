use crate::working_tree::admission::{
    AdmissionError, MAX_CATALOG_BYTES, ObservedDirectory, RootSelectionContext,
    decode_catalog, register_directory_chain, select_root, validate_catalog_structure,
};
use crate::working_tree::records::*;
use name_policy::LookupName;
use platform_io::fs_safety::{EntryName, linux::FileIdentity};
use storage_contract::working_tree::{
    DirectoryRelativePath, DirectoryTargetPath, DirectoryTargetRequest,
};
use storage_contract::{DiskId, FileId, Generation, OperationId, PoolId};

fn operation() -> OperationId {
    OperationId::from_random([3; 16])
}
fn root(index: u8, source_chain: Vec<crate::DiskRegistrationIdentity>) -> RootBinding {
    let local = |offset| LocalDirectoryBinding {
        directory_id: DirectoryId::from_random([index * 4 + offset; 16]),
        entity: LocalEntityIdentity::new(u64::from(index), u64::from(offset) + 10),
    };
    RootBinding {
        layout: StorageLayout::UnifiedWorkingTreeV1,
        root_id: RootId::from_random([index; 16]),
        registration: crate::DiskRegistrationIdentity {
            pool_id: PoolId::from_random([1; 16]).to_string(),
            disk_id: DiskId::from_random([index; 16]).to_string(),
            slot_id: format!("D{index:02}"),
            binding_generation: 1,
        },
        source_chain,
        root_entity: LocalEntityIdentity::new(u64::from(index), 1),
        roots: ThreeRootBinding {
            common_data: local(0),
            deleted_data: local(1),
            zxs_data: local(2),
        },
        operation_id: operation(),
        generation: Generation::new(1),
    }
}
fn catalog(roots: Vec<RootBinding>) -> WorkingTreeCatalog {
    WorkingTreeCatalog {
        storage_layout: StorageLayout::UnifiedWorkingTreeV1,
        roots,
        directories: vec![],
        containers: vec![],
        snapshot_repositories: vec![],
        replicas: vec![],
    }
}
fn authorities(roots: &[RootBinding]) -> Vec<RootSelectionContext<'_>> {
    roots.iter().map(|root| RootSelectionContext {
        storage_layout: StorageLayout::UnifiedWorkingTreeV1,
        registration: &root.registration,
        source_chain: &root.source_chain,
    }).collect()
}
fn directory(root: &RootBinding, id: u8, parent: DirectoryId, name: &str) -> DirectoryBinding {
    DirectoryBinding {
        directory_id: DirectoryId::from_random([id; 16]),
        root_id: root.root_id,
        parent_directory_id: parent,
        name: LookupName::parse(name).unwrap(),
        entity: LocalEntityIdentity::new(root.root_entity.device(), u64::from(id) + 100),
        lifecycle: BindingLifecycle::Current,
        operation_id: operation(),
    }
}
fn commitment() -> MaterialCommitment {
    MaterialCommitment::from_digest(blake3::hash(b"admitted FILE material"))
}
fn add_container(
    catalog: &mut WorkingTreeCatalog,
    root_index: usize,
    replica_index: u8,
    owner_disk: DiskId,
) -> FileId {
    let root = &catalog.roots[root_index];
    let file_id = FileId::from_random([9; 16]);
    let replica_id = ReplicaId::from_random([replica_index; 16]);
    catalog.containers.push(ContainerBinding {
        file_id,
        owner_disk,
        replica_id,
        root_id: root.root_id,
        parent_directory_id: root.roots.common_data.directory_id,
        name: LookupName::parse("report.txt").unwrap(),
        lifecycle: BindingLifecycle::Current,
        file_authority: commitment(),
        operation_id: operation(),
    });
    catalog.replicas.push(ReplicaBinding {
        replica_id,
        object: ReplicaObject::Container(file_id),
        root_id: root.root_id,
        registration: root.registration.clone(),
        source_chain: root.source_chain.clone(),
        entity: LocalEntityIdentity::new(root.root_entity.device(), 1000),
        purpose: if root.source_chain.is_empty() {
            ReplicaPurpose::Current
        } else {
            ReplicaPurpose::Backup
        },
        operation_id: operation(),
    });
    file_id
}

#[test]
fn working_tree_root_selection_requires_current_full_identity_and_local_three_roots() {
    let expected = root(1, vec![]);
    let current = authorities(std::slice::from_ref(&expected));
    let mut record = catalog(vec![expected.clone()]);
    let view = validate_catalog_structure(&record).unwrap();
    assert!(select_root(&view, expected.root_id, &current[0]).is_ok());
    record.roots[0].registration.binding_generation = 2;
    let view = validate_catalog_structure(&record).unwrap();
    assert!(matches!(
        select_root(&view, expected.root_id, &current[0]),
        Err(AdmissionError::Registration),
    ));
    record.roots[0] = expected.clone();
    record.roots[0].roots.zxs_data.entity = expected.roots.common_data.entity;
    assert!(matches!(
        validate_catalog_structure(&record),
        Err(AdmissionError::PhysicalIdentity),
    ));
    record.roots[0] = expected;
    record.roots[0].roots.deleted_data.entity = LocalEntityIdentity::new(99, 10);
    assert!(matches!(
        validate_catalog_structure(&record),
        Err(AdmissionError::PhysicalIdentity),
    ));
}

#[test]
fn working_tree_decode_has_byte_budget_and_no_missing_table_fallback() {
    assert!(matches!(
        decode_catalog(&vec![b' '; MAX_CATALOG_BYTES + 1]),
        Err(AdmissionError::Budget),
    ));
    assert!(matches!(decode_catalog(b"{}"), Err(AdmissionError::Decode(_))));
    let record = catalog(vec![root(1, vec![])]);
    let bytes = storage_contract::canonical_json(&record).unwrap();
    assert_eq!(decode_catalog(&bytes).unwrap(), record);
}

#[test]
fn working_tree_admission_rejects_duplicate_ids_and_equivalent_sibling_names() {
    let first = root(1, vec![]);
    let mut record = catalog(vec![first.clone()]);
    let parent = first.roots.common_data.directory_id;
    record.directories = vec![
        directory(&first, 10, parent, "Photos"),
        directory(&first, 11, parent, "photos"),
    ];
    assert!(matches!(
        validate_catalog_structure(&record),
        Err(AdmissionError::NameConflict),
    ));
    record.directories[1] = record.directories[0].clone();
    assert!(matches!(
        validate_catalog_structure(&record),
        Err(AdmissionError::Order),
    ));
}

#[test]
fn working_tree_admission_rejects_cycles_cross_root_parents_and_management_names() {
    let first = root(1, vec![]);
    let second = root(2, vec![]);
    let baseline = vec![first.clone(), second.clone()];
    let mut record = catalog(baseline.clone());
    let mut child = directory(&first, 20, second.roots.common_data.directory_id, "Photos");
    record.directories.push(child.clone());
    assert!(matches!(
        validate_catalog_structure(&record),
        Err(AdmissionError::Parent),
    ));
    child.parent_directory_id = child.directory_id;
    record.directories[0] = child.clone();
    assert!(matches!(
        validate_catalog_structure(&record),
        Err(AdmissionError::Parent),
    ));
    child.parent_directory_id = first.roots.common_data.directory_id;
    child.name = LookupName::parse(".ZXS").unwrap();
    record.directories[0] = child;
    assert!(matches!(
        validate_catalog_structure(&record),
        Err(AdmissionError::NameConflict),
    ));
}

#[test]
fn working_tree_lifecycle_and_physical_directory_aliases_are_rejected() {
    let first = root(1, vec![]);
    let baseline = vec![first.clone()];
    let mut record = catalog(baseline.clone());
    let child = directory(&first, 10, first.roots.deleted_data.directory_id, "retained");
    record.directories.push(child);
    assert!(matches!(
        validate_catalog_structure(&record),
        Err(AdmissionError::Lifecycle),
    ));
    record.directories[0].lifecycle = BindingLifecycle::Retained { operation_id: operation() };
    assert!(validate_catalog_structure(&record).is_ok());
    record.directories[0].entity = first.roots.zxs_data.entity;
    assert!(matches!(
        validate_catalog_structure(&record),
        Err(AdmissionError::PhysicalIdentity),
    ));
}

#[test]
fn working_tree_backup_keeps_every_source_identity_and_local_replica() {
    let primary = root(1, vec![]);
    let backup = root(2, vec![primary.registration.clone()]);
    let terminal = root(3, vec![backup.registration.clone(), primary.registration.clone()]);
    let baseline = vec![primary, backup, terminal];
    let mut record = catalog(baseline.clone());
    add_container(&mut record, 0, 1, DiskId::from_random([1; 16]));
    add_container(&mut record, 1, 2, DiskId::from_random([1; 16]));
    add_container(&mut record, 2, 3, DiskId::from_random([1; 16]));
    assert!(validate_catalog_structure(&record).is_ok());
    record.replicas[2].source_chain.remove(0);
    assert!(matches!(validate_catalog_structure(&record), Err(AdmissionError::Replica)));
    record.replicas[2].source_chain = baseline[2].source_chain.clone();
    record.replicas[2].entity = record.replicas[0].entity;
    assert!(matches!(
        validate_catalog_structure(&record),
        Err(AdmissionError::PhysicalIdentity),
    ));
}

#[test]
fn working_tree_replicas_require_consistent_commitments_and_one_current_copy() {
    let primary = root(1, vec![]);
    let backup = root(2, vec![primary.registration.clone()]);
    let mut record = catalog(vec![primary, backup]);
    add_container(&mut record, 0, 1, DiskId::from_random([1; 16]));
    add_container(&mut record, 1, 2, DiskId::from_random([1; 16]));
    assert!(validate_catalog_structure(&record).is_ok());
    record.containers[1].file_authority = MaterialCommitment::from_digest(blake3::hash(b"other"));
    assert!(matches!(
        validate_catalog_structure(&record),
        Err(AdmissionError::FileCommitment),
    ));
    record.containers[1].file_authority = commitment();
    record.roots[1].source_chain.clear();
    record.replicas[1].source_chain.clear();
    record.replicas[1].purpose = ReplicaPurpose::Current;
    assert!(matches!(validate_catalog_structure(&record), Err(AdmissionError::Replica)));
}

fn retain_container(record: &mut WorkingTreeCatalog, index: usize, root_index: usize) {
    record.containers[index].parent_directory_id =
        record.roots[root_index].roots.deleted_data.directory_id;
    record.containers[index].lifecycle = BindingLifecycle::Retained { operation_id: operation() };
    record.replicas[index].purpose = ReplicaPurpose::Retained;
}

#[test]
fn working_tree_retained_original_and_independent_destination_keep_distinct_owners() {
    let disk_a = DiskId::from_random([1; 16]);
    let disk_b = DiskId::from_random([2; 16]);
    let mut record = catalog(vec![root(1, vec![]), root(2, vec![])]);
    let file_a = add_container(&mut record, 0, 1, disk_a);
    add_container(&mut record, 1, 2, disk_b);
    let file_b = FileId::from_random([10; 16]);
    record.containers[1].file_id = file_b;
    record.replicas[1].object = ReplicaObject::Container(file_b);
    retain_container(&mut record, 0, 0);

    // Deliberately equal names and opaque commitments do not create identity.
    // This fixture is structural, not a proof that either FILE body was read.
    assert_eq!(record.containers[0].name, record.containers[1].name);
    assert_eq!(record.containers[0].file_authority, record.containers[1].file_authority);
    let bytes = storage_contract::canonical_json(&record).unwrap();
    let decoded = decode_catalog(&bytes).unwrap();
    let view = validate_catalog_structure(&decoded).unwrap();
    let source = view.container(record.containers[0].replica_id).unwrap();
    let destination = view.container(record.containers[1].replica_id).unwrap();
    assert_eq!((source.binding().file_id, source.binding().owner_disk), (file_a, disk_a));
    assert_eq!(
        (destination.binding().file_id, destination.binding().owner_disk),
        (file_b, disk_b),
    );
    assert_eq!(source.role(), ContainerPlacementRole::Original);
    assert_eq!(source.replica().purpose, ReplicaPurpose::Retained);
    assert_eq!(source.binding().lifecycle, BindingLifecycle::Retained {
        operation_id: operation(),
    });
    assert_eq!(destination.role(), ContainerPlacementRole::Original);
    assert_eq!(destination.binding().lifecycle, BindingLifecycle::Current);
    assert_eq!(storage_contract::canonical_json(&decoded).unwrap(), bytes);
}

#[test]
fn working_tree_retained_backup_is_not_an_original_even_without_original_placement() {
    let owner = DiskId::from_random([1; 16]);
    let primary = root(1, vec![]);
    let backup = root(2, vec![primary.registration.clone()]);
    let mut record = catalog(vec![primary, backup]);
    add_container(&mut record, 0, 1, owner);
    add_container(&mut record, 1, 2, owner);
    retain_container(&mut record, 0, 0);
    retain_container(&mut record, 1, 1);
    let view = validate_catalog_structure(&record).unwrap();
    assert_eq!(
        view.container(record.containers[0].replica_id).unwrap().role(),
        ContainerPlacementRole::Original,
    );
    assert_eq!(
        view.container(record.containers[1].replica_id).unwrap().role(),
        ContainerPlacementRole::Backup,
    );
    let original = record.containers.remove(0);
    record.replicas.remove(0);
    let view = validate_catalog_structure(&record).unwrap();
    assert!(view.container(original.replica_id).is_none());
    let retained = view.container(record.containers[0].replica_id).unwrap();
    assert_eq!(retained.role(), ContainerPlacementRole::Backup);
    assert_eq!(retained.binding().owner_disk, owner);
    assert_ne!(retained.replica().registration.disk_id, owner.to_string());
    // Missing structural coverage is not a proven physical deletion, and a
    // remaining backup cannot become original by claiming the local disk.
    record.containers[0].owner_disk = DiskId::from_random([2; 16]);
    assert!(matches!(
        validate_catalog_structure(&record),
        Err(AdmissionError::ContainerOwner),
    ));
}

#[test]
fn working_tree_owner_claim_must_match_every_copy_and_original_registration() {
    let owner = DiskId::from_random([1; 16]);
    let primary = root(1, vec![]);
    let backup = root(2, vec![primary.registration.clone()]);
    let mut record = catalog(vec![primary, backup]);
    add_container(&mut record, 0, 1, owner);
    add_container(&mut record, 1, 2, owner);
    record.containers[1].owner_disk = DiskId::from_random([2; 16]);
    let rejected = record.clone();
    assert!(matches!(
        validate_catalog_structure(&record),
        Err(AdmissionError::ContainerOwner),
    ));
    assert_eq!(record, rejected);
    record.containers[1].owner_disk = owner;
    record.containers[0].owner_disk = DiskId::from_random([2; 16]);
    assert!(matches!(
        validate_catalog_structure(&record),
        Err(AdmissionError::ContainerOwner),
    ));
}

#[test]
fn working_tree_same_file_cannot_claim_two_retained_originals() {
    let owner = DiskId::from_random([1; 16]);
    let mut record = catalog(vec![root(1, vec![]), root(2, vec![])]);
    add_container(&mut record, 0, 1, owner);
    add_container(&mut record, 1, 2, owner);
    retain_container(&mut record, 0, 0);
    retain_container(&mut record, 1, 1);
    assert!(matches!(
        validate_catalog_structure(&record),
        Err(AdmissionError::ContainerOwner),
    ));
    record.containers[1].owner_disk = DiskId::from_random([2; 16]);
    assert!(matches!(
        validate_catalog_structure(&record),
        Err(AdmissionError::ContainerOwner),
    ));
}

#[test]
fn working_tree_owner_survives_slot_changes_but_not_disk_identity_substitution() {
    let owner = DiskId::from_random([1; 16]);
    let mut record = catalog(vec![root(1, vec![])]);
    add_container(&mut record, 0, 1, owner);
    record.roots[0].registration.slot_id = "D09".into();
    record.roots[0].registration.binding_generation = 2;
    record.replicas[0].registration = record.roots[0].registration.clone();
    let view = validate_catalog_structure(&record).unwrap();
    let placement = view.container(record.containers[0].replica_id).unwrap();
    assert_eq!(placement.binding().owner_disk, owner);
    record.roots[0].registration.disk_id = DiskId::from_random([2; 16]).to_string();
    record.replicas[0].registration = record.roots[0].registration.clone();
    assert!(matches!(
        validate_catalog_structure(&record),
        Err(AdmissionError::ContainerOwner),
    ));
}

#[test]
fn directory_target_allocates_missing_ancestors_in_one_operation_without_file_records() {
    let primary = root(1, vec![]);
    let baseline = vec![primary.clone()];
    let current = authorities(&baseline);
    let record = catalog(baseline.clone());
    let view = validate_catalog_structure(&record).unwrap();
    let root = select_root(&view, primary.root_id, &current[0]).unwrap();
    let request = DirectoryTargetRequest {
        root_id: primary.root_id,
        path: DirectoryTargetPath::Relative(
            DirectoryRelativePath::parse("Projects/Photos").unwrap(),
        ),
        expected_root_generation: primary.generation,
        operation_id: operation(),
    };
    let observations = [
        ObservedDirectory {
            name: EntryName::new("Projects").unwrap(),
            entity: FileIdentity { device: 1, inode: 90 },
        },
        ObservedDirectory {
            name: EntryName::new("Photos").unwrap(),
            entity: FileIdentity { device: 1, inode: 91 },
        },
    ];
    let prepared = register_directory_chain(root, &request, &observations).unwrap();
    assert_eq!(prepared.additions.len(), 2);
    assert_eq!(
        prepared.additions[0].parent_directory_id,
        primary.roots.common_data.directory_id,
    );
    assert_eq!(prepared.additions[1].parent_directory_id, prepared.additions[0].directory_id);
    assert_eq!(prepared.host_directory_id, prepared.additions[1].directory_id);
    assert!(prepared.additions.iter().all(|row| row.operation_id == request.operation_id));
    assert!(view.catalog().containers.is_empty());
    assert!(view.catalog().replicas.is_empty());
    let mut wrong = request.clone();
    wrong.expected_root_generation = Generation::new(2);
    assert!(matches!(
        register_directory_chain(root, &wrong, &observations),
        Err(AdmissionError::Generation),
    ));
}

#[test]
fn snapshot_repository_retention_preserves_host_and_requires_deleted_location() {
    let primary = root(1, vec![]);
    let baseline = vec![primary.clone()];
    let current = authorities(&baseline);
    let mut record = catalog(baseline.clone());
    let repository_id = SnapshotRepositoryId::from_random([12; 16]);
    let replica_id = ReplicaId::from_random([12; 16]);
    let host = directory(&primary, 20, primary.roots.common_data.directory_id, "Projects");
    record.snapshot_repositories.push(SnapshotRepositoryBinding {
        repository_id,
        replica_id,
        root_id: primary.root_id,
        host_directory_id: host.directory_id,
        location: SnapshotRepositoryLocation::Attached,
        material: commitment(),
        operation_id: operation(),
    });
    record.replicas.push(ReplicaBinding {
        replica_id,
        object: ReplicaObject::SnapshotRepository(repository_id),
        root_id: primary.root_id,
        registration: primary.registration.clone(),
        source_chain: vec![],
        entity: LocalEntityIdentity::new(1, 1000),
        purpose: ReplicaPurpose::Current,
        operation_id: operation(),
    });
    record.directories.push(host.clone());
    assert!(validate_catalog_structure(&record).is_ok());
    record.snapshot_repositories[0].location = SnapshotRepositoryLocation::Retained {
        parent_directory_id: primary.roots.deleted_data.directory_id,
        name: LookupName::parse("retained-repository").unwrap(),
        operation_id: operation(),
    };
    assert!(matches!(
        validate_catalog_structure(&record),
        Err(AdmissionError::Lifecycle),
    ));
    record.replicas[0].purpose = ReplicaPurpose::Retained;
    let view = validate_catalog_structure(&record).unwrap();
    let root = select_root(&view, primary.root_id, &current[0]).unwrap();
    assert!(root.entry(host.directory_id, &EntryName::new(".zxs").unwrap()).is_none());
    assert!(matches!(
        root.entry(
            primary.roots.deleted_data.directory_id,
            &EntryName::new("retained-repository").unwrap(),
        ),
        Some(crate::working_tree::RegisteredEntry::SnapshotRepository(_)),
    ));
    assert_eq!(view.catalog().directories[0], host);
    record.directories.clear();
    assert!(validate_catalog_structure(&record).is_ok());
    record.snapshot_repositories[0].location = SnapshotRepositoryLocation::Retained {
        parent_directory_id: primary.roots.common_data.directory_id,
        name: LookupName::parse("retained-repository").unwrap(),
        operation_id: operation(),
    };
    assert!(matches!(
        validate_catalog_structure(&record),
        Err(AdmissionError::Lifecycle),
    ));
}

#[test]
fn structural_catalog_does_not_claim_current_registration_or_expected_file_coverage() {
    let primary = root(1, vec![]);
    let current = authorities(std::slice::from_ref(&primary));
    let mut record = catalog(vec![primary.clone()]);
    add_container(&mut record, 0, 1, DiskId::from_random([1; 16]));
    assert!(validate_catalog_structure(&record).is_ok());
    // Structural closure intentionally cannot detect omission of an entire FILE.
    // No complete expected FILE set or authenticated catalog is supplied here.
    record.containers.clear();
    record.replicas.clear();
    assert!(validate_catalog_structure(&record).is_ok());
    record.roots[0].registration.binding_generation = 2;
    let view = validate_catalog_structure(&record).unwrap();
    assert!(matches!(
        select_root(&view, primary.root_id, &current[0]),
        Err(AdmissionError::Registration),
    ));
}

#[test]
fn raw_catalog_duplicate_fields_are_rejected_before_admission() {
    let bytes = concat!(
        "{\"storage_layout\":\"unified_working_tree_v1\",",
        "\"roots\":[],\"roots\":[],\"directories\":[],\"containers\":[],",
        "\"snapshot_repositories\":[],\"replicas\":[]}",
    );
    assert!(matches!(decode_catalog(bytes.as_bytes()), Err(AdmissionError::Decode(_))));
}

#[test]
fn directory_target_reuses_exact_registered_ancestors_and_rejects_replacement() {
    let primary = root(1, vec![]);
    let baseline = vec![primary.clone()];
    let current = authorities(&baseline);
    let mut record = catalog(baseline.clone());
    let ancestor = directory(&primary, 20, primary.roots.common_data.directory_id, "Photos");
    record.directories.push(ancestor.clone());
    let view = validate_catalog_structure(&record).unwrap();
    let selected = select_root(&view, primary.root_id, &current[0]).unwrap();
    let request = DirectoryTargetRequest {
        root_id: primary.root_id,
        path: DirectoryTargetPath::Relative(
            DirectoryRelativePath::parse("Photos/Holidays").unwrap(),
        ),
        expected_root_generation: primary.generation,
        operation_id: operation(),
    };
    let mut observations = [
        ObservedDirectory {
            name: EntryName::new("Photos").unwrap(),
            entity: file_identity(ancestor.entity),
        },
        ObservedDirectory {
            name: EntryName::new("Holidays").unwrap(),
            entity: FileIdentity { device: 1, inode: 121 },
        },
    ];
    let prepared = register_directory_chain(selected, &request, &observations).unwrap();
    assert_eq!(prepared.additions.len(), 1);
    assert_eq!(prepared.additions[0].parent_directory_id, ancestor.directory_id);
    observations[0].entity.inode = 999;
    assert!(matches!(
        register_directory_chain(selected, &request, &observations),
        Err(AdmissionError::PhysicalIdentity),
    ));
}

#[test]
fn working_tree_source_chain_closure_is_separate_from_current_root_selection() {
    let primary = root(1, vec![]);
    let backup = root(2, vec![primary.registration.clone()]);
    let baseline = vec![primary.clone(), backup.clone()];
    let current = authorities(&baseline);
    let record = catalog(baseline.clone());
    let view = validate_catalog_structure(&record).unwrap();
    assert!(select_root(&view, backup.root_id, &current[1]).is_ok());
    let stale_context = RootSelectionContext {
        storage_layout: StorageLayout::UnifiedWorkingTreeV1,
        registration: &backup.registration,
        source_chain: &[],
    };
    assert!(matches!(
        select_root(&view, backup.root_id, &stale_context),
        Err(AdmissionError::SourceChain),
    ));
    assert!(matches!(
        validate_catalog_structure(&catalog(vec![backup])),
        Err(AdmissionError::SourceChain),
    ));
    let mut repeated = root(3, vec![primary.registration.clone(), primary.registration]);
    repeated.source_chain[1].slot_id = "D04".into();
    assert!(matches!(
        validate_catalog_structure(&catalog(vec![repeated])),
        Err(AdmissionError::SourceChain),
    ));
}
