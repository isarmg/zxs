//! Actual Initial creation, D3 and retained Current workspace admission.
use super::*;
use std::{fs, os::unix::fs::MetadataExt};

#[derive(Debug, PartialEq, Eq)]
struct Image {
    device: u64,
    inode: u64,
    mode: u32,
    bytes: Option<Vec<u8>>,
}

fn images(root: &Path) -> BTreeMap<PathBuf, Image> {
    fn visit(root: &Path, path: &Path, result: &mut BTreeMap<PathBuf, Image>) {
        let metadata = fs::symlink_metadata(path).unwrap();
        assert!(metadata.is_dir() || metadata.is_file());
        result.insert(
            path.strip_prefix(root).unwrap().to_owned(),
            Image {
                device: metadata.dev(),
                inode: metadata.ino(),
                mode: metadata.mode(),
                bytes: metadata.is_file().then(|| fs::read(path).unwrap()),
            },
        );
        if metadata.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                visit(root, &entry.unwrap().path(), result);
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}

fn remove_idle_control_lock(system: &Path) -> BTreeMap<PathBuf, Image> {
    let directory = system.join(disk_lifecycle::bootstrap::runtime::DIRECTORY);
    let lock = layout::CONTROL_WRITER_LOCK.as_path();
    let mut evidence = images(&directory);
    let previous = evidence.remove(lock).unwrap();
    assert_eq!(previous.bytes.as_deref(), Some(b"".as_slice()));
    assert_eq!(previous.mode & 0o7777, 0o600);
    // The Engine and every lock owner were dropped. The initialized locator is
    // left intact: current-format admission permits recreating this lock only.
    fs::remove_file(directory.join(lock)).unwrap();
    evidence
}

fn require_control_lock_recreated(system: &Path, evidence: BTreeMap<PathBuf, Image>) {
    let directory = system.join(disk_lifecycle::bootstrap::runtime::DIRECTORY);
    let mut actual = images(&directory);
    let lock = actual.remove(layout::CONTROL_WRITER_LOCK.as_path()).unwrap();
    assert_eq!(lock.bytes.as_deref(), Some(b"".as_slice()));
    assert_eq!(lock.mode & 0o7777, 0o600);
    assert_eq!(actual, evidence);
}

fn already_locked(error: &(dyn std::error::Error + 'static)) -> bool {
    if matches!(
        error.downcast_ref::<platform_io::fs_safety::Error>(),
        Some(platform_io::fs_safety::Error::AlreadyLocked(_))
    ) {
        return true;
    }
    if let Some(error) = error.downcast_ref::<std::io::Error>()
        && let Some(source) = error.get_ref()
        && already_locked(source)
    {
        return true;
    }
    error.source().is_some_and(already_locked)
}

#[test]
fn current_exit_holds_discovery_until_read_only_workspace_admission() {
    let temp = tempfile::tempdir().unwrap();
    let system = temp.path().join("system");
    let mut config = Config::development(crate::test_disks(
        (0..4).map(|index| temp.path().join(format!("disk-{index}"))),
    ))
    .unwrap();
    config.disks[3].level = 1;
    config.disks[3].upstream_slot_id = None;
    config.disks[3].metadata_member = false;
    config.max_retained_workspaces = 1;
    config.max_retained_workspaces_per_principal = 1;
    let target = config.disks[0].clone();
    let engine = Engine::initialize(&system, config).unwrap();
    let initialized = engine.config.clone();
    drop(engine);
    let locator = remove_idle_control_lock(&system);
    let probe = Engine::begin_initial_activation_probe(&system, &initialized).unwrap();
    let directories = initialized
        .io_disks()
        .map(|disk| File::open(&disk.root).unwrap())
        .collect();
    let mut engine =
        Engine::open_initial_activation_for_recovery_held(probe, None, directories).unwrap();
    require_control_lock_recreated(&system, locator);
    assert!(!engine.is_read_only());
    let repository = engine
        .create_disk_root("D04", "admin", OperationId::generate().unwrap())
        .unwrap();
    let limits = RetainedWorkspaceLimits {
        max_retained_workspaces: 2,
        max_retained_workspaces_per_principal: 2,
        ..RetainedWorkspaceLimits::from_config(&engine.config)
    };

    let operation = OperationId::generate().unwrap();
    engine
        .begin_disk_exit(&target.disk_id, MembershipExitAction::Deregister, "admin", operation)
        .unwrap();
    let archive = engine.build_managed_membership_control_archive().unwrap();
    engine
        .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
        .unwrap();
    assert!(engine.resume_membership_joint_decision().unwrap().joint_quorum_accepted());
    let published = engine.activate_pending_disk_exit().unwrap();
    assert_eq!(published.operation_id, operation);
    assert_eq!(published.commit_state, CommitState::Published);
    assert!(engine.current_namespaces.is_some());
    assert!(engine.disk_resources.available(&target).is_none());
    drop(engine);
    let parked = temp.path().join("stopped-primary");
    fs::rename(&target.root, &parked).unwrap();
    fs::write(&target.root, b"stopped address must not be admitted").unwrap();
    let stopped = images(&parked);
    let locator = remove_idle_control_lock(&system);

    // The real Current namespace admission publishes both permanent workspace
    // receipts. Only this process raises the original one-record quota.
    let mut engine = Engine::open_with_workspace_limits(&system, Some(limits)).unwrap();
    require_control_lock_recreated(&system, locator);
    assert_eq!(engine.operation(operation).unwrap(), published);
    let first = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
    let second = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
    assert_ne!(first.workspace_id, second.workspace_id);
    let state = canonical_json(engine.state()).unwrap();
    drop(engine);
    let before = images(temp.path());

    // This is the same preparation/preflight producer used by ordinary open,
    // not a synthetic Stop or a replacement receipt validator. The original
    // bootstrap lock is still held before journal writers or Exit stop sync.
    let (root, config) = Engine::recovery_projection(&system, None).unwrap();
    let (mut blocked, warnings, bootstrap) =
        Engine::prepare_workspace_recovery(root, config).unwrap();
    assert!(warnings.is_empty());
    assert!(blocked.pending_disk_exit.is_none());
    assert!(!blocked.disk_exit_io_exclusion_admitted().unwrap());
    let locked = disk_lifecycle::bootstrap::runtime::open_store(&system).err().unwrap();
    assert!(already_locked(&locked));
    let source = Arc::clone(&blocked.state);
    let error = blocked.preflight_startup_recovery(warnings, bootstrap).err().unwrap();
    assert_eq!(error.code(), ErrorCode::ResourceLimit);
    assert!(Arc::ptr_eq(&blocked.state, &source));
    assert!(blocked.pending_disk_exit.is_none());
    assert!(!blocked.disk_exit_io_exclusion_admitted().unwrap());
    drop(source);
    drop(blocked);
    assert_eq!(images(temp.path()), before);

    // Failure released the very same lock. The public recovery entry can now
    // admit the unchanged evidence under an explicit, sufficient process quota.
    // No workspace/stop receipt is edited, dropped or synthesized for the retry.
    let engine = Engine::open_with_workspace_limits(&system, Some(limits)).unwrap();
    engine.require_recovery_locks().unwrap();
    assert!(engine.disk_exit_io_exclusion_admitted().unwrap());
    assert!(!engine.is_read_only());
    assert_eq!(canonical_json(engine.state()).unwrap(), state);
    assert_eq!(engine.operation(operation).unwrap(), published);
    for workspace in [&first, &second] {
        assert_eq!(
            engine.workspace(&workspace.workspace_id).unwrap().state,
            WorkspaceState::Open,
        );
    }
    let directories = engine
        .config
        .io_disks()
        .map(|disk| File::open(&disk.root).unwrap())
        .collect();
    drop(engine);
    let engine = Engine::open_for_recovery_held(&system, Some(limits), directories).unwrap();
    engine.require_recovery_locks().unwrap();
    assert!(engine.disk_exit_io_exclusion_admitted().unwrap());
    assert_eq!(canonical_json(engine.state()).unwrap(), state);
    assert_eq!(engine.operation(operation).unwrap(), published);
    assert_eq!(images(&parked), stopped);
    assert_eq!(fs::read(&target.root).unwrap(), b"stopped address must not be admitted");
}
