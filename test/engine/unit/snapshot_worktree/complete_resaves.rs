//! Two sealed saves share one original occurrence, never one mutable payload.
use crate::{CommitRequest, Config, Engine, ReadFile, WorktreeEntryContent};
use name_policy::{LookupPath, Name};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use storage_contract::snapshot_worktree::{
    CommitWorktree, CreateWorktree, EditWorktree, InitializeSnapshot, WorktreeHandle,
};
use storage_contract::{ContentHash, Mode, OperationId, Principal};

fn source(directory: &Path, name: &str, bytes: &[u8]) -> ReadFile {
    let path = directory.join(name);
    fs::write(&path, bytes).unwrap();
    fs::File::open(path).unwrap().into()
}

fn read(engine: &Engine, target: WorktreeHandle, path: &str) -> Vec<u8> {
    let mut content = engine.open_directory_worktree_content(
        target, &LookupPath::parse(path).unwrap(),
    ).unwrap();
    let mut bytes = Vec::new();
    content.file.read_to_end(&mut bytes).unwrap();
    bytes
}

fn inode(engine: &Engine, target: WorktreeHandle, path: &str) -> (u64, u64) {
    let content = engine.open_directory_worktree_content(
        target, &LookupPath::parse(path).unwrap(),
    ).unwrap();
    let metadata = content.file.metadata().unwrap();
    (metadata.dev(), metadata.ino())
}

fn inode_path(root: &Path, identity: (u64, u64)) -> PathBuf {
    fn collect(here: &Path, identity: (u64, u64), found: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(here).unwrap() {
            let entry = entry.unwrap();
            let metadata = entry.metadata().unwrap();
            if metadata.is_dir() {
                collect(&entry.path(), identity, found);
            } else if (metadata.dev(), metadata.ino()) == identity {
                found.push(entry.path());
            }
        }
    }
    let mut found = Vec::new();
    collect(root, identity, &mut found);
    assert_eq!(found.len(), 1, "published material must not be a shared hard link");
    found.pop().unwrap()
}

fn inventory(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn collect(base: &Path, here: &Path, found: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(here).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                collect(base, &entry.path(), found);
            } else {
                found.insert(entry.path().strip_prefix(base).unwrap().into(),
                    fs::read(entry.path()).unwrap());
            }
        }
    }
    let mut found = BTreeMap::new();
    collect(root, root, &mut found);
    found
}

#[test]
fn complete_resaves_keep_original_occurrence_and_move_only_exact_current_revision() {
    let temporary = tempfile::tempdir().unwrap();
    let source_directory = tempfile::tempdir().unwrap();
    let system = temporary.path().join("system");
    let disk = temporary.path().join("primary");
    let config = Config::independent_roots(Mode::Development, [
        (Name::parse("Actual primary").unwrap(), disk.clone()),
    ]).unwrap();
    let slot = config.disks[0].slot_id.clone();
    let mut engine = Engine::initialize(&system, config).unwrap();
    let principal: Principal = "admin".parse().unwrap();
    let repository = engine.create_disk_root(&slot, principal.as_str(),
        OperationId::generate().unwrap()).unwrap();
    assert!(engine.directory_worktree_mounts(repository.repository_id.parse().unwrap())
        .unwrap().is_empty());
    assert!(engine.directory_worktree_mount(repository.repository_id.parse().unwrap(),
        &name_policy::LookupName::parse("worktree-main-001").unwrap()).unwrap().is_none());
    let workspace = engine.create_workspace(&repository.main_branch_id,
        principal.as_str()).unwrap();
    engine.stage_bytes(&workspace.workspace_id, "file.txt", b"original immutable body").unwrap();
    engine.stage_bytes(&workspace.workspace_id, "other.txt", b"unchanged independent copy")
        .unwrap();
    engine.commit_workspace(CommitRequest {
        workspace_id: workspace.workspace_id.clone(),
        operation_id: OperationId::generate().unwrap(),
        principal: principal.as_str().into(), expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token, message: "real snapshot source".into(),
        request_digest: None,
    }).unwrap();
    let context = engine.snapshot_root_context(repository.repository_id.parse().unwrap()).unwrap();
    let original = engine.initialize_directory_snapshot(&InitializeSnapshot {
        host: context.host, source: context.source, operation_id: OperationId::generate().unwrap(),
    }, &principal).unwrap();
    let target = engine.create_directory_worktree(&CreateWorktree {
        source: original, parent_directory_id: context.host.directory_id,
        name: Name::parse("worktree-main-001").unwrap(),
        expected_root_generation: context.host.expected_root_generation,
        operation_id: OperationId::generate().unwrap(),
    }, &principal).unwrap();
    let independent = engine.create_directory_worktree(&CreateWorktree {
        source: original, parent_directory_id: context.host.directory_id,
        name: Name::parse("worktree-main-002").unwrap(),
        expected_root_generation: context.host.expected_root_generation,
        operation_id: OperationId::generate().unwrap(),
    }, &principal).unwrap();
    // The SMB authority projection resolves exact registered instances, not
    // filesystem display names or the first occurrence sharing a FileId.
    let repository_id = repository.repository_id.parse().unwrap();
    let sequence = engine.state().sequence;
    let mounts = engine.directory_worktree_mounts(repository_id).unwrap();
    assert_eq!(mounts.iter().map(|mount| mount.target).collect::<Vec<_>>(),
        [target, independent]);
    assert_eq!(engine.directory_worktree_mount(repository_id,
        &name_policy::LookupName::parse("WORKTREE-MAIN-001").unwrap()).unwrap(),
        Some(mounts[0].clone()));
    assert!(engine.directory_worktree_mount(repository_id,
        &name_policy::LookupName::parse("worktree-main-999").unwrap()).unwrap().is_none());
    let page = engine.directory_worktree_entries(target, None, None).unwrap();
    engine.check_directory_worktree_page(target, None, &page.stamp).unwrap();
    assert_eq!(page.revision, mounts[0].revision);
    assert!(page.next.is_none());
    assert_eq!(page.entries.iter().map(|entry| entry.path.as_str()).collect::<Vec<_>>(),
        ["file.txt", "other.txt"]);
    let original_metadata = engine.directory_worktree_entry(target,
        &LookupPath::parse("FILE.TXT").unwrap()).unwrap();
    assert_eq!(original_metadata.path.as_str(), "file.txt");
    assert_eq!(original_metadata, page.entries[0]);
    engine.check_directory_worktree_edit(target, &original_metadata.path,
        page.revision).unwrap();
    assert_eq!(engine.state().sequence, sequence);
    let unchanged_source_inode = inode(&engine, independent, "other.txt");
    let selected = engine.file_context(&repository.repository_id, "file.txt").unwrap();
    let initial_file_head = engine.file_branch_head(&selected.branch_id).unwrap();
    let initial_versions = engine.state().versions.clone();
    let a = b"first complete successful save A";
    let first = EditWorktree {
        target, path: name_policy::PortablePath::parse("file.txt").unwrap(),
        expected: engine.directory_worktree_status(target).unwrap().revision,
        size: a.len() as u64, content_hash: ContentHash::from_digest(blake3::hash(a)),
        operation_id: OperationId::generate().unwrap(),
    };
    let revision_a = engine.edit_directory_worktree(&first, &principal,
        source(source_directory.path(), "A", a)).unwrap();
    let inode_a = inode(&engine, target, "file.txt");
    let path_a = inode_path(&disk, inode_a);
    let receipt_a = engine.operation(first.operation_id).unwrap();
    let b = b"second complete successful save B with a different length";
    let second = EditWorktree {
        target, path: first.path.clone(), expected: revision_a,
        size: b.len() as u64, content_hash: ContentHash::from_digest(blake3::hash(b)),
        operation_id: OperationId::generate().unwrap(),
    };
    let revision_b = engine.edit_directory_worktree(&second, &principal,
        source(source_directory.path(), "B", b)).unwrap();
    assert!(engine.check_directory_worktree_page(target, None, &page.stamp).is_err());
    let saved_page = engine.directory_worktree_entries(target, None, None).unwrap();
    engine.check_directory_worktree_page(target, None, &saved_page.stamp).unwrap();
    let saved_metadata = engine.directory_worktree_entry(target,
        &LookupPath::parse("file.txt").unwrap()).unwrap();
    let WorktreeEntryContent::File { file, version, size, content_hash } = saved_metadata.content
    else { panic!("the saved original occurrence remains a file"); };
    let WorktreeEntryContent::File { file: original_file, version: original_version, .. } =
        original_metadata.content else { panic!("the original occurrence is a file"); };
    assert_eq!(file, original_file);
    assert_ne!(version, original_version);
    assert_eq!(size, b.len() as u64);
    assert_eq!(content_hash, second.content_hash);
    assert_eq!(engine.directory_worktree_entry(independent,
        &LookupPath::parse("file.txt").unwrap()).unwrap(), original_metadata);
    engine.check_directory_worktree_edit(target, &second.path, revision_b).unwrap();
    assert!(engine.check_directory_worktree_edit(target, &second.path, revision_a).is_err());
    assert!(engine.check_directory_worktree_edit(target,
        &name_policy::PortablePath::parse("other.txt").unwrap(), revision_b).is_err());
    assert_eq!(revision_b.generation, revision_a.generation.next().unwrap());
    assert_ne!(revision_b.manifest, revision_a.manifest);
    assert_eq!(engine.state().versions, initial_versions);
    assert_eq!(engine.file_branch_head(&selected.branch_id).unwrap(), initial_file_head);
    assert_eq!(read(&engine, target, "file.txt"), b);
    assert_eq!(read(&engine, independent, "file.txt"), b"original immutable body");
    let inode_b = inode(&engine, target, "file.txt");
    assert_ne!(inode_a, inode_b);
    let path_b = inode_path(&disk, inode_b);
    assert_eq!(fs::read(&path_a).unwrap(), a);
    let receipt_b = engine.operation(second.operation_id).unwrap();
    let before = inventory(&disk);
    let sequence = engine.state().sequence;
    let mut stale = second.clone();
    stale.operation_id = OperationId::generate().unwrap();
    assert_eq!(engine.edit_directory_worktree(&stale, &principal,
        source(source_directory.path(), "stale", b)).unwrap_err().code(),
        storage_contract::ErrorCode::UnsupportedOperation);
    let mut different_occurrence = second.clone();
    different_occurrence.operation_id = OperationId::generate().unwrap();
    different_occurrence.expected = revision_b;
    different_occurrence.path = name_policy::PortablePath::parse("other.txt").unwrap();
    assert!(engine.edit_directory_worktree(&different_occurrence, &principal,
        source(source_directory.path(), "different", b)).is_err());
    assert_eq!(engine.state().sequence, sequence);
    assert_eq!(inventory(&disk), before);
    drop(engine);

    let mut engine = Engine::open(&system).unwrap();
    assert_eq!(read(&engine, target, "file.txt"), b);
    assert_eq!(fs::read(&path_a).unwrap(), a);
    let before = inventory(&disk);
    let sequence = engine.state().sequence;
    assert_eq!(engine.edit_directory_worktree(&first, &principal,
        source(source_directory.path(), "retry-A", a)).unwrap(), revision_a);
    assert_eq!(engine.edit_directory_worktree(&second, &principal,
        source(source_directory.path(), "retry-B", b)).unwrap(), revision_b);
    assert_eq!(engine.directory_worktree_status(target).unwrap().revision, revision_b);
    assert_eq!(engine.operation(first.operation_id).unwrap(), receipt_a);
    assert_eq!(engine.operation(second.operation_id).unwrap(), receipt_b);
    assert_eq!(engine.state().sequence, sequence);
    assert_eq!(inventory(&disk), before);
    let request = CommitWorktree {
        target, expected: revision_b, message: "publish final saved revision B only".into(),
        operation_id: OperationId::generate().unwrap(),
    };
    let committed = engine.commit_directory_worktree(&request, &principal).unwrap();
    // A completed directory publication leaves the revision unchanged. An
    // adapter must use the scope gate, not merely compare revision equality.
    assert_eq!(engine.directory_worktree_status(target).unwrap().revision, revision_b);
    assert!(engine.check_directory_worktree_page(target, None, &saved_page.stamp).is_err());
    assert!(engine.check_directory_worktree_edit(target, &second.path, revision_b).is_err());
    assert_eq!(engine.directory_worktree_entry(target,
        &LookupPath::parse("file.txt").unwrap()).unwrap(), saved_metadata);
    assert_eq!(committed.branch_id, original.branch_id);
    assert_ne!(committed.node_id, original.node_id);
    assert_eq!(engine.state().versions.len(), initial_versions.len() + 1);
    assert_eq!(read(&engine, target, "file.txt"), b);
    assert_eq!(inode(&engine, target, "file.txt"), inode_b);
    assert!(!path_b.exists());
    assert_eq!(fs::read(&path_a).unwrap(), a);
    assert_ne!(inode(&engine, target, "other.txt"), unchanged_source_inode);
    assert_eq!(read(&engine, target, "other.txt"), b"unchanged independent copy");
    let file_head = engine.file_branch_head(&selected.branch_id).unwrap();
    assert_ne!(file_head, initial_file_head);
    assert_eq!(engine.state().versions[&file_head].source_version_ids,
        [initial_file_head.clone()]);
    for (version, record) in initial_versions.iter() {
        assert_eq!(&engine.state().versions[version], record);
    }
    drop(engine);
    let mut engine = Engine::open(&system).unwrap();
    assert_eq!(read(&engine, target, "file.txt"), b);
    assert_eq!(engine.directory_worktree_mounts(repository_id).unwrap().len(), 2);
    assert_eq!(engine.directory_worktree_entries(target, None, None).unwrap().entries[0],
        saved_metadata);
    assert!(engine.check_directory_worktree_edit(target, &second.path, revision_b).is_err());
    let before = inventory(&disk);
    let sequence = engine.state().sequence;
    assert_eq!(engine.commit_directory_worktree(&request, &principal).unwrap(), committed);
    assert_eq!(engine.state().sequence, sequence);
    assert_eq!(inventory(&disk), before);
}
