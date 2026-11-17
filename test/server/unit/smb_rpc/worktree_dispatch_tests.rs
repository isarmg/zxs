//! Real EngineBackend, production Session/DefaultShare dispatch and real held
//! writable descriptors. Unix framing/SCM_RIGHTS and real Samba are not run here.
use super::{Fixture, LostCommitReply, fixture_backend, identity, named_session};
use crate::smb_rpc::{Reply, Shared, default_share::DefaultShare};
use crate::{Backend, Identity};
use name_policy::{LookupPath, Name};
use smb_protocol::protocol::{DirectoryEntry, OpenIo, OpenResult, Request};
use std::{
    collections::BTreeMap, fs::{self, File}, io::Read, os::unix::fs::{FileExt, MetadataExt},
    path::{Path, PathBuf}, sync::{Arc, atomic::Ordering},
};
use storage_contract::snapshot_worktree::{CommitWorktree, CreateWorktree, InitializeSnapshot};
use storage_contract::snapshot_worktree::WorktreeHandle;
use storage_contract::{Mode, OperationId};
use storage_engine::{CommitRequest, Config, Engine};

struct Setup {
    fixture: Fixture,
    shared: Arc<Shared>,
    backend: Arc<LostCommitReply>,
    repository: storage_contract::Repository,
    target: WorktreeHandle,
    independent: WorktreeHandle,
    disk: PathBuf,
    slot: String,
}

fn setup() -> Setup {
    let root = tempfile::tempdir().unwrap();
    let disk = root.path().join("primary");
    let config = Config::independent_roots(Mode::Development, [
        (Name::parse("p").unwrap(), disk.clone()),
    ]).unwrap();
    let slot = config.disks[0].slot_id.clone();
    let mut engine = Engine::initialize(&root.path().join("state"), config).unwrap();
    let repository = engine.create_disk_root(&slot, "alice", OperationId::generate().unwrap())
        .unwrap();
    let workspace = engine.create_workspace(&repository.main_branch_id, "alice").unwrap();
    engine.stage_bytes(&workspace.workspace_id, "file.txt", b"original immutable body").unwrap();
    engine.stage_bytes(&workspace.workspace_id, "other.txt", b"unchanged COPY body").unwrap();
    engine.commit_workspace(CommitRequest {
        workspace_id: workspace.workspace_id, operation_id: OperationId::generate().unwrap(),
        principal: "alice".into(), expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token, message: "source".into(), request_digest: None,
    }).unwrap();
    let context = engine.snapshot_root_context(repository.repository_id.parse().unwrap()).unwrap();
    let original = engine.initialize_directory_snapshot(&InitializeSnapshot {
        host: context.host, source: context.source, operation_id: OperationId::generate().unwrap(),
    }, &"alice".parse().unwrap()).unwrap();
    let mut create = CreateWorktree {
        source: original, parent_directory_id: context.host.directory_id,
        name: Name::parse("worktree-main-001").unwrap(),
        expected_root_generation: context.host.expected_root_generation,
        operation_id: OperationId::generate().unwrap(),
    };
    let target = engine.create_directory_worktree(&create, &"alice".parse().unwrap()).unwrap();
    create.name = Name::parse("worktree-main-002").unwrap();
    create.operation_id = OperationId::generate().unwrap();
    let independent = engine.create_directory_worktree(&create, &"alice".parse().unwrap()).unwrap();
    let (fixture, shared, backend) = fixture_backend(root, engine);
    Setup { fixture, shared, backend, repository, target, independent, disk, slot }
}

fn open(share: &mut DefaultShare, path: &str, writable: bool, truncate: bool)
    -> (OpenResult, File)
{
    let reply = share.dispatch(Request::Open {
        path: path.into(), writable, create: false, truncate,
        exclusive: false, write_through: false,
    }).unwrap();
    let Reply::Open { metadata, descriptor } = reply else { panic!("Open reply"); };
    (metadata, File::from(descriptor))
}

fn call(share: &mut DefaultShare, request: Request) -> Reply {
    share.dispatch(request).unwrap()
}

fn write(share: &mut DefaultShare, opened: &OpenResult, file: &File, bytes: &[u8]) {
    call(share, Request::PrepareWrite {
        handle: opened.handle, end: bytes.len() as u64, bytes: bytes.len() as u64,
    });
    file.write_all_at(bytes, 0).unwrap();
    file.set_len(bytes.len() as u64).unwrap();
    call(share, Request::Dirty { handle: opened.handle });
}

fn read(share: &mut DefaultShare, path: &str) -> Vec<u8> {
    let (opened, fd) = open(share, path, false, false);
    assert_eq!(opened.data_io, OpenIo::VerifiedRanges);
    assert!(fd.metadata().unwrap().is_dir());
    let Reply::Read(bytes) = call(share, Request::Read {
        handle: opened.handle, offset: 0, length: 4096,
    }) else { panic!("Read reply"); };
    call(share, Request::Close { handle: opened.handle });
    bytes.to_vec()
}

fn observe<T>(setup: &Setup, read: impl FnOnce(&Engine) -> T) -> T {
    setup.backend.inner.with_disk_registration(&identity(&setup.shared), &setup.slot,
        |_, engine| Ok(read(engine))).unwrap()
}

fn inode(setup: &Setup, target: WorktreeHandle, path: &str) -> (u64, u64) {
    observe(setup, |engine| {
        let file = engine.open_directory_worktree_content(target, &LookupPath::parse(path).unwrap())
            .unwrap().file;
        let metadata = file.metadata().unwrap();
        (metadata.dev(), metadata.ino())
    })
}

fn inventory(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, here: &Path, found: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(here).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() { visit(root, &entry.path(), found); }
            else { found.insert(entry.path().strip_prefix(root).unwrap().into(),
                fs::read(entry.path()).unwrap()); }
        }
    }
    let mut found = BTreeMap::new();
    visit(root, root, &mut found);
    found
}

fn share(setup: &Setup) -> DefaultShare {
    let generation = setup.shared.account.read().unwrap().selection().generation;
    DefaultShare::new(Arc::clone(&setup.shared), identity(&setup.shared), generation, 4096).unwrap()
}

#[test]
fn worktree_dispatch_overwrite_resaves_close_then_explicit_commit_moves_only_final_body() {
    let setup = setup();
    let mut share = share(&setup);
    let original_versions = observe(&setup, |engine| engine.state().versions.clone());
    let original_other = inode(&setup, setup.independent, "other.txt");
    let (root, root_fd) = open(&mut share, "p", false, false);
    let mut mounts = Vec::new();
    loop {
        let Reply::Directory(page) = call(&mut share, Request::ReadDirectory {
            handle: root.handle, restart: false,
        }) else { panic!("Directory reply"); };
        for entry in page.entries {
            if let DirectoryEntry::Worktree { target, relative_path: None, .. } = entry {
                mounts.push(target);
            }
        }
        if !page.more { break; }
    }
    assert_eq!(mounts, [setup.target, setup.independent]);
    call(&mut share, Request::Close { handle: root.handle });
    drop(root_fd);
    let (directory, _) = open(&mut share, "p/worktree-main-001", false, false);
    let Reply::Directory(page) = call(&mut share, Request::ReadDirectory {
        handle: directory.handle, restart: false,
    }) else { panic!("Directory reply"); };
    assert_eq!(page.entries.len(), 2);
    assert!(page.entries.iter().all(|entry| matches!(entry, DirectoryEntry::Worktree { .. })));
    call(&mut share, Request::Close { handle: directory.handle });
    let before_open = inventory(&setup.disk);
    let (opened, file) = open(&mut share, "p/worktree-main-001/file.txt", true, true);
    assert_eq!(opened.data_io, OpenIo::Descriptor);
    assert!(opened.version_id.is_none() && opened.workspace_id.is_none());
    assert_eq!(file.metadata().unwrap().len(), 0);
    assert_eq!(inventory(&setup.disk), before_open, "O_TRUNC only creates private empty staging");
    write(&mut share, &opened, &file, b"first complete save A");
    call(&mut share, Request::Flush { handle: opened.handle });
    let revision_a = observe(&setup, |engine| engine.directory_worktree_status(setup.target)
        .unwrap().revision);
    let inode_a = inode(&setup, setup.target, "file.txt");
    let after_a = inventory(&setup.disk);
    let sequence = observe(&setup, |engine| engine.state().sequence);
    call(&mut share, Request::Flush { handle: opened.handle });
    assert_eq!(observe(&setup, |engine| engine.state().sequence), sequence);
    assert_eq!(inventory(&setup.disk), after_a);
    write(&mut share, &opened, &file, b"second complete save B");
    call(&mut share, Request::Flush { handle: opened.handle });
    let revision_b = observe(&setup, |engine| engine.directory_worktree_status(setup.target)
        .unwrap().revision);
    assert_eq!(revision_b.generation, revision_a.generation.next().unwrap());
    assert_ne!(inode(&setup, setup.target, "file.txt"), inode_a);
    assert_eq!(read(&mut share, "p/worktree-main-001/file.txt"), b"second complete save B");
    assert_eq!(read(&mut share, "p/worktree-main-002/file.txt"), b"original immutable body");
    assert_eq!(observe(&setup, |engine| engine.state().versions.clone()), original_versions);
    // Close publishes a later dirty generation, not the already completed B.
    write(&mut share, &opened, &file, b"final complete save C from Close");
    call(&mut share, Request::Close { handle: opened.handle });
    drop(file);
    let final_inode = inode(&setup, setup.target, "file.txt");
    let final_status = setup.backend.inner.directory_worktree_status(
        &identity(&setup.shared), setup.target,
    ).unwrap();
    assert_eq!(final_status.revision.generation, revision_b.generation.next().unwrap());
    assert_eq!(observe(&setup, |engine| engine.state().versions.clone()), original_versions);
    let commit = CommitWorktree {
        target: setup.target, expected: final_status.revision,
        message: "explicit commit only".into(), operation_id: OperationId::generate().unwrap(),
    };
    setup.backend.inner.commit_directory_worktree(&identity(&setup.shared), commit).unwrap();
    assert_eq!(inode(&setup, setup.target, "file.txt"), final_inode, "modified body is MOVEd");
    assert_ne!(inode(&setup, setup.target, "other.txt"), original_other,
        "unchanged body is COPYed");
    assert_eq!(observe(&setup, |engine| engine.state().versions.len()),
        original_versions.len() + 1);
    assert_eq!(read(&mut share, "p/worktree-main-001/file.txt"),
        b"final complete save C from Close");
    let target = setup.target;
    let system = setup.fixture.path().join("state");
    drop(share);
    let Setup { fixture, shared, backend, .. } = setup;
    drop(shared);
    drop(backend);
    let Fixture { auth, root: _root } = fixture;
    drop(auth);
    let engine = Engine::open(&system).unwrap();
    let mut bytes = Vec::new();
    engine.open_directory_worktree_content(target, &LookupPath::parse("file.txt").unwrap())
        .unwrap().file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"final complete save C from Close");
}

#[test]
fn worktree_dispatch_exact_lost_reply_pending_guard_and_committed_handle_reject_new_writes() {
    let setup = setup();
    let mut share = share(&setup);
    let (cached, cached_fd) = open(&mut share, "p/worktree-main-001", false, false);
    let (opened, file) = open(&mut share, "p/worktree-main-001/file.txt", true, true);
    write(&mut share, &opened, &file, b"real save whose reply is lost");
    setup.backend.fail.store(true, Ordering::SeqCst);
    assert!(share.dispatch(Request::Flush { handle: opened.handle }).is_err());
    let sequence = observe(&setup, |engine| engine.state().sequence);
    let before = inventory(&setup.disk);
    assert!(share.dispatch(Request::PrepareWrite {
        handle: opened.handle, end: 1, bytes: 1,
    }).is_err());
    call(&mut share, Request::Flush { handle: opened.handle });
    assert_eq!(observe(&setup, |engine| engine.state().sequence), sequence);
    assert_eq!(inventory(&setup.disk), before);
    assert!(share.dispatch(Request::ReadDirectory {
        handle: cached.handle, restart: false,
    }).is_err(), "a buffered page cannot outlive an edit checkpoint");
    call(&mut share, Request::ReadDirectory { handle: cached.handle, restart: true });
    call(&mut share, Request::Close { handle: cached.handle });
    drop(cached_fd);
    let (cached, cached_fd) = open(&mut share, "p/worktree-main-001", false, false);
    let status = setup.backend.inner.directory_worktree_status(
        &identity(&setup.shared), setup.target,
    ).unwrap();
    let mut named = named_session(Arc::clone(&setup.shared), &setup.repository.main_branch_id);
    let mut invalid: Identity = named.identity.clone();
    invalid.smb_binding.as_mut().unwrap().generation += 1;
    assert!(setup.backend.inner.authorize_smb_worktree(
        &invalid, setup.target, crate::SmbWorktreeAccess::Read,
    ).is_err());
    // SMB identities cannot call the management commit publisher.
    let commit = CommitWorktree {
        target: setup.target, expected: status.revision,
        message: "explicit commit".into(), operation_id: OperationId::generate().unwrap(),
    };
    assert!(setup.backend.inner.commit_directory_worktree(
        &named.identity, commit.clone(),
    ).is_err());
    setup.backend.inner.commit_directory_worktree(&identity(&setup.shared), commit).unwrap();
    assert!(share.dispatch(Request::ReadDirectory {
        handle: cached.handle, restart: false,
    }).is_err(), "commit changes the page checkpoint/material even at the same revision");
    call(&mut share, Request::ReadDirectory { handle: cached.handle, restart: true });
    call(&mut share, Request::Close { handle: cached.handle });
    drop(cached_fd);
    let before = inventory(&setup.disk);
    assert!(share.dispatch(Request::PrepareWrite {
        handle: opened.handle, end: 1, bytes: 1,
    }).is_err(), "same revision after directory commit is not writable authority");
    assert_eq!(inventory(&setup.disk), before);
    call(&mut share, Request::Close { handle: opened.handle });
    drop(file);
    // Unsupported namespace changes must not enter the ordinary container writer.
    assert!(named.dispatch(Request::Namespace {
        kind: smb_protocol::protocol::NamespaceOperation::Unlink,
        path: "worktree-main-001/file.txt".into(), destination: None, replace: false,
        operation_id: OperationId::generate().unwrap().to_string(),
    }).is_err());
    assert_eq!(read(&mut share, "p/worktree-main-001/file.txt"), b"real save whose reply is lost");
    // Repeated opens use the real platform result, not a substituted CoW probe.
    // Unsupported opens have never exposed a descriptor or publication.
    let before_spool = inventory(&setup.shared.spool);
    for _ in 0..3 {
        let result = share.dispatch(Request::Open {
            path: "p/worktree-main-002/file.txt".into(), writable: true, create: false,
            truncate: false, exclusive: false, write_through: false,
        });
        match result {
            Ok(Reply::Open { metadata, descriptor }) => {
                let mut file = File::from(descriptor);
                let mut bytes = Vec::new();
                file.read_to_end(&mut bytes).unwrap();
                assert_eq!(bytes, b"original immutable body");
                call(&mut share, Request::Close { handle: metadata.handle });
            }
            Err(error) => assert_eq!(error.code, storage_contract::ErrorCode::UnsupportedOperation),
            Ok(_) => panic!("Open reply"),
        }
        assert_eq!(inventory(&setup.shared.spool), before_spool);
    }
    // The fixture's complete 16 MiB account allowance is still available, not
    // merely free of on-disk sources while leaked in-memory charges remain.
    let capacity = 16 * 1024 * 1024 - 64 * 1024;
    let reservation = setup.shared.budget.reserve(
        &identity(&setup.shared).principal,
        &setup.shared.spool.join("capacity-probe"),
        capacity,
        capacity,
    ).unwrap();
    drop(reservation);
    assert_eq!(inventory(&setup.shared.spool), before_spool);
}
