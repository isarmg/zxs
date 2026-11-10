//! Real retained publications, resources and restart paths. These are source
//! tests, not a model receipt or a claim that physical power-loss was exercised.
use super::*;
use std::{fs, io::Read};

struct Fixture {
    temp: tempfile::TempDir,
    engine: Engine,
    original: Config,
    operation: OperationId,
    branch: String,
    version: String,
}
impl Fixture {
    fn prepared(action: MembershipExitAction, copies: u32) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let original =
            Config::development((0..3).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
        let mut engine = Engine::initialize(&temp.path().join("system"), original.clone()).unwrap();
        let repository = engine
            .create_repository(
                &RepositoryName::parse("exit-source").unwrap(),
                "P01",
                copies,
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        let branch = repository.main_branch_id;
        let workspace = engine.create_workspace(&branch, "admin").unwrap();
        engine
            .stage_bytes(
                &workspace.workspace_id,
                "body.txt",
                b"physical bytes are not deleted",
            )
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
            .unwrap();
        let version = engine.list_tree(&branch).unwrap()[0].version_id.clone();
        let operation = OperationId::generate().unwrap();
        engine
            .begin_disk_exit(&original.disks[0].disk_id, action, "admin", operation)
            .unwrap();
        let archive = engine.build_managed_membership_control_archive().unwrap();
        engine
            .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
            .unwrap();
        let joint = engine.resume_membership_joint_decision().unwrap();
        assert_eq!((joint.original_members, joint.original_quorum), (3, 2));
        assert_eq!((joint.target_members, joint.target_quorum), (2, 2));
        Self {
            temp,
            engine,
            original,
            operation,
            branch,
            version,
        }
    }
}
fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut found = BTreeMap::new();
    for item in fs::read_dir(root).unwrap() {
        let path = item.unwrap().path();
        if path.is_dir() {
            found.extend(files(&path));
        } else {
            found.insert(path.clone(), fs::read(path).unwrap());
        }
    }
    found
}

#[test]
fn disk_exit_d3_publishes_registry_and_closes_actual_target_without_backup_or_body_change() {
    for action in [
        MembershipExitAction::PlannedOffline,
        MembershipExitAction::Deregister,
    ] {
        let mut fixture = Fixture::prepared(action, 1);
        let disk = fixture.original.disks[0].clone();
        let old = Arc::clone(fixture.engine.disk_resources.get(&disk).unwrap());
        let before = files(&disk.root);
        let versions = canonical_json(&fixture.engine.state.versions).unwrap();
        let commits = canonical_json(&fixture.engine.state.commits).unwrap();
        let source = fixture.engine.snapshot.clone().unwrap();
        let result = fixture.engine.activate_pending_disk_exit().unwrap();
        assert_eq!(result.operation_id, fixture.operation);
        assert_eq!(
            result.kind,
            match action {
                MembershipExitAction::PlannedOffline => "disk-planned-offline",
                MembershipExitAction::Deregister => "disk-deregister",
            }
        );
        assert_eq!(result.result["target_io"], "closed");
        assert_ne!(fixture.engine.snapshot.as_ref().unwrap(), &source);
        assert_eq!(
            fixture.engine.disk_exit_status().unwrap().unwrap().phase,
            DiskExitPhase::Published
        );
        assert_eq!(
            canonical_json(&fixture.engine.state.versions).unwrap(),
            versions
        );
        assert_eq!(
            canonical_json(&fixture.engine.state.commits).unwrap(),
            commits
        );
        assert_eq!(
            files(&disk.root),
            before,
            "no target marker, recovery file or payload write is needed"
        );
        old.root.io_binding().unwrap().closed_proof().unwrap();
        assert!(
            matches!(old.root.directory(&disk.root), Err(error) if error.code() == ErrorCode::DiskUnavailable)
        );
        assert!(fixture.engine.disk_resources.available(&disk).is_none());
        let target = DiskRegistrationIdentity {
            pool_id: fixture.original.pool_id.clone(),
            disk_id: disk.disk_id.clone(),
            slot_id: disk.slot_id.clone(),
            binding_generation: disk.binding_generation,
        };
        let generation = fixture.engine.config.generation().unwrap();
        let registry = generation
            .portable
            .exit_core()
            .unwrap()
            .registry_from_source(&generation.base)
            .unwrap();
        assert_eq!(
            registry.registration(&target).unwrap(),
            match action {
                MembershipExitAction::PlannedOffline => Some(crate::RegistrationMode::Disabled),
                MembershipExitAction::Deregister => None,
            }
        );
        assert_eq!(
            fixture.engine.list_tree(&fixture.branch).unwrap().len(),
            usize::from(action == MembershipExitAction::PlannedOffline)
        );
        assert_eq!(fixture.engine.activate_pending_disk_exit().unwrap(), result);
    }
}

#[test]
fn disk_exit_preserves_shared_content_and_historical_bytes_on_retained_roots() {
    let mut fixture = Fixture::prepared(MembershipExitAction::Deregister, 2);
    let backup = fixture.original.disks[1].clone();
    let version = fixture.engine.state.versions.get(&fixture.version).unwrap();
    let proof = version_dir(&backup, &version.history_tree_id, &fixture.version)
        .join("publication-proof.json");
    let historical_proof = fs::read(&proof).unwrap();
    fixture.engine.activate_pending_disk_exit().unwrap();
    assert_eq!(fixture.engine.list_tree(&fixture.branch).unwrap().len(), 1);
    assert_eq!(fs::read(&proof).unwrap(), historical_proof);
    let mut body = Vec::new();
    fixture
        .engine
        .open_content(&fixture.version)
        .unwrap()
        .file
        .read_to_end(&mut body)
        .unwrap();
    assert_eq!(body, b"physical bytes are not deleted");
    assert!(
        fixture
            .engine
            .create_workspace(&fixture.branch, "admin")
            .is_err(),
        "removing the primary owner never promotes its backup to a writable primary"
    );
}

#[test]
fn disk_exit_busy_tail_resumes_without_rebuilding_or_reopening_target_control() {
    let mut fixture = Fixture::prepared(MembershipExitAction::Deregister, 1);
    let disk = fixture.original.disks[0].clone();
    let handle = Arc::clone(fixture.engine.disk_resources.get(&disk).unwrap());
    let tail = handle.root.admit_io().unwrap();
    assert_eq!(
        fixture
            .engine
            .activate_pending_disk_exit()
            .unwrap_err()
            .code(),
        ErrorCode::DiskUnavailable
    );
    assert_eq!(
        fixture.engine.disk_exit_status().unwrap().unwrap().phase,
        DiskExitPhase::Activating
    );
    assert!(
        !fixture
            .engine
            .state
            .operations
            .contains_key(&fixture.operation)
    );
    assert!(handle.root.directory(&disk.root).is_err());
    drop(tail);
    let target_after_close = files(&disk.root);
    fixture.engine.activate_pending_disk_exit().unwrap();
    assert_eq!(files(&disk.root), target_after_close);
}

#[test]
fn disk_exit_source_generation_and_complete_mask_are_verified_before_stop() {
    let mut fixture = Fixture::prepared(MembershipExitAction::Deregister, 1);
    fixture.engine.prepare_disk_exit_generation().unwrap();
    let generation = &fixture
        .engine
        .pending_disk_exit
        .as_ref()
        .unwrap()
        .generation;
    let mut core = generation.portable.exit_core().unwrap().clone();
    core.source_business_generation += 1;
    assert!(core.registry_from_source(&generation.base).is_err());
    let mut core = generation.portable.exit_core().unwrap().clone();
    let objects = json_codec::to_value(&core.objects).unwrap();
    let key = objects
        .as_object()
        .unwrap()
        .iter()
        .find(|(_, object)| object["kind"] == "disk-occurrence-mask")
        .unwrap()
        .0
        .clone();
    core.objects.remove(&key);
    assert!(core.registry_from_source(&generation.base).is_err());
    assert!(
        fixture
            .engine
            .state
            .operations
            .get(&fixture.operation)
            .is_none()
    );
    let source = &fixture.original.disks[0];
    assert!(
        fixture
            .engine
            .disk_resources
            .get(source)
            .unwrap()
            .root
            .directory(&source.root)
            .is_ok()
    );
}

#[test]
fn disk_exit_unpublished_terminal_and_redundant_offline_fail_before_new_fence() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([temp.path().join("only")]).unwrap();
    let mut engine = Engine::initialize(&temp.path().join("system"), config.clone()).unwrap();
    let before = files(temp.path());
    assert_eq!(
        engine
            .begin_disk_exit(
                &config.disks[0].disk_id,
                MembershipExitAction::Deregister,
                "admin",
                OperationId::generate().unwrap()
            )
            .unwrap_err()
            .code(),
        ErrorCode::RecoveryRequired
    );
    assert!(engine.membership_fence_status().is_none());
    assert_eq!(files(temp.path()), before);
    let mut fixture = Fixture::prepared(MembershipExitAction::PlannedOffline, 1);
    fixture.engine.activate_pending_disk_exit().unwrap();
    let before = files(fixture.temp.path());
    assert_eq!(
        fixture
            .engine
            .begin_disk_exit(
                &fixture.original.disks[0].disk_id,
                MembershipExitAction::PlannedOffline,
                "admin",
                OperationId::generate().unwrap()
            )
            .unwrap_err()
            .code(),
        ErrorCode::ConfigConflict
    );
    assert!(fixture.engine.membership_fence_status().is_none());
    assert_eq!(files(fixture.temp.path()), before);
}

#[test]
fn disk_exit_reopen_uses_retained_authority_even_when_saved_config_still_names_target() {
    let mut fixture = Fixture::prepared(MembershipExitAction::Deregister, 1);
    let result = fixture.engine.activate_pending_disk_exit().unwrap();
    let before = files(&fixture.original.disks[0].root);
    write_json(
        &fixture.temp.path().join("system/config.json"),
        &fixture.original,
        true,
    )
    .unwrap();
    drop(fixture.engine);
    let mut engine = Engine::open(&fixture.temp.path().join("system")).unwrap();
    assert_eq!(
        engine.disk_exit_status().unwrap().unwrap().phase,
        DiskExitPhase::Published
    );
    assert_eq!(engine.activate_pending_disk_exit().unwrap(), result);
    assert!(engine.list_tree(&fixture.branch).unwrap().is_empty());
    assert_eq!(files(&fixture.original.disks[0].root), before);
}

#[cfg(feature = "fault-injection")]
#[test]
fn disk_exit_every_post_stop_cut_resumes_from_retained_preparation_without_target_writes() {
    for point in [
        "after_disk_exit_stop",
        "after_disk_exit_io_closed",
        "after_publication_1",
        "before_disk_exit_config_projection",
    ] {
        let mut fixture = Fixture::prepared(MembershipExitAction::Deregister, 1);
        fixture.engine.inject_fault_once(point);
        assert!(
            fixture.engine.activate_pending_disk_exit().is_err(),
            "{point}"
        );
        let before = files(&fixture.original.disks[0].root);
        drop(fixture.engine);
        let mut engine =
            Engine::resume_membership_activation(&fixture.temp.path().join("system")).unwrap();
        let result = engine.activate_pending_disk_exit().unwrap();
        assert_eq!(result.operation_id, fixture.operation, "{point}");
        assert_eq!(
            engine.disk_exit_status().unwrap().unwrap().phase,
            DiskExitPhase::Published
        );
        assert_eq!(files(&fixture.original.disks[0].root), before, "{point}");
    }
}

#[test]
fn disk_exit_live_activation_excludes_every_logical_io_domain_before_d3() {
    let mut fixture = Fixture::prepared(MembershipExitAction::Deregister, 1);
    let disk = fixture.original.disks[0].clone();
    let original_authority = fixture.engine.config.clone();
    let mut alternate = fixture
        .engine
        .disk_resources
        .get(&disk)
        .unwrap()
        .seal
        .clone();
    alternate.inode = alternate.inode.checked_add(1).unwrap();
    // A distinct physical-domain owner with the same exact registration must
    // not be ignored merely because this Engine's own descriptor set closed.
    let other = DiskIoBinding::new(&alternate, &original_authority).unwrap();
    let tail = other.admit().unwrap();
    let bytes = files(&disk.root);
    assert_eq!(
        fixture
            .engine
            .activate_pending_disk_exit()
            .unwrap_err()
            .code(),
        ErrorCode::DiskUnavailable
    );
    assert!(fixture.engine.blocked);
    assert!(
        !fixture
            .engine
            .state
            .operations
            .contains_key(&fixture.operation)
    );
    drop(other.begin_quiesce().unwrap().unwrap());
    assert!(other.closed_proof().is_err());
    drop(tail);
    other.closed_proof().unwrap();
    let result = fixture.engine.activate_pending_disk_exit().unwrap();
    assert_eq!(result.operation_id, fixture.operation);
    assert_eq!(
        fixture.engine.disk_exit_status().unwrap().unwrap().phase,
        DiskExitPhase::Published
    );
    assert_eq!(files(&disk.root), bytes);
    // Completion retains the negative capability even after all old callers
    // relinquish their physical domains. A fresh physical identity cannot enter.
    drop(other);
    alternate.inode = alternate.inode.checked_add(1).unwrap();
    assert!(
        matches!(DiskIoBinding::new(&alternate, &original_authority), Err(error) if error.code() == ErrorCode::DiskUnavailable)
    );
}

fn prepare_following_exit(
    engine: &mut Engine,
    disk: &str,
    action: MembershipExitAction,
) -> OperationId {
    let operation = OperationId::generate().unwrap();
    engine
        .begin_disk_exit(disk, action, "admin", operation)
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
    operation
}

fn prepare_following_add(
    engine: &mut Engine,
    root: &Path,
) -> (OperationId, DiskConfig, MembershipControlArchive) {
    let mut target = engine.config().clone();
    // The original primary has left the registry. Add an independent primary,
    // rather than extending a backup chain whose ancestor is no longer enabled.
    let mut added = Config::development([root.join("next-primary")])
        .unwrap()
        .disks
        .pop()
        .unwrap();
    added.slot_id = "P02".into();
    private_dir(&added.root).unwrap();
    target.disks.push(added.clone());
    let operation = OperationId::generate().unwrap();
    engine
        .begin_membership_handoff(target, "admin", operation)
        .unwrap();
    let archive = engine.build_managed_membership_control_archive().unwrap();
    (operation, added, archive)
}

#[test]
fn disabled_binding_detaches_then_adds_and_exits_again_without_reopening_history() {
    let mut fixture = Fixture::prepared(MembershipExitAction::PlannedOffline, 1);
    fixture.engine.activate_pending_disk_exit().unwrap();
    let first = fixture.original.disks[0].clone();
    let original_stop = fixture
        .engine
        .config
        .generation()
        .unwrap()
        .portable
        .bootstrap_stops()
        .unwrap();
    let parked = fixture.temp.path().join("stopped-data-retained");
    fs::rename(&first.root, &parked).unwrap();
    let physical = files(&parked);
    let detach = prepare_following_exit(
        &mut fixture.engine,
        &first.disk_id,
        MembershipExitAction::Deregister,
    );
    assert_eq!(
        fixture
            .engine
            .activate_pending_disk_exit()
            .unwrap()
            .operation_id,
        detach
    );
    assert!(fixture.engine.registered_disk(&first.disk_id).is_none());
    assert!(
        fixture
            .engine
            .list_tree(&fixture.branch)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fixture
            .engine
            .config
            .generation()
            .unwrap()
            .portable
            .bootstrap_stops()
            .unwrap(),
        original_stop
    );
    assert!(!first.root.exists());
    assert_eq!(files(&parked), physical);
    let before_registry = fixture
        .engine
        .config
        .generation()
        .unwrap()
        .portable
        .exit_registry()
        .unwrap()
        .clone();
    let (addition, added, archive) =
        prepare_following_add(&mut fixture.engine, fixture.temp.path());
    fixture
        .engine
        .enroll_pending_members(
            &archive.archive_path,
            &archive.root_hash,
            vec![PendingMemberDirectory {
                disk_id: added.disk_id.clone(),
                directory: std::fs::File::open(&added.root).unwrap(),
            }],
        )
        .unwrap();
    let joint = fixture.engine.resume_membership_joint_decision().unwrap();
    assert_eq!((joint.original_members, joint.original_quorum), (2, 2));
    assert_eq!((joint.target_members, joint.target_quorum), (3, 2));
    assert_eq!(
        fixture
            .engine
            .activate_pending_membership()
            .unwrap()
            .operation_id,
        addition
    );
    assert!(fixture.engine.registered_disk(&first.disk_id).is_none());
    assert!(fixture.engine.registered_disk(&added.disk_id).is_some());
    assert!(
        fixture
            .engine
            .list_tree(&fixture.branch)
            .unwrap()
            .is_empty()
    );
    assert_ne!(
        fixture
            .engine
            .config
            .generation()
            .unwrap()
            .portable
            .exit_registry(),
        Some(&before_registry)
    );
    let generation = fixture.engine.config.generation().unwrap();
    let mut stripped = json_codec::to_value(&generation.portable).unwrap();
    stripped
        .get_mut("head")
        .unwrap()
        .as_object_mut()
        .unwrap()
        .remove("registry");
    assert!(
        json_codec::from_value::<PortableAuthority>(stripped).is_err(),
        "membership ancestry cannot silently discard registry ancestry"
    );
    let mut wrong_source = json_codec::to_value(&generation.portable).unwrap();
    wrong_source["head"]["registry"]["source_business_generation"] =
        (generation.base.business_generation + 1).to_string().into();
    let wrong_source: PortableAuthority = json_codec::from_value(wrong_source).unwrap();
    assert!(
        wrong_source
            .verify_registry_source(&generation.base)
            .is_err()
    );
    let second = fixture.original.disks[1].clone();
    prepare_following_exit(
        &mut fixture.engine,
        &second.disk_id,
        MembershipExitAction::Deregister,
    );
    let second_bytes = files(&second.root);
    fixture.engine.activate_pending_disk_exit().unwrap();
    assert_eq!(files(&second.root), second_bytes);
    assert_eq!(fixture.engine.config.metadata_members().len(), 2);
    assert_eq!(fixture.engine.config.metadata_quorum(), 2);
    assert!(
        fixture
            .engine
            .list_tree(&fixture.branch)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fixture
            .engine
            .pending_disk_exit
            .as_ref()
            .unwrap()
            .ownership
            .len(),
        2
    );
    let system = fixture.temp.path().join("system");
    write_json(&system.join("config.json"), &fixture.original, true).unwrap();
    drop(fixture.engine);
    let engine = Engine::open(&system).unwrap();
    assert!(engine.registered_disk(&first.disk_id).is_none());
    assert!(engine.registered_disk(&second.disk_id).is_none());
    assert!(engine.registered_disk(&added.disk_id).is_some());
    assert!(engine.list_tree(&fixture.branch).unwrap().is_empty());
    assert_eq!(
        engine.pending_disk_exit.as_ref().unwrap().ownership.len(),
        2
    );
    assert!(!first.root.exists());
    assert_eq!(files(&parked), physical);
    assert_eq!(files(&second.root), second_bytes);
}

#[test]
fn post_exit_add_abort_inherits_current_registry_and_exact_hidden_view() {
    let mut fixture = Fixture::prepared(MembershipExitAction::Deregister, 1);
    fixture.engine.activate_pending_disk_exit().unwrap();
    let registry = fixture
        .engine
        .config
        .generation()
        .unwrap()
        .portable
        .exit_registry()
        .unwrap()
        .clone();
    let stopped = fixture.original.disks[0].clone();
    let before = files(&stopped.root);
    let (operation, added, archive) =
        prepare_following_add(&mut fixture.engine, fixture.temp.path());
    let result = fixture
        .engine
        .cancel_membership_handoff(&archive.archive_path, &archive.root_hash)
        .unwrap();
    assert_eq!(result.operation_id, operation);
    assert_eq!(
        fixture
            .engine
            .config
            .generation()
            .unwrap()
            .portable
            .exit_registry(),
        Some(&registry)
    );
    assert!(fixture.engine.registered_disk(&added.disk_id).is_none());
    assert!(fixture.engine.registered_disk(&stopped.disk_id).is_none());
    assert!(
        fixture
            .engine
            .list_tree(&fixture.branch)
            .unwrap()
            .is_empty()
    );
    assert_eq!(files(&stopped.root), before);
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    let engine = Engine::open(&system).unwrap();
    assert!(engine.list_tree(&fixture.branch).unwrap().is_empty());
    assert_eq!(
        engine.config.generation().unwrap().portable.exit_registry(),
        Some(&registry)
    );
    assert_eq!(files(&stopped.root), before);
}

#[cfg(feature = "fault-injection")]
#[test]
fn disabled_detach_crash_replays_prior_d3_without_a_second_stop_or_target_probe() {
    for point in [
        "after_disk_exit_stop",
        "after_publication_1",
        "before_disk_exit_config_projection",
    ] {
        let mut fixture = Fixture::prepared(MembershipExitAction::PlannedOffline, 1);
        fixture.engine.activate_pending_disk_exit().unwrap();
        let disk = fixture.original.disks[0].clone();
        let parked = fixture.temp.path().join("absent-offline-root");
        fs::rename(&disk.root, &parked).unwrap();
        let before = files(&parked);
        let operation = prepare_following_exit(
            &mut fixture.engine,
            &disk.disk_id,
            MembershipExitAction::Deregister,
        );
        fixture.engine.inject_fault_once(point);
        assert!(fixture.engine.activate_pending_disk_exit().is_err());
        let system = fixture.temp.path().join("system");
        drop(fixture.engine);
        let mut engine = if point == "after_disk_exit_stop" {
            Engine::open(&system).unwrap()
        } else {
            Engine::resume_membership_activation(&system).unwrap()
        };
        if !engine.state.operations.contains_key(&operation) {
            // Before a new marker, the unchanged first-stop locator selects the
            // old fenced generation. Resume its exact controls on retained roots.
            if engine
                .disk_exit_status()
                .unwrap()
                .is_some_and(|status| status.phase == DiskExitPhase::Fenced)
            {
                let archive = engine.build_managed_membership_control_archive().unwrap();
                engine
                    .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
                    .unwrap();
                engine.resume_membership_joint_decision().unwrap();
            }
            engine.activate_pending_disk_exit().unwrap();
        }
        assert!(engine.registered_disk(&disk.disk_id).is_none());
        assert!(engine.list_tree(&fixture.branch).unwrap().is_empty());
        assert!(!disk.root.exists());
        assert_eq!(files(&parked), before);
    }
}

#[test]
fn archived_namespace_proof_cannot_install_a_live_engine_view() {
    let mut fixture = Fixture::prepared(MembershipExitAction::Deregister, 1);
    fixture.engine.activate_pending_disk_exit().unwrap();
    let generation = Arc::clone(fixture.engine.config.generation().unwrap());
    let core = generation.portable.exit_core().unwrap();
    let boundary = generation.intent.boundary.as_ref().unwrap();
    let prepared = core.current_namespaces(&generation.base, None).unwrap();
    let proof = AdmittedDiskExitPublication {
        admission: PublicationAdmission::Archived,
        registry: core.registry().clone(),
        occurrence_mask_hash: generation.portable.activation_mask_hash().unwrap(),
        source_snapshot: boundary.snapshot_hash.clone(),
        source_sequence: boundary.sequence,
        source_business_generation: generation.base.business_generation,
        published_snapshot: fixture.engine.snapshot.clone().unwrap(),
        published_sequence: fixture.engine.state.sequence,
    };
    assert!(!proof.permits_engine_install());
    assert_eq!(
        fixture
            .engine
            .install_current_namespaces(prepared, &proof)
            .unwrap_err()
            .code(),
        ErrorCode::PublicationInvalid
    );
    assert!(
        fixture
            .engine
            .list_tree(&fixture.branch)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn registry_successor_add_and_abort_never_recreate_lost_promoted_journals() {
    for abort in [false, true] {
        let mut fixture = Fixture::prepared(MembershipExitAction::Deregister, 1);
        fixture.engine.activate_pending_disk_exit().unwrap();
        let stopped = fixture.original.disks[0].clone();
        let before = files(&stopped.root);
        let (_, added, archive) = prepare_following_add(&mut fixture.engine, fixture.temp.path());
        if abort {
            fixture
                .engine
                .cancel_membership_handoff(&archive.archive_path, &archive.root_hash)
                .unwrap();
        } else {
            fixture
                .engine
                .enroll_pending_members(
                    &archive.archive_path,
                    &archive.root_hash,
                    vec![PendingMemberDirectory {
                        disk_id: added.disk_id,
                        directory: std::fs::File::open(added.root).unwrap(),
                    }],
                )
                .unwrap();
            fixture.engine.resume_membership_joint_decision().unwrap();
            fixture.engine.activate_pending_membership().unwrap();
        }
        let generation = fixture.engine.config.generation_hash().unwrap().to_owned();
        let journals = fixture
            .engine
            .config
            .metadata_members()
            .map(|disk| generation_directory(disk, &generation).join("journal.bin"))
            .collect::<Vec<_>>();
        let config = fixture.engine.config.clone();
        let system = fixture.temp.path().join("system");
        drop(fixture.engine);
        for path in &journals {
            fs::remove_file(path).unwrap();
        }
        for disk in config.metadata_members() {
            let root = DiskRoot::open(&disk.root).unwrap();
            let bytes = root
                .read_bytes(&disk.root.join("disk.json"), MAX_METADATA)
                .unwrap();
            let expected =
                identity_at(&config, disk, &root.directory(root.path()).unwrap()).unwrap();
            assert_eq!(
                predecessor_seal_matches(&config, disk, &root, &bytes, &expected)
                    .unwrap_err()
                    .code(),
                ErrorCode::RecoveryRequired,
                "a promoted seal cannot admit a registry successor without its own journal"
            );
        }
        assert!(Engine::open(&system).is_err());
        assert!(Engine::resume_membership_activation(&system).is_err());
        assert!(journals.iter().all(|path| !path.exists()));
        assert_eq!(files(&stopped.root), before);
    }
}
