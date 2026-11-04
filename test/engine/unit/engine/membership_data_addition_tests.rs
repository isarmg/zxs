use super::*;

fn initialize() -> (tempfile::TempDir, Engine, String, String) {
    let temp = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let config = Config::development((0..2).map(|n| temp.path().join(format!("old-{n}")))).unwrap();
    let mut engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
    let repository = engine
        .create_repository(
            &RepositoryName::parse("original").unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let branch = repository.main_branch_id;
    let version = write_body(&mut engine, &branch, b"original body");
    (temp, engine, branch, version)
}

fn begin_additions(
    engine: &mut Engine,
    root: &Path,
    roles: &[bool],
    label: &str,
) -> (Vec<DiskConfig>, MembershipControlArchive, OperationId) {
    let mut target = (*engine.config).clone();
    let previous = target.disks.len();
    let generated =
        Config::development((0..previous + roles.len()).map(|n| root.join(format!("{label}-{n}"))))
            .unwrap();
    let added = generated
        .disks
        .into_iter()
        .skip(previous)
        .zip(roles)
        .map(|(mut disk, metadata)| {
            disk.metadata_member = *metadata;
            disk.level = 1;
            disk.upstream_slot_id = None;
            private_dir(&disk.root).unwrap();
            disk
        })
        .collect::<Vec<_>>();
    target.disks.extend(added.iter().cloned());
    let operation = OperationId::generate().unwrap();
    engine
        .begin_membership_handoff(target, "admin", operation)
        .unwrap();
    let path = root.join(format!("{label}-archive"));
    private_dir(&path).unwrap();
    let archive = engine.build_membership_control_archive(&path).unwrap();
    (added, archive, operation)
}
fn enroll(
    engine: &mut Engine,
    root: &Path,
    label: &str,
    added: &[DiskConfig],
    archive: &MembershipControlArchive,
) -> Result<PendingMembershipStatus> {
    engine.enroll_pending_members(
        &root.join(format!("{label}-archive")),
        &archive.root_hash,
        added
            .iter()
            .map(|disk| PendingMemberDirectory {
                disk_id: disk.disk_id.clone(),
                directory: File::open(&disk.root).unwrap(),
            })
            .collect(),
    )
}
fn write_new_primary(engine: &mut Engine, disk: &DiskConfig, name: &str, body: &[u8]) -> String {
    let repository = engine
        .create_repository(
            &RepositoryName::parse(name).unwrap(),
            &disk.slot_id,
            1,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    write_body(engine, &repository.main_branch_id, body)
}
fn assert_lock_held(root: &Path) {
    let file = File::open(root.join(".writer.lock")).unwrap();
    assert!(
        rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive).is_err()
    );
}

#[test]
fn membership_activation_data_only_then_mixed_keeps_voters_and_standalone_proofs() {
    let (temp, mut engine, _, original) = initialize();
    let original_config = engine.config.clone();
    let old_operations = engine.state.operations.clone();
    let (first_added, archive, first_operation) =
        begin_additions(&mut engine, temp.path(), &[false], "data");
    let pending = enroll(&mut engine, temp.path(), "data", &first_added, &archive).unwrap();
    assert_eq!(pending.target_metadata_members, 2);
    assert_eq!(pending.added_data_disks, 1);
    assert_eq!(
        pending.ready_data_disks,
        vec![first_added[0].disk_id.clone()]
    );
    assert_lock_held(&first_added[0].root);
    let joint = engine.resume_membership_joint_decision().unwrap();
    assert_eq!((joint.original_members, joint.original_quorum), (2, 2));
    assert_eq!((joint.target_members, joint.target_quorum), (2, 2));
    assert_eq!(joint.votes.len(), 2);
    assert!(joint.all_certificates_synchronized());
    assert!(
        joint
            .votes
            .iter()
            .all(|vote| vote.disk_id != first_added[0].disk_id)
    );
    let first = engine.activate_pending_membership().unwrap();
    assert_eq!(first.operation_id, first_operation);
    assert_eq!(first.result["membership_epoch"], "2");
    assert!(original_config.same_metadata_membership(&engine.config));
    assert_eq!(engine.disk_resources.handles().count(), 3);
    let portable = &engine.config.generation().unwrap().portable;
    assert_eq!(portable.joint.verify().unwrap().data_disks.len(), 1);
    let mut missing = json_codec::to_value(portable).unwrap();
    let joint = missing["joint"].as_object_mut().unwrap();
    let proposal_key = format!(
        "membership-joint-proposal/{}",
        joint["proposal_hash"].as_str().unwrap()
    );
    let objects = joint.get_mut("objects").unwrap().as_object_mut().unwrap();
    let reference = &objects[&proposal_key]["data_disks"];
    let page = format!(
        "{}/{}",
        reference["kind"].as_str().unwrap(),
        reference["hash"].as_str().unwrap()
    );
    assert!(objects.remove(&page).is_some());
    let missing: PortableAuthority = json_codec::from_value(missing).unwrap();
    assert!(missing.intent().is_err());
    assert_lock_held(&first_added[0].root);
    assert!(
        !first_added[0]
            .root
            .join("metadata/membership-history")
            .exists()
    );
    let middle = write_new_primary(&mut engine, &first_added[0], "middle", b"data epoch two");
    let middle_record = engine.state.versions[&middle].clone();
    let middle_proof = version_dir(&first_added[0], &middle_record.history_tree_id, &middle)
        .join("publication-proof.json");
    let middle_bytes = fs::read(&middle_proof).unwrap();
    let (mixed, archive, second_operation) =
        begin_additions(&mut engine, temp.path(), &[true, false], "mixed");
    enroll(&mut engine, temp.path(), "mixed", &mixed, &archive).unwrap();
    let joint = engine.resume_membership_joint_decision().unwrap();
    assert_eq!((joint.original_members, joint.original_quorum), (2, 2));
    assert_eq!((joint.target_members, joint.target_quorum), (3, 2));
    assert_eq!(joint.votes.len(), 3);
    assert_eq!(joint.added_data_disks, 1);
    let second = engine.activate_pending_membership().unwrap();
    assert_eq!(second.operation_id, second_operation);
    assert_eq!(second.result["membership_epoch"], "3");
    assert_eq!(engine.disk_resources.handles().count(), 5);
    for (id, result) in old_operations.iter() {
        assert_eq!(&engine.state.operations[id], result);
    }
    assert_eq!(
        engine.operation_record(first_operation).unwrap().result(),
        &first
    );
    assert_eq!(fs::read(&middle_proof).unwrap(), middle_bytes);
    let latest = write_new_primary(&mut engine, &mixed[1], "latest", b"data epoch three");
    let dependencies = [&first_added[0], &mixed[1]].map(|disk| {
        engine
            .config
            .generation()
            .unwrap()
            .portable
            .data_dependency(&disk.disk_id)
            .unwrap()
            .unwrap()
    });
    let config = engine.config.clone();
    drop(engine);
    let mut reopened = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(reopened.activate_pending_membership().unwrap(), second);
    reopened.open_content(&original).unwrap();
    reopened.open_content(&middle).unwrap();
    reopened.open_content(&latest).unwrap();
    assert_lock_held(&first_added[0].root);
    assert_lock_held(&mixed[1].root);
    drop(reopened);
    fs::rename(
        temp.path().join("system"),
        temp.path().join("hidden-system"),
    )
    .unwrap();
    for (i, disk) in config.metadata_members().enumerate() {
        fs::rename(&disk.root, temp.path().join(format!("hidden-meta-{i}"))).unwrap();
    }
    for (disk, version, expected, dependency) in [
        (
            &first_added[0],
            middle,
            b"data epoch two".as_slice(),
            &dependencies[0],
        ),
        (
            &mixed[1],
            latest,
            b"data epoch three".as_slice(),
            &dependencies[1],
        ),
    ] {
        let readiness = dependency.path(&disk.root);
        let saved = fs::read(&readiness).unwrap();
        fs::remove_file(&readiness).unwrap();
        assert!(
            offline_export(
                &disk.root,
                &version,
                &temp.path().join(format!("missing-{version}"))
            )
            .is_err()
        );
        fs::write(readiness, saved).unwrap();
        let output = temp.path().join(format!("standalone-{version}"));
        offline_export(&disk.root, &version, &output).unwrap();
        assert_eq!(fs::read(output).unwrap(), expected);
        assert!(offline_check(&disk.root).unwrap().issues.is_empty());
        assert!(files(&disk.root).keys().all(|path| {
            path.file_name().unwrap() != "journal.bin"
                && !path
                    .components()
                    .any(|part| part.as_os_str() == CONTROL_ARCHIVE_DIRECTORY)
        }));
    }
}

#[test]
fn membership_activation_data_ready_loss_blocks_joint_and_activation_without_votes() {
    let (temp, mut engine, _, _) = initialize();
    let (added, archive, _) = begin_additions(&mut engine, temp.path(), &[false], "ready");
    enroll(&mut engine, temp.path(), "ready", &added, &archive).unwrap();
    let readiness = engine.pending_membership.as_ref().unwrap().data_disks[0]
        .dependency
        .path(&added[0].root);
    let bytes = fs::read(&readiness).unwrap();
    fs::remove_file(&readiness).unwrap();
    assert!(engine.resume_membership_joint_decision().is_err());
    assert!(engine.membership_joint_decision_status().is_none());
    assert!(engine.blocked);
    fs::write(&readiness, &bytes).unwrap();
    engine.resume_membership_joint_decision().unwrap();
    fs::write(&readiness, b"unbound readiness").unwrap();
    let before = files(&added[0].root);
    assert!(engine.activate_pending_membership().is_err());
    assert_eq!(files(&added[0].root), before);
    assert!(engine.config.generation().is_none());
    fs::write(readiness, bytes).unwrap();
    engine.resume_membership_joint_decision().unwrap();
    engine.activate_pending_membership().unwrap();
}

#[test]
fn membership_activation_data_ready_interrupt_reopens_exact_pending_root() {
    let (temp, mut engine, _, _) = initialize();
    let (added, archive, operation) =
        begin_additions(&mut engine, temp.path(), &[false], "ready-crash");
    engine.inject_fault_once(&format!("after_membership_data_ready_{}", added[0].disk_id));
    assert!(enroll(&mut engine, temp.path(), "ready-crash", &added, &archive).is_err());
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    assert!(engine.blocked);
    assert!(engine.disk_resources.available(&added[0]).is_none());
    enroll(&mut engine, temp.path(), "ready-crash", &added, &archive).unwrap();
    engine.resume_membership_joint_decision().unwrap();
    assert_eq!(
        engine.activate_pending_membership().unwrap().operation_id,
        operation
    );
    assert_lock_held(&added[0].root);
}

#[test]
fn membership_activation_data_addition_faults_resume_same_authority() {
    for roles in [&[false][..], &[true, false][..]] {
        for stage in [
            "marker",
            "seal",
            "after_prepare_1",
            "after_accept_1",
            "after_finalize",
            "after_publication_1",
            "before_membership_activation_projection",
        ] {
            let (temp, mut engine, _, original) = initialize();
            let (added, archive, operation) =
                begin_additions(&mut engine, temp.path(), roles, "fault");
            enroll(&mut engine, temp.path(), "fault", &added, &archive).unwrap();
            engine.resume_membership_joint_decision().unwrap();
            let data = added.iter().find(|disk| !disk.metadata_member).unwrap();
            let point = match stage {
                "marker" => format!("after_membership_activation_marker_{}", data.disk_id),
                "seal" => format!("after_membership_activation_seal_{}", data.disk_id),
                other => other.to_owned(),
            };
            engine.inject_fault_once(&point);
            assert!(engine.activate_pending_membership().is_err(), "{point}");
            drop(engine);
            let mut reopened = if matches!(stage, "marker" | "seal") {
                Engine::resume_membership_activation(&temp.path().join("system")).unwrap()
            } else {
                Engine::open(&temp.path().join("system")).unwrap()
            };
            let expected_voters = 2 + roles.iter().filter(|voter| **voter).count();
            assert_eq!(reopened.config.metadata_members().len(), expected_voters);
            assert_eq!(reopened.config.metadata_quorum(), 2);
            assert_eq!(reopened.config.membership_epoch(), 2);
            let result = reopened
                .operation_record(operation)
                .unwrap()
                .result()
                .clone();
            assert_eq!(reopened.activate_pending_membership().unwrap(), result);
            reopened.open_content(&original).unwrap();
            let version =
                write_new_primary(&mut reopened, data, "after-crash", b"resumed data root");
            reopened.open_content(&version).unwrap();
            assert_lock_held(&data.root);
        }
    }
}

#[test]
fn membership_activation_data_addition_restores_from_voters_not_data_roots() {
    for roles in [&[false][..], &[true, false][..]] {
        let (temp, mut engine, _, _) = initialize();
        let (added, archive, operation) =
            begin_additions(&mut engine, temp.path(), roles, "restore-data");
        enroll(&mut engine, temp.path(), "restore-data", &added, &archive).unwrap();
        engine.resume_membership_joint_decision().unwrap();
        let result = engine.activate_pending_membership().unwrap();
        let data = added.iter().find(|disk| !disk.metadata_member).unwrap();
        let version = write_new_primary(
            &mut engine,
            data,
            "restore-body",
            b"restore data registration",
        );
        let config = engine.config.clone();
        drop(engine);
        fs::rename(temp.path().join("system"), temp.path().join("lost-system")).unwrap();
        assert!(
            Engine::restore_from_disks(
                &temp.path().join("not-a-quorum"),
                &[
                    config.metadata_members().next().unwrap().root.clone(),
                    data.root.clone()
                ]
            )
            .is_err()
        );
        let count = config.metadata_members().len();
        let mut roots = Vec::new();
        for (i, disk) in config.metadata_members().enumerate() {
            let moved = temp.path().join(format!("moved-{i}"));
            fs::rename(&disk.root, &moved).unwrap();
            if count == 2 || i != 0 {
                roots.push(moved);
            }
        }
        let restored = Engine::restore_from_disks(&temp.path().join("restored"), &roots).unwrap();
        assert_eq!(restored.config.metadata_members().len(), count);
        assert_eq!(restored.config.metadata_quorum(), 2);
        assert_eq!(restored.config.membership_epoch(), 2);
        assert_eq!(
            restored.operation_record(operation).unwrap().result(),
            &result
        );
        restored.open_content(&version).unwrap();
        assert_lock_held(&data.root);
        if count == 3 {
            assert!(restored.blocked);
        }
    }
}

#[test]
fn membership_activation_data_identity_cannot_be_rebound_by_rewriting_pending_seal() {
    let (temp, mut engine, _, _) = initialize();
    let (added, archive, _) = begin_additions(&mut engine, temp.path(), &[false], "identity-data");
    enroll(&mut engine, temp.path(), "identity-data", &added, &archive).unwrap();
    engine.resume_membership_joint_decision().unwrap();
    let first = engine.members()[0].disk_id.clone();
    engine.inject_fault_once(&format!("after_membership_activation_marker_{first}"));
    assert!(engine.activate_pending_membership().is_err());
    drop(engine);
    let disk = &added[0];
    let original = temp.path().join("held-original-data");
    fs::rename(&disk.root, &original).unwrap();
    private_dir(&disk.root).unwrap();
    for (path, bytes) in files(&original) {
        let destination = disk.root.join(path.strip_prefix(&original).unwrap());
        private_dir(destination.parent().unwrap()).unwrap();
        fs::write(destination, bytes).unwrap();
    }
    let root = DiskRoot::open(&disk.root).unwrap();
    let physical = FileIdentity::from_fd(root.directory(root.path()).unwrap()).unwrap();
    let mut seal: PendingDiskSeal = read_json(&disk.root.join("disk.json")).unwrap();
    seal.identity.device = physical.device;
    seal.identity.inode = physical.inode;
    write_json(&disk.root.join("disk.json"), &seal, true).unwrap();
    let before = files(&disk.root);
    let error = match Engine::resume_membership_activation(&temp.path().join("system")) {
        Ok(_) => panic!("self-rewritten pending seal cannot replace certified identity"),
        Err(error) => error,
    };
    assert_eq!(error.code(), ErrorCode::DiskIdentityMismatch);
    assert_eq!(files(&disk.root), before);
}

#[test]
fn membership_activation_data_backup_addition_claims_only_new_body_replicas() {
    let (temp, mut engine, _, original) = initialize();
    let original_version = engine.state.versions[&original].clone();
    let mut target = (*engine.config).clone();
    let mut backup = Config::development((0..3).map(|n| temp.path().join(format!("backup-{n}"))))
        .unwrap()
        .disks
        .pop()
        .unwrap();
    backup.metadata_member = false;
    private_dir(&backup.root).unwrap();
    target.disks.push(backup.clone());
    engine
        .begin_membership_handoff(target, "admin", OperationId::generate().unwrap())
        .unwrap();
    let archive_path = temp.path().join("backup-archive");
    private_dir(&archive_path).unwrap();
    let archive = engine
        .build_membership_control_archive(&archive_path)
        .unwrap();
    enroll(
        &mut engine,
        temp.path(),
        "backup",
        std::slice::from_ref(&backup),
        &archive,
    )
    .unwrap();
    engine.resume_membership_joint_decision().unwrap();
    engine.activate_pending_membership().unwrap();
    assert!(!version_dir(&backup, &original_version.history_tree_id, &original).exists());
    let repository = engine
        .create_repository(
            &RepositoryName::parse("three replicas").unwrap(),
            "P01",
            3,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let version = write_body(
        &mut engine,
        &repository.main_branch_id,
        b"new third replica",
    );
    let config = engine.config.clone();
    drop(engine);
    fs::rename(temp.path().join("system"), temp.path().join("lost-system")).unwrap();
    for (i, disk) in config.metadata_members().enumerate() {
        fs::rename(&disk.root, temp.path().join(format!("unavailable-{i}"))).unwrap();
    }
    let output = temp.path().join("backup-export");
    offline_export(&backup.root, &version, &output).unwrap();
    assert_eq!(fs::read(output).unwrap(), b"new third replica");
    assert!(!version_dir(&backup, &original_version.history_tree_id, &original).exists());
}

struct InterruptedReadiness {
    temp: tempfile::TempDir,
    engine: Engine,
    added: Vec<DiskConfig>,
    archive: MembershipControlArchive,
    operation: OperationId,
    path: PathBuf,
    expected: Vec<u8>,
}
fn interrupted_readiness() -> InterruptedReadiness {
    let (temp, mut engine, _, _) = initialize();
    let (added, archive, operation) =
        begin_additions(&mut engine, temp.path(), &[false], "staging");
    engine.inject_fault_once("after_membership_pending_seal_1");
    assert!(enroll(&mut engine, temp.path(), "staging", &added, &archive).is_err());
    let dependency = &engine.pending_membership.as_ref().unwrap().data_disks[0].dependency;
    let path = dependency.path(&added[0].root);
    let expected = encode_metadata(dependency, MAX_METADATA).unwrap();
    private_dir(path.parent().unwrap()).unwrap();
    assert!(!path.exists());
    InterruptedReadiness {
        temp,
        engine,
        added,
        archive,
        operation,
        path,
        expected,
    }
}

#[test]
fn membership_activation_data_ready_staging_prefixes_resume_without_consuming_evidence() {
    for part in ["empty", "partial", "complete"] {
        let InterruptedReadiness {
            temp,
            engine,
            added,
            archive,
            operation,
            path,
            expected,
        } = interrupted_readiness();
        let bytes = match part {
            "empty" => &expected[..0],
            "partial" => &expected[..expected.len() / 2],
            "complete" => expected.as_slice(),
            _ => unreachable!(),
        };
        let staging = path
            .parent()
            .unwrap()
            .join(".state-atomic-0123456789abcdef0123456789abcdef.tmp");
        fs::write(&staging, bytes).unwrap();
        drop(engine);
        let mut reopened = Engine::open(&temp.path().join("system")).unwrap();
        assert!(reopened.blocked);
        enroll(&mut reopened, temp.path(), "staging", &added, &archive).unwrap();
        assert_eq!(fs::read(&path).unwrap(), expected);
        assert_eq!(fs::read(&staging).unwrap(), bytes);
        reopened.resume_membership_joint_decision().unwrap();
        assert_eq!(
            reopened.activate_pending_membership().unwrap().operation_id,
            operation
        );
        assert_eq!(fs::read(staging).unwrap(), bytes);
    }
}

#[test]
fn membership_activation_data_ready_foreign_staging_and_corrupt_destination_stay_untouched() {
    for case in ["wrong", "foreign", "bad-ready", "bad-temp-with-ready"] {
        let InterruptedReadiness {
            temp,
            engine,
            added,
            archive,
            path,
            expected,
            ..
        } = interrupted_readiness();
        let staging = path
            .parent()
            .unwrap()
            .join(".state-atomic-fedcba9876543210fedcba9876543210.tmp");
        let bytes = match case {
            "foreign" => {
                let mut value: json_codec::Value = decode_strict(&expected).unwrap();
                value["seal"]["transition_hash"] = json_codec::json!("f".repeat(64));
                canonical_json(&value).unwrap()
            }
            "bad-ready" => {
                fs::write(&path, b"broken official readiness").unwrap();
                expected.clone()
            }
            "bad-temp-with-ready" => {
                fs::write(&path, &expected).unwrap();
                b"unknown staging".to_vec()
            }
            "wrong" => b"unknown staging".to_vec(),
            _ => unreachable!(),
        };
        fs::write(&staging, bytes).unwrap();
        drop(engine);
        let mut reopened = Engine::open(&temp.path().join("system")).unwrap();
        let before = files(&added[0].root);
        assert!(
            enroll(&mut reopened, temp.path(), "staging", &added, &archive).is_err(),
            "{case}"
        );
        assert_eq!(files(&added[0].root), before, "{case}");
        assert!(reopened.blocked);
        assert!(reopened.membership_joint_decision_status().is_none());
    }
}
