//! Production native FILE preparation and instance ledger, on real empty roots.
#[path = "draft_gateway/fixture.rs"]
mod fixture;
use crate::persist::snapshot_worktree::{self as codec, Record};
use crate::{Engine, NativeCurrentEntry, NativeWorkingFile, WorktreeEntryContent};
use fixture::Fixture;
use name_policy::{LookupPath, PortablePath};
use std::fs;
use std::os::unix::fs::MetadataExt;
use storage_contract::file_container::ContainerParent;
use storage_contract::file_draft::UnbornBody;
use storage_contract::snapshot_worktree::additions::{CreateWorktreeFile, WorktreeFileTarget};
use storage_contract::snapshot_worktree::creation::{AllocateWorktree, WorktreeNaming};
use storage_contract::snapshot_worktree::WorktreeHandle;
use storage_contract::{OperationId, Principal};

fn principal() -> Principal {
    "admin".parse().unwrap()
}
fn instance(fixture: &Fixture, engine: &mut Engine) -> WorktreeHandle {
    let host = engine
        .snapshot_root_context(fixture.targets[0].repository)
        .unwrap()
        .host;
    engine
        .allocate_directory_worktree(
            &AllocateWorktree {
                source: fixture.source,
                parent_directory_id: host.directory_id,
                expected_root_generation: host.expected_root_generation,
                operation_id: OperationId::generate().unwrap(),
                naming: WorktreeNaming::Automatic,
            },
            &principal(),
        )
        .unwrap()
}
fn create(engine: &mut Engine, target: WorktreeHandle) -> (CreateWorktreeFile, WorktreeFileTarget) {
    let request = CreateWorktreeFile {
        target,
        path: PortablePath::parse("entered.txt").unwrap(),
        expected: engine.directory_worktree_status(target).unwrap().revision,
        operation_id: OperationId::generate().unwrap(),
    };
    let file = engine
        .create_directory_worktree_file(&request, &principal())
        .unwrap();
    (request, file)
}
fn read(engine: &Engine, opened: &NativeWorkingFile) -> Vec<u8> {
    let mut bytes = vec![0; engine.native_working_size(opened).unwrap() as usize];
    let length = engine.read_native_working(opened, &mut bytes, 0).unwrap();
    assert_eq!(length, bytes.len());
    bytes
}
fn assert_inode(engine: &Engine, target: &WorktreeFileTarget) {
    let record = &engine.state().file_containers[&target.file];
    let ContainerParent::Worktree {
        target: owner,
        path,
        name,
    } = &record.namespace.parent
    else {
        panic!("actual worktree physical parent required");
    };
    assert_eq!(*owner, target.worktree);
    assert_eq!(path, &target.path);
    let root = engine
        .namespace_host_root(record.namespace.repository, record.namespace.branch)
        .unwrap();
    let deleted = root.deleted().unwrap();
    let container = deleted.path().join(name.as_str());
    let metadata = fs::metadata(&container).unwrap();
    let replica = &record.replicas[&record.owner_disk];
    assert_eq!(
        (metadata.dev(), metadata.ino()),
        (replica.entity.device(), replica.entity.inode())
    );
    assert!(
        container
            .join(crate::container_storage::file_database::NAME)
            .is_file()
    );
    assert_eq!(
        fs::read_dir(container.join(crate::persist::layout::VERSIONS.as_path()))
            .unwrap()
            .count(),
        0
    );
    assert!(!root.common().unwrap().path().join("entered.txt").exists());
}

#[test]
fn native_added_files_birth_deleted_containers_save_twice_and_cold_reopen_without_history() {
    let (fixture, mut engine) = Fixture::new();
    let one = instance(&fixture, &mut engine);
    let two = instance(&fixture, &mut engine);
    let history = engine.state().versions.clone();
    let branches = engine.state().branches.clone();
    let (original, file) = create(&mut engine, one);
    let (_, independent) = create(&mut engine, two);
    assert_ne!(file.file, independent.file);
    assert_ne!(file.entry, independent.entry);
    assert_inode(&engine, &file);
    assert_inode(&engine, &independent);
    let ordinary = engine
        .native_current_entries(fixture.targets[0].repository)
        .unwrap();
    assert!(!ordinary.iter().any(|entry| matches!(entry,
        NativeCurrentEntry::Unborn { target, .. }
            if target.file == file.file || target.file == independent.file)));
    let mut opened = engine
        .open_directory_worktree_working(&file, &principal(), true)
        .unwrap();
    assert!(read(&engine, &opened).is_empty());
    assert_eq!(
        engine
            .write_native_working(&mut opened, &principal(), 0, b"first")
            .unwrap(),
        5
    );
    let first = engine
        .flush_native_working(&mut opened, &principal())
        .unwrap()
        .unwrap();
    let first_record = engine.state().file_containers[&file.file].clone();
    let retained = engine
        .open_directory_worktree_working(&file, &principal(), false)
        .unwrap();
    assert_eq!(read(&engine, &retained), b"first");
    engine
        .write_native_working(&mut opened, &principal(), 0, b"other")
        .unwrap();
    engine
        .write_native_working(&mut opened, &principal(), 9, b"B")
        .unwrap();
    let second = engine
        .flush_native_working(&mut opened, &principal())
        .unwrap()
        .unwrap();
    assert_ne!(first.operation_id, second.operation_id);
    assert_eq!(read(&engine, &opened), b"other\0\0\0\0B");
    assert_eq!(read(&engine, &retained), b"first");
    let UnbornBody::Saved { manifest: old } = first_record.working.unborn().unwrap().body() else {
        panic!("first actual immutable extent save required");
    };
    let UnbornBody::Saved { manifest: current } = engine.state().file_containers[&file.file]
        .working
        .unborn()
        .unwrap()
        .body()
    else {
        panic!("second actual save required");
    };
    assert_ne!(old.commitment(), current.commitment());
    assert_eq!(old.operation(), first.operation_id);
    assert_eq!(current.operation(), second.operation_id);
    let before = fixture.media();
    assert_eq!(
        engine
            .flush_native_working(&mut opened, &principal())
            .unwrap(),
        Some(second.clone())
    );
    assert_eq!(
        engine
            .create_directory_worktree_file(&original, &principal())
            .unwrap(),
        file
    );
    assert_eq!(fixture.media(), before);
    let entry = engine
        .directory_worktree_entry(one, &LookupPath::from(&file.path))
        .unwrap();
    assert!(
        matches!(entry.content, WorktreeEntryContent::Working { target, working }
        if target == file && working.logical_size() == 10)
    );
    assert_eq!(engine.state().versions, history);
    assert_eq!(engine.state().branches, branches);
    drop(retained);
    drop(opened);
    drop(engine);

    let mut engine = Engine::open(&fixture.system).unwrap();
    assert_inode(&engine, &file);
    assert_inode(&engine, &independent);
    let opened = engine
        .open_directory_worktree_working(&file, &principal(), false)
        .unwrap();
    assert_eq!(read(&engine, &opened), b"other\0\0\0\0B");
    let untouched = engine
        .open_directory_worktree_working(&independent, &principal(), false)
        .unwrap();
    assert!(read(&engine, &untouched).is_empty());
    let before = fixture.media();
    assert_eq!(
        engine
            .create_directory_worktree_file(&original, &principal())
            .unwrap(),
        file
    );
    for result in [&first, &second] {
        let Record::SaveFile(plan) = codec::decode(result).unwrap().unwrap().record else {
            panic!("original native worktree save receipt required");
        };
        assert_eq!(
            engine
                .snapshot_receipts()
                .unwrap()
                .retry_file_save(&plan.request, &principal())
                .unwrap(),
            Some(result.clone())
        );
    }
    assert_eq!(fixture.media(), before);
    assert_eq!(engine.state().versions, history);
    assert_eq!(engine.state().branches, branches);
}
