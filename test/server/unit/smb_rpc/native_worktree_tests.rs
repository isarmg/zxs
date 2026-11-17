//! The actual DefaultShare dispatcher creates an Added container and drives the
//! existing native range writer. No Unix transport or external Samba is implied.
use super::*;
use name_policy::{LookupPath, Name, PortablePath};
use std::os::unix::fs::MetadataExt;
use storage_contract::file_draft::{CommitMode, UnbornBody};
use storage_contract::snapshot_worktree::additions::WorktreeFileTarget;
use storage_contract::snapshot_worktree::creation::{AllocateWorktree, WorktreeNaming};
use storage_contract::snapshot_worktree::{InitializeSnapshot, WorktreeHandle};
use storage_contract::{OperationId, RepositoryId};
use storage_engine::{CommitRequest, NamespaceOperation, WorktreeEntryContent};

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
    added_metadata_at(engine, target, "added.txt")
}

fn check_readonly_birth_open(
    direct: &mut Direct, slot: &str, path: &str, opened: &OpenResult, target: &WorktreeFileTarget,
) {
    use storage_contract::ErrorCode;

    let generation = direct.shared.account.read().unwrap().selection().generation;
    let mut peer = DefaultShare::new(
        Arc::clone(&direct.shared), direct._auth.sole_identity().unwrap(),
        generation, direct.shared.max_file_bytes,
    ).unwrap();
    let snapshot = |engine: &Engine| (
        engine.directory_worktree_status(target.worktree).unwrap().revision,
        engine.state().file_containers.len(), engine.state().versions.len(),
    );
    let before = observe(direct, slot, snapshot);
    let open = |exclusive| Request::Open {
        path: path.replace("added.txt", "ADDED.TXT"), writable: true, create: true,
        truncate: false, exclusive, write_through: false,
    };
    // A second SMB connection opens the existing occurrence for write while
    // the read-only creator still has its handle. Neither operation is a save.
    let Reply::Open { metadata: writer, descriptor: _fd } = peer.dispatch(open(false)).unwrap()
        else { panic!("native working writer"); };
    assert_eq!(writer.data_io, OpenIo::NativeWorking);
    assert_eq!(writer.entry_id, opened.entry_id);
    assert_eq!(writer.entry_id, target.entry.to_string());
    assert_eq!(writer.size, 0);
    assert_eq!(direct.share.dispatch(open(false)).unwrap_err().code, ErrorCode::FileWriteBusy);
    assert_eq!(direct.share.dispatch(open(true)).unwrap_err().code, ErrorCode::NameConflict);
    assert_eq!(direct.share.dispatch(Request::WorkingResize {
        handle: opened.handle, length: 1,
    }).unwrap_err().code, ErrorCode::Forbidden);
    assert!(direct.read(opened.handle).is_empty());
    assert_eq!(direct.flush(opened.handle)["changed"], false);
    peer.dispatch(Request::Close { handle: writer.handle }).unwrap();
    assert_eq!(observe(direct, slot, snapshot), before);
}

fn added_metadata_at(
    engine: &Engine,
    target: WorktreeHandle,
    path: &str,
) -> (WorktreeFileTarget, u64) {
    let entry = engine
        .directory_worktree_entry(target, &LookupPath::parse(path).unwrap())
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
    assert_native_worktree_birth(false);
}

#[test]
fn native_worktree_nested_birth_resaves_close_and_cold_reopen_keep_independent_identity() {
    assert_native_worktree_birth(true);
}

fn assert_native_worktree_birth(nested: bool) {
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
    if nested {
        engine
            .file_namespace(
                &owner.repository_id,
                NamespaceOperation::Mkdir {
                    path: PortablePath::parse("Folder/Child").unwrap(),
                },
                "alice",
                OperationId::generate().unwrap(),
            )
            .unwrap();
    }
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

    let relative = if nested { "Folder/Child/added.txt" } else { "added.txt" };
    let directory = if nested { "/Folder/Child" } else { "" };
    let first_mount = format!("p/worktree-main-001{directory}");
    let second_mount = format!("p/worktree-main-002{directory}");
    let first_path = format!("p/worktree-main-001/{}", relative.to_ascii_lowercase());
    let second_path = format!("p/worktree-main-002/{}", relative.to_ascii_lowercase());
    let (opened, placeholder) = direct.open(&first_path, true, true);
    assert_eq!(opened.data_io, OpenIo::NativeWorking);
    assert_eq!(opened.size, 0);
    assert!(opened.version_id.is_none() && opened.workspace_id.is_none());
    assert!(placeholder.metadata().unwrap().is_dir());
    assert!(
        placeholder
            .write_all_at(b"not an application BODY", 0)
            .is_err()
    );
    let first_file = listed(&mut direct, &first_mount, 0);
    assert_eq!(opened.entry_id, first_file.entry.to_string());
    assert_eq!(first_file.path.as_str(), relative);
    observe(&direct, &slot, |engine| {
        let record = &engine.state().file_containers[&first_file.file];
        assert!(matches!(
            record.working.unborn().unwrap().body(),
            UnbornBody::Empty {}
        ));
        assert_deleted_container(&disk, record);
        assert!(!engine.native_current_entries(repository).unwrap().iter().any(|entry| {
            matches!(entry, storage_engine::NativeCurrentEntry::Unborn { target, .. }
                if target.file == first_file.file)
        }));
        assert_eq!(engine.state().versions, versions);
    });
    let (other, other_placeholder) = direct.open(&second_path, false, true);
    let second_file = listed(&mut direct, &second_mount, 0);
    assert_ne!(first_file.file, second_file.file);
    assert_ne!(first_file.entry, second_file.entry);
    check_readonly_birth_open(&mut direct, &slot, &second_path, &other, &second_file);
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
    assert_eq!(listed(&mut direct, &first_mount, 0), first_file);
    let first_save = direct.flush(opened.handle);
    assert_eq!(direct.flush(opened.handle), first_save);
    assert_eq!(listed(&mut direct, &first_mount, 10), first_file);
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
    assert_eq!(listed(&mut direct, &first_mount, 11), first_file);
    assert_eq!(listed(&mut direct, &second_mount, 0), second_file);
    let Reply::Stat(stat) = direct.call(Request::Stat {
        path: first_path.clone(),
    }) else {
        panic!("actual Added stat reply");
    };
    assert_eq!(stat.entry_id, first_file.entry.to_string());
    assert_eq!(stat.size, 11);
    drop(direct);

    let engine = Engine::open(&system).unwrap();
    assert_eq!(engine.state().versions, versions);
    assert_eq!(engine.state().branches, branches);
    assert_eq!(added_metadata_at(&engine, first, relative), (first_file.clone(), 11));
    assert_eq!(added_metadata_at(&engine, second, relative), (second_file.clone(), 0));
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
    let (read, descriptor) = direct.open(&first_path, false, false);
    assert_eq!(read.data_io, OpenIo::NativeWorking);
    assert_eq!(read.entry_id, first_file.entry.to_string());
    assert_eq!(direct.read(read.handle), b"later close");
    direct.call(Request::Close {
        handle: read.handle,
    });
    drop(descriptor);
    let (other, descriptor) = direct.open(&second_path, false, false);
    assert_eq!(other.entry_id, second_file.entry.to_string());
    assert!(direct.read(other.handle).is_empty());
    direct.call(Request::Close {
        handle: other.handle,
    });
    drop(descriptor);
}

#[path = "worktree_commit_tests.rs"]
mod commit;
