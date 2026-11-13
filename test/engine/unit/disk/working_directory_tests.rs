use super::*;
use crate::{Config, DiskRegistrationIdentity, ValidatedConfig};
use crate::working_tree::{
    StructurallyValidWorkingTreeCatalog,
    admission::{RootSelectionContext, select_root, validate_catalog_structure},
    records::local_entity,
};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::Command,
};
use storage_contract::{Generation, OperationId};
use storage_contract::working_tree::{
    BindingLifecycle, DirectoryBinding, DirectoryRelativePath, LocalDirectoryBinding,
    RootBinding, RootId, StorageLayout, ThreeRootBinding, WorkingTreeCatalog,
};

struct Fixture {
    temporary: Option<tempfile::TempDir>,
    root: PathBuf,
    disk: DiskHandle,
    catalog: WorkingTreeCatalog,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let mut fixture = Self::at(temporary.path());
        fixture.temporary = Some(temporary);
        fixture
    }

    fn at(root: &Path) -> Self {
        fs::create_dir_all(root).unwrap();
        fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
        let config = ValidatedConfig::try_from(
            Config::development([root.to_path_buf()]).unwrap(),
        ).unwrap();
        let disk = DiskHandle::open(&config, &config.disks[0], false).unwrap();
        let roots = ThreeRootBinding {
            common_data: local(&disk.root, &layout::COMMON_DATA_ROOT, 2),
            deleted_data: local(&disk.root, &layout::DELETED_DATA_ROOT, 3),
            zxs_data: local(&disk.root, &layout::ZXS_DATA_ROOT, 4),
        };
        let binding = RootBinding {
            layout: StorageLayout::UnifiedWorkingTreeV1,
            root_id: RootId::from_random([1; 16]),
            registration: DiskRegistrationIdentity {
                pool_id: disk.seal.pool_id.clone(),
                disk_id: disk.seal.disk_id.clone(),
                slot_id: disk.seal.slot_id.clone(),
                binding_generation: disk.seal.binding_generation,
            },
            source_chain: Vec::new(),
            root_entity: local_entity(disk.root.held_identity().unwrap()),
            roots,
            operation_id: OperationId::from_random([5; 16]),
            generation: Generation::new(1),
        };
        Self {
            temporary: None,
            root: root.to_path_buf(),
            disk,
            catalog: WorkingTreeCatalog {
                storage_layout: StorageLayout::UnifiedWorkingTreeV1,
                roots: vec![binding],
                directories: Vec::new(),
                containers: Vec::new(),
                snapshot_repositories: Vec::new(),
                replicas: Vec::new(),
            },
        }
    }

    fn ordinary(&self, path: &str) -> PathBuf {
        let path = self.root.join(layout::COMMON_DATA_ROOT.as_path()).join(path);
        fs::create_dir_all(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn stage(&self) -> (DiskRoot, EntryName, DiskRoot) {
        let zxs = self.disk.root.private_entry_matching(
            &layout::ZXS_DATA_ROOT,
            file_identity(self.catalog.roots[0].roots.zxs_data.entity),
        ).unwrap();
        let parent = create(&zxs, &EntryName::new("operation").unwrap()).0;
        let name = EntryName::new("prepared").unwrap();
        let child = create(&parent, &name).0;
        (parent, name, child)
    }
}

fn local(root: &DiskRoot, name: &EntryName, id: u8) -> LocalDirectoryBinding {
    let (_, identity) = create(root, name);
    LocalDirectoryBinding {
        directory_id: DirectoryId::from_random([id; 16]),
        entity: local_entity(identity),
    }
}

fn create(parent: &DiskRoot, name: &EntryName) -> (DiskRoot, FileIdentity) {
    match parent.create_private_entry(name) {
        DirectoryCreateOutcome::Created { directory, identity, durability } => {
            durability.unwrap();
            (directory, identity)
        }
        DirectoryCreateOutcome::Rejected(source)
        | DirectoryCreateOutcome::Uncertain { source, .. } => panic!("{source}"),
    }
}

fn structurally_valid(fixture: &Fixture) -> StructurallyValidWorkingTreeCatalog<'_> {
    validate_catalog_structure(&fixture.catalog).unwrap()
}

fn select<'a>(
    fixture: &Fixture,
    tree: &'a StructurallyValidWorkingTreeCatalog<'a>,
) -> CurrentRootSelection<'a> {
    // The current tuple comes from the held disk, independently of the catalog.
    let seal = &fixture.disk.seal;
    let current = DiskRegistrationIdentity {
        pool_id: seal.pool_id.clone(),
        disk_id: seal.disk_id.clone(),
        slot_id: seal.slot_id.clone(),
        binding_generation: seal.binding_generation,
    };
    select_root(tree, fixture.catalog.roots[0].root_id, &RootSelectionContext {
        storage_layout: StorageLayout::UnifiedWorkingTreeV1,
        registration: &current,
        source_chain: &[],
    }).unwrap()
}

fn target(path: &str) -> DirectoryTargetPath {
    DirectoryTargetPath::Relative(DirectoryRelativePath::parse(path).unwrap())
}

#[test]
fn working_directory_installs_private_repository_without_changing_ordinary_mode() {
    let fixture = Fixture::new();
    let host = fixture.ordinary("host");
    fs::write(host.join("ordinary.txt"), b"retained body").unwrap();
    let tree = structurally_valid(&fixture);
    let admitted = select(&fixture, &tree);
    let working = WorkingDirectory::open(&fixture.disk, admitted, &target("host")).unwrap();
    let before = fs::metadata(&host).unwrap();
    assert_eq!(working.observed_chain().len(), 1);
    assert_eq!(working.registered_host_id(), None);
    assert_eq!(working.probe_repository(1).unwrap(), RepositoryLeaf::Absent);
    let (parent, name, prepared) = fixture.stage();
    let expected = prepared.held_identity().unwrap();
    match working.install_repository(&parent, &name, &prepared, 1) {
        DirectoryInstallOutcome::Installed { identity, durability } => {
            durability.unwrap();
            assert_eq!(identity, expected);
        }
        DirectoryInstallOutcome::Rejected(source)
        | DirectoryInstallOutcome::Uncertain(source) => panic!("{source}"),
    }
    let after = fs::metadata(&host).unwrap();
    assert_eq!(before.mode() & 0o7777, 0o755);
    assert_eq!(after.mode(), before.mode());
    assert_eq!(FileIdentity::from_metadata(&after), FileIdentity::from_metadata(&before));
    let repository = host.join(layout::SNAPSHOT_REPOSITORY.as_path());
    assert_eq!(fs::metadata(repository).unwrap().mode() & 0o7777, 0o700);
    assert_eq!(fs::read(host.join("ordinary.txt")).unwrap(), b"retained body");
    assert!(!parent.path().join(name.as_path()).exists());
    assert_eq!(working.probe_repository(100).unwrap(), RepositoryLeaf::Occupied);
}

#[test]
fn working_directory_refuses_file_directory_and_symlink_collisions_without_overwrite() {
    for kind in ["file", "directory", "link", "casefold"] {
        let fixture = Fixture::new();
        let host = fixture.ordinary("host");
        let collision = if kind == "casefold" { ".ZXS" } else { ".zxs" };
        let leaf = host.join(collision);
        match kind {
            "file" | "casefold" => fs::write(&leaf, b"do not overwrite").unwrap(),
            "directory" => fs::create_dir(&leaf).unwrap(),
            "link" => symlink("missing", &leaf).unwrap(),
            _ => panic!("fixture kind is explicit"),
        }
        let before = fs::symlink_metadata(&leaf).unwrap();
        let tree = structurally_valid(&fixture);
        let admitted = select(&fixture, &tree);
        let working = WorkingDirectory::open(&fixture.disk, admitted, &target("host")).unwrap();
        assert_eq!(working.probe_repository(100).unwrap(), RepositoryLeaf::Occupied);
        let (parent, name, prepared) = fixture.stage();
        assert!(matches!(
            working.install_repository(&parent, &name, &prepared, 100),
            DirectoryInstallOutcome::Rejected(DirectoryIoError::Collision(_))
        ));
        let after = fs::symlink_metadata(&leaf).unwrap();
        assert_eq!(FileIdentity::from_metadata(&before), FileIdentity::from_metadata(&after));
        assert!(parent.path().join(name.as_path()).exists());
        if matches!(kind, "file" | "casefold") {
            assert_eq!(fs::read(&leaf).unwrap(), b"do not overwrite");
        }
    }
}

#[test]
fn working_directory_parent_replacement_does_not_retarget_publication() {
    let fixture = Fixture::new();
    let host = fixture.ordinary("ancestor/host");
    let tree = structurally_valid(&fixture);
    let admitted = select(&fixture, &tree);
    let working = WorkingDirectory::open(
        &fixture.disk, admitted, &target("ancestor/host"),
    ).unwrap();
    let ancestor = host.parent().unwrap();
    let moved = ancestor.with_file_name("moved");
    fs::rename(ancestor, &moved).unwrap();
    fixture.ordinary("ancestor/host");
    let (parent, name, prepared) = fixture.stage();
    assert!(matches!(working.revalidate(), Err(DirectoryIoError::IdentityChanged(_))));
    assert!(matches!(
        working.install_repository(&parent, &name, &prepared, 100),
        DirectoryInstallOutcome::Rejected(DirectoryIoError::IdentityChanged(_))
    ));
    assert!(!host.join(".zxs").exists());
    assert!(!moved.join("host/.zxs").exists());
    assert!(parent.path().join(name.as_path()).exists());
}

#[test]
fn working_directory_rejects_ordinary_symlink_ancestors() {
    let fixture = Fixture::new();
    let target_directory = fixture.ordinary("target");
    symlink(&target_directory, target_directory.with_file_name("link")).unwrap();
    let tree = structurally_valid(&fixture);
    let admitted = select(&fixture, &tree);
    assert!(matches!(
        WorkingDirectory::open(&fixture.disk, admitted, &target("link")),
        Err(DirectoryIoError::Io(_))
    ));
}

#[test]
fn working_directory_registered_original_spelling_cannot_change_with_same_inode() {
    let mut fixture = Fixture::new();
    let host = fixture.ordinary("host");
    let root = &fixture.catalog.roots[0];
    fixture.catalog.directories.push(DirectoryBinding {
        directory_id: DirectoryId::from_random([9; 16]),
        root_id: root.root_id,
        parent_directory_id: root.roots.common_data.directory_id,
        name: name_policy::LookupName::parse("host").unwrap(),
        entity: local_entity(FileIdentity::from_metadata(&fs::metadata(&host).unwrap())),
        lifecycle: BindingLifecycle::Current,
        operation_id: root.operation_id,
    });
    fs::rename(&host, host.with_file_name("Host")).unwrap();
    let tree = structurally_valid(&fixture);
    let admitted = select(&fixture, &tree);
    assert!(matches!(
        WorkingDirectory::open(&fixture.disk, admitted, &target("Host")),
        Err(DirectoryIoError::IdentityChanged(_))
    ));
}

#[test]
fn working_directory_stage_parent_replacement_does_not_move_unknown_stage() {
    let fixture = Fixture::new();
    let host = fixture.ordinary("host");
    let tree = structurally_valid(&fixture);
    let admitted = select(&fixture, &tree);
    let working = WorkingDirectory::open(&fixture.disk, admitted, &target("host")).unwrap();
    let (parent, name, prepared) = fixture.stage();
    let moved = parent.path().with_file_name("moved");
    fs::rename(parent.path(), &moved).unwrap();
    fs::create_dir(parent.path()).unwrap();
    fs::set_permissions(parent.path(), fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir(parent.path().join(name.as_path())).unwrap();
    assert!(matches!(
        working.install_repository(&parent, &name, &prepared, 100),
        DirectoryInstallOutcome::Rejected(DirectoryIoError::WrongRoot)
    ));
    assert!(!host.join(".zxs").exists());
    assert!(moved.join(name.as_path()).is_dir());
    assert!(parent.path().join(name.as_path()).is_dir());
}

#[test]
fn working_directory_rejects_wrong_registered_disk_and_foreign_stage() {
    let first = Fixture::new();
    let second = Fixture::new();
    first.ordinary("host");
    let tree = structurally_valid(&first);
    let admitted = select(&first, &tree);
    assert!(matches!(
        WorkingDirectory::open(&second.disk, admitted, &DirectoryTargetPath::Root),
        Err(DirectoryIoError::WrongRoot)
    ));
    let working = WorkingDirectory::open(&first.disk, admitted, &target("host")).unwrap();
    let (parent, name, prepared) = second.stage();
    assert!(matches!(
        working.install_repository(&parent, &name, &prepared, 100),
        DirectoryInstallOutcome::Rejected(DirectoryIoError::WrongFilesystem)
    ));
    assert!(parent.path().join(name.as_path()).exists());
}

#[test]
fn working_directory_idle_handles_close_with_disk_revocation() {
    let fixture = Fixture::new();
    fixture.ordinary("host");
    let tree = structurally_valid(&fixture);
    let admitted = select(&fixture, &tree);
    let working = WorkingDirectory::open(&fixture.disk, admitted, &target("host")).unwrap();
    let binding = fixture.disk.root.io_binding().unwrap();
    let transition = binding.begin_quiesce().unwrap().unwrap();
    drop(transition);
    assert!(binding.status().unwrap().drained());
    assert!(matches!(working.probe_repository(100), Err(DirectoryIoError::Gate(_))));
    assert!(matches!(working.identity(), Err(DirectoryIoError::Gate(_))));
}

#[test]
fn working_directory_probe_budget_refuses_incomplete_absence() {
    let fixture = Fixture::new();
    let host = fixture.ordinary("host");
    fs::write(host.join("ordinary"), b"body").unwrap();
    let tree = structurally_valid(&fixture);
    let admitted = select(&fixture, &tree);
    let working = WorkingDirectory::open(&fixture.disk, admitted, &target("host")).unwrap();
    assert!(matches!(
        working.probe_repository(0),
        Err(DirectoryIoError::InventoryBudget)
    ));
}

#[test]
fn private_entry_creation_never_adopts_an_existing_directory() {
    let fixture = Fixture::new();
    let name = EntryName::new("existing").unwrap();
    let (directory, identity) = create(&fixture.disk.root, &name);
    assert!(matches!(
        fixture.disk.root.create_private_entry(&name),
        DirectoryCreateOutcome::Rejected(DirectoryIoError::Collision(_))
    ));
    assert_eq!(directory.held_identity().unwrap(), identity);
    fixture.disk.root.sync_private_entry_matching(&name, identity).unwrap();
}

#[test]
#[ignore = "requires directory_retry.c LD_PRELOAD in an isolated test child"]
fn private_entry_sync_failure_preserves_created_identity_and_held_directory() {
    let path = PathBuf::from(std::env::var_os("ZXS_DIRECTORY_ROOT").unwrap());
    let root = DiskRoot::open(&path).unwrap();
    let name = EntryName::new("branch").unwrap();
    let (directory, identity) = match root.create_private_entry(&name) {
        DirectoryCreateOutcome::Created { directory, identity, durability } => {
            assert!(matches!(durability, Err(DirectoryIoError::Sync { .. })));
            (directory, identity)
        }
        DirectoryCreateOutcome::Rejected(source)
        | DirectoryCreateOutcome::Uncertain { source, .. } => panic!("{source}"),
    };
    assert_eq!(directory.held_identity().unwrap(), identity);
    root.sync_private_entry_matching(&name, identity).unwrap();
    assert!(matches!(
        root.create_private_entry(&name),
        DirectoryCreateOutcome::Rejected(DirectoryIoError::Collision(_))
    ));
    let stage = std::env::var("ZXS_DIRECTORY_STAGE").unwrap();
    println!("\nZXS_WORKING_DIRECTORY_COMPLETE operation=create stage={stage}");
}

#[test]
#[ignore = "requires directory_retry.c LD_PRELOAD with parent stage and host as root"]
fn working_directory_rename_sync_failure_retains_installed_outcome() {
    let host = PathBuf::from(std::env::var_os("ZXS_DIRECTORY_ROOT").unwrap());
    let disk_path = host.parent().unwrap().parent().unwrap();
    let fixture = Fixture::at(disk_path);
    assert_eq!(fixture.ordinary("host"), host);
    let tree = structurally_valid(&fixture);
    let admitted = select(&fixture, &tree);
    let working = WorkingDirectory::open(&fixture.disk, admitted, &target("host")).unwrap();
    let (parent, name, prepared) = fixture.stage();
    // The existing interposer arms only after this exact child exists.
    fs::create_dir(host.join("branch")).unwrap();
    let identity = prepared.held_identity().unwrap();
    match working.install_repository(&parent, &name, &prepared, 100) {
        DirectoryInstallOutcome::Installed { identity: installed, durability } => {
            assert_eq!(installed, identity);
            assert!(matches!(durability, Err(DirectoryIoError::Sync { .. })));
        }
        DirectoryInstallOutcome::Rejected(source)
        | DirectoryInstallOutcome::Uncertain(source) => panic!("{source}"),
    }
    assert_eq!(
        FileIdentity::from_metadata(&fs::metadata(host.join(".zxs")).unwrap()),
        identity,
    );
    assert!(!parent.path().join(name.as_path()).exists());
    let log = fs::read_to_string(std::env::var_os("ZXS_DIRECTORY_LOG").unwrap()).unwrap();
    let failed = format!("fail {}\n", host.display());
    let synced_source = format!("sync {}\n", parent.path().display());
    assert!(log.split_once(&failed).unwrap().1.contains(&synced_source));
    println!("\nZXS_WORKING_DIRECTORY_COMPLETE operation=install stage=parent");
}

#[test]
#[ignore = "requires isolated Linux mount namespace and mount privileges"]
fn working_directory_refuses_same_device_bind_mount_ancestor() {
    if std::env::var("ZXS_WORKING_DIRECTORY_MOUNT_TEST").as_deref() != Ok("1") {
        let test = concat!(
            "disk::owned::working_directory::tests::",
            "working_directory_refuses_same_device_bind_mount_ancestor",
        );
        let status = Command::new("/usr/bin/sudo")
            .args(["-n", "--", "/usr/bin/env", "-i", "ZXS_WORKING_DIRECTORY_MOUNT_TEST=1"])
            .args(["/usr/bin/timeout", "--signal=TERM", "--kill-after=5s", "240s"])
            .args(["/usr/bin/unshare", "--mount", "--propagation", "private"])
            .arg(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", test, "--test-threads=1", "--nocapture"])
            .status()
            .unwrap();
        assert!(status.success(), "isolated bind mount test failed: {status}");
        return;
    }
    let fixture = Fixture::new();
    let host = fixture.ordinary("host");
    let source = fixture.ordinary("source");
    let status = Command::new("/usr/bin/mount")
        .arg("--bind")
        .arg(&source)
        .arg(&host)
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(fs::metadata(&source).unwrap().dev(), fs::metadata(&host).unwrap().dev());
    let tree = structurally_valid(&fixture);
    let admitted = select(&fixture, &tree);
    assert!(matches!(
        WorkingDirectory::open(&fixture.disk, admitted, &target("host")),
        Err(DirectoryIoError::Io(source))
            if source.raw_os_error() == Some(rustix::io::Errno::XDEV.raw_os_error())
    ));
}

#[test]
fn working_directory_unknown_management_spelling_never_becomes_an_ordinary_ancestor() {
    for spelling in [".zxs", ".ZXS"] {
        let fixture = Fixture::new();
        fixture.ordinary(spelling);
        let tree = structurally_valid(&fixture);
        let selected = select(&fixture, &tree);
        assert!(matches!(
            WorkingDirectory::open(&fixture.disk, selected, &target(spelling)),
            Err(DirectoryIoError::ProtectedEntry(_)),
        ));
    }
}

#[test]
fn working_directory_recorded_container_cannot_be_traversed_as_an_ordinary_directory() {
    use storage_contract::working_tree::{
        ContainerBinding, MaterialCommitment, ReplicaBinding, ReplicaId, ReplicaObject,
        ReplicaPurpose,
    };
    use storage_contract::FileId;

    let mut fixture = Fixture::new();
    let directory = fixture.ordinary("report.txt");
    let root = &fixture.catalog.roots[0];
    let file_id = FileId::from_random([10; 16]);
    let replica_id = ReplicaId::from_random([11; 16]);
    fixture.catalog.containers.push(ContainerBinding {
        file_id,
        owner_disk: fixture.disk.seal.disk_id.parse().unwrap(),
        replica_id,
        root_id: root.root_id,
        parent_directory_id: root.roots.common_data.directory_id,
        name: name_policy::LookupName::parse("report.txt").unwrap(),
        lifecycle: BindingLifecycle::Current,
        file_authority: MaterialCommitment::from_digest(blake3::hash(b"recorded material")),
        operation_id: root.operation_id,
    });
    fixture.catalog.replicas.push(ReplicaBinding {
        replica_id,
        object: ReplicaObject::Container(file_id),
        root_id: root.root_id,
        registration: root.registration.clone(),
        source_chain: Vec::new(),
        entity: local_entity(FileIdentity::from_metadata(&fs::metadata(directory).unwrap())),
        purpose: ReplicaPurpose::Current,
        operation_id: root.operation_id,
    });
    let tree = structurally_valid(&fixture);
    let selected = select(&fixture, &tree);
    assert!(matches!(
        WorkingDirectory::open(&fixture.disk, selected, &target("report.txt")),
        Err(DirectoryIoError::ProtectedEntry(_)),
    ));
}

#[test]
fn working_directory_fault_children_preserve_create_and_install_durability_outcomes() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test/platform/unit/fs_safety/directory-retry.sh");
    let status = Command::new("/usr/bin/timeout")
        .args(["--signal=TERM", "--kill-after=5s", "240s", "/usr/bin/bash"])
        .arg(script)
        .arg(std::env::current_exe().unwrap())
        .arg("engine")
        .status()
        .unwrap();
    assert!(status.success(), "directory fault children failed: {status}");
}
