//! The actual DefaultShare dispatcher creates an Added container and drives the
//! existing native range writer. No Unix transport or external Samba is implied.
use super::*;
use name_policy::{LookupPath, Name};
use std::os::unix::fs::MetadataExt;
use storage_contract::file_draft::{CommitMode, UnbornBody};
use storage_contract::snapshot_worktree::additions::WorktreeFileTarget;
use storage_contract::snapshot_worktree::creation::{AllocateWorktree, WorktreeNaming};
use storage_contract::snapshot_worktree::{InitializeSnapshot, WorktreeHandle};
use storage_contract::{OperationId, RepositoryId};
use storage_engine::{CommitRequest, WorktreeEntryContent};

fn observe<T>(direct: &Direct, slot: &str, action: impl FnOnce(&Engine) -> T) -> T {
    direct
        .backend
        .with_disk_registration(&direct._auth.sole_identity().unwrap(), slot, |_, engine| {
            Ok(action(engine))
        })
        .unwrap()
}

fn listed(direct: &mut Direct, mount: &str, size: u64) -> WorktreeFileTarget {
    let (directory, placeholder) = direct.open(mount, false, false);
    let Reply::Directory(page) = direct.call(Request::ReadDirectory {
        handle: directory.handle,
        restart: true,
    }) else {
        panic!("worktree directory reply");
    };
    assert!(!page.more);
    let added = page
        .entries
        .iter()
        .find(|entry| entry.path().file_name() == "added.txt")
        .unwrap();
    let DirectoryEntry::WorktreeWorking {
        target,
        logical_size,
        ..
    } = added
    else {
        panic!("Added must carry its own stable occurrence, not a historical Version");
    };
    assert_eq!(*logical_size, size);
    assert_eq!(added.entry_id(), target.entry.to_string());
    assert!(added.published().is_none());
    let wire = json_codec::to_value(added).unwrap();
    assert!(wire.get("version_id").is_none());
    let decoded: DirectoryEntry =
        json_codec::from_slice(&json_codec::to_vec(added).unwrap()).unwrap();
    assert_eq!(decoded.entry_id(), target.entry.to_string());
    assert_eq!(decoded.path(), added.path());
    let target = target.clone();
    direct.call(Request::Close {
        handle: directory.handle,
    });
    drop(placeholder);
    target
}

fn added_metadata(engine: &Engine, target: WorktreeHandle) -> (WorktreeFileTarget, u64) {
    let entry = engine
        .directory_worktree_entry(target, &LookupPath::parse("added.txt").unwrap())
        .unwrap();
    let WorktreeEntryContent::Working { target, working } = entry.content else {
        panic!("Added does not have a historical body");
    };
    assert_eq!(working.mode(), CommitMode::Manual);
    (target, working.logical_size())
}

fn assert_deleted_container(
    root: &Path,
    record: &storage_contract::file_container::FileContainerRecord,
) {
    let owner = &record.replicas[&record.owner_disk];
    fn find(
        root: &Path,
        entity: storage_contract::file_container::ContainerEntity,
    ) -> Option<std::path::PathBuf> {
        let metadata = fs::metadata(root).unwrap();
        if metadata.dev() == entity.device() && metadata.ino() == entity.inode() {
            return Some(root.to_path_buf());
        }
        if metadata.is_dir() {
            for entry in fs::read_dir(root).unwrap() {
                if let Some(found) = find(&entry.unwrap().path(), entity) {
                    return Some(found);
                }
            }
        }
        None
    }
    let container = find(root, owner.entity).expect("the real container inode exists on this disk");
    assert!(
        container
            .components()
            .any(|part| part.as_os_str() == "deleted-data")
    );
}

#[test]
fn native_worktree_added_birth_resaves_close_and_cold_reopen_keep_independent_identity() {
    let root = tempfile::tempdir().unwrap();
    let system = root.path().join("state");
    let disk = root.path().join("mounted-primary");
    let config = Config::development([(Name::parse("p").unwrap(), disk.clone())]).unwrap();
    let slot = config.disks[0].slot_id.clone();
    let mut engine = Engine::initialize(&system, config).unwrap();
    let owner = engine
        .create_disk_root(&slot, "alice", OperationId::generate().unwrap())
        .unwrap();
    let repository: RepositoryId = owner.repository_id.parse().unwrap();
    let workspace = engine
        .create_workspace(&owner.main_branch_id, "alice")
        .unwrap();
    engine
        .stage_bytes(
            &workspace.workspace_id,
            "original.txt",
            b"unchanged original",
        )
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "alice".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "source".into(),
            request_digest: None,
        })
        .unwrap();
    let context = engine.snapshot_root_context(repository).unwrap();
    let principal = "alice".parse().unwrap();
    let source = engine
        .initialize_directory_snapshot(
            &InitializeSnapshot {
                host: context.host,
                source: context.source,
                operation_id: OperationId::generate().unwrap(),
            },
            &principal,
        )
        .unwrap();
    let create = || AllocateWorktree {
        source,
        parent_directory_id: context.host.directory_id,
        expected_root_generation: context.host.expected_root_generation,
        operation_id: OperationId::generate().unwrap(),
        naming: WorktreeNaming::Automatic,
    };
    let first = engine
        .allocate_directory_worktree(&create(), &principal)
        .unwrap();
    let second = engine
        .allocate_directory_worktree(&create(), &principal)
        .unwrap();
    let versions = engine.state().versions.clone();
    let branches = engine.state().branches.clone();
    let auth = AuthStore::initialize(&system, "alice", "long-test-password").unwrap();
    let spool = root.path().join("spool");
    fs::create_dir(&spool).unwrap();
    fs::set_permissions(&spool, fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir(spool.join("empty-directory")).unwrap();
    let mut direct = Direct::start(root.path(), engine, auth);

    let first_mount = "p/worktree-main-001";
    let second_mount = "p/worktree-main-002";
    let (opened, placeholder) = direct.open("p/worktree-main-001/added.txt", true, true);
    assert_eq!(opened.data_io, OpenIo::NativeWorking);
    assert_eq!(opened.size, 0);
    assert!(opened.version_id.is_none() && opened.workspace_id.is_none());
    assert!(placeholder.metadata().unwrap().is_dir());
    assert!(
        placeholder
            .write_all_at(b"not an application BODY", 0)
            .is_err()
    );
    let first_file = listed(&mut direct, first_mount, 0);
    assert_eq!(opened.entry_id, first_file.entry.to_string());
    observe(&direct, &slot, |engine| {
        let record = &engine.state().file_containers[&first_file.file];
        assert!(matches!(
            record.working.unborn().unwrap().body(),
            UnbornBody::Empty {}
        ));
        assert_deleted_container(&disk, record);
        assert_eq!(engine.native_current_entries(repository).unwrap().len(), 1);
        assert_eq!(engine.state().versions, versions);
    });
    let (other, other_placeholder) = direct.open("p/worktree-main-002/added.txt", false, true);
    let second_file = listed(&mut direct, second_mount, 0);
    assert_ne!(first_file.file, second_file.file);
    assert_ne!(first_file.entry, second_file.entry);
    direct.call(Request::Close {
        handle: other.handle,
    });
    drop(other_placeholder);

    direct.write(
        opened.handle,
        WorkingPosition::At { offset: 0 },
        b"first save",
    );
    assert_eq!(direct.read(opened.handle), b"first save");
    assert_eq!(listed(&mut direct, first_mount, 0), first_file);
    let first_save = direct.flush(opened.handle);
    assert_eq!(direct.flush(opened.handle), first_save);
    assert_eq!(listed(&mut direct, first_mount, 10), first_file);
    direct.write(opened.handle, WorkingPosition::At { offset: 0 }, b"later");
    direct.call(Request::WorkingResize {
        handle: opened.handle,
        length: 5,
    });
    let second_save = direct.flush(opened.handle);
    assert_ne!(first_save, second_save);
    assert_eq!(direct.flush(opened.handle), second_save);
    direct.write(opened.handle, WorkingPosition::Append {}, b" close");
    direct.call(Request::Close {
        handle: opened.handle,
    });
    drop(placeholder);
    assert_eq!(listed(&mut direct, first_mount, 11), first_file);
    assert_eq!(listed(&mut direct, second_mount, 0), second_file);
    let Reply::Stat(stat) = direct.call(Request::Stat {
        path: "p/worktree-main-001/added.txt".into(),
    }) else {
        panic!("actual Added stat reply");
    };
    assert_eq!(stat.entry_id, first_file.entry.to_string());
    assert_eq!(stat.size, 11);
    drop(direct);

    let engine = Engine::open(&system).unwrap();
    assert_eq!(engine.state().versions, versions);
    assert_eq!(engine.state().branches, branches);
    assert_eq!(added_metadata(&engine, first), (first_file.clone(), 11));
    assert_eq!(added_metadata(&engine, second), (second_file.clone(), 0));
    assert_deleted_container(&disk, &engine.state().file_containers[&first_file.file]);
    let mut original = engine
        .open_directory_worktree_content(first, &LookupPath::parse("original.txt").unwrap())
        .unwrap();
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut original.file, &mut bytes).unwrap();
    assert_eq!(bytes, b"unchanged original");
    drop(original);
    let auth = AuthStore::open(&system).unwrap();
    let mut direct = Direct::start(root.path(), engine, auth);
    let (read, descriptor) = direct.open("p/worktree-main-001/added.txt", false, false);
    assert_eq!(read.data_io, OpenIo::NativeWorking);
    assert_eq!(read.entry_id, first_file.entry.to_string());
    assert_eq!(direct.read(read.handle), b"later close");
    direct.call(Request::Close {
        handle: read.handle,
    });
    drop(descriptor);
    let (other, descriptor) = direct.open("p/worktree-main-002/added.txt", false, false);
    assert_eq!(other.entry_id, second_file.entry.to_string());
    assert!(direct.read(other.handle).is_empty());
    direct.call(Request::Close {
        handle: other.handle,
    });
    drop(descriptor);
}

#[path = "worktree_commit_tests.rs"]
mod commit;
