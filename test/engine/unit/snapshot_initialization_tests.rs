//! Intent persistence only: synthetic registration evidence is not authority.
use super::{intent::*, store};
use crate::disk::DiskRoot;
use crate::persist::layout::snapshot_init;
use crate::working_tree::snapshot::{SnapshotEntry, SnapshotEntryContent, SnapshotFile,
    SnapshotFileSource};
use name_policy::PortablePath;
use platform_io::fs_safety::PrivateDirectory;
use std::{fs, os::unix::fs::MetadataExt};
use storage_contract::working_tree::*;
use storage_contract::{BranchId, CommitId, ContentHash, DiskId, FileId, FileNodeId,
    Generation, OperationId, PoolId, Principal, RepositoryId, Sequence, VersionId};

fn root() -> (tempfile::TempDir, DiskRoot) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("disk");
    PrivateDirectory::create(&path).unwrap();
    let root = DiskRoot::open(&path).unwrap();
    (temp, root)
}

// These identities exercise syntax closure, never an Engine registration.
fn intent(operation: OperationId, target_operation: OperationId, snapshot_only: bool)
    -> Result<AdmittedIntent, IntentError>
{
    let registration = DiskRegistrationIdentity {
        pool_id: PoolId::from_random([1; 16]).to_string(),
        disk_id: DiskId::from_random([2; 16]).to_string(),
        slot_id: "D01".to_owned(),
        binding_generation: 1,
    };
    let local = |id| LocalDirectoryBinding {
        directory_id: DirectoryId::from_random([id; 16]),
        entity: LocalEntityIdentity::new(1, u64::from(id)),
    };
    let root = RootBinding {
        layout: StorageLayout::UnifiedWorkingTreeV1,
        root_id: RootId::from_random([3; 16]),
        registration: registration.clone(),
        source_chain: vec![],
        root_entity: LocalEntityIdentity::new(1, 1),
        roots: ThreeRootBinding {
            common_data: local(4), deleted_data: local(5), zxs_data: local(6),
        },
        operation_id: operation,
        generation: Generation::new(1),
    };
    let target = DirectoryTargetRequest {
        root_id: root.root_id,
        path: DirectoryTargetPath::Root,
        expected_root_generation: root.generation,
        operation_id: target_operation,
    };
    let version = VersionId::from_random([7; 16]);
    let branch = BranchId::from_random([8; 16]);
    AdmittedIntent::prepare(
        operation,
        Principal::new("intent-test".into()).unwrap(),
        Source {
            repository: RepositoryId::from_random([9; 16]),
            branch,
            head: CommitId::from_random([10; 16]),
            scope: None,
            publication: MaterialCommitment::from_digest(blake3::hash(b"source publication")),
            sequence: Sequence::new(1),
        },
        target,
        root,
        LocalEntityIdentity::new(1, 4),
        DirectoryId::from_random([4; 16]),
        Vec::new(),
        vec![registration],
        SnapshotRepositoryId::from_random([11; 16]),
        SnapshotBranchId::from_random([12; 16]),
        SnapshotNodeId::from_random([13; 16]),
        vec![SnapshotEntry {
            path: PortablePath::parse("original.txt").unwrap(),
            content: SnapshotEntryContent::File {
                material: SnapshotFile {
                    file_id: FileId::from_random([14; 16]),
                    version_id: version,
                    size: 4,
                    content_hash: ContentHash::from_digest(blake3::hash(b"body")),
                },
                source: if snapshot_only { SnapshotFileSource::SnapshotOnly } else {
                    SnapshotFileSource::Container { branch, node: FileNodeId::Version(version) }
                },
            },
        }],
    )
}

fn admitted() -> AdmittedIntent {
    let operation = OperationId::from_random([15; 16]);
    intent(operation, operation, false).unwrap()
}

fn image(root: &DiskRoot) -> (Vec<u8>, u64, u64, u32) {
    let path = root.path().join(snapshot_init::INTENT.as_path());
    let metadata = fs::metadata(&path).unwrap();
    (fs::read(path).unwrap(), metadata.dev(), metadata.ino(), metadata.mode())
}

#[test]
fn intent_cold_read_and_exact_retry_preserve_original_record_and_entity() {
    let (_temp, disk) = root();
    assert!(store::read(&disk).unwrap().is_none());
    let original = admitted();
    store::save(&disk, &original).unwrap();
    let before = image(&disk);
    let path = disk.path().to_owned();
    drop(disk);
    let reopened = DiskRoot::open(&path).unwrap();
    let recovered = store::read(&reopened).unwrap().unwrap();
    assert_eq!(recovered.record(), original.record());
    assert_eq!(recovered.bytes(), original.bytes());
    store::save(&reopened, &recovered).unwrap();
    assert_eq!(image(&reopened), before);
    assert!(!path.join(".zxs").exists());
}

#[test]
fn another_operation_or_same_operation_changed_principal_cannot_replace_original() {
    let (_temp, disk) = root();
    let original = admitted();
    store::save(&disk, &original).unwrap();
    let before = image(&disk);
    let operation = OperationId::from_random([16; 16]);
    let other = intent(operation, operation, false).unwrap();
    assert!(matches!(store::save(&disk, &other), Err(store::StoreError::Occupied)));
    assert_eq!(image(&disk), before);
    let mut changed: json_codec::Value = json_codec::from_slice(original.bytes()).unwrap();
    changed["principal"] = json_codec::json!("different actor");
    let changed = AdmittedIntent::decode(&storage_contract::canonical_json(&changed).unwrap())
        .unwrap();
    assert!(matches!(store::save(&disk, &changed), Err(store::StoreError::Occupied)));
    assert_eq!(image(&disk), before);
}

#[test]
fn unknown_noncanonical_and_missing_required_field_are_preserved_not_adopted() {
    let original = admitted();
    let value: json_codec::Value = json_codec::from_slice(original.bytes()).unwrap();
    let mut unknown = value.clone();
    unknown["unknown"] = json_codec::json!(true);
    let mut missing = value.clone();
    missing.as_object_mut().unwrap().remove("metadata_members");
    let mut missing_path = value.clone();
    missing_path["target"].as_object_mut().unwrap().remove("path");
    let mut noncanonical = original.bytes().to_vec();
    noncanonical.push(b'\n');
    for bytes in [
        b"unknown original bytes".to_vec(),
        storage_contract::canonical_json(&unknown).unwrap(),
        storage_contract::canonical_json(&missing).unwrap(),
        storage_contract::canonical_json(&missing_path).unwrap(),
        noncanonical,
    ] {
        let (_temp, disk) = root();
        fs::write(disk.path().join(snapshot_init::INTENT.as_path()), bytes).unwrap();
        let before = image(&disk);
        assert!(store::read(&disk).is_err());
        assert!(store::save(&disk, &original).is_err());
        assert_eq!(image(&disk), before);
    }
}

#[test]
fn corrupted_body_is_rejected_on_cold_read_without_repair() {
    let (_temp, disk) = root();
    store::save(&disk, &admitted()).unwrap();
    let path = disk.path().join(snapshot_init::INTENT.as_path());
    // Replace with invalid bytes explicitly; no internal verify hook is exposed.
    fs::remove_file(&path).unwrap();
    fs::write(&path, b"{truncated").unwrap();
    let before = image(&disk);
    let directory = disk.path().to_owned();
    drop(disk);
    let reopened = DiskRoot::open(&directory).unwrap();
    assert!(store::read(&reopened).is_err());
    assert_eq!(image(&reopened), before);
}

#[test]
fn unsupported_source_target_mismatch_and_oversized_input_fail_before_store_io() {
    let (_temp, disk) = root();
    let operation = OperationId::from_random([15; 16]);
    assert!(matches!(intent(operation, operation, true), Err(IntentError::Source)));
    assert!(matches!(intent(operation, OperationId::from_random([16; 16]), false),
        Err(IntentError::Target)));
    assert!(matches!(AdmittedIntent::decode(&vec![b' '; MAX_INTENT_BYTES + 1]),
        Err(IntentError::Budget)));
    assert_eq!(fs::read_dir(disk.path()).unwrap().count(), 0);
}

#[test]
fn retained_intent_cold_discovery_denies_ordinary_writes_without_adopting_material() {
    let temp = tempfile::tempdir().unwrap();
    let system = temp.path().join("system");
    let engine = crate::Engine::initialize(
        &system, crate::Config::development([temp.path().join("disk")]).unwrap(),
    ).unwrap();
    let state = storage_contract::canonical_json(engine.state()).unwrap();
    // The storage fixture has no production directory authority. Its presence
    // is only a deny signal, never permission to adopt its synthetic entities.
    let mut value: json_codec::Value = json_codec::from_slice(admitted().bytes()).unwrap();
    value["root"]["registration"]["pool_id"] = json_codec::json!(engine.config.pool_id);
    value["metadata_members"][0]["pool_id"] = json_codec::json!(engine.config.pool_id);
    let original = AdmittedIntent::decode(&storage_contract::canonical_json(&value).unwrap())
        .unwrap();
    let held = DiskRoot::open(&system).unwrap();
    store::save(&held, &original).unwrap();
    let before = image(&held);
    drop(engine);
    let mut reopened = crate::Engine::open(&system).unwrap();
    let error = reopened.create_repository(
        &storage_contract::RepositoryName::parse("must-not-publish").unwrap(),
        "P01", 1, "admin", OperationId::generate().unwrap(),
    ).unwrap_err();
    assert_eq!(error.code(), crate::ErrorCode::RecoveryRequired);
    assert_eq!(storage_contract::canonical_json(reopened.state()).unwrap(), state);
    assert_eq!(image(&held), before);
    assert!(!system.join(".zxs").exists());
}

#[test]
fn directory_candidate_binds_original_registration_without_changing_legacy_candidate_bytes() {
    use super::registration::{RegistrationRecord, INITIALIZE_KIND};
    use crate::journal::{Candidate, CandidateData};
    use crate::working_tree::snapshot::HeldSnapshotNode;
    let (_temp, repository) = root();
    let original = admitted();
    let node = HeldSnapshotNode::prepare(&repository, original.manifest().unwrap(),
        |_| Ok(std::io::Cursor::new(b"body"))).unwrap();
    let (record, binding) = RegistrationRecord::prepare(&original, node.binding()).unwrap();
    let data = CandidateData {
        pool_id: original.record().root.registration.pool_id.clone(),
        sequence: 2,
        transaction_id: "snapshot-test".into(),
        operation_id: original.record().operation,
        request_digest: binding.intent.hash().to_string(),
        previous_snapshot: Some(original.record().source.publication.hash().to_string()),
        snapshot_hash: blake3::hash(b"original state").to_hex().to_string(),
        versions: vec![],
        metadata_disk_ids: vec![original.record().metadata_members[0].disk_id.clone()],
        history_roots: crate::metadata_map::Encoder::new(16 * 1024 * 1024)
            .map(json_codec::Map::new()).unwrap(),
    };
    let legacy = Candidate::new(data.clone(), None);
    let legacy_bytes = storage_contract::canonical_json(&legacy).unwrap();
    assert_eq!(legacy_bytes, storage_contract::canonical_json(&data).unwrap());
    let candidate = Candidate::new_directory_snapshot(data, None, None, binding).unwrap();
    let bytes = storage_contract::canonical_json(&candidate).unwrap();
    let candidate: Candidate = storage_contract::decode_strict(&bytes).unwrap();
    assert!(candidate.generation().is_none());
    assert!(candidate.registry().is_none());
    let mut result = crate::OperationResult {
        operation_id: candidate.operation_id,
        kind: INITIALIZE_KIND.into(),
        commit_id: None,
        history_tree_id: None,
        branch_id: None,
        commit_state: crate::CommitState::Published,
        protection_at_publish: crate::ProtectionAtPublish::NotApplicable,
        current_health: crate::CurrentHealth::Healthy,
        request_digest: candidate.request_digest.clone(),
        sequence: candidate.sequence,
        result: record.result_value().unwrap(),
    };
    assert_eq!(RegistrationRecord::from_candidate_result(&candidate, &result).unwrap(), record);
    result.result["intent"]["principal"] = json_codec::json!("different actor");
    assert!(RegistrationRecord::from_candidate_result(&candidate, &result).is_err());
    let mut missing: json_codec::Value = json_codec::from_slice(&bytes).unwrap();
    missing.as_object_mut().unwrap().remove("directory_material");
    assert!(storage_contract::decode_strict::<Candidate>(
        &storage_contract::canonical_json(&missing).unwrap(),
    ).is_err());
    assert_eq!(storage_contract::canonical_json(&legacy).unwrap(), legacy_bytes);
}

#[test]
fn exact_directory_chain_is_required_without_reassigning_host_identity() {
    let operation = OperationId::generate().unwrap();
    let original = intent(operation, operation, false).unwrap();
    let mut record = original.record().clone();
    let directory = DirectoryBinding {
        directory_id: DirectoryId::from_random([31; 16]),
        root_id: record.root.root_id,
        parent_directory_id: record.root.roots.common_data.directory_id,
        name: name_policy::LookupName::parse("existing directory").unwrap(),
        entity: LocalEntityIdentity::new(1, 31),
        lifecycle: BindingLifecycle::Current,
        operation_id: operation,
    };
    record.target.path = DirectoryTargetPath::Relative(
        DirectoryRelativePath::parse("existing directory").unwrap(),
    );
    record.host_directory_id = directory.directory_id;
    record.host_entity = directory.entity;
    record.directories = vec![directory];
    let bytes = storage_contract::canonical_json(&record).unwrap();
    let decoded = AdmittedIntent::decode(&bytes).unwrap();
    assert_eq!(decoded.record(), &record);
    assert_eq!(decoded.bytes(), bytes);

    for attack in 0..6 {
        let mut changed = record.clone();
        match attack {
            0 => changed.directories.clear(),
            1 => changed.directories[0].parent_directory_id =
                changed.root.roots.deleted_data.directory_id,
            2 => changed.directories[0].entity = LocalEntityIdentity::new(1, 32),
            3 => changed.directories[0].name = name_policy::LookupName::parse("other").unwrap(),
            4 => changed.host_directory_id = DirectoryId::from_random([32; 16]),
            _ => changed.directories[0].directory_id =
                changed.root.roots.common_data.directory_id,
        }
        let rejected = storage_contract::canonical_json(&changed).unwrap();
        assert!(AdmittedIntent::decode(&rejected).is_err());
        assert_eq!(storage_contract::canonical_json(&record).unwrap(), bytes);
    }
    // Missing new evidence or an older intent format is not a migration request.
    let value: json_codec::Value = storage_contract::decode_strict(&bytes).unwrap();
    for field in ["directories", "host_directory_id"] {
        let mut old = value.clone();
        old.as_object_mut().unwrap().remove(field);
        assert!(AdmittedIntent::decode(&storage_contract::canonical_json(&old).unwrap()).is_err());
    }
    let mut old = value;
    old["format"] = json_codec::json!("directory-snapshot-initialization-intent-v1");
    assert!(AdmittedIntent::decode(&storage_contract::canonical_json(&old).unwrap()).is_err());
}
