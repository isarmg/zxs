use super::*;
use std::{fs, os::unix::fs::PermissionsExt};

struct Fixture {
    temp: tempfile::TempDir,
    original: Config,
    target: Config,
    engine: Engine,
    operation: OperationId,
    branch: String,
    history: String,
    version: String,
}
impl Fixture {
    fn new() -> Self {
        Self::prepared(|_, _| {})
    }
    fn prepared(prepare: impl FnOnce(&mut Engine, &str)) -> Self {
        let temp = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let mut original =
            Config::development((0..3).map(|n| temp.path().join(format!("old-{n}")))).unwrap();
        original.disks[0].metadata_member = false;
        let mut engine = Engine::initialize(&temp.path().join("system"), original.clone()).unwrap();
        let repository = engine
            .create_repository(
                &RepositoryName::parse("preserved").unwrap(),
                "P01",
                1,
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        let branch = repository.main_branch_id.clone();
        let history = repository.history_tree_id.clone();
        let version = write_body(&mut engine, &branch, b"before membership");
        prepare(&mut engine, &branch);
        engine
            .export_history_package(&history, &temp.path().join("old-package"))
            .unwrap();
        let mut target = original.clone();
        let additions =
            Config::development((0..4).map(|n| temp.path().join(format!("new-{n}")))).unwrap();
        let disk = additions.disks.into_iter().nth(3).unwrap();
        private_dir(&disk.root).unwrap();
        target.disks.push(disk);
        let operation = OperationId::generate().unwrap();
        engine
            .begin_membership_handoff(target.clone(), "admin", operation)
            .unwrap();
        let source = temp.path().join("archive");
        private_dir(&source).unwrap();
        let archive = engine.build_membership_control_archive(&source).unwrap();
        engine
            .enroll_pending_members(
                &source,
                &archive.root_hash,
                vec![PendingMemberDirectory {
                    disk_id: target.disks[3].disk_id.clone(),
                    directory: File::open(&target.disks[3].root).unwrap(),
                }],
            )
            .unwrap();
        engine.resume_membership_joint_decision().unwrap();
        Self {
            temp,
            original,
            target,
            engine,
            operation,
            branch,
            history,
            version,
        }
    }
    fn reopen(self) -> (tempfile::TempDir, Config, Engine, OperationId) {
        let Self {
            temp,
            target,
            engine,
            operation,
            ..
        } = self;
        drop(engine);
        let opened = Engine::open(&temp.path().join("system")).unwrap();
        (temp, target, opened, operation)
    }
}
#[test]
fn membership_activation_publishes_target_generation_and_reopens() {
    let mut fixture = Fixture::new();
    let logs = fixture
        .original
        .disks
        .iter()
        .filter(|disk| disk.metadata_member)
        .map(|disk| fs::read(crate::journal::log_path(disk)).unwrap())
        .collect::<Vec<_>>();
    let old_snapshot = fixture.engine.snapshot.clone();
    let old_operations = fixture.engine.state.operations.clone();
    let result = fixture.engine.activate_pending_membership().unwrap();
    assert_eq!(result.operation_id, fixture.operation);
    assert_eq!(result.result["membership_epoch"], "2");
    for (operation, original) in old_operations {
        assert_eq!(original.result["membership_epoch"], "1");
        assert_eq!(fixture.engine.state.operations[&operation], original);
    }
    assert_eq!(fixture.engine.state.topology_generation, 2);
    assert_ne!(fixture.engine.snapshot, old_snapshot);
    assert_eq!(fixture.engine.config.metadata_members().len(), 3);
    assert!(!fixture.engine.blocked);
    for (disk, bytes) in fixture
        .original
        .disks
        .iter()
        .filter(|disk| disk.metadata_member)
        .zip(logs)
    {
        assert_eq!(fs::read(crate::journal::log_path(disk)).unwrap(), bytes);
    }
    assert_eq!(
        fixture.engine.activate_pending_membership().unwrap(),
        result
    );
    let after_operation = OperationId::generate().unwrap();
    fixture
        .engine
        .create_repository(
            &RepositoryName::parse("after").unwrap(),
            "P01",
            1,
            "admin",
            after_operation,
        )
        .unwrap();
    let after = fixture.engine.operation_record(after_operation).unwrap();
    assert_eq!(after.result().result["membership_epoch"], "2");
    let (_temp, _target, mut engine, operation) = fixture.reopen();
    assert_eq!(engine.state.topology_generation, 2);
    assert_eq!(engine.state.repositories.len(), 2);
    assert_eq!(
        engine.operation_record(operation).unwrap().result(),
        &result
    );
    assert_eq!(engine.activate_pending_membership().unwrap(), result);
}

fn write_body(engine: &mut Engine, branch: &str, bytes: &[u8]) -> String {
    write_body_receipt(engine, branch, bytes).0
}
fn write_body_receipt(engine: &mut Engine, branch: &str, bytes: &[u8]) -> (String, Workspace) {
    let workspace = engine.create_workspace(branch, "admin").unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "body.txt", bytes)
        .unwrap();
    let before = engine.workspace(&workspace.workspace_id).unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: String::new(),
            request_digest: None,
        })
        .unwrap();
    (
        engine.list_tree(branch).unwrap()[0].version_id.clone(),
        before,
    )
}
fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.extend(files(&path));
        } else {
            out.insert(path.clone(), fs::read(path).unwrap());
        }
    }
    out
}
#[test]
fn membership_activation_data_only_old_new_proofs_and_packages_are_independent() {
    let mut fixture = Fixture::new();
    let primary = fixture.original.disks[0].clone();
    let old_proof =
        version_dir(&primary, &fixture.history, &fixture.version).join("publication-proof.json");
    let bytes = fs::read(&old_proof).unwrap();
    let old_publication: crate::journal::LocalPublication = decode_strict(&bytes).unwrap();
    fixture.engine.activate_pending_membership().unwrap();
    assert_eq!(fs::read(&old_proof).unwrap(), bytes);
    fixture.engine.open_content(&fixture.version).unwrap();
    let new_version = write_body(&mut fixture.engine, &fixture.branch, b"after membership");
    fixture
        .engine
        .export_history_package(&fixture.history, &fixture.temp.path().join("new-package"))
        .unwrap();
    let Fixture {
        temp,
        target,
        engine,
        version,
        ..
    } = fixture;
    drop(engine);
    fs::rename(temp.path().join("system"), temp.path().join("lost-system")).unwrap();
    for (n, disk) in target
        .disks
        .iter()
        .filter(|disk| disk.metadata_member)
        .enumerate()
    {
        fs::rename(&disk.root, temp.path().join(format!("offline-{n}"))).unwrap();
    }
    let history_proof = primary
        .root
        .join("metadata/membership-history")
        .join(format!(
            "{}.json",
            object_hash("candidate", &old_publication.candidate).unwrap()
        ));
    let saved_history = history_proof.with_extension("saved");
    fs::rename(&history_proof, &saved_history).unwrap();
    assert!(
        offline_export(
            &primary.root,
            &version,
            &temp.path().join("missing-history")
        )
        .is_err()
    );
    fs::rename(&saved_history, &history_proof).unwrap();
    let authority_path = primary.root.join(GENERATION_FILE);
    let authority_bytes = fs::read(&authority_path).unwrap();
    let mut incomplete: json_codec::Value = decode_strict(&authority_bytes).unwrap();
    let objects = incomplete["joint"]["objects"].as_object_mut().unwrap();
    let missing = objects.keys().next().unwrap().clone();
    objects.remove(&missing);
    fs::write(&authority_path, json_codec::to_vec(&incomplete).unwrap()).unwrap();
    assert!(
        offline_export(
            &primary.root,
            &new_version,
            &temp.path().join("missing-authority")
        )
        .is_err()
    );
    fs::write(&authority_path, authority_bytes).unwrap();
    for (id, expected) in [
        (version.clone(), b"before membership".as_slice()),
        (new_version.clone(), b"after membership".as_slice()),
    ] {
        let output = temp.path().join(format!("export-{id}"));
        offline_export(&primary.root, &id, &output).unwrap();
        assert_eq!(fs::read(output).unwrap(), expected);
    }
    assert!(offline_check(&primary.root).unwrap().issues.is_empty());
    crate::history_package::offline_check_history_package(&temp.path().join("old-package"))
        .unwrap();
    crate::history_package::offline_check_history_package(&temp.path().join("new-package"))
        .unwrap();
    for (package, id, expected) in [
        ("old-package", version, b"before membership".as_slice()),
        ("new-package", new_version, b"after membership".as_slice()),
    ] {
        let destination = temp.path().join(format!("restore-{package}"));
        let config = Config::development([destination.join("disk")]).unwrap();
        let mut restored = Engine::initialize(&destination.join("system"), config).unwrap();
        restored
            .restore_history_package(
                &temp.path().join(package),
                "P01",
                1,
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        let output = temp.path().join(format!("restored-{id}"));
        restored.export_version(&id, &output).unwrap();
        assert_eq!(fs::read(output).unwrap(), expected);
    }
    assert!(
        !primary
            .root
            .join("metadata/objects/membership-transition")
            .exists()
    );
}
#[test]
fn membership_activation_new_tombstone_keeps_old_body_proof() {
    let mut fixture = Fixture::new();
    let primary = fixture.original.disks[0].clone();
    let old_proof =
        version_dir(&primary, &fixture.history, &fixture.version).join("publication-proof.json");
    let bytes = fs::read(&old_proof).unwrap();
    fixture.engine.activate_pending_membership().unwrap();
    write_body(&mut fixture.engine, &fixture.branch, b"replacement");
    let plan = fixture
        .engine
        .create_purge_plan(&fixture.history, vec![fixture.version.clone()], "admin")
        .unwrap();
    fixture
        .engine
        .commit_plan(&plan.plan_id, "admin", OperationId::generate().unwrap())
        .unwrap();
    assert_eq!(fs::read(old_proof).unwrap(), bytes);
    let Fixture {
        temp,
        engine,
        version,
        ..
    } = fixture;
    drop(engine);
    assert_eq!(
        offline_export(&primary.root, &version, &temp.path().join("denied"))
            .unwrap_err()
            .code(),
        ErrorCode::ContentPurged
    );
    let report = offline_check(&primary.root).unwrap();
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    assert!(report.tombstoned_versions > 0);
}
#[test]
fn membership_activation_recovers_partial_markers_seals_and_publications() {
    for stage in [
        "marker",
        "seal",
        "after_prepare_1",
        "after_accept_1",
        "after_finalize",
        "after_publication_1",
        "before_membership_activation_projection",
    ] {
        let mut fixture = Fixture::new();
        let point = match stage {
            "marker" => format!(
                "after_membership_activation_marker_{}",
                fixture.target.disks[0].disk_id
            ),
            "seal" => format!(
                "after_membership_activation_seal_{}",
                fixture.target.disks[0].disk_id
            ),
            other => other.to_owned(),
        };
        fixture.engine.inject_fault_once(&point);
        assert!(
            fixture.engine.activate_pending_membership().is_err(),
            "{point}"
        );
        let Fixture {
            temp,
            engine,
            operation,
            ..
        } = fixture;
        drop(engine);
        let mut recovered = if matches!(stage, "marker" | "seal") {
            assert!(Engine::open(&temp.path().join("system")).is_err());
            Engine::resume_membership_activation(&temp.path().join("system")).unwrap()
        } else {
            Engine::open(&temp.path().join("system")).unwrap()
        };
        assert_eq!(recovered.state.topology_generation, 2, "{point}");
        assert!(!recovered.blocked, "{point}");
        let first = recovered
            .operation_record(operation)
            .unwrap()
            .result()
            .clone();
        assert_eq!(recovered.activate_pending_membership().unwrap(), first);
    }
}
#[test]
fn membership_activation_bad_generation_anchor_isolated_without_lowering_quorum() {
    let mut fixture = Fixture::new();
    fixture.engine.activate_pending_membership().unwrap();
    let latest = write_body(&mut fixture.engine, &fixture.branch, b"latest");
    let bad = fixture
        .target
        .disks
        .iter()
        .find(|disk| disk.metadata_member)
        .unwrap()
        .clone();
    let marker = bad.root.join(GENERATION_FILE);
    fs::write(&marker, b"broken").unwrap();
    let before = files(&bad.root);
    let (_temp, target, mut recovered, _operation) = fixture.reopen();
    assert!(recovered.blocked);
    assert_eq!(recovered.config.metadata_members().len(), 3);
    assert_eq!(recovered.config.metadata_quorum(), 2);
    assert!(recovered.disk_resources.available(&bad).is_none());
    let warnings = recovered.take_recovery_warnings();
    assert_eq!(warnings.seals.len(), 1);
    assert_eq!(
        warnings.seals[0].context(),
        &crate::engine::RecoverySealContext::MembershipAnchor {
            disk_id: bad.disk_id.clone(),
            slot_id: bad.slot_id.clone(),
        }
    );
    assert!(recovered.take_recovery_warnings().seals.is_empty());
    recovered.open_content(&latest).unwrap();
    assert!(
        recovered
            .create_repository(
                &RepositoryName::parse("blocked").unwrap(),
                "P01",
                1,
                "admin",
                OperationId::generate().unwrap()
            )
            .is_err()
    );
    assert_eq!(files(&bad.root), before);
    assert_eq!(target.disks.len(), 4);
}
#[test]
fn membership_activation_system_loss_and_rebound_metadata_roots_preserve_state() {
    let mut fixture = Fixture::new();
    let result = fixture.engine.activate_pending_membership().unwrap();
    let latest = write_body(&mut fixture.engine, &fixture.branch, b"latest");
    let Fixture {
        temp,
        target,
        engine,
        ..
    } = fixture;
    drop(engine);
    fs::rename(temp.path().join("system"), temp.path().join("lost-system")).unwrap();
    let mut moved = Vec::new();
    for (n, disk) in target
        .disks
        .iter()
        .filter(|disk| disk.metadata_member)
        .enumerate()
    {
        let path = temp.path().join(format!("moved-{n}"));
        fs::rename(&disk.root, &path).unwrap();
        moved.push(path);
    }
    let restored = Engine::restore_from_disks(&temp.path().join("restored"), &moved).unwrap();
    assert_eq!(restored.state.topology_generation, 2);
    assert_eq!(
        restored
            .operation_record(result.operation_id)
            .unwrap()
            .result(),
        &result
    );
    restored.open_content(&latest).unwrap();
}
#[test]
fn membership_activation_rejects_unarchived_old_candidate_and_repeat_handoff() {
    let mut fixture = Fixture::new();
    let old_snapshot = fixture.engine.snapshot.clone().unwrap();
    let old_disk = fixture
        .original
        .disks
        .iter()
        .find(|disk| disk.metadata_member)
        .unwrap();
    let scan = scan_at(
        &fixture.engine.disk_resources.get(old_disk).unwrap().root,
        old_disk,
    )
    .unwrap();
    let record = scan
        .records
        .iter()
        .find(|(record, _)| {
            record.record_type == RecordType::Publication
                && record.sequence == fixture.engine.state.sequence
        })
        .unwrap();
    let mut old: Candidate = load_object_at(
        &fixture.engine.disk_resources.get(old_disk).unwrap().root,
        old_disk,
        "candidate",
        &record.0.candidate_hash,
    )
    .unwrap();
    fixture.engine.activate_pending_membership().unwrap();
    fixture.engine.verify_candidate_membership(&old).unwrap();
    old.transaction_id.push('x');
    assert!(fixture.engine.verify_candidate_membership(&old).is_err());
    assert!(
        fixture
            .engine
            .begin_membership_handoff(
                fixture.target.clone(),
                "admin",
                OperationId::generate().unwrap()
            )
            .is_err()
    );
    assert_ne!(
        fixture.engine.snapshot.as_deref(),
        Some(old_snapshot.as_str())
    );
}

#[test]
fn membership_activation_target_majority_survives_loss_of_original_quorum() {
    let mut fixture = Fixture::new();
    let result = fixture.engine.activate_pending_membership().unwrap();
    let latest = write_body(&mut fixture.engine, &fixture.branch, b"new majority");
    let Fixture {
        temp,
        target,
        engine,
        ..
    } = fixture;
    drop(engine);
    fs::rename(temp.path().join("system"), temp.path().join("lost-system")).unwrap();
    fs::rename(
        &target.disks[1].root,
        temp.path().join("unavailable-original"),
    )
    .unwrap();
    let inputs = vec![target.disks[2].root.clone(), target.disks[3].root.clone()];
    let mut restored = Engine::restore_from_disks(&temp.path().join("restored"), &inputs).unwrap();
    assert_eq!(restored.config.metadata_members().len(), 3);
    assert_eq!(restored.config.metadata_quorum(), 2);
    assert!(restored.blocked);
    assert_eq!(restored.state.topology_generation, 2);
    assert_eq!(
        restored
            .operation_record(result.operation_id)
            .unwrap()
            .result(),
        &result
    );
    restored.open_content(&latest).unwrap();
    assert!(
        restored
            .create_repository(
                &RepositoryName::parse("blocked").unwrap(),
                "P01",
                1,
                "admin",
                OperationId::generate().unwrap()
            )
            .is_err()
    );
}

#[test]
fn membership_activation_empty_genesis_prescribes_first_publication() {
    let temp = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let original =
        Config::development((0..2).map(|n| temp.path().join(format!("old-{n}")))).unwrap();
    let mut engine = Engine::initialize(&temp.path().join("system"), original.clone()).unwrap();
    let mut target = original;
    let addition = Config::development((0..3).map(|n| temp.path().join(format!("new-{n}"))))
        .unwrap()
        .disks
        .pop()
        .unwrap();
    private_dir(&addition.root).unwrap();
    target.disks.push(addition.clone());
    let operation = OperationId::generate().unwrap();
    engine
        .begin_membership_handoff(target, "admin", operation)
        .unwrap();
    assert!(
        engine
            .membership_fence
            .as_ref()
            .unwrap()
            .intent
            .boundary
            .is_none()
    );
    let archive = temp.path().join("archive");
    private_dir(&archive).unwrap();
    let root = engine
        .build_membership_control_archive(&archive)
        .unwrap()
        .root_hash;
    engine
        .enroll_pending_members(
            &archive,
            &root,
            vec![PendingMemberDirectory {
                disk_id: addition.disk_id,
                directory: File::open(addition.root).unwrap(),
            }],
        )
        .unwrap();
    engine.resume_membership_joint_decision().unwrap();
    let result = engine.activate_pending_membership().unwrap();
    assert_eq!(result.sequence, 2);
    assert_eq!(engine.state.topology_generation, 2);
    engine
        .create_repository(
            &RepositoryName::parse("first").unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    drop(engine);
    let reopened = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(reopened.state.repositories.len(), 1);
    assert_eq!(
        reopened.operation_record(operation).unwrap().result(),
        &result
    );
}

#[test]
fn membership_activation_later_fence_prevents_reopening_for_writes() {
    let mut fixture = Fixture::new();
    fixture.engine.activate_pending_membership().unwrap();
    let disk = fixture.engine.members()[0].clone();
    let hash = fixture.engine.config.generation_hash().unwrap().to_owned();
    let sequence = fixture.engine.next_slot;
    fixture
        .engine
        .journals
        .get_mut(&disk.disk_id)
        .unwrap()
        .append(&disk, RecordType::MembershipFence, sequence, &hash, &hash)
        .unwrap();
    let Fixture { temp, engine, .. } = fixture;
    drop(engine);
    assert_eq!(
        Engine::open(&temp.path().join("system"))
            .err()
            .unwrap()
            .code(),
        ErrorCode::RecoveryRequired
    );
}

#[test]
fn membership_activation_unenrolled_empty_target_does_not_block_old_fence_reopen() {
    let temp = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let original =
        Config::development((0..2).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
    let mut engine = Engine::initialize(&temp.path().join("system"), original.clone()).unwrap();
    let mut target = original;
    let new = Config::development((0..3).map(|n| temp.path().join(format!("new-{n}"))))
        .unwrap()
        .disks
        .pop()
        .unwrap();
    private_dir(&new.root).unwrap();
    target.disks.push(new.clone());
    let status = engine
        .begin_membership_handoff(target, "admin", OperationId::generate().unwrap())
        .unwrap();
    drop(engine);
    let reopened = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(reopened.membership_fence_status(), Some(status));
    assert!(reopened.blocked);
    assert!(fs::read_dir(&new.root).unwrap().next().is_none());
}

#[test]
fn membership_activation_scrub_checks_frozen_archive_and_obeys_cancellation() {
    let mut fixture = Fixture::new();
    fixture.engine.activate_pending_membership().unwrap();
    assert!(
        fixture
            .engine
            .scrub(&crate::engine::IntegrityScrubCancellation::new())
            .unwrap()
            .is_healthy()
    );
    let generation = fixture.engine.config.generation().unwrap();
    let disk = fixture.engine.members()[0].clone();
    let root = &fixture.engine.disk_resources.get(&disk).unwrap().root;
    let mut read = 0;
    let cancelled = generation
        .verify_archive_observed(
            root,
            &disk,
            &mut |bytes| {
                read += bytes;
                if read > 0 {
                    return Err(Error::new(
                        ErrorCode::OperationAborted,
                        "cancel archive inspection",
                    ));
                }
                Ok(())
            },
            262_144,
        )
        .unwrap_err();
    assert_eq!(cancelled.code(), ErrorCode::OperationAborted);
    let transition = object_hash(INTENT_KIND, &generation.intent).unwrap();
    let archive = member_directory(&disk, &transition).join(CONTROL_ARCHIVE_DIRECTORY);
    let object = archive
        .join("objects")
        .join(INTENT_KIND)
        .join(&transition[..2])
        .join(format!("{transition}.json"));
    fs::write(&object, b"broken").unwrap();
    let before = files(&archive);
    let report = fixture
        .engine
        .scrub(&crate::engine::IntegrityScrubCancellation::new())
        .unwrap();
    assert!(!report.is_healthy());
    assert!(report.metadata_failures > 0);
    assert_eq!(files(&archive), before);
}

#[test]
fn membership_activation_same_process_retry_retains_pending_locks() {
    let mut fixture = Fixture::new();
    let point = format!(
        "after_membership_activation_marker_{}",
        fixture.target.disks[0].disk_id
    );
    fixture.engine.inject_fault_once(&point);
    assert!(fixture.engine.activate_pending_membership().is_err());
    assert!(fixture.engine.pending_membership.is_some());
    let result = fixture.engine.activate_pending_membership().unwrap();
    assert_eq!(result.operation_id, fixture.operation);
    assert_eq!(fixture.engine._locks.len(), fixture.target.disks.len() + 1);
    assert!(!fixture.engine.blocked);
}

#[test]
fn membership_activation_rebound_roots_ignore_archived_paths_from_bad_first_marker() {
    let mut fixture = Fixture::new();
    fixture.engine.activate_pending_membership().unwrap();
    let latest = write_body(&mut fixture.engine, &fixture.branch, b"rebound majority");
    let Fixture {
        temp,
        mut target,
        engine,
        ..
    } = fixture;
    drop(engine);
    for (n, disk) in target
        .disks
        .iter_mut()
        .filter(|disk| disk.metadata_member)
        .enumerate()
    {
        let moved = temp.path().join(format!("moved-{n}"));
        fs::rename(&disk.root, &moved).unwrap();
        disk.root = moved;
    }
    let bad = target.disks.remove(1);
    fs::write(bad.root.join(GENERATION_FILE), b"broken").unwrap();
    target.disks.push(bad.clone());
    let before = files(&bad.root);
    write_json(&temp.path().join("system/config.json"), &target, true).unwrap();
    let recovered = Engine::open(&temp.path().join("system")).unwrap();
    assert!(recovered.blocked);
    assert_eq!(recovered.config.metadata_quorum(), 2);
    recovered.open_content(&latest).unwrap();
    assert_eq!(files(&bad.root), before);
}

#[test]
fn membership_activation_candidate_schema_and_topology_overflow_are_strict() {
    let mut fixture = Fixture::new();
    fixture.engine.activate_pending_membership().unwrap();
    let generation = fixture.engine.config.generation().unwrap();
    let disk = fixture.engine.members()[0].clone();
    let scan = scan_configured_at(
        &fixture.engine.disk_resources.get(&disk).unwrap().root,
        &disk,
        &fixture.engine.config,
    )
    .unwrap();
    let hash = &scan.records.last().unwrap().0.candidate_hash;
    let candidate: Candidate = load_object_at(
        &fixture.engine.disk_resources.get(&disk).unwrap().root,
        &disk,
        "candidate",
        hash,
    )
    .unwrap();
    assert_eq!(candidate.generation(), Some(generation.hash.as_str()));
    for field in ["authority_hash", "format", "decision"] {
        let mut value = json_codec::to_value(&candidate).unwrap();
        value.as_object_mut().unwrap().remove(field);
        assert!(json_codec::from_value::<Candidate>(value).is_err());
    }
    let mut value = json_codec::to_value(&candidate).unwrap();
    value["unknown"] = json_codec::json!(true);
    assert!(json_codec::from_value::<Candidate>(value).is_err());
    let mut exhausted = (**generation).clone();
    Arc::make_mut(&mut exhausted.base).topology_generation = u64::MAX;
    assert_eq!(
        exhausted.next_state().unwrap_err().code(),
        ErrorCode::ResourceLimit
    );
    assert_eq!(
        fixture
            .engine
            .create_repository(
                &RepositoryName::parse("reused").unwrap(),
                "P01",
                1,
                "admin",
                fixture.operation
            )
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyKeyReused
    );
}

#[test]
fn membership_activation_reconciles_old_and_new_workspace_completion_contexts() {
    fn save(engine: &mut Engine, branch: &str, path: &str) -> (Workspace, Workspace) {
        let workspace = engine.create_workspace(branch, "admin").unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, path, b"new selected file")
            .unwrap();
        engine
            .seal_workspace(&workspace.workspace_id, "admin")
            .unwrap();
        let before = engine.workspace(&workspace.workspace_id).unwrap();
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id.clone(),
                operation_id: OperationId::generate().unwrap(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: String::new(),
                request_digest: None,
            })
            .unwrap();
        let completed = engine.workspace(&workspace.workspace_id).unwrap();
        assert_ne!(
            before.selected_file_contexts,
            completed.selected_file_contexts
        );
        (before, completed)
    }
    let mut old = None;
    let mut fixture = Fixture::prepared(|engine, branch| {
        old = Some(save(engine, branch, "before.txt"));
    });
    fixture.engine.activate_pending_membership().unwrap();
    let new = save(&mut fixture.engine, &fixture.branch, "after.txt");
    // Current selection cannot substitute for either exact publication snapshot.
    for path in ["before.txt", "after.txt"] {
        let entry = fixture
            .engine
            .list_tree(&fixture.branch)
            .unwrap()
            .into_iter()
            .find(|entry| entry.path == path)
            .unwrap();
        let alternative = fixture
            .engine
            .create_file_branch(
                &entry.file_id,
                &entry.version_id,
                &storage_contract::BranchName::parse("later selection").unwrap(),
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        fixture
            .engine
            .select_file_branch(
                &entry.file_id,
                alternative.result["branch_id"].as_str().unwrap(),
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
    }
    let pairs = [old.unwrap(), new];
    let Fixture { temp, engine, .. } = fixture;
    drop(engine);
    for (before, _) in &pairs {
        write_json(
            &temp
                .path()
                .join("system/workspaces")
                .join(&before.workspace_id)
                .join("workspace.json"),
            before,
            true,
        )
        .unwrap();
    }
    let recovered = Engine::open(&temp.path().join("system")).unwrap();
    for (before, expected) in pairs {
        let completed = recovered.workspace(&before.workspace_id).unwrap();
        assert_eq!(completed.state, WorkspaceState::Committed);
        assert_eq!(
            completed.selected_file_contexts,
            expected.selected_file_contexts
        );
    }
}
