//! Production authority, independent archives, namespace masks and real cold
//! activation. Temporary directories intentionally share one filesystem.
use super::*;
use crate::engine::disk_lifecycle::bootstrap::store::{WriteFault, inject_next};
use crate::{CommitRequest, MembershipExitAction, RepositoryName};
use std::{fs, os::unix::fs::PermissionsExt};

struct Fixture {
    temporary: tempfile::TempDir,
    original: Config,
    management: Option<TerminalManagement>,
    initial: OperationId,
    terminal: OperationId,
    resume: OperationId,
    branch: String,
    historical_versions: Vec<u8>,
    historical_commits: Vec<u8>,
}
impl Fixture {
    fn new(action: MembershipExitAction, data_only: usize) -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let mut original =
            Config::development((0..=data_only).map(|n| temporary.path().join(format!("old-{n}"))))
                .unwrap();
        for disk in original.disks.iter_mut().skip(1) {
            disk.metadata_member = false;
        }
        let mut engine =
            Engine::initialize(&temporary.path().join("system"), original.clone()).unwrap();
        let initial = OperationId::generate().unwrap();
        let repository = engine
            .create_repository(
                &RepositoryName::parse("preserved").unwrap(),
                "P01",
                1,
                "admin",
                initial,
            )
            .unwrap();
        let branch = repository.main_branch_id;
        let workspace = engine.create_workspace(&branch, "admin").unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, "old.txt", b"immutable old payload")
            .unwrap();
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: OperationId::generate().unwrap(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "old history".into(),
                request_digest: None,
            })
            .unwrap();
        let historical_versions = crate::canonical_json(&engine.state.versions).unwrap();
        let historical_commits = crate::canonical_json(&engine.state.commits).unwrap();
        let terminal = OperationId::generate().unwrap();
        engine
            .begin_disk_exit(&original.disks[0].disk_id, action, "admin", terminal)
            .unwrap();
        let mut exit = engine.prepare_terminal_disk_exit().unwrap();
        let management = exit.complete(&mut engine).unwrap();
        drop(exit);
        drop(engine);
        Self {
            temporary,
            original,
            management: Some(management),
            initial,
            terminal,
            resume: OperationId::generate().unwrap(),
            branch,
            historical_versions,
            historical_commits,
        }
    }
    fn system(&self) -> PathBuf {
        self.temporary.path().join("system")
    }
    fn target(&self, count: usize) -> Config {
        let old = self.original.disks.len();
        let additions = Config::development(
            (0..old + count).map(|n| self.temporary.path().join(format!("new-{n}"))),
        )
        .unwrap();
        let mut target = self.original.clone();
        for disk in &mut target.disks {
            disk.metadata_member = false;
        }
        for disk in additions.disks.into_iter().skip(old) {
            fs::create_dir(&disk.root).unwrap();
            fs::set_permissions(&disk.root, fs::Permissions::from_mode(0o700)).unwrap();
            target.disks.push(disk);
        }
        target
    }
    fn prepare(&mut self, target: &Config) -> TerminalMetadataResume {
        let directories = directories(target);
        self.management
            .take()
            .unwrap()
            .prepare_metadata_resume(target.clone(), "admin", self.resume, directories)
            .unwrap()
    }
    fn source_path(&self) -> PathBuf {
        self.system()
            .join("terminal-management")
            .join(self.terminal.to_string())
    }
}
fn directories(target: &Config) -> Vec<PendingMemberDirectory> {
    target
        .disks
        .iter()
        .filter(|disk| disk.metadata_member)
        .map(|disk| PendingMemberDirectory {
            disk_id: disk.disk_id.clone(),
            directory: File::open(&disk.root).unwrap(),
        })
        .collect()
}
fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(base: &Path, path: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for item in fs::read_dir(path).unwrap() {
            let path = item.unwrap().path();
            if path.is_dir() {
                visit(base, &path, result);
            } else {
                result.insert(
                    path.strip_prefix(base).unwrap().to_owned(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}
fn select_generation(pending: &TerminalMetadataResume) {
    let core = pending.core.as_ref().unwrap().clone();
    let portable = activation::PortableAuthority::new_terminal_resume(core).unwrap();
    let system = pending.management.system_root();
    let path = system
        .path()
        .join("terminal-management")
        .join(
            pending
                .management
                .verified_authority()
                .authority
                .operation_id()
                .to_string(),
        )
        .join("resume-generation.json");
    system.write_json(&path, &portable, false).unwrap();
}

#[test]
fn terminal_resume_preserves_all_history_and_distinct_disable_detach_namespaces() {
    for action in [
        MembershipExitAction::PlannedOffline,
        MembershipExitAction::Deregister,
    ] {
        let mut fixture = Fixture::new(action, 0);
        let old_result = fixture
            .management
            .as_ref()
            .unwrap()
            .operation(fixture.initial)
            .unwrap()
            .clone();
        let terminal_result = fixture.management.as_ref().unwrap().result().clone();
        let original_files = files(&fixture.original.disks[0].root);
        let moved = fixture.temporary.path().join("stopped-old-directory");
        fs::rename(&fixture.original.disks[0].root, &moved).unwrap();
        let target = fixture.target(3);
        let mut pending = fixture.prepare(&target);
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
            (3, 2)
        );
        assert!(status.target_quorum_accepted());
        assert!(status.all_control_metadata_synchronized());
        assert!(Engine::open(&fixture.system()).is_err());
        assert!(!fixture.source_path().join("resume-complete.json").exists());
        let engine = pending.activate().unwrap();
        assert!(!engine.blocked);
        assert_eq!(engine.config.metadata_members().len(), 3);
        assert_eq!(engine.config.metadata_quorum(), 2);
        assert_eq!(
            crate::canonical_json(&engine.state.versions).unwrap(),
            fixture.historical_versions
        );
        assert_eq!(
            crate::canonical_json(&engine.state.commits).unwrap(),
            fixture.historical_commits
        );
        assert_eq!(engine.state.operations[&fixture.initial], old_result);
        assert_eq!(engine.state.operations[&fixture.terminal], terminal_result);
        let result = engine.state.operations[&fixture.resume].clone();
        assert_eq!(result.kind, "terminal-metadata-resume");
        assert_eq!(
            result.commit_state,
            storage_contract::CommitState::Published
        );
        let branch = &engine.state.branches[&fixture.branch];
        let visible = engine
            .current_namespaces
            .as_ref()
            .unwrap()
            .branch_entries(&fixture.branch, &branch.head_commit_id)
            .unwrap();
        assert_eq!(
            visible.is_empty(),
            action == MembershipExitAction::Deregister
        );
        assert_eq!(
            files(&moved),
            original_files,
            "resume never reads/writes the stopped root or makes a backup there"
        );
        assert!(fixture.source_path().join("resume-complete.json").is_file());
        assert!(
            !TerminalManagement::requires_management(&fixture.system()).unwrap(),
            "status observation must not reacquire the active Engine writer"
        );
        drop(engine);
        assert!(!TerminalManagement::requires_management(&fixture.system()).unwrap());
        let mut reopened = Engine::open(&fixture.system()).unwrap();
        assert_eq!(reopened.state.operations[&fixture.resume], result);
        assert_eq!(reopened.config.metadata_members().len(), 3);
        assert_eq!(files(&moved), original_files);
        reopened
            .create_repository(
                &RepositoryName::parse("resumed").unwrap(),
                &target.disks[1].slot_id,
                1,
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
    }
}

#[test]
fn terminal_resume_reenters_before_and_after_the_durable_selector() {
    let mut fixture = Fixture::new(MembershipExitAction::Deregister, 1);
    let target = fixture.target(2);
    let mut pending = fixture.prepare(&target);
    pending.synchronize().unwrap();
    drop(pending);
    let recovery = TerminalManagement::metadata_resume_recovery(&fixture.system())
        .unwrap()
        .unwrap();
    assert_eq!(
        recovery.phase,
        crate::TerminalMetadataResumePhase::ResupplyDirectories
    );
    assert_eq!(recovery.operation_id, fixture.resume);
    assert_eq!(recovery.principal, "admin");
    let mut pending =
        TerminalManagement::reopen_metadata_resume(&fixture.system(), directories(&target))
            .unwrap();
    pending.synchronize().unwrap();
    select_generation(&pending);
    drop(pending);
    let recovery = TerminalManagement::metadata_resume_recovery(&fixture.system())
        .unwrap()
        .unwrap();
    assert_eq!(
        recovery.phase,
        crate::TerminalMetadataResumePhase::CompleteGeneration
    );
    assert_eq!(
        recovery.control_archive.root_hash,
        TerminalManagement::open_for_resume(&fixture.system())
            .unwrap()
            .control_archive()
            .root_hash
    );
    let engine = TerminalManagement::complete_metadata_resume(&fixture.system()).unwrap();
    assert!(engine.state.operations.contains_key(&fixture.initial));
    assert!(engine.state.operations.contains_key(&fixture.terminal));
    assert!(engine.state.operations.contains_key(&fixture.resume));
    assert!(
        engine
            .disk_resources
            .available(&engine.config.disks[1])
            .is_some(),
        "still-registered data-only directory remains in the resumed resource set"
    );
}

#[test]
fn terminal_resume_exact_bootstrap_barriers_reenter_without_opening_stopped_roots() {
    for phase in [
        WriteFault::BeforeRecordBarrier,
        WriteFault::AfterRecordBarrier,
        WriteFault::BeforeHeadBarrier,
        WriteFault::AfterHeadBarrier,
    ] {
        let mut fixture = Fixture::new(MembershipExitAction::Deregister, 0);
        let target = fixture.target(1);
        let pending = fixture.prepare(&target);
        let moved = fixture.temporary.path().join("stopped");
        fs::rename(&fixture.original.disks[0].root, &moved).unwrap();
        let before = files(&moved);
        inject_next(phase);
        assert!(pending.activate().is_err(), "{phase:?}");
        assert!(
            fixture
                .source_path()
                .join("resume-generation.json")
                .is_file()
        );
        assert!(
            fixture
                .source_path()
                .join("resume-bootstrap.json")
                .is_file()
        );
        assert!(!fixture.source_path().join("resume-complete.json").exists());
        assert!(Engine::open(&fixture.system()).is_err());
        let recovery = TerminalManagement::metadata_resume_recovery(&fixture.system())
            .unwrap()
            .unwrap();
        assert_eq!(
            recovery.phase,
            crate::TerminalMetadataResumePhase::CompleteGeneration
        );
        let engine = TerminalManagement::complete_metadata_resume(&fixture.system()).unwrap();
        assert_eq!(
            engine.state.operations[&fixture.resume].kind,
            "terminal-metadata-resume"
        );
        assert_eq!(files(&moved), before);
        drop(engine);
        assert!(Engine::open(&fixture.system()).is_ok());
    }
}

#[test]
fn terminal_resume_requires_each_own_archive_before_first_activation() {
    let mut fixture = Fixture::new(MembershipExitAction::Deregister, 0);
    let target = fixture.target(2);
    let mut pending = fixture.prepare(&target);
    pending.synchronize().unwrap();
    let transition = object_hash(INTENT_KIND, &pending.proposal.intent).unwrap();
    let missing = member_directory(&target.disks[2], &transition)
        .join(CONTROL_ARCHIVE_DIRECTORY)
        .join("archive-root.json");
    select_generation(&pending);
    drop(pending);
    fs::remove_file(missing).unwrap();
    assert!(TerminalManagement::complete_metadata_resume(&fixture.system()).is_err());
    assert!(!fixture.source_path().join("resume-complete.json").exists());
    assert!(Engine::open(&fixture.system()).is_err());
}

#[test]
fn terminal_resume_cannot_change_a_reserved_actor_target_or_operation() {
    let mut fixture = Fixture::new(MembershipExitAction::Deregister, 0);
    let target = fixture.target(1);
    drop(fixture.prepare(&target));
    for (principal, operation) in [
        ("other", fixture.resume),
        ("admin", OperationId::generate().unwrap()),
    ] {
        let management = TerminalManagement::open_for_resume(&fixture.system()).unwrap();
        assert!(
            management
                .prepare_metadata_resume(target.clone(), principal, operation, directories(&target))
                .is_err()
        );
    }
    let pending =
        TerminalManagement::reopen_metadata_resume(&fixture.system(), directories(&target))
            .unwrap();
    assert_eq!(pending.status().unwrap().operation_id, fixture.resume);
}

#[test]
fn completed_terminal_resume_never_reduces_new_fixed_quorum() {
    let mut fixture = Fixture::new(MembershipExitAction::Deregister, 0);
    let target = fixture.target(3);
    drop(fixture.prepare(&target).activate().unwrap());
    for (index, disk) in target
        .disks
        .iter()
        .filter(|disk| disk.metadata_member)
        .take(2)
        .enumerate()
    {
        fs::rename(
            &disk.root,
            fixture
                .temporary
                .path()
                .join(format!("missing-new-{index}")),
        )
        .unwrap();
    }
    assert!(
        Engine::open(&fixture.system()).is_err(),
        "one surviving new member cannot become N1/Q1"
    );
}
