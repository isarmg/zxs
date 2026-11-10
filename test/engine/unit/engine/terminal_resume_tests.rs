//! Production authority, independent archives, namespace masks and real cold
//! activation. Temporary directories intentionally share one filesystem.
use super::*;
use crate::engine::disk_lifecycle::bootstrap::store::{WriteFault, inject_next};
use crate::{CommitRequest, MembershipExitAction, RepositoryName};
use std::{fs, os::unix::fs::PermissionsExt};

#[path = "terminal_head_rotation_tests.rs"]
mod head_rotation;

#[path = "terminal_resume_mixed_tests.rs"]
mod mixed;
#[path = "terminal_readmission_tests.rs"]
mod retained_readmission;

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
        for mut disk in additions.disks.into_iter().skip(old) {
            // The resumed writable route is a fresh primary, independent of the
            // stopped original primary retained in historical topology.
            disk.level = 1;
            disk.upstream_slot_id = None;
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
        let mut target = fixture.target(3);
        // This case creates a repository on a fresh primary after cold reopen.
        // Config::development's later disks are backup levels, not primaries.
        for disk in target.disks.iter_mut().skip(fixture.original.disks.len()) {
            disk.level = 1;
            disk.upstream_slot_id = None;
        }
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
fn resume_source_without_chain_evidence_keeps_ordinary_management_fenced() {
    let mut fixture = Fixture::new(MembershipExitAction::Deregister, 0);
    let target = fixture.target(1);
    let pending = fixture.prepare(&target);
    drop(pending);
    let recovery = TerminalManagement::metadata_resume_recovery(&fixture.system())
        .unwrap()
        .unwrap();
    let before = files(&fixture.system());
    assert!(TerminalManagement::open(&fixture.system()).is_err());
    let owner =
        TerminalManagement::open_metadata_resume_checked(&fixture.system(), &recovery).unwrap();
    assert!(!owner.status().writable);
    assert_eq!(owner.status().operation_id, fixture.terminal);
    drop(owner);
    assert!(TerminalManagement::open(&fixture.system()).is_err());
    assert_eq!(files(&fixture.system()), before);
    let mut pending =
        TerminalManagement::reopen_metadata_resume(&fixture.system(), directories(&target))
            .unwrap();
    pending.synchronize().unwrap();
}

#[test]
fn resume_source_rejects_chain_evidence_without_consuming_the_request() {
    for name in [
        "chain-readmission.json",
        "chain-readmission-target.json",
        "chain-first-selected.json",
        "chain-publication-selected.json",
        "chain-activation-plan.json",
        "chain-io-decision-request.json",
        "chain-generation-log-selected.json",
        "chain-generation-selector-request.json",
    ] {
        let mut fixture = Fixture::new(MembershipExitAction::Deregister, 0);
        let target = fixture.target(1);
        drop(fixture.prepare(&target));
        let recovery = TerminalManagement::metadata_resume_recovery(&fixture.system())
            .unwrap()
            .unwrap();
        fs::write(fixture.source_path().join(name), b"{}").unwrap();
        let before = files(&fixture.system());
        assert!(
            TerminalManagement::open_metadata_resume_checked(&fixture.system(), &recovery).is_err(),
            "resume must reject {name}"
        );
        assert_eq!(files(&fixture.system()), before);
    }
}

#[test]
fn terminal_resume_exact_bootstrap_barriers_reenter_without_opening_stopped_roots() {
    for phase in [
        WriteFault::BeforeRecord,
        WriteFault::AfterRecord,
        WriteFault::BeforeHead,
        WriteFault::AfterHead,
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

#[path = "terminal_resume_cancel_tests.rs"]
mod cancellation_tests;

fn watch_resume_recovery_paths(paths: impl IntoIterator<Item = PathBuf>) -> rustix::fd::OwnedFd {
    use rustix::fs::inotify;
    let watch =
        inotify::init(inotify::CreateFlags::CLOEXEC | inotify::CreateFlags::NONBLOCK).unwrap();
    for path in paths {
        inotify::add_watch(
            &watch,
            &path,
            inotify::WatchFlags::OPEN | inotify::WatchFlags::ACCESS,
        )
        .unwrap();
    }
    watch
}

fn assert_no_resume_recovery_path_open(watch: &rustix::fd::OwnedFd) {
    let mut bytes = [0u8; 4096];
    assert_eq!(
        rustix::io::read(watch, &mut bytes),
        Err(rustix::io::Errno::WOULDBLOCK)
    );
}

// Copy every caller-visible field but retain the private exact proposal and
// selected-generation hashes from the older observation. The checked boundary
// must not mistake a matching public projection for that complete authority.
fn copy_resume_recovery_projection(
    to: &mut crate::TerminalMetadataResumeRecovery,
    from: &crate::TerminalMetadataResumeRecovery,
) {
    to.original = from.original.clone();
    to.terminal_operation_id = from.terminal_operation_id;
    to.admission = from.admission.clone();
    to.readmission_preview = from.readmission_preview.clone();
    to.operation_id = from.operation_id;
    to.principal = from.principal.clone();
    to.target = from.target.clone();
    to.terminal_authority_hash = from.terminal_authority_hash.clone();
    to.transition_hash = from.transition_hash.clone();
    to.control_archive = from.control_archive.clone();
    to.phase = from.phase;
}

#[test]
fn checked_resume_rejects_a_new_attempt_before_opening_locator_or_storage() {
    let mut fixture = Fixture::new(MembershipExitAction::Deregister, 0);
    let first_target = fixture.target(1);
    let mut pending = fixture.prepare(&first_target);
    let mut previous = TerminalManagement::metadata_resume_recovery(&fixture.system())
        .unwrap()
        .unwrap();
    let status = pending.status().unwrap();
    let cancelled = match pending
        .cancel("admin", status.operation_id, &status.transition_hash)
        .unwrap()
    {
        crate::TerminalMetadataResumeCancelOutcome::Cancelled(result) => result,
        crate::TerminalMetadataResumeCancelOutcome::ContinueOriginal(_) => {
            panic!("an unpublished fresh attempt must remain cancellable")
        }
    };
    let management = pending.into_cancelled_management().unwrap();
    let mut target = first_target.clone();
    for disk in target.disks.iter_mut().filter(|disk| disk.metadata_member) {
        disk.root = fixture
            .temporary
            .path()
            .join(format!("next-{}", disk.disk_id));
        fs::create_dir(&disk.root).unwrap();
        fs::set_permissions(&disk.root, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let operation = OperationId::generate().unwrap();
    drop(
        management
            .prepare_metadata_resume(target.clone(), "admin", operation, directories(&target))
            .unwrap(),
    );
    let current = TerminalManagement::metadata_resume_recovery(&fixture.system())
        .unwrap()
        .unwrap();
    assert_ne!(previous.operation_id, current.operation_id);
    copy_resume_recovery_projection(&mut previous, &current);
    let before = files(&fixture.system());
    let watch = watch_resume_recovery_paths(
        first_target
            .disks
            .iter()
            .chain(&target.disks)
            .map(|disk| disk.root.clone())
            .chain([fixture.system().join("bootstrap-discovery")]),
    );
    let error = TerminalManagement::open_metadata_resume_checked(&fixture.system(), &previous)
        .err()
        .expect("a different exact attempt cannot consume the caller's old proof");
    assert_eq!(error.code(), ErrorCode::IdempotencyKeyReused);
    assert_no_resume_recovery_path_open(&watch);
    drop(watch);
    assert_eq!(files(&fixture.system()), before);
    let owner =
        TerminalManagement::open_metadata_resume_checked(&fixture.system(), &current).unwrap();
    let engine = owner
        .reopen_metadata_resume_held(directories(&target))
        .unwrap()
        .activate()
        .unwrap();
    assert_eq!(engine.state.operations[&fixture.resume], cancelled);
    assert_eq!(engine.state.operations[&operation].operation_id, operation);
}

#[test]
fn checked_resume_rejects_new_selected_generation_before_locator_or_root_io() {
    let mut fixture = Fixture::new(MembershipExitAction::Deregister, 0);
    let target = fixture.target(1);
    let mut pending = fixture.prepare(&target);
    let mut previous = TerminalManagement::metadata_resume_recovery(&fixture.system())
        .unwrap()
        .unwrap();
    pending.synchronize().unwrap();
    select_generation(&pending);
    drop(pending);
    let current = TerminalManagement::metadata_resume_recovery(&fixture.system())
        .unwrap()
        .unwrap();
    assert_ne!(previous.phase, current.phase);
    copy_resume_recovery_projection(&mut previous, &current);
    let before = files(&fixture.system());
    let watch = watch_resume_recovery_paths(
        target
            .disks
            .iter()
            .map(|disk| disk.root.clone())
            .chain([fixture.system().join("bootstrap-discovery")]),
    );
    let error = TerminalManagement::open_metadata_resume_checked(&fixture.system(), &previous)
        .err()
        .expect("a selected generation must be bound by the caller's exact proof");
    assert_eq!(error.code(), ErrorCode::IdempotencyKeyReused);
    assert_no_resume_recovery_path_open(&watch);
    drop(watch);
    assert_eq!(files(&fixture.system()), before);
    let owner =
        TerminalManagement::open_metadata_resume_checked(&fixture.system(), &current).unwrap();
    let engine = owner.complete_metadata_resume_held(None).unwrap();
    assert_eq!(
        engine.state.operations[&fixture.resume].operation_id,
        fixture.resume
    );
}
