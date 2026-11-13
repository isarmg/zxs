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
    }, source, "recovery-owner".parse().unwrap(), vec![crate::RecoveryDiskSelection::parse("selected-members").unwrap()],
        crate::StorageRecoveryPredecessor::NewControl)
}

fn assert_history_index_tracks_admitted_state(engine: &Engine) {
    assert!(matches!(engine.index_health(), crate::IndexHealth::Ready { sequence }
        if sequence == engine.state().sequence));
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
    let original_seal = fs::read_dir(config.disks[0].root.join("metadata/publication-seals"))
        .unwrap().map(|entry| entry.unwrap().path()).find(|path| {
            let seal: crate::journal::PublicationSeal = crate::decode_strict(
                &fs::read(path).unwrap(),
            ).unwrap();
            let publication: crate::journal::Publication = crate::journal::load_object_at(
                &engine.disk_resources.get(&config.disks[0]).unwrap().root,
                &config.disks[0], "publication", &seal.publication_hash,
            ).unwrap();
            publication.result.sequence == original.sequence
        }).unwrap();
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
    engine.require_recovery_control(Some(&control)).unwrap();
    assert_history_index_tracks_admitted_state(&engine);
    let mut changed_control = control.clone();
    changed_control.disk_ids.push(crate::RecoveryDiskSelection::parse("changed-selection").unwrap());
    let control_before = image(temp.path());
    assert!(engine.require_recovery_control(Some(&changed_control)).is_err());
    assert_eq!(image(temp.path()), control_before);
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
    for _ in 0..2 {
        assert!(Engine::open(&system).is_err());
        assert_eq!(image(temp.path()), before_old_carrier);
        assert!(Engine::open_storage_held(
            platform_io::fs_safety::PrivateDirectory::open_existing(&system).unwrap(),
            None,
        ).is_err());
        assert_eq!(image(temp.path()), before_old_carrier);
        assert!(Engine::open_storage_for_recovery_held(
            platform_io::fs_safety::PrivateDirectory::open_existing(&system).unwrap(),
            None,
        ).is_err());
        assert_eq!(image(temp.path()), before_old_carrier);
    }
    // Missing one old seal still leaves the complete journal barrier as an
    // independent tip proof. It must not authorize repair on the old carrier.
    let retained_seal = original_seal.with_extension("retained");
    fs::rename(&original_seal, &retained_seal).unwrap();
    let before_missing_seal = image(temp.path());
    for _ in 0..2 {
        assert!(Engine::open(&system).is_err());
        assert_eq!(image(temp.path()), before_missing_seal);
        assert!(Engine::open_storage_for_recovery_held(
            PrivateDirectory::open_existing(&system).unwrap(), None,
        ).is_err());
        assert_eq!(image(temp.path()), before_missing_seal);
    }
    fs::rename(&retained_seal, &original_seal).unwrap();
    fs::rename(&system, &lost).unwrap();
    fs::rename(&recovered_carrier, &system).unwrap();
    let cold = Engine::open(&system).unwrap();
    assert_history_index_tracks_admitted_state(&cold);
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
    assert_history_index_tracks_admitted_state(&repeated);
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
    assert_history_index_tracks_admitted_state(&third);
    drop(third);
    let third_cold = Engine::open(&system).unwrap();
    assert_history_index_tracks_admitted_state(&third_cold);
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

#[test]
fn recovery_inventory_labels_require_the_local_selection_contract() {
    for value in ["", "../disk", "/srv/disk", "disk one"] {
        let error = crate::RecoveryDiskSelection::parse(value).unwrap_err();
        assert_eq!(error.code(), storage_contract::ErrorCode::InvalidId);
        assert!(matches!(error, crate::Error::RecoveryDiskSelection(
            crate::InvalidRecoveryDiskSelection)));
        let bytes = storage_contract::canonical_json(&value).unwrap();
        assert!(storage_contract::decode_strict::<crate::RecoveryDiskSelection>(&bytes).is_err());
    }
    assert!(crate::RecoveryDiskSelection::parse("local-disk_1").is_ok());
}

#[test]
fn obsolete_carrier_cannot_repair_a_short_barrier_prefix_before_later_recovery() {
    use crate::journal::{JournalFrameReader, RecordType};

    fn assert_short_prefix_refused(result: crate::Result<Engine>) {
        match result {
            Err(crate::Error::FreshBootstrap(source)) => assert!(matches!(
                source.as_ref(), crate::authority_selection::FreshBootstrapError::Incomplete
            ), "expected the incomplete published-prefix guard, got {source:?}"),
            Err(error) => panic!("expected the published-prefix guard, got {error:?}"),
            Ok(_) => panic!("obsolete carrier admitted a short published prefix"),
        }
    }

    let (temp, system, config) = fixture();
    let mut engine = Engine::initialize_storage(
        &system, config.clone(), OperationId::generate().unwrap(),
    ).unwrap();
    let repository = engine.create_repository(
        &storage_contract::RepositoryName::parse("short barrier source").unwrap(),
        "P01", 1, "admin", OperationId::generate().unwrap(),
    ).unwrap();
    let workspace = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
    engine.stage_bytes(&workspace.workspace_id, "original.txt", b"before recovery").unwrap();
    let saved = engine.commit_workspace(crate::CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: OperationId::generate().unwrap(),
        principal: "admin".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "ordinary FILE before recovery".into(),
        request_digest: None,
    }).unwrap();
    let mut seals = Vec::new();
    for disk in &config.disks {
        let seal = fs::read_dir(disk.root.join("metadata/publication-seals"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                let seal: crate::journal::PublicationSeal = crate::decode_strict(
                    &fs::read(path).unwrap(),
                ).unwrap();
                let publication: crate::journal::Publication = crate::journal::load_object_at(
                    &engine.disk_resources.get(disk).unwrap().root,
                    disk, "publication", &seal.publication_hash,
                ).unwrap();
                publication.result.sequence == saved.sequence
            })
            .expect("each member retains the pre-recovery FILE publication seal");
        seals.push(seal);
    }
    drop(engine);
    let original_system = temp.path().join("preserved-original-system");
    fs::rename(&system, &original_system).unwrap();
    fs::create_dir(&system).unwrap();
    fs::set_permissions(&system, fs::Permissions::from_mode(0o700)).unwrap();
    let handles = config.disks.iter()
        .map(|disk| PrivateDirectory::open_existing(&disk.root).unwrap()).collect();
    let admitted = Engine::admit_storage_recovery(handles).unwrap();
    let operation = OperationId::generate().unwrap();
    let control = control(&system, operation, admitted.evidence_commitment());
    let recovered = Engine::restore_storage_held(
        PrivateDirectory::open_existing(&system).unwrap(), operation,
        "recovery-owner".parse().unwrap(), admitted,
        control.commitment().unwrap(), control, None,
    ).unwrap();
    let recovery_sequence = recovered.state().operations.get(&operation).unwrap().sequence;
    assert!(recovery_sequence > saved.sequence);
    drop(recovered);

    // Retain the exact complete journal bytes outside the managed metadata.
    // Only member zero loses a suffix, at a real authenticated frame boundary.
    let retained = temp.path().join("retained-test-damage");
    fs::create_dir(&retained).unwrap();
    let journal = crate::journal::log_path(&config.disks[0]);
    let original_bytes = fs::read(&journal).unwrap();
    fs::write(retained.join("complete-member-zero-journal"), &original_bytes).unwrap();
    let mut reader = JournalFrameReader::new(original_bytes.as_slice());
    let boundary = loop {
        let (record, _) = reader.next_record().unwrap().expect("FILE Finalize exists");
        if record.sequence == saved.sequence && record.record_type == RecordType::Finalize {
            break reader.valid_bytes;
        }
    };
    assert!(boundary < original_bytes.len() as u64);
    let file = fs::OpenOptions::new().write(true).open(&journal).unwrap();
    file.set_len(boundary).unwrap();
    file.sync_all().unwrap();
    drop(file);
    for (index, seal) in seals.iter().enumerate() {
        fs::rename(seal, retained.join(format!("member-{index}-FILE-publication-seal"))).unwrap();
    }

    // All streams remain valid, untorn journals. The union still contains the
    // later recovery, but slot S cannot assemble all three Publication receipts
    // and has no remaining seal fallback. This exercises the actual early break,
    // rather than relying on missing seals with a complete journal barrier.
    for (index, disk) in config.disks.iter().enumerate() {
        let bytes = fs::read(crate::journal::log_path(disk)).unwrap();
        let mut reader = JournalFrameReader::new(bytes.as_slice());
        let mut last = None;
        let mut recovery_publication = false;
        let mut saved_publication = false;
        while let Some((record, _)) = reader.next_record().unwrap() {
            recovery_publication |= record.sequence == recovery_sequence
                && record.record_type == RecordType::Publication;
            saved_publication |= record.sequence == saved.sequence
                && record.record_type == RecordType::Publication;
            last = Some(record);
        }
        assert!(!reader.torn_tail);
        assert_eq!(reader.valid_bytes, bytes.len() as u64);
        if index == 0 {
            let last = last.unwrap();
            assert_eq!(last.sequence, saved.sequence);
            assert_eq!(last.record_type, RecordType::Finalize);
            assert!(!saved_publication);
            assert!(!recovery_publication);
        } else {
            assert!(saved_publication);
            assert!(recovery_publication);
        }
        assert!(!seals[index].exists());
    }
    fs::rename(&system, temp.path().join("preserved-recovery-system")).unwrap();
    fs::rename(&original_system, &system).unwrap();
    let before = image(temp.path());
    for _ in 0..2 {
        assert_short_prefix_refused(Engine::open(&system));
        assert_eq!(image(temp.path()), before);
        assert_short_prefix_refused(Engine::open_storage_held(
            PrivateDirectory::open_existing(&system).unwrap(), None,
        ));
        assert_eq!(image(temp.path()), before);
        assert_short_prefix_refused(Engine::open_storage_for_recovery_held(
            PrivateDirectory::open_existing(&system).unwrap(), None,
        ));
        assert_eq!(image(temp.path()), before);
    }
    // This deliberately damaged fixture ends at refusal. It does not claim
    // that a short prefix is sufficient authority to repair the current carrier.
}
