//! Real missing-root preparation, durable journal evidence and retained recovery.
use super::*;
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

struct Fixture {
    temp: tempfile::TempDir,
    engine: Engine,
    original: Config,
    missing: PathBuf,
    operation: OperationId,
    action: MembershipExitAction,
    branch: String,
    version: String,
}

impl Fixture {
    fn new(voters: usize, action: MembershipExitAction) -> Self {
        Self::with_target_role(voters, action, true)
    }

    fn with_target_role(
        voters: usize,
        action: MembershipExitAction,
        metadata_member: bool,
    ) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let mut original =
            Config::development((0..voters).map(|n| temp.path().join(format!("disk-{n}"))))
                .unwrap();
        original.disks[0].metadata_member = metadata_member;
        let system = temp.path().join("system");
        let mut engine = Engine::initialize(&system, original.clone()).unwrap();
        let repository = engine
            .create_repository(
                &RepositoryName::parse("missing-exit").unwrap(),
                "P01",
                voters as u32,
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        let branch = repository.main_branch_id;
        let workspace = engine.create_workspace(&branch, "admin").unwrap();
        engine
            .stage_bytes(
                &workspace.workspace_id,
                "shared.txt",
                b"retained historical bytes",
            )
            .unwrap();
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: OperationId::generate().unwrap(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "published before loss".into(),
                request_digest: None,
            })
            .unwrap();
        let version = engine.list_tree(&branch).unwrap()[0].version_id.clone();
        drop(engine);
        let missing = temp.path().join("unavailable-original");
        fs::rename(&original.disks[0].root, &missing).unwrap();
        let engine = Engine::open(&system).unwrap();
        Self {
            temp,
            engine,
            original,
            missing,
            operation: OperationId::generate().unwrap(),
            action,
            branch,
            version,
        }
    }

    fn begin(&mut self) -> Result<MembershipFenceStatus> {
        self.engine.begin_disk_exit(
            &self.original.disks[0].disk_id,
            self.action,
            "admin",
            self.operation,
        )
    }

    fn controls(&mut self) -> MembershipControlArchive {
        let archive = self
            .engine
            .build_managed_membership_control_archive()
            .unwrap();
        let inspected = verify_membership_control_archive(
            &archive.archive_path,
            &archive.root_hash,
            &archive.transition_hash,
        )
        .unwrap();
        assert_eq!(inspected.original_members, self.original.disks.len());
        assert_eq!(inspected.journals.len(), self.original.disks.len() - 1);
        assert!(inspected.published_candidates >= 2);
        assert!(
            inspected
                .journals
                .iter()
                .all(|journal| journal.disk_id != self.original.disks[0].disk_id)
        );
        let pending = self
            .engine
            .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
            .unwrap();
        assert_eq!(pending.control_participants, self.original.disks.len() - 1);
        assert!(pending.all_control_metadata_synchronized());
        let joint = self.engine.resume_membership_joint_decision().unwrap();
        assert_eq!(joint.original_members, self.original.disks.len());
        assert_eq!(joint.original_quorum, self.original.disks.len() / 2 + 1);
        assert_eq!(joint.union_members, self.original.disks.len());
        assert_eq!(joint.target_members, self.original.disks.len() - 1);
        assert_eq!(joint.target_quorum, (self.original.disks.len() - 1) / 2 + 1);
        assert_eq!(
            joint.certificate_quorum_counts(),
            (self.original.disks.len() - 1, self.original.disks.len() - 1)
        );
        assert!(!joint.all_certificates_synchronized());
        assert!(joint.retained_certificates_synchronized());
        archive
    }

    fn reopen(self) -> Self {
        self.reopen_with(Engine::open)
    }

    fn reopen_with(self, open: fn(&Path) -> Result<Engine>) -> Self {
        let Self {
            temp,
            engine,
            original,
            missing,
            operation,
            action,
            branch,
            version,
        } = self;
        drop(engine);
        let engine = open(&temp.path().join("system")).unwrap();
        Self {
            temp,
            engine,
            original,
            missing,
            operation,
            action,
            branch,
            version,
        }
    }

    fn assert_body(&self) {
        let mut actual = Vec::new();
        self.engine
            .open_content(&self.version)
            .unwrap()
            .file
            .read_to_end(&mut actual)
            .unwrap();
        assert_eq!(actual, b"retained historical bytes");
    }
}

fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(base: &Path, path: &Path, entries: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(base, &path, entries);
            } else {
                entries.insert(
                    path.strip_prefix(base).unwrap().to_owned(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut entries = BTreeMap::new();
    visit(root, root, &mut entries);
    entries
}

#[test]
fn missing_exit_target_completes_with_original_quorum_and_no_fabricated_journal() {
    for (voters, action) in [
        (3, MembershipExitAction::Deregister),
        (4, MembershipExitAction::PlannedOffline),
    ] {
        let mut fixture = Fixture::new(voters, action);
        let missing_before = files(&fixture.missing);
        let historical_versions = canonical_json(&fixture.engine.state.versions).unwrap();
        let historical_commits = canonical_json(&fixture.engine.state.commits).unwrap();
        assert!(fixture.engine.blocked);
        let fenced = fixture.begin().unwrap();
        assert_eq!(
            (fenced.original_members, fenced.original_quorum),
            (voters, voters / 2 + 1)
        );
        assert_eq!(fenced.confirmed_members, voters - 1);
        assert!(fenced.quorum_fenced());
        assert!(!fenced.all_members_fenced());
        let intent = &fixture.engine.membership_fence.as_ref().unwrap().intent;
        assert_eq!(intent.fence_scope, Some(FenceScope::RetainedExitQuorum));
        assert_eq!(intent.origin.config().metadata_members().len(), voters);
        let archive = fixture.controls();
        let result = fixture.engine.activate_pending_disk_exit().unwrap();
        assert_eq!(result.operation_id, fixture.operation);
        assert_eq!(result.result["target_io"], "closed");
        assert_eq!(
            fixture.engine.disk_exit_status().unwrap().unwrap().phase,
            DiskExitPhase::Published
        );
        assert_eq!(
            canonical_json(&fixture.engine.state.versions).unwrap(),
            historical_versions
        );
        assert_eq!(
            canonical_json(&fixture.engine.state.commits).unwrap(),
            historical_commits
        );
        assert_eq!(files(&fixture.missing), missing_before);
        assert!(!fixture.original.disks[0].root.exists());
        fixture.assert_body();
        let mut fixture = fixture.reopen();
        assert_eq!(fixture.engine.activate_pending_disk_exit().unwrap(), result);
        fixture.assert_body();
        assert_eq!(files(&fixture.missing), missing_before);
        assert_eq!(
            verify_membership_control_archive(
                &archive.archive_path,
                &archive.root_hash,
                &archive.transition_hash
            )
            .unwrap()
            .journals
            .len(),
            voters - 1
        );
    }
}

#[test]
fn missing_exit_target_resumes_exact_partial_fence_and_archive_after_restart() {
    for point in [
        "after_membership_fence_1",
        "after_membership_archive_journals",
        "before_membership_archive_complete",
    ] {
        let mut fixture = Fixture::new(3, MembershipExitAction::Deregister);
        let before = files(&fixture.missing);
        fixture.engine.inject_fault_once(point);
        if point == "after_membership_fence_1" {
            assert_eq!(
                fixture.begin().unwrap_err().code(),
                ErrorCode::InjectedFailure
            );
            assert!(
                !fixture
                    .engine
                    .membership_fence_status()
                    .unwrap()
                    .quorum_fenced()
            );
        } else {
            fixture.begin().unwrap();
            assert_eq!(
                fixture
                    .engine
                    .build_managed_membership_control_archive()
                    .unwrap_err()
                    .code(),
                ErrorCode::InjectedFailure
            );
        }
        let hash = fixture
            .engine
            .membership_fence_status()
            .unwrap()
            .transition_hash;
        let mut fixture = fixture.reopen();
        assert_eq!(fixture.begin().unwrap().transition_hash, hash);
        let archive = fixture.controls();
        assert_eq!(archive.transition_hash, hash);
        fixture.engine.activate_pending_disk_exit().unwrap();
        assert_eq!(files(&fixture.missing), before);
    }
}

#[test]
fn missing_exit_target_stop_and_retained_publication_interruptions_recover() {
    for point in ["after_disk_exit_stop", "before_disk_exit_final_admission"] {
        let mut fixture = Fixture::new(3, MembershipExitAction::PlannedOffline);
        let before = files(&fixture.missing);
        fixture.begin().unwrap();
        fixture.controls();
        fixture.engine.inject_fault_once(point);
        assert_eq!(
            fixture
                .engine
                .activate_pending_disk_exit()
                .unwrap_err()
                .code(),
            ErrorCode::InjectedFailure
        );
        // A durable stop with only prepared authority is not an ordinary-open
        // permission to promote the first generation marker or publish D3.
        let mut fixture = if point == "after_disk_exit_stop" {
            fixture.reopen_with(Engine::resume_membership_activation)
        } else {
            fixture.reopen()
        };
        fixture.engine.activate_pending_disk_exit().unwrap();
        assert_eq!(
            fixture.engine.disk_exit_status().unwrap().unwrap().phase,
            DiskExitPhase::Published
        );
        fixture.assert_body();
        assert_eq!(files(&fixture.missing), before);
        assert!(!fixture.original.disks[0].root.exists());
    }
}

#[test]
fn missing_exit_target_rejects_missing_history_before_any_fence() {
    let mut fixture = Fixture::new(3, MembershipExitAction::Deregister);
    let disk = &fixture.original.disks[1];
    let record = scan(disk)
        .unwrap()
        .records
        .into_iter()
        .find(|(record, _)| record.record_type == RecordType::Publication)
        .unwrap()
        .0;
    let candidate: Candidate = verified_object_at(
        &fixture.engine.disk_resources,
        &fixture.engine.members(),
        "candidate",
        &record.candidate_hash,
    )
    .unwrap();
    for disk in &fixture.original.disks[1..] {
        fs::remove_file(object_path(
            disk,
            candidate.history_roots.kind().domain(),
            candidate.history_roots.hash(),
        ))
        .unwrap();
    }
    let before = fixture.original.disks[1..]
        .iter()
        .map(|disk| files(&disk.root))
        .collect::<Vec<_>>();
    assert!(fixture.begin().is_err());
    assert!(fixture.engine.membership_fence_status().is_none());
    assert_eq!(
        fixture.original.disks[1..]
            .iter()
            .map(|disk| files(&disk.root))
            .collect::<Vec<_>>(),
        before
    );
    assert!(
        !fixture
            .temp
            .path()
            .join("system/membership-control-archives")
            .exists()
    );
}

#[test]
fn missing_exit_target_does_not_skip_an_unresolved_accepted_candidate() {
    let mut fixture = Fixture::new(3, MembershipExitAction::Deregister);
    drop(fixture.engine);
    fs::rename(&fixture.missing, &fixture.original.disks[0].root).unwrap();
    let system = fixture.temp.path().join("system");
    let mut engine = Engine::open(&system).unwrap();
    engine.inject_fault_once("after_accept_2");
    assert!(
        engine
            .create_repository(
                &RepositoryName::parse("unresolved").unwrap(),
                "P01",
                1,
                "admin",
                OperationId::generate().unwrap()
            )
            .is_err()
    );
    drop(engine);
    fs::rename(&fixture.original.disks[0].root, &fixture.missing).unwrap();
    fixture.engine = Engine::open_for_recovery(&system).unwrap();
    let before = fixture.original.disks[1..]
        .iter()
        .map(|disk| files(&disk.root))
        .collect::<Vec<_>>();
    assert!(fixture.begin().is_err());
    assert!(fixture.engine.membership_fence_status().is_none());
    assert_eq!(
        fixture.original.disks[1..]
            .iter()
            .map(|disk| files(&disk.root))
            .collect::<Vec<_>>(),
        before
    );
}

#[test]
fn missing_exit_target_recovers_from_only_retained_roots_after_system_loss() {
    let mut fixture = Fixture::new(3, MembershipExitAction::Deregister);
    fixture.begin().unwrap();
    fixture.controls();
    let result = fixture.engine.activate_pending_disk_exit().unwrap();
    let before = files(&fixture.missing);
    drop(fixture.engine);
    let system = fixture.temp.path().join("system");
    fs::rename(&system, fixture.temp.path().join("unavailable-system")).unwrap();
    let retained = fixture.original.disks[1..]
        .iter()
        .map(|disk| disk.root.clone())
        .collect::<Vec<_>>();
    fixture.engine = Engine::restore_from_disks(&system, &retained).unwrap();
    assert_eq!(fixture.engine.activate_pending_disk_exit().unwrap(), result);
    fixture.assert_body();
    assert!(!fixture.original.disks[0].root.exists());
    assert_eq!(files(&fixture.missing), before);
}

#[test]
fn missing_exit_archive_remains_a_complete_predecessor_for_a_later_exit() {
    let mut fixture = Fixture::new(3, MembershipExitAction::PlannedOffline);
    let before = files(&fixture.missing);
    fixture.begin().unwrap();
    fixture.controls();
    let first = fixture.engine.activate_pending_disk_exit().unwrap();
    let first_authority = fixture.engine.config.generation_hash().unwrap().to_owned();
    let second_operation = OperationId::generate().unwrap();
    fixture
        .engine
        .begin_disk_exit(
            &fixture.original.disks[1].disk_id,
            MembershipExitAction::PlannedOffline,
            "admin",
            second_operation,
        )
        .unwrap();
    let fence = fixture.engine.membership_fence.as_ref().unwrap();
    assert_eq!(
        fence.intent.origin.predecessor(),
        Some(first_authority.as_str())
    );
    assert!(fence.intent.fence_scope.is_none());
    assert!(
        json_codec::to_value(&fence.intent)
            .unwrap()
            .get("fence_scope")
            .is_none(),
        "the all-member intent representation remains unchanged"
    );
    let archive = fixture
        .engine
        .build_managed_membership_control_archive()
        .unwrap();
    fixture
        .engine
        .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
        .unwrap();
    fixture.engine.resume_membership_joint_decision().unwrap();
    let second = fixture.engine.activate_pending_disk_exit().unwrap();
    assert_eq!(second.operation_id, second_operation);
    assert_eq!(fixture.engine.state.operations[&first.operation_id], first);
    fixture.assert_body();
    let fixture = fixture.reopen();
    assert_eq!(fixture.engine.state.operations[&first.operation_id], first);
    assert_eq!(
        fixture.engine.state.operations[&second.operation_id],
        second
    );
    fixture.assert_body();
    assert_eq!(files(&fixture.missing), before);
}

#[test]
fn missing_exit_scope_cannot_shrink_original_quorum_or_name_another_absent_voter() {
    let mut fixture = Fixture::new(3, MembershipExitAction::Deregister);
    fixture.begin().unwrap();
    let intent = fixture
        .engine
        .membership_fence
        .as_ref()
        .unwrap()
        .intent
        .clone();
    let mut changed = intent.clone();
    changed
        .effect
        .as_mut()
        .unwrap()
        .disk_id
        .clone_from(&fixture.original.disks[1].disk_id);
    assert!(changed.verify_fence_scope().is_err());
    let mut changed = intent;
    let mut target = (*changed.target).clone();
    target.disks[1].metadata_member = false;
    changed.target = ValidatedConfig::try_from(target).unwrap();
    assert!(changed.verify_fence_scope().is_err());
    assert_eq!(
        fixture
            .engine
            .membership_fence_status()
            .unwrap()
            .original_quorum,
        2
    );
}

#[test]
fn missing_exit_target_data_only_uses_complete_unchanged_metadata_membership() {
    for action in [
        MembershipExitAction::PlannedOffline,
        MembershipExitAction::Deregister,
    ] {
        let mut fixture = Fixture::with_target_role(4, action, false);
        let before = files(&fixture.missing);
        let fenced = fixture.begin().unwrap();
        assert_eq!(
            (
                fenced.original_members,
                fenced.original_quorum,
                fenced.confirmed_members
            ),
            (3, 2, 3)
        );
        assert!(fenced.all_members_fenced());
        assert!(
            fixture
                .engine
                .membership_fence
                .as_ref()
                .unwrap()
                .intent
                .fence_scope
                .is_none()
        );
        let archive = fixture
            .engine
            .build_managed_membership_control_archive()
            .unwrap();
        let inspected = verify_membership_control_archive(
            &archive.archive_path,
            &archive.root_hash,
            &archive.transition_hash,
        )
        .unwrap();
        assert_eq!(
            (inspected.original_members, inspected.journals.len()),
            (3, 3)
        );
        let pending = fixture
            .engine
            .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
            .unwrap();
        assert_eq!(
            (
                pending.target_metadata_members,
                pending.control_participants
            ),
            (3, 3)
        );
        let joint = fixture.engine.resume_membership_joint_decision().unwrap();
        assert_eq!(
            (
                joint.original_members,
                joint.target_members,
                joint.union_members
            ),
            (3, 3, 3)
        );
        assert_eq!(joint.certificate_quorum_counts(), (3, 3));
        assert!(joint.all_certificates_synchronized());
        let result = fixture.engine.activate_pending_disk_exit().unwrap();
        fixture.assert_body();
        let mut fixture = fixture.reopen();
        assert_eq!(fixture.engine.activate_pending_disk_exit().unwrap(), result);
        fixture.assert_body();
        assert_eq!(files(&fixture.missing), before);
        assert!(!fixture.original.disks[0].root.exists());
    }
}

#[test]
fn missing_exit_fence_rebinds_only_authenticated_current_root_paths() {
    let mut fixture = Fixture::new(3, MembershipExitAction::Deregister);
    fixture.begin().unwrap();
    let intent = &fixture.engine.membership_fence.as_ref().unwrap().intent;
    let saved = intent.fence_members();
    let roots = fixture
        .engine
        .config
        .disks
        .iter()
        .enumerate()
        .map(|(index, _)| fixture.temp.path().join(format!("relocated-{index}")))
        .collect();
    let projected = fixture
        .engine
        .config
        .clone()
        .with_disk_roots(roots)
        .unwrap();
    let rebound = intent.fence_members_at(&projected).unwrap();
    assert_eq!(rebound.len(), 2);
    for (original, rebound) in saved.iter().zip(&rebound) {
        assert_eq!(original.disk_id, rebound.disk_id);
        assert_eq!(original.slot_id, rebound.slot_id);
        assert_eq!(original.binding_generation, rebound.binding_generation);
        assert_ne!(original.root, rebound.root);
        assert_eq!(
            rebound.root,
            projected
                .metadata_members()
                .find(|disk| disk.disk_id == rebound.disk_id)
                .unwrap()
                .root
        );
    }
    assert!(intent.fence_members_at(&intent.target).is_err());
    assert_eq!(
        json_codec::to_value(&saved).unwrap(),
        json_codec::to_value(intent.fence_members()).unwrap()
    );
}
