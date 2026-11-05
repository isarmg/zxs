//! Real system control records, original voter journals and I/O domain closure.
//! These tests never treat a zero-member configuration as an active pool.
use super::*;
use crate::{CommitRequest, Config, RegistrationMode, RepositoryName};
use std::{collections::BTreeMap, fs, path::PathBuf};

struct Fixture {
    temp: tempfile::TempDir,
    engine: Engine,
    config: Config,
    original_operation: OperationId,
    operation: OperationId,
}

impl Fixture {
    fn new(data_only: usize) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let mut config =
            Config::development((0..=data_only).map(|n| temp.path().join(format!("disk-{n}"))))
                .unwrap();
        for disk in config.disks.iter_mut().skip(1) {
            disk.metadata_member = false;
        }
        let mut engine = Engine::initialize(&temp.path().join("system"), config.clone()).unwrap();
        let original_operation = OperationId::generate().unwrap();
        let repository = engine
            .create_repository(
                &RepositoryName::parse("retained").unwrap(),
                "P01",
                1,
                "admin",
                original_operation,
            )
            .unwrap();
        let workspace = engine
            .create_workspace(&repository.main_branch_id, "admin")
            .unwrap();
        engine
            .stage_bytes(
                &workspace.workspace_id,
                "kept.txt",
                b"original payload stays on its original disk",
            )
            .unwrap();
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: OperationId::generate().unwrap(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "keep immutable history".into(),
                request_digest: None,
            })
            .unwrap();
        Self {
            temp,
            engine,
            config,
            original_operation,
            operation: OperationId::generate().unwrap(),
        }
    }
    fn begin(&mut self, action: MembershipExitAction) -> TerminalDiskExit {
        let status = self
            .engine
            .begin_disk_exit(
                &self.config.disks[0].disk_id,
                action,
                "admin",
                self.operation,
            )
            .unwrap();
        assert_eq!((status.original_members, status.original_quorum), (1, 1));
        assert!(status.all_members_fenced());
        assert!(self.engine.disk_exit_requires_management());
        assert_eq!(self.engine.config.metadata_members().len(), 1);
        self.engine.prepare_terminal_disk_exit().unwrap()
    }
    fn system(&self) -> PathBuf {
        self.temp.path().join("system")
    }
    fn completion(&self) -> PathBuf {
        self.system()
            .join("terminal-management")
            .join(self.operation.to_string())
            .join("complete.json")
    }
}

fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(base: &Path, path: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(base, &path, result);
            } else {
                result.insert(
                    path.strip_prefix(base).unwrap().to_owned(),
                    fs::read(&path).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}

#[test]
fn final_voter_offline_and_delete_publish_distinct_management_registration() {
    for action in [
        MembershipExitAction::PlannedOffline,
        MembershipExitAction::Deregister,
    ] {
        let mut fixture = Fixture::new(0);
        let historical_versions = crate::canonical_json(&fixture.engine.state.versions).unwrap();
        let historical_commits = crate::canonical_json(&fixture.engine.state.commits).unwrap();
        let old_result = fixture.engine.state.operations[&fixture.original_operation].clone();
        let target = fixture
            .engine
            .registered_disk(&fixture.config.disks[0].disk_id)
            .unwrap()
            .identity();
        let mut exit = fixture.begin(action);
        let before = files(&fixture.config.disks[0].root);
        let management = exit.complete(&mut fixture.engine).unwrap();
        assert_eq!(
            files(&fixture.config.disks[0].root),
            before,
            "no stopped-target recovery backup or payload write"
        );
        assert_eq!(
            crate::canonical_json(&management.state().versions).unwrap(),
            historical_versions
        );
        assert_eq!(
            crate::canonical_json(&management.state().commits).unwrap(),
            historical_commits
        );
        assert_eq!(
            management.operation(fixture.original_operation),
            Some(&old_result)
        );
        assert_eq!(
            management.state().operation_principals[&fixture.operation],
            "admin"
        );
        assert_eq!(management.result().operation_id, fixture.operation);
        assert_eq!(
            management.result().current_health,
            crate::CurrentHealth::Unknown
        );
        assert!(!management.status().writable);
        assert_eq!(management.status().active_metadata_members, 0);
        assert_eq!(management.result().result["target_io"], "closed");
        assert_eq!(
            management.original_configuration().metadata_members().len(),
            1
        );
        assert_eq!(management.original_configuration().metadata_quorum(), 1);
        assert_eq!(management.registry().historical_topology().len(), 1);
        let (registered, mode) = match action {
            MembershipExitAction::PlannedOffline => (1, Some(RegistrationMode::Disabled)),
            MembershipExitAction::Deregister => (0, None),
        };
        assert_eq!(management.registered_disks().len(), registered);
        assert_eq!(management.registry().registration(&target).unwrap(), mode);
        let statuses = management.disk_status();
        assert_eq!(statuses.len(), registered);
        assert!(statuses.iter().all(|disk| !disk.online
            && !disk.metadata_member
            && disk.available_bytes.is_none()
            && disk.physical_device.is_none()));
        if action == MembershipExitAction::PlannedOffline {
            assert!(
                statuses[0].used_bytes > 0,
                "logical usage is retained, not replaced with a fake capacity sample"
            );
        }
        assert_eq!(
            management.status().registrations,
            if registered == 0 {
                TerminalRegistrationState::EmptyRegistry
            } else {
                TerminalRegistrationState::RetainedRegistrations
            }
        );
        assert!(fixture.completion().is_file());
        let expected = management.result().clone();
        let system = fixture.system();
        drop(management);
        drop(exit);
        drop(fixture.engine);
        let moved = fixture.temp.path().join("removed-storage");
        fs::rename(&fixture.config.disks[0].root, &moved).unwrap();
        assert_eq!(
            Engine::open(&system).err().unwrap().code(),
            ErrorCode::RecoveryRequired
        );
        let recovered = TerminalManagement::open(&system).unwrap();
        assert_eq!(recovered.result(), &expected);
        assert_eq!(recovered.registry().registration(&target).unwrap(), mode);
        assert_eq!(files(&moved), before);
    }
}

#[test]
fn final_voter_delete_retains_data_only_registrations_without_probing_any_storage_root() {
    let mut fixture = Fixture::new(2);
    let retained = fixture
        .config
        .disks
        .iter()
        .skip(1)
        .map(|disk| disk.disk_id.clone())
        .collect::<Vec<_>>();
    let mut exit = fixture.begin(MembershipExitAction::Deregister);
    let management = exit.complete(&mut fixture.engine).unwrap();
    assert_eq!(
        management.status().registrations,
        TerminalRegistrationState::RetainedRegistrations
    );
    assert_eq!(management.status().registered_disks, 2);
    assert!(
        management
            .registered_disks()
            .all(|disk| !disk.metadata_member
                && disk.registration_mode == RegistrationMode::Enabled
                && retained.contains(&disk.disk_id.to_owned()))
    );
    let system = fixture.system();
    drop(management);
    drop(exit);
    drop(fixture.engine);
    for (index, disk) in fixture.config.disks.iter().enumerate() {
        fs::rename(
            &disk.root,
            fixture.temp.path().join(format!("unplugged-{index}")),
        )
        .unwrap();
    }
    let recovered = TerminalManagement::open(&system).unwrap();
    assert_eq!(recovered.registered_disks().len(), 2);
    assert_eq!(recovered.registry().historical_topology().len(), 3);
}

#[test]
fn terminal_completion_waits_for_real_io_tail_and_retries_same_request() {
    let mut fixture = Fixture::new(0);
    let mut exit = fixture.begin(MembershipExitAction::Deregister);
    let tail = exit.binding.admit().unwrap();
    let error = exit.complete(&mut fixture.engine).err().unwrap();
    assert_eq!(error.code(), ErrorCode::DiskUnavailable);
    assert!(!fixture.completion().exists());
    assert!(exit.binding.status().unwrap().quiescing);
    let bootstrap = open_store(&fixture.system()).unwrap();
    let locator = bootstrap.reload(&fixture.config.pool_id).unwrap();
    assert_eq!(locator.stop_hints().count(), 1);
    assert!(locator.select(&fixture.config).is_err());
    drop(bootstrap);
    drop(tail);
    let management = exit.complete(&mut fixture.engine).unwrap();
    assert_eq!(management.status().operation_id, fixture.operation);
    assert!(exit.binding.closed_proof().is_ok());
}

#[test]
fn prepared_terminal_recovers_after_source_disappears_without_new_genesis() {
    let mut fixture = Fixture::new(0);
    let exit = fixture.begin(MembershipExitAction::Deregister);
    let expected_hash = exit.authority_hash().to_owned();
    let before = files(&fixture.config.disks[0].root);
    let system = fixture.system();
    assert!(!fixture.completion().exists());
    drop(exit);
    drop(fixture.engine);
    fs::rename(
        &fixture.config.disks[0].root,
        fixture.temp.path().join("physically-unavailable"),
    )
    .unwrap();
    let recovered = TerminalManagement::open(&system).unwrap();
    assert_eq!(recovered.status().authority_hash, expected_hash);
    assert!(recovered.operation(fixture.original_operation).is_some());
    assert_eq!(
        recovered.original_configuration().pool_id,
        fixture.config.pool_id
    );
    assert_eq!(
        files(&fixture.temp.path().join("physically-unavailable")),
        before
    );
}

#[test]
fn corrupt_control_archive_cannot_write_terminal_stop_or_report_completion() {
    let mut fixture = Fixture::new(0);
    let mut exit = fixture.begin(MembershipExitAction::Deregister);
    let archive = exit.control_archive();
    fs::write(archive.archive_path.join("archive-root.json"), b"{}").unwrap();
    assert!(exit.complete(&mut fixture.engine).is_err());
    assert!(!fixture.completion().exists());
    assert!(!exit.binding.status().unwrap().quiescing);
    let bootstrap = open_store(&fixture.system()).unwrap();
    assert_eq!(
        bootstrap
            .reload(&fixture.config.pool_id)
            .unwrap()
            .stop_hints()
            .count(),
        0
    );
}

#[test]
fn ordinary_membership_enrollment_cannot_consume_terminal_fence() {
    let mut fixture = Fixture::new(0);
    let exit = fixture.begin(MembershipExitAction::PlannedOffline);
    let archive = exit.control_archive();
    assert_eq!(
        fixture
            .engine
            .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
            .err()
            .unwrap()
            .code(),
        ErrorCode::ConfigConflict
    );
    assert!(!fixture.completion().exists());
    assert_eq!(fixture.engine.config.metadata_members().len(), 1);
}

#[test]
fn terminal_startup_rejects_unverified_resume_and_changed_authority() {
    for corrupt_authority in [false, true] {
        let mut fixture = Fixture::new(0);
        let mut exit = fixture.begin(MembershipExitAction::Deregister);
        let management = exit.complete(&mut fixture.engine).unwrap();
        let directory = fixture.completion().parent().unwrap().to_path_buf();
        let system = fixture.system();
        drop(management);
        drop(exit);
        drop(fixture.engine);
        if corrupt_authority {
            fs::write(directory.join("authority.json"), b"{}").unwrap();
        } else {
            fs::write(directory.join("resume.json"), b"{}").unwrap();
        }
        let before = files(fixture.temp.path());
        // Selection now authenticates the complete authority and attempt chain,
        // so corrupt evidence is a recovery error, not a presence-only route.
        let error = TerminalManagement::requires_management(&system).unwrap_err();
        assert_eq!(error.code(), ErrorCode::RecoveryRequired);
        assert!(!error.is_retryable());
        let mut source = std::error::Error::source(&error)
            .expect("selector recovery error preserves its concrete cause");
        while let Some(next) = source.source() {
            source = next;
        }
        if corrupt_authority {
            assert_eq!(
                source.downcast_ref::<Box<Error>>().unwrap().code(),
                ErrorCode::ObjectCorrupt
            );
        } else {
            assert!(source.is::<json_codec::Error>());
        }
        assert!(TerminalManagement::open(&system).is_err());
        for error in [
            Engine::open(&system).err().unwrap(),
            Engine::open_for_recovery(&system).err().unwrap(),
        ] {
            assert_eq!(error.code(), ErrorCode::RecoveryRequired);
            assert!(!error.is_retryable());
        }
        assert_eq!(files(fixture.temp.path()), before);
    }
}

#[test]
fn terminal_recovery_preserves_operation_across_stop_close_and_publish_failures() {
    for point in [
        "after_terminal_stop",
        "after_terminal_io_closed",
        "after_terminal_publication",
    ] {
        let mut fixture = Fixture::new(0);
        let mut exit = fixture.begin(MembershipExitAction::Deregister);
        let authority = exit.authority_hash().to_owned();
        fixture.engine.inject_fault_once(point);
        assert_eq!(
            exit.complete(&mut fixture.engine).err().unwrap().code(),
            ErrorCode::InjectedFailure
        );
        let system = fixture.system();
        let before = files(&fixture.config.disks[0].root);
        drop(exit);
        drop(fixture.engine);
        fs::rename(
            &fixture.config.disks[0].root,
            fixture.temp.path().join("stopped-root"),
        )
        .unwrap();
        let recovered = TerminalManagement::open(&system).unwrap();
        assert_eq!(recovered.status().authority_hash, authority);
        assert_eq!(recovered.result().operation_id, fixture.operation);
        assert_eq!(files(&fixture.temp.path().join("stopped-root")), before);
    }
}

#[test]
fn terminal_continues_prior_exit_authority_and_replayed_occurrence_history() {
    let temp = tempfile::tempdir().unwrap();
    let config =
        Config::development((0..3).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
    let system = temp.path().join("system");
    let mut engine = Engine::initialize(&system, config.clone()).unwrap();
    let seed = OperationId::generate().unwrap();
    let repository = engine
        .create_repository(
            &RepositoryName::parse("lineage").unwrap(),
            "P01",
            1,
            "admin",
            seed,
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(
            &workspace.workspace_id,
            "exclusive.txt",
            b"keep removed content as history",
        )
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "preserve".into(),
            request_digest: None,
        })
        .unwrap();
    let mut exits = Vec::new();
    for disk in config.disks.iter().take(2) {
        let operation = OperationId::generate().unwrap();
        engine
            .begin_disk_exit(
                &disk.disk_id,
                MembershipExitAction::Deregister,
                "admin",
                operation,
            )
            .unwrap();
        let archive = engine.build_managed_membership_control_archive().unwrap();
        engine
            .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
            .unwrap();
        engine.resume_membership_joint_decision().unwrap();
        exits.push(engine.activate_pending_disk_exit().unwrap());
    }
    assert_eq!(engine.config.metadata_members().len(), 1);
    let predecessor = engine.config.generation_hash().unwrap().to_owned();
    let operation = OperationId::generate().unwrap();
    engine
        .begin_disk_exit(
            &config.disks[2].disk_id,
            MembershipExitAction::Deregister,
            "admin",
            operation,
        )
        .unwrap();
    let mut pending = engine.prepare_terminal_disk_exit().unwrap();
    let management = pending.complete(&mut engine).unwrap();
    assert_eq!(
        management
            .verified_authority()
            .authority
            .predecessor()
            .unwrap()
            .hash()
            .unwrap(),
        predecessor
    );
    assert_eq!(management.registry().historical_topology().len(), 3);
    assert_eq!(management.registered_disks().len(), 0);
    assert!(management.operation(seed).is_some());
    for result in &exits {
        assert_eq!(management.operation(result.operation_id), Some(result));
    }
    let terminal = management.result().clone();
    drop(management);
    drop(pending);
    drop(engine);
    for (index, disk) in config.disks.iter().enumerate() {
        fs::rename(&disk.root, temp.path().join(format!("removed-{index}"))).unwrap();
    }
    let reopened = TerminalManagement::open(&system).unwrap();
    assert_eq!(reopened.result(), &terminal);
    for result in &exits {
        assert_eq!(reopened.operation(result.operation_id), Some(result));
    }
}
