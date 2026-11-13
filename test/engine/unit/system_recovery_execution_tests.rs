//! Recovery changes carrier identity only through an actual member publication.
use crate::{Engine, OperationId};
use crate::engine::snapshot_initialization::bootstrap::system_recovery::tests::{fixture, image};
use platform_io::fs_safety::PrivateDirectory;
use std::fs;
use std::os::unix::fs::{PermissionsExt, MetadataExt};
use storage_contract::working_tree::MaterialCommitment;

fn control(system: &std::path::Path, operation: OperationId, source: MaterialCommitment)
    -> crate::StorageRecoveryControl
{
    let parent = system.parent().unwrap().metadata().unwrap();
    let child = system.metadata().unwrap();
    crate::StorageRecoveryControl::new(crate::StorageControlBinding {
        operation,
        control: storage_contract::working_tree::LocalEntityIdentity::new(parent.dev(), parent.ino()),
        system: storage_contract::working_tree::LocalEntityIdentity::new(child.dev(), child.ino()),
    }, source, "recovery-owner".parse().unwrap(), vec!["selected-members".into()],
        crate::StorageRecoveryPredecessor::NewControl)
}

#[test]
fn new_carrier_preserves_original_file_history_and_cold_exact_recovery_receipt() {
    let (temp, system, config) = fixture();
    let original_operation = OperationId::generate().unwrap();
    let mut engine = Engine::initialize_storage(&system, config.clone(), original_operation).unwrap();
    let repository = engine.create_repository(
        &storage_contract::RepositoryName::parse("retained file").unwrap(),
        "P01", 1, "admin", OperationId::generate().unwrap(),
    ).unwrap();
    let workspace = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
    engine.stage_bytes(&workspace.workspace_id, "original.txt", b"original immutable bytes").unwrap();
    engine.commit_workspace(crate::CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: OperationId::generate().unwrap(),
        principal: "admin".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "before SYSTEM loss".into(),
        request_digest: None,
    }).unwrap();
    let original = engine.state().clone();
    let descriptions = engine.directory_storage_roots().unwrap();
    let file = engine.list_tree(&repository.main_branch_id).unwrap().into_iter()
        .find(|entry| entry.path() == "original.txt").unwrap();
    drop(engine);
    let lost = temp.path().join("preserved-original-system");
    fs::rename(&system, &lost).unwrap();
    fs::create_dir(&system).unwrap();
    fs::set_permissions(&system, fs::Permissions::from_mode(0o700)).unwrap();
    let lost_image = image(&lost);
    let handles = config.disks.iter()
        .map(|disk| PrivateDirectory::open_existing(&disk.root).unwrap()).collect();
    let admitted = Engine::admit_storage_recovery(handles).unwrap();
    let source_evidence = admitted.evidence_commitment();
    let operation = OperationId::generate().unwrap();
    let control = control(&system, operation, source_evidence);
    let intent = control.commitment().unwrap();
    let mut engine = Engine::restore_storage_held(
        PrivateDirectory::open_existing(&system).unwrap(), operation, "recovery-owner".parse().unwrap(),
        admitted, intent, control.clone(), None,
    ).unwrap();
    assert_eq!(engine.state().versions, original.versions);
    assert_eq!(engine.state().commits, original.commits);
    assert_eq!(engine.state().branches, original.branches);
    for (operation, result) in original.operations.iter() {
        assert_eq!(engine.state().operations.get(operation), Some(result));
        assert_eq!(engine.state().operation_principals.get(operation),
            original.operation_principals.get(operation));
    }
    assert_eq!(engine.state().operations.len(), original.operations.len() + 1);
    assert_eq!(engine.directory_storage_roots().unwrap(), descriptions);
    assert_eq!(fs::read(engine.valid_replica(&file.version_id).unwrap().0.path()).unwrap(),
        b"original immutable bytes");
    // A legitimate FILE successor must survive cold admission and an exact
    // query of the earlier recovery operation. Recovery is not a state reset.
    let recovery_receipt = engine.state().operations.get(&operation).unwrap().clone();
    let workspace = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
    engine.stage_bytes(
        &workspace.workspace_id, "original.txt", b"ordinary successor after recovery",
    ).unwrap();
    engine.commit_workspace(crate::CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: OperationId::generate().unwrap(),
        principal: "admin".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "ordinary successor".into(),
        request_digest: None,
    }).unwrap();
    let successor = engine.list_tree(&repository.main_branch_id).unwrap().into_iter()
        .find(|entry| entry.path() == "original.txt").unwrap();
    assert_ne!(successor.version_id, file.version_id);
    assert_eq!(engine.state().operations.get(&operation), Some(&recovery_receipt));
    let published = engine.state().clone();
    drop(engine);
    // Restore the ORIGINAL inode at its ORIGINAL path, after releasing every
    // Engine/member lock. Neither path mismatch nor a competing writer can
    // stand in for rejection of the obsolete carrier against the latest ledger.
    let recovered_carrier = temp.path().join("temporarily-held-recovery-carrier");
    fs::rename(&system, &recovered_carrier).unwrap();
    fs::rename(&lost, &system).unwrap();
    let before_old_carrier = image(temp.path());
    assert!(Engine::open(&system).is_err());
    assert_eq!(image(temp.path()), before_old_carrier);
    fs::rename(&system, &lost).unwrap();
    fs::rename(&recovered_carrier, &system).unwrap();
    let cold = Engine::open(&system).unwrap();
    assert_eq!(cold.state(), &published);
    assert_eq!(fs::read(cold.valid_replica(&successor.version_id).unwrap().0.path()).unwrap(),
        b"ordinary successor after recovery");
    assert_eq!(fs::read(cold.valid_replica(&file.version_id).unwrap().0.path()).unwrap(),
        b"original immutable bytes");
    drop(cold);
    let handles = config.disks.iter()
        .map(|disk| PrivateDirectory::open_existing(&disk.root).unwrap()).collect();
    let wrong_owner = Engine::admit_storage_recovery(handles).unwrap();
    assert_ne!(wrong_owner.evidence_commitment(), source_evidence);
    wrong_owner.require_recovery_intent_source(
        operation, &"recovery-owner".parse().unwrap(), source_evidence, intent,
    ).unwrap();
    assert_eq!(wrong_owner.origin_operation(), original_operation);
    wrong_owner.require_initialization_origin(original_operation, &config).unwrap();
    let before = image(temp.path());
    assert!(Engine::restore_storage_held(
        PrivateDirectory::open_existing(&system).unwrap(), operation, "different-owner".parse().unwrap(),
        wrong_owner, intent, control.clone(), None,
    ).is_err());
    assert_eq!(image(temp.path()), before);
    let handles = config.disks.iter()
        .map(|disk| PrivateDirectory::open_existing(&disk.root).unwrap()).collect();
    let admitted = Engine::admit_storage_recovery(handles).unwrap();
    let repeated = Engine::restore_storage_held(
        PrivateDirectory::open_existing(&system).unwrap(), operation, "recovery-owner".parse().unwrap(),
        admitted, intent, control.clone(), None,
    ).unwrap();
    assert_eq!(repeated.state(), &published);
    assert_eq!(repeated.state().operations.get(&operation), Some(&recovery_receipt));
    assert_eq!(fs::read(repeated.valid_replica(&successor.version_id).unwrap().0.path()).unwrap(),
        b"ordinary successor after recovery");
    assert_eq!(image(&lost), lost_image);
    drop(repeated);

    // Lose the recovered SYSTEM as well. A third physical carrier must replay
    // both the immutable original bootstrap and the prior recovery receipt.
    let second_lost = temp.path().join("preserved-second-system");
    fs::rename(&system, &second_lost).unwrap();
    fs::create_dir(&system).unwrap();
    fs::set_permissions(&system, fs::Permissions::from_mode(0o700)).unwrap();
    let second_lost_image = image(&second_lost);
    let handles = config.disks.iter()
        .map(|disk| PrivateDirectory::open_existing(&disk.root).unwrap()).collect();
    let admitted = Engine::admit_storage_recovery(handles).unwrap();
    let third_operation = OperationId::generate().unwrap();
    let third_control = self::control(&system, third_operation, admitted.evidence_commitment());
    let third = Engine::restore_storage_held(
        PrivateDirectory::open_existing(&system).unwrap(), third_operation,
        "recovery-owner".parse().unwrap(), admitted,
        third_control.commitment().unwrap(), third_control, None,
    ).unwrap();
    assert_eq!(third.state().versions, published.versions);
    assert_eq!(third.state().commits, published.commits);
    assert_eq!(third.state().branches, published.branches);
    for (prior_operation, receipt) in published.operations.iter() {
        assert_eq!(third.state().operations.get(prior_operation), Some(receipt));
        assert_eq!(third.state().operation_principals.get(prior_operation),
            published.operation_principals.get(prior_operation));
    }
    assert_eq!(third.state().operations.len(), published.operations.len() + 1);
    assert_eq!(third.directory_storage_roots().unwrap(), descriptions);
    let third_state = third.state().clone();
    drop(third);
    let third_cold = Engine::open(&system).unwrap();
    assert_eq!(third_cold.state(), &third_state);
    assert_eq!(fs::read(third_cold.valid_replica(&successor.version_id).unwrap().0.path()).unwrap(),
        b"ordinary successor after recovery");
    assert_eq!(fs::read(third_cold.valid_replica(&file.version_id).unwrap().0.path()).unwrap(),
        b"original immutable bytes");
    assert_eq!(image(&second_lost), second_lost_image);
    assert_eq!(image(&lost), lost_image);
    drop(third_cold);
    let retained = temp.path().join("retained-recovery-carrier");
    fs::rename(&system, &retained).unwrap();
    fn copy_tree(source: &std::path::Path, destination: &std::path::Path) {
        fs::create_dir(destination).unwrap();
        fs::set_permissions(destination, fs::metadata(source).unwrap().permissions()).unwrap();
        for entry in fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let target = destination.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_tree(&entry.path(), &target);
            } else {
                assert!(entry.file_type().unwrap().is_file());
                fs::copy(entry.path(), target).unwrap();
            }
        }
    }
    copy_tree(&retained, &system);
    let before = image(temp.path());
    assert!(Engine::open(&system).is_err());
    assert_eq!(image(temp.path()), before);
}

#[test]
fn recovery_refuses_existing_destination_before_source_publication() {
    let (temp, system, config) = fixture();
    drop(Engine::initialize_storage(
        &system, config.clone(), OperationId::generate().unwrap(),
    ).unwrap());
    let destination = temp.path().join("explicit-destination");
    fs::create_dir(&destination).unwrap();
    fs::set_permissions(&destination, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(destination.join("unclaimed"), b"preserve this evidence").unwrap();
    let handles = config.disks.iter()
        .map(|disk| PrivateDirectory::open_existing(&disk.root).unwrap()).collect();
    let admitted = Engine::admit_storage_recovery(handles).unwrap();
    let operation = OperationId::generate().unwrap();
    let control = control(&destination, operation, admitted.evidence_commitment());
    let before = image(temp.path());
    assert!(Engine::restore_storage_held(
        PrivateDirectory::open_existing(&destination).unwrap(), operation,
        "recovery-owner".parse().unwrap(), admitted,
        control.commitment().unwrap(), control, None,
    ).is_err());
    assert_eq!(image(temp.path()), before);
}
