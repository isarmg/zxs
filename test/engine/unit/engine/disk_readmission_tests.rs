//! Real retained-directory transitions and interrupted durable consumers.
//! These tests use actual roots, journals, locks and archives, not mock votes.
use super::*;
use std::{fs, io::Read, path::PathBuf};

struct Fixture {
    temp: tempfile::TempDir,
    engine: Engine,
    config: Config,
    branch: String,
    version: String,
}
impl Fixture {
    fn new(metadata: bool) -> Self {
        Self::with_relocation(metadata, false)
    }
    fn with_relocation(metadata: bool, relocate: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let mut config =
            Config::development((0..3).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
        config.disks[0].metadata_member = metadata;
        Self::with_config(temp, config, relocate)
    }
    fn with_config(temp: tempfile::TempDir, mut config: Config, relocate: bool) -> Self {
        let mut engine = Engine::initialize(&temp.path().join("system"), config.clone()).unwrap();
        if relocate {
            // Move before creating a workspace: Genesis has no historical
            // generation aliases for already persisted absolute staging paths.
            let admitted = engine.config.clone();
            drop(engine);
            let roots = config
                .disks
                .iter()
                .enumerate()
                .map(|(index, disk)| {
                    let root = temp.path().join(format!("relocated-{index}"));
                    fs::rename(&disk.root, &root).unwrap();
                    root
                })
                .collect();
            let relocated = admitted.with_disk_roots(roots).unwrap();
            write_json(&temp.path().join("system/config.json"), &relocated, true).unwrap();
            config = (*relocated).clone();
            engine = Engine::open(&temp.path().join("system")).unwrap();
        }
        let repository = engine
            .create_repository(
                &RepositoryName::parse("readmit-source").unwrap(),
                &config.disks[0].slot_id,
                1,
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        let branch = repository.main_branch_id;
        write(&mut engine, &branch, "retained.txt", b"retained bytes");
        let version = engine.list_tree(&branch).unwrap()[0].version_id.clone();
        Self {
            temp,
            engine,
            config,
            branch,
            version,
        }
    }
    fn disable(&mut self) -> OperationResult {
        exit(
            &mut self.engine,
            &self.config.disks[0].disk_id,
            MembershipExitAction::PlannedOffline,
        )
    }
    fn begin(&mut self, operation: OperationId) -> MembershipControlArchive {
        let disk = &self.config.disks[0];
        let status = self
            .engine
            .begin_disk_readmission(&disk.disk_id, "admin", operation, directory(disk))
            .unwrap();
        assert_eq!(status.phase, DiskReadmissionPhase::Fenced);
        assert_eq!(status.metadata_member, disk.metadata_member);
        self.engine
            .build_managed_membership_control_archive()
            .unwrap()
    }
    fn activate(&mut self, archive: &MembershipControlArchive) -> OperationResult {
        self.engine
            .activate_disk_readmission(
                &archive.archive_path,
                &archive.root_hash,
                directory(&self.config.disks[0]),
            )
            .unwrap()
    }
}
fn directory(disk: &DiskConfig) -> PendingMemberDirectory {
    PendingMemberDirectory {
        disk_id: disk.disk_id.clone(),
        directory: fs::File::open(&disk.root).unwrap(),
    }
}
fn write(engine: &mut Engine, branch: &str, path: &str, bytes: &[u8]) -> OperationResult {
    let workspace = engine.create_workspace(branch, "admin").unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, path, bytes)
        .unwrap();
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
        .unwrap()
}
fn exit(engine: &mut Engine, disk: &str, action: MembershipExitAction) -> OperationResult {
    engine
        .begin_disk_exit(disk, action, "admin", OperationId::generate().unwrap())
        .unwrap();
    let archive = engine.build_managed_membership_control_archive().unwrap();
    engine
        .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
        .unwrap();
    assert!(
        engine
            .resume_membership_joint_decision()
            .unwrap()
            .all_certificates_synchronized()
    );
    engine.activate_pending_disk_exit().unwrap()
}
fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut result = BTreeMap::new();
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            result.extend(files(&path));
        } else {
            result.insert(path.clone(), fs::read(path).unwrap());
        }
    }
    result
}

#[test]
fn relocated_genesis_anchors_survive_exit_readmission_and_cold_restart() {
    for metadata in [true, false] {
        let mut fixture = Fixture::with_relocation(metadata, true);
        let anchors = fixture
            .config
            .disks
            .iter()
            .map(|disk| {
                let bytes =
                    fs::read(disk.root.join("metadata/pool-membership/config.json")).unwrap();
                let saved: Config = decode_strict(&bytes).unwrap();
                assert_ne!(saved.disks[0].root, fixture.config.disks[0].root);
                bytes
            })
            .collect::<Vec<_>>();
        let history = canonical_json(&fixture.engine.state.commits).unwrap();
        let offline = fixture.disable();
        let operation = OperationId::generate().unwrap();
        let archive = fixture.begin(operation);
        let disk = fixture.config.disks[0].clone();
        let anchor = disk.root.join("metadata/pool-membership/config.json");
        let parked = fixture.temp.path().join("retained-anchor.json");
        fs::rename(&anchor, &parked).unwrap();
        assert!(
            fixture
                .engine
                .activate_disk_readmission(
                    &archive.archive_path,
                    &archive.root_hash,
                    directory(&disk),
                )
                .is_err()
        );
        assert!(!fixture.engine.state.operations.contains_key(&operation));
        assert_eq!(
            fs::symlink_metadata(&anchor).unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );
        assert_eq!(fs::read(&parked).unwrap(), anchors[0]);
        fs::rename(&parked, &anchor).unwrap();
        let result = fixture.activate(&archive);
        assert_eq!(result.operation_id, operation);
        assert_eq!(result.kind, "disk-readmit");
        assert_eq!(
            fixture.engine.state.operations[&offline.operation_id],
            offline
        );
        assert_eq!(
            canonical_json(&fixture.engine.state.commits).unwrap(),
            history
        );
        for (disk, bytes) in fixture.config.disks.iter().zip(&anchors) {
            assert_eq!(
                fs::read(disk.root.join("metadata/pool-membership/config.json")).unwrap(),
                *bytes
            );
        }
        drop(fixture.engine);
        let engine = Engine::open(&fixture.temp.path().join("system")).unwrap();
        assert_eq!(engine.state.operations[&operation], result);
        assert_eq!(engine.state.operations[&offline.operation_id], offline);
        let registration = engine.registered_disk(&disk.disk_id).unwrap();
        assert_eq!(registration.registration_mode, RegistrationMode::Enabled);
        assert_eq!(registration.metadata_member, metadata);
        assert_eq!(registration.binding_generation, disk.binding_generation);
        let mut body = Vec::new();
        engine
            .open_content(&fixture.version)
            .unwrap()
            .file
            .read_to_end(&mut body)
            .unwrap();
        assert_eq!(body, b"retained bytes");
        for (disk, bytes) in fixture.config.disks.iter().zip(&anchors) {
            assert_eq!(
                fs::read(disk.root.join("metadata/pool-membership/config.json")).unwrap(),
                *bytes
            );
        }
    }
}

#[test]
fn disabled_metadata_and_data_only_readmit_into_new_io_lifetime_and_cold_restart() {
    for metadata in [true, false] {
        let mut fixture = Fixture::new(metadata);
        let disk = fixture.config.disks[0].clone();
        let old_config = fixture.engine.config.clone();
        let old = Arc::clone(fixture.engine.disk_resources.get(&disk).unwrap());
        let old_binding = old.root.io_binding().unwrap().clone();
        let old_observer = fixture
            .engine
            .open_disk_observation_directory(&disk.slot_id)
            .unwrap();
        let old_plan = fixture.engine.prepare_disk_observation();
        let stale_workspace = fixture
            .engine
            .create_workspace(&fixture.branch, "admin")
            .unwrap();
        let version = &fixture.engine.state.versions[&fixture.version];
        let payload_directory = version_dir(&disk, &version.history_tree_id, &fixture.version);
        let payload = files(&payload_directory);
        let history = canonical_json(&fixture.engine.state.commits).unwrap();
        let offline = fixture.disable();
        let old_locator_records = files(&fixture.temp.path().join("system/bootstrap-discovery"))
            .into_iter()
            .filter(|(path, _)| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("record-")
            })
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            fixture
                .engine
                .registered_disk(&disk.disk_id)
                .unwrap()
                .registration_mode,
            RegistrationMode::Disabled
        );
        assert!(
            fixture
                .engine
                .create_workspace(&fixture.branch, "admin")
                .is_err()
        );
        assert_eq!(fixture.engine.list_tree(&fixture.branch).unwrap().len(), 1);
        assert!(old_observer.metadata().is_err());
        assert!(old.root.directory(&disk.root).is_err());
        old_binding.closed_proof().unwrap();
        assert_eq!(
            fixture
                .engine
                .preview_disk_readmission_metadata_member(&disk.disk_id)
                .unwrap(),
            metadata
        );
        let operation = OperationId::generate().unwrap();
        let archive = fixture.begin(operation);
        let result = fixture.activate(&archive);
        assert_eq!(result.operation_id, operation);
        assert_eq!(result.kind, "disk-readmit");
        for (path, bytes) in old_locator_records {
            assert_eq!(
                fs::read(path).unwrap(),
                bytes,
                "old locator bytes and hashes must remain immutable"
            );
        }
        assert_eq!(result.result["registration"], "enabled");
        assert_eq!(result.result["io_admission_epoch"], result.request_digest);
        assert_eq!(
            fixture
                .engine
                .disk_readmission_status()
                .unwrap()
                .unwrap()
                .phase,
            DiskReadmissionPhase::Published
        );
        let registration = fixture.engine.registered_disk(&disk.disk_id).unwrap();
        assert_eq!(registration.registration_mode, RegistrationMode::Enabled);
        assert_eq!(registration.metadata_member, metadata);
        assert_eq!(registration.binding_generation, disk.binding_generation);
        assert_eq!(
            canonical_json(&fixture.engine.state.commits).unwrap(),
            history
        );
        assert_eq!(files(&payload_directory), payload);
        assert!(old_binding.admit().is_err());
        assert!(old.root.directory(&disk.root).is_err());
        assert!(old_observer.metadata().is_err());
        assert!(
            !old_plan
                .sample()
                .into_iter()
                .find(|item| item.status.disk_id == disk.disk_id)
                .unwrap()
                .status
                .online
        );
        assert!(DiskIoBinding::new(&old.seal, &old_config).is_err());
        assert!(
            fixture
                .engine
                .stage_bytes(
                    &stale_workspace.workspace_id,
                    "stale.txt",
                    b"never admitted"
                )
                .is_err()
        );
        fixture
            .engine
            .open_disk_observation_directory(&disk.slot_id)
            .unwrap()
            .metadata()
            .unwrap();
        write(
            &mut fixture.engine,
            &fixture.branch,
            "after.txt",
            b"new lifetime writes",
        );
        assert_eq!(
            fixture.engine.state.operations[&offline.operation_id],
            offline
        );
        assert_eq!(fixture.activate(&archive), result);
        drop(fixture.engine);
        let mut engine = Engine::open(&fixture.temp.path().join("system")).unwrap();
        assert_eq!(engine.state.operations[&operation], result);
        assert_eq!(
            engine
                .registered_disk(&disk.disk_id)
                .unwrap()
                .metadata_member,
            metadata
        );
        assert!(
            engine
                .stage_bytes(&stale_workspace.workspace_id, "stale.txt", b"still fenced")
                .is_err()
        );
        let mut body = Vec::new();
        engine
            .open_content(&fixture.version)
            .unwrap()
            .file
            .read_to_end(&mut body)
            .unwrap();
        assert_eq!(body, b"retained bytes");
        write(&mut engine, &fixture.branch, "cold.txt", b"cold admitted");
        assert!(old_binding.admit().is_err());
        assert!(DiskIoBinding::new(&old.seal, &old_config).is_err());
    }
}

#[test]
fn readmission_preserves_detached_masks_and_cannot_restore_a_deleted_registration() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::independent_roots(
        storage_contract::Mode::Development,
        (0..3).map(|n| temp.path().join(format!("disk-{n}"))),
    )
    .unwrap();
    let mut fixture = Fixture::with_config(temp, config, false);
    let other = fixture.config.disks[1].clone();
    let repository = fixture
        .engine
        .create_repository(
            &RepositoryName::parse("detached-history").unwrap(),
            &other.slot_id,
            1,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    write(
        &mut fixture.engine,
        &repository.main_branch_id,
        "gone.txt",
        b"historical only",
    );
    fixture.disable();
    let detached = exit(
        &mut fixture.engine,
        &other.disk_id,
        MembershipExitAction::Deregister,
    );
    assert!(
        fixture
            .engine
            .list_tree(&repository.main_branch_id)
            .unwrap()
            .is_empty()
    );
    assert!(
        fixture
            .engine
            .preview_disk_readmission_metadata_member(&other.disk_id)
            .is_err()
    );
    let before = files(&other.root);
    assert!(
        fixture
            .engine
            .begin_disk_readmission(
                &other.disk_id,
                "admin",
                OperationId::generate().unwrap(),
                directory(&other)
            )
            .is_err()
    );
    assert_eq!(files(&other.root), before);
    let archive = fixture.begin(OperationId::generate().unwrap());
    fixture.activate(&archive);
    assert!(fixture.engine.registered_disk(&other.disk_id).is_none());
    assert!(
        fixture
            .engine
            .list_tree(&repository.main_branch_id)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fixture.engine.state.operations[&detached.operation_id],
        detached
    );
    write(
        &mut fixture.engine,
        &fixture.branch,
        "retained-owner.txt",
        b"only retained owner",
    );
}

#[test]
fn readmission_repeated_cycles_keep_all_old_epochs_closed_and_receipts_permanent() {
    let mut fixture = Fixture::new(true);
    let disk = fixture.config.disks[0].clone();
    let mut old = Vec::new();
    let mut receipts = Vec::new();
    let mut epochs = BTreeSet::new();
    for round in 0..3 {
        old.push((
            Arc::clone(fixture.engine.disk_resources.get(&disk).unwrap()),
            fixture.engine.config.clone(),
        ));
        receipts.push(fixture.disable());
        let operation = OperationId::generate().unwrap();
        let archive = fixture.begin(operation);
        assert!(
            !Engine::readmission_recovery_pending(&fixture.temp.path().join("system")).unwrap()
        );
        let result = fixture.activate(&archive);
        assert!(epochs.insert(result.request_digest.clone()));
        receipts.push(result);
        write(
            &mut fixture.engine,
            &fixture.branch,
            &format!("cycle-{round}.txt"),
            b"cycle",
        );
        for (handle, config) in &old {
            assert!(handle.root.directory(&disk.root).is_err());
            assert!(DiskIoBinding::new(&handle.seal, config).is_err());
        }
    }
    let portable = &fixture.engine.config.generation().unwrap().portable;
    assert_eq!(portable.historical_bootstrap_stops().unwrap().len(), 3);
    assert!(portable.bootstrap_stops().unwrap().is_empty());
    for result in receipts {
        assert_eq!(
            fixture.engine.state.operations[&result.operation_id],
            result
        );
    }
    let (last, config) = old.pop().unwrap();
    let seal = last.seal.clone();
    drop(last);
    drop(old);
    assert!(
        DiskIoBinding::new(&seal, &config).is_err(),
        "last old handle release cannot revoke the tombstone"
    );
}

#[test]
fn readmission_rejects_replaced_identity_and_unclosed_old_logical_tail() {
    let mut fixture = Fixture::new(true);
    let disk = fixture.config.disks[0].clone();
    let old = Arc::clone(fixture.engine.disk_resources.get(&disk).unwrap());
    let tail = old.root.admit_io().unwrap();
    fixture
        .engine
        .begin_disk_exit(
            &disk.disk_id,
            MembershipExitAction::PlannedOffline,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let archive = fixture
        .engine
        .build_managed_membership_control_archive()
        .unwrap();
    fixture
        .engine
        .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
        .unwrap();
    fixture.engine.resume_membership_joint_decision().unwrap();
    assert!(fixture.engine.activate_pending_disk_exit().is_err());
    assert!(
        fixture
            .engine
            .begin_disk_readmission(
                &disk.disk_id,
                "admin",
                OperationId::generate().unwrap(),
                directory(&disk)
            )
            .is_err()
    );
    drop(tail);
    fixture.engine.activate_pending_disk_exit().unwrap();
    let held = directory(&disk);
    let moved = fixture.temp.path().join("original-disabled");
    fs::rename(&disk.root, &moved).unwrap();
    private_dir(&disk.root).unwrap();
    fs::copy(moved.join("disk.json"), disk.root.join("disk.json")).unwrap();
    assert!(
        fixture
            .engine
            .begin_disk_readmission(
                &disk.disk_id,
                "admin",
                OperationId::generate().unwrap(),
                held
            )
            .is_err()
    );
    assert!(fixture.engine.membership_fence_status().is_none());
}

#[cfg(feature = "fault-injection")]
#[test]
fn readmission_every_durable_cut_resumes_only_exact_saved_operation() {
    for cut in 0..17 {
        let mut fixture = Fixture::new(true);
        let mut points = vec![
            "after_readmission_target_archive".into(),
            "after_readmission_target_decision".into(),
            "after_readmission_probe_admission".into(),
            "after_readmission_activation_plan".into(),
            "after_readmission_locator".into(),
            "after_publication_1".into(),
            "before_membership_activation_projection".into(),
        ];
        for disk in &fixture.config.disks[1..] {
            points.push(format!("after_readmission_archive_{}", disk.disk_id));
            points.push(format!("after_readmission_decision_{}", disk.disk_id));
        }
        for disk in &fixture.config.disks {
            points.push(format!("after_readmission_marker_{}", disk.disk_id));
            points.push(format!("after_readmission_seal_{}", disk.disk_id));
        }
        let point = points.remove(cut);
        fixture.disable();
        let operation = OperationId::generate().unwrap();
        let archive = fixture.begin(operation);
        fixture.engine.inject_fault_once(&point);
        assert!(
            fixture
                .engine
                .activate_disk_readmission(
                    &archive.archive_path,
                    &archive.root_hash,
                    directory(&fixture.config.disks[0])
                )
                .is_err(),
            "{point}"
        );
        drop(fixture.engine);
        let root = fixture.temp.path().join("system");
        let mut engine = if Engine::readmission_recovery_pending(&root).unwrap() {
            assert!(
                Engine::resume_disk_readmission_operation(
                    &root,
                    OperationId::generate().unwrap(),
                    "admin",
                    None
                )
                .is_err()
            );
            assert!(
                Engine::resume_disk_readmission_operation(&root, operation, "another-actor", None)
                    .is_err()
            );
            Engine::resume_disk_readmission_operation(&root, operation, "admin", None).unwrap()
        } else {
            let mut engine = Engine::open_for_recovery(&root).unwrap();
            engine
                .begin_disk_readmission(
                    &fixture.config.disks[0].disk_id,
                    "admin",
                    operation,
                    directory(&fixture.config.disks[0]),
                )
                .unwrap();
            engine
                .activate_disk_readmission(
                    &archive.archive_path,
                    &archive.root_hash,
                    directory(&fixture.config.disks[0]),
                )
                .unwrap();
            engine
        };
        assert_eq!(
            engine.disk_readmission_status().unwrap().unwrap().phase,
            DiskReadmissionPhase::Published,
            "{point}"
        );
        assert_eq!(
            engine.state.operations[&operation].kind, "disk-readmit",
            "{point}"
        );
        assert!(
            !engine.is_read_only(),
            "published readmission remains read-only at {point}"
        );
        write(
            &mut engine,
            &fixture.branch,
            "resumed.txt",
            point.as_bytes(),
        );
        drop(engine);
        assert!(Engine::open(&root).is_ok(), "{point}");
    }
}

#[test]
fn certified_probe_scopes_allow_cold_start_after_every_original_root_has_a_new_epoch() {
    let mut fixture = Fixture::new(true);
    for disk in fixture.config.disks.clone() {
        exit(
            &mut fixture.engine,
            &disk.disk_id,
            MembershipExitAction::PlannedOffline,
        );
        let operation = OperationId::generate().unwrap();
        fixture
            .engine
            .begin_disk_readmission(&disk.disk_id, "admin", operation, directory(&disk))
            .unwrap();
        let archive = fixture
            .engine
            .build_managed_membership_control_archive()
            .unwrap();
        fixture
            .engine
            .activate_disk_readmission(&archive.archive_path, &archive.root_hash, directory(&disk))
            .unwrap();
        assert!(
            !Engine::readmission_recovery_pending(&fixture.temp.path().join("system")).unwrap()
        );
    }
    drop(fixture.engine);
    let mut engine = Engine::open(&fixture.temp.path().join("system")).unwrap();
    assert_eq!(engine.config.metadata_members().len(), 3);
    write(
        &mut engine,
        &fixture.branch,
        "all-readmitted.txt",
        b"fixed current quorum remains three",
    );
}

#[test]
fn missing_probe_certificate_never_treats_locator_epoch_as_authority() {
    let mut fixture = Fixture::new(true);
    fixture.disable();
    let archive = fixture.begin(OperationId::generate().unwrap());
    let result = fixture.activate(&archive);
    drop(fixture.engine);
    let system = fixture.temp.path().join("system");
    let proof = system
        .join("readmission-probe-admissions")
        .join(format!("{}.json", result.request_digest));
    let parked = fixture.temp.path().join("probe-certificate-preserved");
    fs::rename(proof, parked).unwrap();
    let before = files(&fixture.config.disks[0].root);
    assert!(Engine::open(&system).is_err());
    assert_eq!(files(&fixture.config.disks[0].root), before);
}

#[cfg(feature = "fault-injection")]
#[test]
fn cold_readmission_without_original_quorum_never_opens_target_for_activation() {
    let mut fixture = Fixture::new(true);
    fixture.disable();
    let operation = OperationId::generate().unwrap();
    let archive = fixture.begin(operation);
    fixture
        .engine
        .inject_fault_once("after_readmission_activation_plan");
    assert!(
        fixture
            .engine
            .activate_disk_readmission(
                &archive.archive_path,
                &archive.root_hash,
                directory(&fixture.config.disks[0])
            )
            .is_err()
    );
    drop(fixture.engine);
    for (n, disk) in fixture.config.disks[1..].iter().enumerate() {
        fs::rename(
            &disk.root,
            fixture.temp.path().join(format!("missing-old-voter-{n}")),
        )
        .unwrap();
    }
    let before = files(&fixture.config.disks[0].root);
    assert!(
        Engine::resume_disk_readmission_operation(
            &fixture.temp.path().join("system"),
            operation,
            "admin",
            None
        )
        .is_err()
    );
    assert_eq!(files(&fixture.config.disks[0].root), before);
}

#[test]
fn explicit_system_loss_recovery_persists_active_epoch_for_metadata_and_data_roots() {
    for metadata in [true, false] {
        let mut fixture = Fixture::new(metadata);
        fixture.disable();
        let operation = OperationId::generate().unwrap();
        let archive = fixture.begin(operation);
        let result = fixture.activate(&archive);
        let disk_id = fixture.config.disks[0].disk_id.clone();
        let mut metadata_roots = fixture
            .engine
            .config
            .metadata_members()
            .map(|disk| disk.root.clone())
            .collect::<Vec<_>>();
        let mut data_roots = fixture.engine.recovery_data_roots().unwrap();
        let old_root = fixture.config.disks[0].root.clone();
        let moved = fixture.temp.path().join("relocated-retained-directory");
        let before = files(&old_root);
        drop(fixture.engine);
        fs::rename(&old_root, &moved).unwrap();
        if metadata {
            *metadata_roots
                .iter_mut()
                .find(|root| **root == old_root)
                .unwrap() = moved.clone();
        } else {
            data_roots
                .iter_mut()
                .find(|root| root.identity.disk_id == disk_id)
                .unwrap()
                .root = moved.clone();
        }
        let system = fixture.temp.path().join("restored-system");
        let mut engine =
            Engine::restore_from_disks_and_data_roots(&system, &metadata_roots, &data_roots)
                .unwrap();
        assert_eq!(
            engine
                .config
                .io_admission_epoch(&disk_id)
                .unwrap()
                .as_deref(),
            Some(result.request_digest.as_str())
        );
        assert_eq!(engine.state.operations[&operation], result);
        assert_eq!(
            engine.registered_disk(&disk_id).unwrap().registration_mode,
            RegistrationMode::Enabled
        );
        for (path, bytes) in before {
            assert_eq!(
                fs::read(moved.join(path.strip_prefix(&old_root).unwrap())).unwrap(),
                bytes
            );
        }
        let locator_path = system.join("bootstrap-discovery/record-00000000000000000001.json");
        let original_locator = fs::read(&locator_path).unwrap();
        let locator: json_codec::Value = decode_strict(&original_locator).unwrap();
        assert_eq!(
            locator["recovered_readmissions"][&disk_id]["intent_digest"],
            result.request_digest
        );
        assert!(
            system
                .join("readmission-probe-admissions")
                .join(format!("{}.json", result.request_digest))
                .is_file()
        );
        write(
            &mut engine,
            &fixture.branch,
            "restored.txt",
            b"restored active epoch",
        );
        drop(engine);
        let mut engine = Engine::open(&system).unwrap();
        assert_eq!(
            engine
                .config
                .io_admission_epoch(&disk_id)
                .unwrap()
                .as_deref(),
            Some(result.request_digest.as_str())
        );
        let offline = exit(&mut engine, &disk_id, MembershipExitAction::PlannedOffline);
        let disk = engine
            .config
            .disks
            .iter()
            .find(|disk| disk.disk_id == disk_id)
            .unwrap()
            .clone();
        engine
            .begin_disk_readmission(
                &disk_id,
                "admin",
                OperationId::generate().unwrap(),
                directory(&disk),
            )
            .unwrap();
        let archive = engine.build_managed_membership_control_archive().unwrap();
        let next = engine
            .activate_disk_readmission(&archive.archive_path, &archive.root_hash, directory(&disk))
            .unwrap();
        assert_ne!(next.request_digest, result.request_digest);
        assert_eq!(engine.state.operations[&offline.operation_id], offline);
        assert_eq!(
            fs::read(locator_path).unwrap(),
            original_locator,
            "recovery epoch seed remains immutable"
        );
        drop(engine);
        let engine = Engine::open(&system).unwrap();
        assert_eq!(
            engine
                .config
                .io_admission_epoch(&disk_id)
                .unwrap()
                .as_deref(),
            Some(next.request_digest.as_str())
        );
        assert_eq!(engine.state.operations[&operation], result);
    }
}

#[test]
fn system_loss_readmission_epoch_requires_actual_fixed_quorum_and_its_probe_certificate() {
    let mut fixture = Fixture::new(true);
    fixture.disable();
    let archive = fixture.begin(OperationId::generate().unwrap());
    let result = fixture.activate(&archive);
    let metadata_roots = fixture
        .engine
        .config
        .metadata_members()
        .map(|disk| disk.root.clone())
        .collect::<Vec<_>>();
    drop(fixture.engine);
    let incomplete = fixture.temp.path().join("without-quorum");
    let error = Engine::restore_from_disks(&incomplete, &metadata_roots[..1])
        .err()
        .unwrap();
    assert_eq!(error.code(), ErrorCode::MetadataQuorumUnavailable);
    assert!(
        !incomplete.exists(),
        "a portable authority cannot publish a recovery locator without Q"
    );
    let partial = fixture.temp.path().join("fixed-quorum-system");
    let engine = Engine::restore_from_disks(&partial, &metadata_roots[..2]).unwrap();
    assert_eq!(engine.config.metadata_members().len(), 3);
    assert_eq!(engine.config.metadata_quorum(), 2);
    assert_eq!(engine.config.io_disks().count(), 2);
    assert_eq!(engine.state.operations[&result.operation_id], result);
    drop(engine);
    let engine = Engine::open(&partial).unwrap();
    assert_eq!(engine.config.metadata_members().len(), 3);
    assert_eq!(engine.config.metadata_quorum(), 2);
    drop(engine);
    let system = fixture.temp.path().join("restored-system");
    let engine = Engine::restore_from_disks(&system, &metadata_roots).unwrap();
    drop(engine);
    fs::remove_file(
        system
            .join("readmission-probe-admissions")
            .join(format!("{}.json", result.request_digest)),
    )
    .unwrap();
    assert!(
        Engine::open(&system).is_err(),
        "a recovery seed alone never authorizes a storage probe"
    );
}

#[test]
fn stop_hint_cannot_move_a_real_stop_to_a_different_admission_epoch() {
    let mut fixture = Fixture::new(true);
    let first_stop = fixture.disable();
    let archive = fixture.begin(OperationId::generate().unwrap());
    let readmitted = fixture.activate(&archive);
    let current_stop = fixture.disable();
    let disk = &fixture.config.disks[0];
    let identity = DiskRegistrationIdentity {
        pool_id: fixture.config.pool_id.clone(),
        disk_id: disk.disk_id.clone(),
        slot_id: disk.slot_id.clone(),
        binding_generation: disk.binding_generation,
    };
    let authority = &fixture.engine.config.generation().unwrap().portable;
    authority
        .verify_bootstrap_stop_epoch(
            &identity,
            first_stop.operation_id,
            &first_stop.request_digest,
            None,
        )
        .unwrap();
    authority
        .verify_bootstrap_stop_epoch(
            &identity,
            current_stop.operation_id,
            &current_stop.request_digest,
            Some(&readmitted.request_digest),
        )
        .unwrap();
    for (stop, epoch) in [
        (&first_stop, Some(readmitted.request_digest.as_str())),
        (&current_stop, None),
    ] {
        assert_eq!(
            authority
                .verify_bootstrap_stop_epoch(
                    &identity,
                    stop.operation_id,
                    &stop.request_digest,
                    epoch
                )
                .unwrap_err()
                .code(),
            ErrorCode::RecoveryRequired
        );
    }
}

#[cfg(feature = "fault-injection")]
fn readmission_partial_publication() -> (Fixture, OperationId) {
    let mut fixture = Fixture::new(true);
    fixture.disable();
    let operation = OperationId::generate().unwrap();
    let archive = fixture.begin(operation);
    fixture.engine.inject_fault_once("after_publication_1");
    let error = fixture
        .engine
        .activate_disk_readmission(
            &archive.archive_path,
            &archive.root_hash,
            directory(&fixture.config.disks[0]),
        )
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::InDoubt);
    assert!(fixture.engine.is_read_only());
    assert!(!fixture.engine.state.operations.contains_key(&operation));
    assert!(Engine::readmission_recovery_pending(&fixture.temp.path().join("system")).unwrap());
    (fixture, operation)
}

#[cfg(feature = "fault-injection")]
fn readmission_storage_bytes(roots: &[PathBuf]) -> BTreeMap<PathBuf, Vec<u8>> {
    roots.iter().flat_map(|root| files(root)).collect()
}

#[cfg(feature = "fault-injection")]
#[test]
fn cold_published_readmission_requires_every_original_member_before_becoming_writable() {
    let (fixture, operation) = readmission_partial_publication();
    let mut roots = fixture
        .config
        .disks
        .iter()
        .map(|disk| disk.root.clone())
        .collect::<Vec<_>>();
    let original = roots[1].clone();
    let parked = fixture.temp.path().join("preserved-original-member");
    let system = fixture.temp.path().join("system");
    let plan = fs::read(system.join("readmission-activation.json")).unwrap();
    drop(fixture.engine);
    fs::rename(&original, &parked).unwrap();
    roots[1] = parked.clone();
    let before = readmission_storage_bytes(&roots);
    assert!(
        Engine::resume_disk_readmission_operation(&system, operation, "admin", None).is_err(),
        "one missing fixed original member cannot be replaced by the other original and target"
    );
    assert_eq!(readmission_storage_bytes(&roots), before);
    assert_eq!(
        fs::read(system.join("readmission-activation.json")).unwrap(),
        plan
    );
    fs::rename(parked, original).unwrap();
    let mut engine =
        Engine::resume_disk_readmission_operation(&system, operation, "admin", None).unwrap();
    assert!(!engine.is_read_only());
    assert_eq!(engine.state.operations[&operation].kind, "disk-readmit");
    write(
        &mut engine,
        &fixture.branch,
        "after-original-member-restored.txt",
        b"the exact original member returned",
    );
}

#[cfg(feature = "fault-injection")]
#[test]
fn cold_published_readmission_rejects_a_corrupt_current_journal_without_replacing_it() {
    let (fixture, operation) = readmission_partial_publication();
    let roots = fixture
        .config
        .disks
        .iter()
        .map(|disk| disk.root.clone())
        .collect::<Vec<_>>();
    let journal = crate::engine::membership_transition::pending::activation::log_path(
        &fixture.engine.config,
        &fixture.config.disks[1],
    );
    let original = fs::read(&journal).unwrap();
    assert!(
        original.len() > 44,
        "a complete frame must exist before corruption"
    );
    let mut corrupt = original.clone();
    // Keep the complete frame and its original digest, but alter its payload.
    // This is corruption, not an incomplete tail that recovery may repair.
    corrupt[12] ^= 1;
    let system = fixture.temp.path().join("system");
    let plan = fs::read(system.join("readmission-activation.json")).unwrap();
    drop(fixture.engine);
    fs::write(&journal, &corrupt).unwrap();
    let before = readmission_storage_bytes(&roots);
    let error = match Engine::resume_disk_readmission_operation(&system, operation, "admin", None) {
        Ok(_) => panic!("a published operation cannot bypass a corrupt current journal"),
        Err(error) => error,
    };
    assert_eq!(error.code(), ErrorCode::LogCorrupt);
    assert_eq!(readmission_storage_bytes(&roots), before);
    assert_eq!(
        fs::read(system.join("readmission-activation.json")).unwrap(),
        plan
    );
    fs::write(&journal, original).unwrap();
    let mut engine =
        Engine::resume_disk_readmission_operation(&system, operation, "admin", None).unwrap();
    assert!(!engine.is_read_only());
    assert_eq!(engine.state.operations[&operation].kind, "disk-readmit");
    write(
        &mut engine,
        &fixture.branch,
        "after-exact-journal-restored.txt",
        b"the original complete journal was restored",
    );
}

#[cfg(feature = "fault-injection")]
#[test]
fn stale_readmission_plan_cannot_unlock_a_real_later_membership_fence() {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};

    let (fixture, operation) = readmission_partial_publication();
    let roots = fixture
        .config
        .disks
        .iter()
        .map(|disk| disk.root.clone())
        .collect::<Vec<_>>();
    let system = fixture.temp.path().join("system");
    let plan_path = system.join("readmission-activation.json");
    let plan = fs::read(&plan_path).unwrap();
    drop(fixture.engine);
    let mut engine =
        Engine::resume_disk_readmission_operation(&system, operation, "admin", None).unwrap();
    assert!(!engine.is_read_only());
    let published = engine.state.operations[&operation].clone();
    let later = OperationId::generate().unwrap();
    engine
        .begin_disk_exit(
            &fixture.config.disks[0].disk_id,
            MembershipExitAction::PlannedOffline,
            "admin",
            later,
        )
        .unwrap();
    let fence = engine.membership_fence_status().unwrap();
    assert_eq!(fence.operation_id, later);
    assert!(fence.all_members_fenced());
    assert!(engine.is_read_only());
    assert!(!engine.state.operations.contains_key(&later));
    for disk in engine.config.metadata_members() {
        let scan = crate::journal::scan_configured_at(
            &engine.disk_resources.get(disk).unwrap().root,
            disk,
            &engine.config,
        )
        .unwrap();
        assert!(scan.records.iter().any(|(record, _)| {
            record.record_type == crate::journal::RecordType::MembershipFence
        }));
    }
    drop(engine);
    // Reintroduce the exact older SYSTEM hint, not a forged authority or fence.
    let mut restored_plan = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&plan_path)
        .unwrap();
    restored_plan.write_all(&plan).unwrap();
    restored_plan.sync_all().unwrap();
    drop(restored_plan);
    fs::File::open(&system).unwrap().sync_all().unwrap();
    let before = readmission_storage_bytes(&roots);
    match Engine::resume_disk_readmission_operation(&system, operation, "admin", None) {
        Ok(mut engine) => {
            assert!(
                engine.is_read_only(),
                "the later durable fence still blocks writes"
            );
            assert_eq!(
                engine.membership_fence_status().unwrap().operation_id,
                later
            );
            assert_eq!(engine.state.operations[&operation], published);
            assert!(!engine.state.operations.contains_key(&later));
            let error = engine
                .create_workspace(&fixture.branch, "admin")
                .unwrap_err();
            assert_eq!(error.code(), ErrorCode::RecoveryRequired);
        }
        Err(error) => assert_eq!(error.code(), ErrorCode::RecoveryRequired),
    }
    assert_eq!(readmission_storage_bytes(&roots), before);
}
