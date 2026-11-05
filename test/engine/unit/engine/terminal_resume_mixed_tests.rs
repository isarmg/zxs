//! Mixed resumption exercises real owned roots, publication and local payload
//! proof consumers. These source cases are not evidence of executed validation.
use super::*;

fn mixed_target(fixture: &Fixture, roles: &[bool]) -> Config {
    let mut target = fixture.target(roles.len());
    for (disk, metadata) in target
        .disks
        .iter_mut()
        .skip(fixture.original.disks.len())
        .zip(roles)
    {
        disk.metadata_member = *metadata;
        disk.level = 1;
        disk.upstream_slot_id = None;
    }
    target
}

fn held(fixture: &Fixture, target: &Config) -> Vec<PendingMemberDirectory> {
    target
        .disks
        .iter()
        .skip(fixture.original.disks.len())
        .map(|disk| PendingMemberDirectory {
            disk_id: disk.disk_id.clone(),
            directory: File::open(&disk.root).unwrap(),
        })
        .collect()
}

fn prepare(fixture: &mut Fixture, target: &Config) -> TerminalMetadataResume {
    let directories = held(fixture, target);
    fixture
        .management
        .take()
        .unwrap()
        .prepare_metadata_resume(target.clone(), "admin", fixture.resume, directories)
        .unwrap()
}

fn assert_locked(root: &Path) {
    let writer = File::open(root.join(".writer.lock")).unwrap();
    assert!(
        rustix::fs::flock(
            &writer,
            rustix::fs::FlockOperation::NonBlockingLockExclusive
        )
        .is_err()
    );
}

fn write_body(engine: &mut Engine, disk: &DiskConfig, name: &str) -> String {
    let repository = engine
        .create_repository(
            &RepositoryName::parse(name).unwrap(),
            &disk.slot_id,
            1,
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
            b"new nonvoting payload",
        )
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "mixed resume body".into(),
            request_digest: None,
        })
        .unwrap();
    engine.list_tree(&branch).unwrap()[0].version_id.clone()
}

#[test]
fn terminal_mixed_resume_rejects_all_nonvoters_before_reserving_roots() {
    let mut fixture = Fixture::new(MembershipExitAction::Deregister, 0);
    let target = mixed_target(&fixture, &[false, false]);
    let directories = held(&fixture, &target);
    let source = files(&fixture.source_path());
    assert!(
        fixture
            .management
            .take()
            .unwrap()
            .prepare_metadata_resume(target.clone(), "admin", fixture.resume, directories)
            .is_err()
    );
    assert_eq!(files(&fixture.source_path()), source);
    for disk in target.disks.iter().skip(fixture.original.disks.len()) {
        assert_eq!(fs::read_dir(&disk.root).unwrap().count(), 0);
    }
}

#[test]
fn terminal_mixed_resume_data_identity_cannot_replace_a_metadata_vote() {
    let mut fixture = Fixture::new(MembershipExitAction::Deregister, 0);
    let target = mixed_target(&fixture, &[true, true, true, false, false]);
    let mut pending = prepare(&mut fixture, &target);
    let status = pending.synchronize().unwrap();
    assert_eq!(
        (
            status.target_metadata_members,
            status.target_metadata_quorum
        ),
        (3, 2)
    );
    assert_eq!(status.ready_data_disks.len(), 2);
    let voter = target
        .disks
        .iter()
        .find(|disk| disk.metadata_member)
        .unwrap();
    let data = target.disks.last().unwrap();
    let mut votes = pending.votes.clone();
    let receipt = votes.remove(&voter.disk_id).unwrap();
    votes.insert(data.disk_id.clone(), receipt);
    assert_eq!(votes.len(), status.target_metadata_members);
    assert_eq!(
        ResumeCore::with_cancellations(pending.proposal.clone(), votes, Vec::new())
            .unwrap_err()
            .code(),
        ErrorCode::MetadataQuorumUnavailable
    );
}

#[test]
fn terminal_mixed_resume_keeps_roles_history_locks_and_standalone_data_proofs() {
    for action in [
        MembershipExitAction::PlannedOffline,
        MembershipExitAction::Deregister,
    ] {
        let mut fixture = Fixture::new(action, 0);
        let target = mixed_target(&fixture, &[false, true, false]);
        let old = &fixture.original.disks[0].root;
        let stopped = fixture.temporary.path().join("stopped-unopened");
        let old_bytes = files(old);
        fs::rename(old, &stopped).unwrap();
        fs::write(old, b"stopped target must not open").unwrap();
        let mut pending = prepare(&mut fixture, &target);
        let status = pending.synchronize().unwrap();
        assert_eq!(
            (
                status.original_metadata_members,
                status.original_metadata_quorum
            ),
            (1, 1)
        );
        assert_eq!(
            (
                status.target_metadata_members,
                status.target_metadata_quorum
            ),
            (1, 1)
        );
        assert_eq!(status.added_data_disks, 2);
        assert_eq!(status.ready_data_disks.len(), 2);
        assert_eq!(
            status.synchronized_target_votes,
            vec![target.disks[2].disk_id.clone()]
        );
        assert_eq!(
            status.installed_certificates,
            status.synchronized_target_votes
        );
        for disk in target.disks.iter().skip(1) {
            assert_locked(&disk.root);
            if !disk.metadata_member {
                assert!(!disk.root.join("metadata/membership-decisions").exists());
                assert!(files(&disk.root).keys().all(|path| {
                    !path
                        .components()
                        .any(|part| part.as_os_str() == CONTROL_ARCHIVE_DIRECTORY)
                }));
            }
        }
        let mut engine = pending.activate().unwrap();
        assert!(!engine.blocked);
        assert_eq!(engine.config.metadata_members().len(), 1);
        assert_eq!(engine.disk_resources.handles().count(), 3);
        assert_eq!(
            crate::canonical_json(&engine.state.versions).unwrap(),
            fixture.historical_versions
        );
        assert_eq!(
            crate::canonical_json(&engine.state.commits).unwrap(),
            fixture.historical_commits
        );
        assert!(engine.state.operations.contains_key(&fixture.initial));
        assert!(engine.state.operations.contains_key(&fixture.terminal));
        let result = engine.state.operations[&fixture.resume].clone();
        let registry = engine
            .config
            .generation()
            .unwrap()
            .portable
            .verified_registry()
            .unwrap()
            .unwrap();
        assert_eq!(registry.voter_count(), 1);
        for disk in target.disks.iter().skip(1) {
            let definition = registry
                .historical_topology()
                .find(|definition| definition.identity.disk_id == disk.disk_id)
                .unwrap();
            assert_eq!(definition.original_metadata_member, disk.metadata_member);
            assert_eq!(
                registry.is_voter(&definition.identity).unwrap(),
                disk.metadata_member
            );
        }
        let data = &target.disks[1];
        let dependency = engine
            .config
            .generation()
            .unwrap()
            .portable
            .terminal_resume_core()
            .unwrap()
            .data_dependency(&data.disk_id)
            .unwrap()
            .unwrap();
        let version = write_body(&mut engine, data, "nonvoting-primary");
        engine.open_content(&version).unwrap();
        assert_locked(&data.root);
        assert_eq!(files(&stopped), old_bytes);
        drop(engine);
        let reopened = Engine::open(&fixture.system()).unwrap();
        assert_eq!(reopened.state.operations[&fixture.resume], result);
        reopened.open_content(&version).unwrap();
        assert_locked(&data.root);
        drop(reopened);
        fs::rename(
            &target.disks[2].root,
            fixture.temporary.path().join("unavailable-voter"),
        )
        .unwrap();
        let readiness = dependency.path(&data.root);
        let ready = fs::read(&readiness).unwrap();
        fs::remove_file(&readiness).unwrap();
        assert!(
            crate::disk::offline_export(
                &data.root,
                &version,
                &fixture.temporary.path().join("missing-ready-export")
            )
            .is_err()
        );
        fs::write(&readiness, ready).unwrap();
        let output = fixture.temporary.path().join("data-export");
        crate::disk::offline_export(&data.root, &version, &output).unwrap();
        assert_eq!(fs::read(output).unwrap(), b"new nonvoting payload");
        assert!(
            crate::disk::offline_check(&data.root)
                .unwrap()
                .issues
                .is_empty()
        );
        assert!(files(&data.root).keys().all(|path| {
            path.file_name().unwrap() != "journal.bin"
                && !path
                    .components()
                    .any(|part| part.as_os_str() == CONTROL_ARCHIVE_DIRECTORY)
        }));
        assert_eq!(files(&stopped), old_bytes);
    }
}

#[test]
fn terminal_mixed_resume_cold_request_and_data_marker_reenter_same_operation() {
    for promoted in [false, true] {
        let mut fixture = Fixture::new(MembershipExitAction::Deregister, 1);
        let target = mixed_target(&fixture, &[true, false]);
        let mut pending = prepare(&mut fixture, &target);
        pending.synchronize().unwrap();
        drop(pending);
        let mut pending =
            TerminalManagement::reopen_metadata_resume(&fixture.system(), held(&fixture, &target))
                .unwrap();
        assert_eq!(pending.status().unwrap().operation_id, fixture.resume);
        pending.synchronize().unwrap();
        select_generation(&pending);
        let portable = activation::PortableAuthority::new_terminal_resume(
            pending.core.as_ref().unwrap().clone(),
        )
        .unwrap();
        let hash = portable.hash().unwrap();
        let data = target.disks.last().unwrap();
        let resource = &pending.members[&data.disk_id];
        resource
            .root
            .write_json(
                &data.root.join("metadata/pool-membership/generation.json"),
                &portable,
                false,
            )
            .unwrap();
        if promoted {
            let identity = &resource.seal.identity;
            let seal = crate::disk::DiskSeal {
                format: crate::disk::DiskSealFormat::MembershipGeneration {
                    authority_hash: hash.clone(),
                },
                pool_id: identity.pool_id.clone(),
                disk_id: identity.disk_id.clone(),
                slot_id: identity.slot_id.clone(),
                binding_generation: identity.binding_generation,
                device: identity.device,
                inode: identity.inode,
                simulation: identity.simulation,
            };
            resource
                .root
                .write_json(&data.root.join("disk.json"), &seal, true)
                .unwrap();
        }
        drop(pending);
        assert_eq!(
            TerminalManagement::metadata_resume_recovery(&fixture.system())
                .unwrap()
                .unwrap()
                .phase,
            crate::TerminalMetadataResumePhase::CompleteGeneration
        );
        let mut engine = TerminalManagement::complete_metadata_resume(&fixture.system()).unwrap();
        assert_eq!(engine.config.generation_hash(), Some(hash.as_str()));
        assert!(engine.disk_resources.available(data).is_some());
        assert_locked(&data.root);
        let version = write_body(&mut engine, data, "after-data-marker");
        engine.open_content(&version).unwrap();
        let result = engine.state.operations[&fixture.resume].clone();
        drop(engine);
        let reopened = Engine::open(&fixture.system()).unwrap();
        assert_eq!(reopened.state.operations[&fixture.resume], result);
        reopened.open_content(&version).unwrap();
    }
}

#[test]
fn terminal_mixed_resume_bootstrap_barriers_record_data_roles_without_new_votes() {
    for phase in [
        WriteFault::BeforeRecordBarrier,
        WriteFault::AfterRecordBarrier,
        WriteFault::BeforeHeadBarrier,
        WriteFault::AfterHeadBarrier,
    ] {
        let mut fixture = Fixture::new(MembershipExitAction::Deregister, 0);
        let target = mixed_target(&fixture, &[true, false, false]);
        let pending = prepare(&mut fixture, &target);
        inject_next(phase);
        assert!(pending.activate().is_err(), "{phase:?}");
        let engine = TerminalManagement::complete_metadata_resume(&fixture.system()).unwrap();
        assert_eq!(engine.config.metadata_members().len(), 1);
        assert_eq!(engine.disk_resources.handles().count(), 3);
        drop(engine);
        let engine = Engine::open(&fixture.system()).unwrap();
        assert_eq!(engine.config.metadata_quorum(), 1);
        assert_eq!(engine.disk_resources.handles().count(), 3);
    }
}

#[test]
fn terminal_mixed_resume_changed_readiness_blocks_activation_and_retained_roots() {
    let mut fixture = Fixture::new(MembershipExitAction::Deregister, 1);
    let target = mixed_target(&fixture, &[true, false]);
    let mut pending = prepare(&mut fixture, &target);
    pending.synchronize().unwrap();
    let core = pending.core.as_ref().unwrap();
    let data = target.disks.last().unwrap();
    let dependency = core.data_dependency(&data.disk_id).unwrap().unwrap();
    let path = dependency.path(&data.root);
    let bytes = fs::read(&path).unwrap();
    select_generation(&pending);
    drop(pending);
    let mut changed: json_codec::Value = decode_strict(&bytes).unwrap();
    changed["seal"]["transition_hash"] = "f".repeat(64).into();
    let changed = crate::canonical_json(&changed).unwrap();
    let expected = dependency.verify_bytes(&changed).unwrap_err().code();
    fs::write(&path, &changed).unwrap();
    let old_data = &fixture.original.disks[1].root;
    let saved_data = fixture.temporary.path().join("retained-data-away");
    fs::rename(old_data, &saved_data).unwrap();
    fs::write(
        old_data,
        b"must verify added readiness before this retained root",
    )
    .unwrap();
    let error = TerminalManagement::complete_metadata_resume(&fixture.system())
        .err()
        .unwrap();
    assert_eq!(error.code(), expected);
    assert_eq!(fs::read(&path).unwrap(), changed);
    assert!(!fixture.source_path().join("resume-complete.json").exists());
    fs::remove_file(old_data).unwrap();
    fs::rename(saved_data, old_data).unwrap();
    fs::write(&path, bytes).unwrap();
    let engine = TerminalManagement::complete_metadata_resume(&fixture.system()).unwrap();
    assert!(engine.state.operations.contains_key(&fixture.resume));
    assert_locked(&data.root);
}

#[test]
fn terminal_mixed_resume_requires_exact_data_fd_and_preserves_readiness_staging() {
    for prefix in [0, 1, 2] {
        let mut fixture = Fixture::new(MembershipExitAction::Deregister, 0);
        let target = mixed_target(&fixture, &[true, false]);
        let mut pending = prepare(&mut fixture, &target);
        pending.synchronize().unwrap();
        let data = target.disks.last().unwrap();
        let dependency = pending
            .core
            .as_ref()
            .unwrap()
            .data_dependency(&data.disk_id)
            .unwrap()
            .unwrap();
        let path = dependency.path(&data.root);
        let expected = fs::read(&path).unwrap();
        fs::remove_file(&path).unwrap();
        let prefix = &expected[..expected.len() * prefix / 2];
        let staging = path
            .parent()
            .unwrap()
            .join(".state-atomic-0123456789abcdef0123456789abcdef.tmp");
        fs::write(&staging, prefix).unwrap();
        drop(pending);
        let mut missing = held(&fixture, &target);
        missing.pop();
        assert!(TerminalManagement::reopen_metadata_resume(&fixture.system(), missing).is_err());
        let mut pending =
            TerminalManagement::reopen_metadata_resume(&fixture.system(), held(&fixture, &target))
                .unwrap();
        assert_eq!(
            pending.synchronize().unwrap().ready_data_disks,
            vec![data.disk_id.clone()]
        );
        assert_eq!(fs::read(&path).unwrap(), expected);
        assert_eq!(fs::read(&staging).unwrap(), prefix);
        let engine = pending.activate().unwrap();
        assert_eq!(engine.config.metadata_members().len(), 1);
        assert_eq!(fs::read(staging).unwrap(), prefix);
    }
}

#[test]
fn terminal_mixed_resume_roles_and_physical_identity_cannot_be_rebound() {
    let mut fixture = Fixture::new(MembershipExitAction::Deregister, 0);
    let target = mixed_target(&fixture, &[true, false]);
    drop(prepare(&mut fixture, &target));
    let data = target.disks.last().unwrap();
    let mut changed = target.clone();
    changed.disks.last_mut().unwrap().metadata_member = true;
    assert!(
        TerminalManagement::open_for_resume(&fixture.system())
            .unwrap()
            .prepare_metadata_resume(
                changed.clone(),
                "admin",
                fixture.resume,
                held(&fixture, &changed),
            )
            .is_err()
    );
    let saved = fixture.temporary.path().join("original-selected-data");
    fs::rename(&data.root, &saved).unwrap();
    fs::create_dir(&data.root).unwrap();
    fs::set_permissions(&data.root, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        TerminalManagement::reopen_metadata_resume(&fixture.system(), held(&fixture, &target))
            .is_err()
    );
    assert_eq!(fs::read_dir(&data.root).unwrap().count(), 0);
    fs::remove_dir(&data.root).unwrap();
    fs::rename(saved, &data.root).unwrap();
    let pending =
        TerminalManagement::reopen_metadata_resume(&fixture.system(), held(&fixture, &target))
            .unwrap();
    assert_eq!(pending.status().unwrap().operation_id, fixture.resume);
}

#[test]
fn terminal_mixed_resume_data_dependency_survives_a_real_add_successor() {
    let mut fixture = Fixture::new(MembershipExitAction::Deregister, 0);
    let target = mixed_target(&fixture, &[true, false]);
    let mut engine = prepare(&mut fixture, &target).activate().unwrap();
    let data = target.disks.last().unwrap();
    let version = write_body(&mut engine, data, "before-successor");
    let resume_result = engine.state.operations[&fixture.resume].clone();
    let old_authority = engine.config.generation_hash().unwrap().to_owned();
    let original_dependency = engine
        .config
        .generation()
        .unwrap()
        .portable
        .terminal_resume_core()
        .unwrap()
        .data_dependency(&data.disk_id)
        .unwrap()
        .unwrap();
    let readiness = original_dependency.path(&data.root);
    let ready_bytes = fs::read(&readiness).unwrap();
    let mut next = engine.config().clone();
    let previous = next.disks.len();
    let mut added = Config::development(
        (0..previous + 1).map(|n| fixture.temporary.path().join(format!("later-{n}"))),
    )
    .unwrap()
    .disks
    .pop()
    .unwrap();
    added.metadata_member = false;
    added.level = 1;
    added.upstream_slot_id = None;
    fs::create_dir(&added.root).unwrap();
    fs::set_permissions(&added.root, fs::Permissions::from_mode(0o700)).unwrap();
    next.disks.push(added.clone());
    let operation = OperationId::generate().unwrap();
    engine
        .begin_membership_handoff(next, "admin", operation)
        .unwrap();
    let archive_path = fixture.temporary.path().join("later-archive");
    fs::create_dir(&archive_path).unwrap();
    fs::set_permissions(&archive_path, fs::Permissions::from_mode(0o700)).unwrap();
    let archive = engine
        .build_membership_control_archive(&archive_path)
        .unwrap();
    engine
        .enroll_pending_members(
            &archive_path,
            &archive.root_hash,
            vec![PendingMemberDirectory {
                disk_id: added.disk_id.clone(),
                directory: File::open(&added.root).unwrap(),
            }],
        )
        .unwrap();
    let joint = engine.resume_membership_joint_decision().unwrap();
    assert_eq!((joint.original_members, joint.target_members), (1, 1));
    let result = engine.activate_pending_membership().unwrap();
    assert_eq!(result.operation_id, operation);
    assert_ne!(
        engine.config.generation_hash(),
        Some(old_authority.as_str())
    );
    assert_eq!(fs::read(&readiness).unwrap(), ready_bytes);
    assert_eq!(engine.state.operations[&fixture.resume], resume_result);
    engine.open_content(&version).unwrap();
    drop(engine);
    let reopened = Engine::open(&fixture.system()).unwrap();
    reopened.open_content(&version).unwrap();
    assert_eq!(reopened.state.operations[&operation], result);
    assert_locked(&data.root);
    assert_locked(&added.root);
    drop(reopened);
    let output = fixture.temporary.path().join("after-successor-export");
    fs::remove_file(&readiness).unwrap();
    assert!(crate::disk::offline_export(&data.root, &version, &output).is_err());
    fs::write(&readiness, ready_bytes).unwrap();
    crate::disk::offline_export(&data.root, &version, &output).unwrap();
    assert_eq!(fs::read(output).unwrap(), b"new nonvoting payload");
}
