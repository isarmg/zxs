//! Real DefaultShare dispatch and native FILE storage, independent of AF_UNIX.
use super::*;
use storage_contract::{
    NamespaceEntryKind, OperationId,
    file_draft::{CommitMode, DraftCommand},
};

fn one_file(share: &mut Direct) -> DirectoryEntry {
    let (dir, fd) = share.open("p", false, false);
    assert!(fd.metadata().unwrap().is_dir());
    let Reply::Directory(page) = share.call(Request::ReadDirectory {
        handle: dir.handle, restart: true,
    }) else {
        panic!("ordinary folder did not return a directory page");
    };
    assert!(!page.more);
    assert_eq!(page.entries.len(), 1);
    share.call(Request::Close { handle: dir.handle });
    let entry = page.entries.into_iter().next().unwrap();
    assert_eq!(entry.path().as_str(), "project.zxs");
    entry
}

#[test]
fn opaque_project_file_keeps_its_version_descriptor_across_an_ordinary_smb_save() {
    let root = tempfile::tempdir().unwrap();
    let system = root.path().join("state");
    let config = Config::development([(
        name_policy::Name::parse("p").unwrap(), root.path().join("mounted-primary"),
    )]).unwrap();
    let mut engine = Engine::initialize(&system, config).unwrap();
    let repository = engine.create_disk_root("P01", "alice", OperationId::generate().unwrap())
        .unwrap();
    let auth = AuthStore::initialize(&system, "alice", &authentication::random_token().unwrap())
        .unwrap();
    let spool = root.path().join("spool");
    fs::create_dir(&spool).unwrap();
    fs::set_permissions(&spool, fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir(spool.join("empty-directory")).unwrap();
    let mut share = Direct::start(root.path(), engine, auth);

    let path = "p/project.zxs";
    let mut zip = [0_u8; 22];
    zip[..4].copy_from_slice(b"PK\x05\x06");
    let (birth, placeholder) = share.open(path, true, true);
    assert_eq!(birth.data_io, OpenIo::NativeWorking);
    share.write(birth.handle, WorkingPosition::At { offset: 0 }, &zip);
    assert_eq!(share.read(birth.handle), zip);
    share.flush(birth.handle);
    share.call(Request::Close { handle: birth.handle });
    drop(placeholder);
    let DirectoryEntry::Unborn { target, logical_size } = one_file(&mut share) else {
        panic!("a first native working save must not manufacture a version");
    };
    assert_eq!(logical_size, zip.len() as u64);
    assert_eq!(target.entry.to_string(), birth.entry_id);
    let actor = share._auth.sole_identity().unwrap();
    let pending = share.backend.unborn_file_working(&actor, target.clone()).unwrap();
    share.backend.commit_unborn_file(&actor, crate::CommitUnbornFile {
        operation_id: OperationId::generate().unwrap(),
        target: target.clone(), expected: pending.expected, message: "publish opaque FILE".into(),
    }).unwrap();
    let initial = share.backend.file_working(&actor, target.clone()).unwrap();
    share.backend.change_file_working(&actor, crate::ChangeFileWorking {
        operation_id: OperationId::generate().unwrap(),
        command: DraftCommand::SetMode {
            target: target.clone(), expected: initial.working.revision(), mode: CommitMode::Automatic,
        },
    }).unwrap();
    let DirectoryEntry::Published { entry: old, .. } = one_file(&mut share) else {
        panic!("first explicit commit did not publish an ordinary FILE");
    };
    let Reply::Stat(stat) = share.call(Request::Stat { path: path.into() }) else {
        panic!("ordinary FILE did not return a stat");
    };
    assert_eq!(stat.kind, NamespaceEntryKind::File);
    assert_eq!(stat.entry_id, old.entry_id);
    assert_eq!(stat.path, path);
    assert_eq!(old.file_id, target.file.to_string());
    assert_eq!(old.version_id, initial.base.version_id().to_string());
    assert!(share.share.dispatch(Request::Stat { path: format!("{path}/member") }).is_err());

    // A second ordinary save may replace ZIP bytes with non-ZIP bytes. Holding
    // the first reader cannot silently redirect its version to that later save.
    let (held, held_fd) = share.open(path, false, false);
    assert_eq!(held.version_id.as_deref(), Some(old.version_id.as_str()));
    assert_eq!(share.read(held.handle), zip);
    let replacement = b"arbitrary, not ZIP";
    let (writer, fd) = share.open(path, true, false);
    assert_eq!(writer.data_io, OpenIo::Descriptor);
    let size = replacement.len() as u64;
    share.call(Request::PrepareWrite { handle: writer.handle, end: size, bytes: size });
    fd.write_all_at(replacement, 0).unwrap();
    fd.set_len(size).unwrap();
    share.call(Request::Dirty { handle: writer.handle });
    share.flush(writer.handle);
    share.call(Request::Close { handle: writer.handle });
    drop(fd);
    let DirectoryEntry::Published { entry: next, .. } = one_file(&mut share) else {
        panic!("a replacement save must retain the published FILE");
    };
    assert_eq!(next.entry_id, old.entry_id);
    assert_eq!(next.file_id, old.file_id);
    assert_ne!(next.version_id, old.version_id);
    let after = share.backend.file_working(&actor, target).unwrap();
    assert_eq!(after.base.branch_id(), initial.base.branch_id());
    assert_eq!(after.base.version_id().to_string(), next.version_id);
    assert_eq!(share.read(held.handle), zip);
    let (fresh, fresh_fd) = share.open(path, false, false);
    assert_eq!(fresh.version_id.as_deref(), Some(next.version_id.as_str()));
    assert_eq!(share.read(fresh.handle), replacement);
    share.call(Request::Close { handle: fresh.handle });
    share.call(Request::Close { handle: held.handle });
    drop((fresh_fd, held_fd, share));

    let engine = Engine::open(&system).unwrap();
    let tree = engine.namespace_tree(&repository.main_branch_id).unwrap();
    assert_eq!(tree.get("project.zxs").unwrap().version_id, next.version_id);
    assert_eq!(engine.file_history(&old.file_id).unwrap().len(), 2);
    for (version, expected) in [
        (old.version_id, zip.as_slice()), (next.version_id, replacement.as_slice()),
    ] {
        let mut original = engine.open_content(&version).unwrap();
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut original.file, &mut bytes).unwrap();
        assert_eq!(bytes, expected);
    }
}
