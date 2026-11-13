//! Real bootstrap proof bytes constrain a successor's frozen operation ledger.
use crate::engine::snapshot_initialization::bootstrap::decision::BootstrapDecision;
use crate::persist::layout;
use crate::{Config, Engine, OperationId};
use std::fs;
use std::os::unix::fs::PermissionsExt;

#[test]
fn published_bootstrap_operation_and_principal_remain_exact_in_successor_base() {
    let temp = tempfile::tempdir().unwrap();
    let system = temp.path().join("system");
    let disk = temp.path().join("disk");
    for root in [&system, &disk] {
        fs::create_dir(root).unwrap();
        fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let operation = OperationId::generate().unwrap();
    let mut engine = Engine::initialize_storage(
        &system, Config::development([disk.clone()]).unwrap(), operation,
    ).unwrap();
    engine.create_repository(
        &storage_contract::RepositoryName::parse("ordinary successor").unwrap(),
        "P01", 1, "admin", OperationId::generate().unwrap(),
    ).unwrap();
    let proof_path = disk.join(layout::STORAGE_INITIALIZATION_PROOF.as_path());
    let original_bytes = fs::read(&proof_path).unwrap();
    let (proof, _) = BootstrapDecision::admit_source(&original_bytes).unwrap();
    proof.require_retained_operation(engine.state()).unwrap();

    let mut missing = engine.state().clone();
    missing.operations.remove(&operation);
    assert!(proof.require_retained_operation(&missing).is_err());

    let mut changed = engine.state().clone();
    let mut result = changed.operations.get(&operation).unwrap().clone();
    result.request_digest = blake3::hash(b"different registration").to_string();
    changed.operations.insert(operation, result);
    assert!(proof.require_retained_operation(&changed).is_err());

    let mut impersonated = engine.state().clone();
    impersonated.operation_principals.insert(operation, "different-principal".to_owned());
    assert!(proof.require_retained_operation(&impersonated).is_err());
    assert_eq!(fs::read(proof_path).unwrap(), original_bytes);
    proof.require_retained_operation(engine.state()).unwrap();
}

#[test]
fn carrier_preflight_does_not_probe_storage_or_admit_a_changed_configuration() {
    let temp = tempfile::tempdir().unwrap();
    let system = temp.path().join("system");
    let disk = temp.path().join("disk");
    for root in [&system, &disk] {
        fs::create_dir(root).unwrap();
        fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let config = Config::development([disk.clone()]).unwrap();
    let engine = Engine::initialize_storage(
        &system, config.clone(), OperationId::generate().unwrap(),
    ).unwrap();
    let held = platform_io::fs_safety::PrivateDirectory::open_existing(&system).unwrap();
    let unavailable = temp.path().join("disconnected-disk");
    fs::rename(&disk, &unavailable).unwrap();
    let (_, origin) = crate::engine::snapshot_initialization::bootstrap
        ::admit_original_carrier_system(&held).unwrap();
    origin.require_config(&crate::ValidatedConfig::try_from(config.clone()).unwrap()).unwrap();
    let mut changed = config;
    changed.max_entries += 1;
    assert!(origin.require_config(&crate::ValidatedConfig::try_from(changed).unwrap()).is_err());
    assert!(!disk.exists());
    fs::rename(&unavailable, &disk).unwrap();
    drop(engine);
}

#[test]
fn unified_origin_evidence_rejects_wrong_pool_missing_proof_and_changed_receipt() {
    use crate::engine::snapshot_initialization::bootstrap::successor::UnifiedOriginEvidence;

    let temp = tempfile::tempdir().unwrap();
    let system = temp.path().join("system");
    let disk = temp.path().join("disk");
    for root in [&system, &disk] {
        fs::create_dir(root).unwrap();
        fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let engine = Engine::initialize_storage(
        &system, Config::development([disk.clone()]).unwrap(), OperationId::generate().unwrap(),
    ).unwrap();
    let proof_path = disk.join(layout::STORAGE_INITIALIZATION_PROOF.as_path());
    let original_bytes = fs::read(&proof_path).unwrap();
    let evidence = engine.capture_unified_origin().unwrap();
    evidence.verify().unwrap();

    let mut wrong_pool = evidence.clone();
    wrong_pool.configuration.pool_id = Config::development([disk]).unwrap().pool_id;
    assert!(wrong_pool.verify().is_err());

    let bytes = storage_contract::canonical_json(&evidence).unwrap();
    let mut missing: json_codec::Value = storage_contract::decode_strict(&bytes).unwrap();
    missing.as_object_mut().unwrap().remove("decision");
    assert!(storage_contract::decode_strict::<UnifiedOriginEvidence>(
        &storage_contract::canonical_json(&missing).unwrap(),
    ).is_err());

    let mut changed: json_codec::Value = storage_contract::decode_strict(&bytes).unwrap();
    changed["decision"]["publication"]["result"]["request_digest"] =
        json_codec::Value::String(blake3::hash(b"forged original receipt").to_string());
    let forged: UnifiedOriginEvidence = storage_contract::decode_strict(
        &storage_contract::canonical_json(&changed).unwrap(),
    ).unwrap();
    assert!(forged.verify().is_err());
    assert_eq!(fs::read(proof_path).unwrap(), original_bytes);
    assert_eq!(storage_contract::canonical_json(&engine.capture_unified_origin().unwrap()).unwrap(),
        bytes);
}

#[test]
fn unified_add_activates_and_cold_reopens_without_rewriting_bootstrap_receipt() {
    let temp = tempfile::tempdir().unwrap();
    let system = temp.path().join("system");
    let first = temp.path().join("first");
    let second = temp.path().join("second");
    let archive = temp.path().join("archive");
    for root in [&system, &first, &second, &archive] {
        fs::create_dir(root).unwrap();
        fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let mut target = Config::development([first.clone(), second.clone()]).unwrap();
    for disk in &mut target.disks {
        disk.level = 1;
        disk.upstream_slot_id = None;
    }
    let mut initial = target.clone();
    initial.disks.truncate(1);
    let bootstrap_operation = OperationId::generate().unwrap();
    let mut engine = Engine::initialize_storage(&system, initial, bootstrap_operation).unwrap();
    let repository = engine.create_repository(
        &storage_contract::RepositoryName::parse("retained payload").unwrap(),
        "P01", 1, "admin", OperationId::generate().unwrap(),
    ).unwrap();
    let workspace = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
    engine.stage_bytes(&workspace.workspace_id, "body.txt", b"before membership").unwrap();
    engine.commit_workspace(crate::CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: OperationId::generate().unwrap(),
        principal: "admin".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "retain across Add".into(),
        request_digest: None,
    }).unwrap();
    let file = engine.list_tree(&repository.main_branch_id).unwrap().into_iter()
        .find(|entry| entry.path() == "body.txt").unwrap();
    let initial_receipt = engine.state().operations.get(&bootstrap_operation).unwrap().clone();
    let proof_path = first.join(layout::STORAGE_INITIALIZATION_PROOF.as_path());
    let proof_bytes = fs::read(&proof_path).unwrap();
    let operation = OperationId::generate().unwrap();
    assert!(engine.begin_membership_handoff(target.clone(), "admin", operation)
        .unwrap().all_members_fenced());
    let control = engine.build_membership_control_archive(&archive).unwrap();
    assert!(engine.enroll_pending_members(&archive, &control.root_hash, vec![
        crate::PendingMemberDirectory {
            disk_id: target.disks[1].disk_id.clone(),
            directory: fs::File::open(&second).unwrap(),
        },
    ]).unwrap().all_control_metadata_synchronized());
    engine.resume_membership_joint_decision().unwrap();
    assert_eq!(engine.activate_pending_membership().unwrap().operation_id, operation);
    assert_eq!(engine.state().operations.get(&bootstrap_operation), Some(&initial_receipt));
    assert_eq!(fs::read(&proof_path).unwrap(), proof_bytes);
    let published = engine.state().clone();
    drop(engine);
    let cold = Engine::open(&system).unwrap();
    assert_eq!(cold.state(), &published);
    assert_eq!(cold.config().disks.len(), 2);
    assert_eq!(fs::read(cold.valid_replica(&file.version_id).unwrap().0.path()).unwrap(),
        b"before membership");
    assert_eq!(fs::read(proof_path).unwrap(), proof_bytes);
    drop(cold);

    // A current successor seal cannot stand in for its missing portable
    // dependency, nor may cold admission recreate that missing record.
    let marker = first.join("metadata/pool-membership/generation.json");
    let marker_bytes = fs::read(&marker).unwrap();
    fs::remove_file(&marker).unwrap();
    let before = tree_image(temp.path());
    assert!(Engine::open(&system).is_err());
    assert_eq!(tree_image(temp.path()), before);
    fs::write(&marker, marker_bytes).unwrap();
    let restored = Engine::open(&system).unwrap();
    assert_eq!(restored.state(), &published);
}

fn tree_image(root: &std::path::Path) -> std::collections::BTreeMap<
    std::path::PathBuf, (u64, u64, u32, Vec<u8>)
> {
    use std::os::unix::fs::MetadataExt;
    fn visit(
        root: &std::path::Path,
        path: &std::path::Path,
        entries: &mut std::collections::BTreeMap<std::path::PathBuf, (u64, u64, u32, Vec<u8>)>,
    ) {
        let metadata = fs::symlink_metadata(path).unwrap();
        let bytes = if metadata.is_dir() { Vec::new() } else { fs::read(path).unwrap() };
        entries.insert(path.strip_prefix(root).unwrap().to_owned(),
            (metadata.dev(), metadata.ino(), metadata.mode(), bytes));
        if metadata.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                visit(root, &entry.unwrap().path(), entries);
            }
        }
    }
    let mut entries = std::collections::BTreeMap::new();
    visit(root, root, &mut entries);
    entries
}
