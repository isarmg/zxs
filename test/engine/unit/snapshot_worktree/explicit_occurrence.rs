//! Explicit edit receipts, not content equality, select worktree FILE results.
use crate::{CommitRequest, Config, Engine, ReadFile, WorktreeEntryContent};
use name_policy::{LookupPath, Name, PortablePath};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use storage_contract::snapshot_worktree::{
    CommitWorktree, CreateWorktree, EditWorktree, InitializeSnapshot, WorktreeHandle,
};
use storage_contract::{ContentHash, Mode, OperationId, Principal};

const ORIGINAL: &[u8] = b"same bytes are not the same successful edit";
const UNTOUCHED: &[u8] = b"an untouched occurrence still uses the original version";

fn input(root: &Path, body: &[u8]) -> ReadFile {
    let path = root.join("source");
    fs::write(&path, body).unwrap();
    fs::File::open(path).unwrap().into()
}

fn content(engine: &Engine, target: WorktreeHandle, path: &str) -> (Vec<u8>, (u64, u64)) {
    let mut opened = engine.open_directory_worktree_content(
        target, &LookupPath::parse(path).unwrap(),
    ).unwrap();
    let metadata = opened.file.metadata().unwrap();
    let mut bytes = Vec::new();
    opened.file.read_to_end(&mut bytes).unwrap();
    (bytes, (metadata.dev(), metadata.ino()))
}

fn inventory(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(base: &Path, parent: &Path, entries: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(parent).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                visit(base, &entry.path(), entries);
            } else {
                entries.insert(
                    entry.path().strip_prefix(base).unwrap().to_owned(),
                    fs::read(entry.path()).unwrap(),
                );
            }
        }
    }
    let mut entries = BTreeMap::new();
    visit(root, root, &mut entries);
    entries
}

#[test]
fn actual_same_content_edit_publishes_reserved_version_and_moves_exact_inode() {
    publish_occurrence("edited.txt", ORIGINAL);
}

#[test]
fn actual_nested_edit_keeps_parent_paths_and_same_name_sibling_identity() {
    publish_occurrence("Folder/Child/untouched.txt", b"only the nested occurrence changed");
}

fn publish_occurrence(path: &str, edited: &[u8]) {
    let temporary = tempfile::tempdir().unwrap();
    let inputs = tempfile::tempdir().unwrap();
    let system = temporary.path().join("system");
    let disk = temporary.path().join("owner");
    let config = Config::independent_roots(Mode::Development, [
        (Name::parse("Occurrence owner").unwrap(), disk.clone()),
    ]).unwrap();
    let slot = config.disks[0].slot_id.clone();
    let principal: Principal = "admin".parse().unwrap();
    let mut engine = Engine::initialize(&system, config).unwrap();
    let repository = engine.create_disk_root(
        &slot, principal.as_str(), OperationId::generate().unwrap(),
    ).unwrap();
    let path = PortablePath::parse(path).unwrap();
    if let Some(parent) = path.parent() {
        engine.file_namespace(
            &repository.repository_id,
            crate::NamespaceOperation::Mkdir { path: parent },
            principal.as_str(), OperationId::generate().unwrap(),
        ).unwrap();
    }
    let workspace = engine.create_workspace(
        &repository.main_branch_id, principal.as_str(),
    ).unwrap();
    engine.stage_bytes(&workspace.workspace_id, path.as_str(), ORIGINAL).unwrap();
    engine.stage_bytes(&workspace.workspace_id, "untouched.txt", UNTOUCHED).unwrap();
    engine.commit_workspace(CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: OperationId::generate().unwrap(),
        principal: principal.as_str().to_owned(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "actual complete snapshot source".into(),
        request_digest: None,
    }).unwrap();
    let selected = engine.file_context(&repository.repository_id, path.as_str()).unwrap();
    let initial_head = engine.file_branch_head(&selected.branch_id).unwrap();
    let initial_versions = engine.state().versions.clone();

    // The specialized reservation must not change ordinary unchanged-save semantics.
    let workspace = engine.create_workspace(&selected.branch_id, principal.as_str()).unwrap();
    engine.stage_bytes(&workspace.workspace_id, path.as_str(), ORIGINAL).unwrap();
    engine.commit_workspace(CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: OperationId::generate().unwrap(),
        principal: principal.as_str().to_owned(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "ordinary unchanged save".into(),
        request_digest: None,
    }).unwrap();
    assert_eq!(engine.file_branch_head(&selected.branch_id).unwrap(), initial_head);
    assert_eq!(engine.state().versions, initial_versions);

    let context = engine.snapshot_root_context(repository.repository_id.parse().unwrap()).unwrap();
    let source = engine.initialize_directory_snapshot(&InitializeSnapshot {
        host: context.host, source: context.source, operation_id: OperationId::generate().unwrap(),
    }, &principal).unwrap();
    let mut create = CreateWorktree {
        source, parent_directory_id: context.host.directory_id,
        name: Name::parse("worktree-main-001").unwrap(),
        expected_root_generation: context.host.expected_root_generation,
        operation_id: OperationId::generate().unwrap(),
    };
    let target = engine.create_directory_worktree(&create, &principal).unwrap();
    create.name = Name::parse("worktree-main-002").unwrap();
    create.operation_id = OperationId::generate().unwrap();
    let independent = engine.create_directory_worktree(&create, &principal).unwrap();
    let original = content(&engine, independent, path.as_str());
    let untouched = content(&engine, independent, "untouched.txt");
    let before = engine.directory_worktree_status(target).unwrap().revision;
    engine.check_directory_worktree_edit(target, &path, before).unwrap();
    if let Some(parent) = path.parent() {
        let sequence = engine.state().sequence;
        assert!(engine.check_directory_worktree_edit(target, &parent, before).is_err());
        let page = engine.directory_worktree_entries(
            target, Some(&LookupPath::from(&parent)), None,
        ).unwrap();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].path, path);
        assert_eq!(engine.state().sequence, sequence);
    }
    let edit = EditWorktree {
        target, path: path.clone(), expected: before,
        size: edited.len() as u64, content_hash: ContentHash::from_digest(blake3::hash(edited)),
        operation_id: OperationId::generate().unwrap(),
    };
    let revision = engine.edit_directory_worktree(
        &edit, &principal, input(inputs.path(), edited),
    ).unwrap();
    assert_eq!(revision.generation, before.generation.next().unwrap());
    assert_ne!(revision.manifest, before.manifest);
    let sparse = content(&engine, target, path.as_str());
    assert_eq!(sparse.0, edited);
    assert_ne!(sparse.1, original.1);
    assert_eq!(content(&engine, independent, path.as_str()), original);
    assert_eq!(engine.state().versions, initial_versions);
    let metadata = engine.directory_worktree_entry(target, &LookupPath::from(&path)).unwrap();
    let WorktreeEntryContent::File { file, version, .. } = metadata.content else {
        panic!("the explicit original occurrence remains a file");
    };
    assert_ne!(version.to_string(), initial_head);

    let request = CommitWorktree {
        target, expected: revision, message: "publish the explicit occurrence".into(),
        operation_id: OperationId::generate().unwrap(),
    };
    let committed = engine.commit_directory_worktree(&request, &principal).unwrap();
    assert_eq!(committed.branch_id, source.branch_id);
    assert_ne!(committed.node_id, source.node_id);
    assert_eq!(content(&engine, target, path.as_str()), sparse);
    assert_eq!(content(&engine, independent, path.as_str()), original);
    let copied = content(&engine, target, "untouched.txt");
    assert_eq!(copied.0, UNTOUCHED);
    assert_ne!(copied.1, untouched.1);
    assert_eq!(engine.file_branch_head(&selected.branch_id).unwrap(), version.to_string());
    assert_eq!(engine.state().versions.len(), initial_versions.len() + 1);
    let published = &engine.state().versions[&version.to_string()];
    assert_eq!(published.file_id, file.to_string());
    assert_eq!(published.source_version_ids, [initial_head]);
    assert_eq!(published.content_hash, edit.content_hash);
    for (identity, record) in initial_versions.iter() {
        assert_eq!(&engine.state().versions[identity], record);
    }
    let receipt = engine.operation(request.operation_id).unwrap();
    drop(engine);

    let mut engine = Engine::open(&system).unwrap();
    assert_eq!(content(&engine, target, path.as_str()), sparse);
    assert_eq!(content(&engine, independent, path.as_str()), original);
    let physical = inventory(temporary.path());
    let sequence = engine.state().sequence;
    assert_eq!(engine.commit_directory_worktree(&request, &principal).unwrap(), committed);
    assert_eq!(engine.operation(request.operation_id).unwrap(), receipt);
    assert_eq!(engine.state().sequence, sequence);
    assert_eq!(inventory(temporary.path()), physical);
}
