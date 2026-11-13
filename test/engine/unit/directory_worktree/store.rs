use super::*;
use crate::working_tree::snapshot::{
    AdmittedSnapshotManifest, SnapshotEntry, SnapshotEntryContent, SnapshotFile,
    SnapshotNodeBinding,
};
use name_policy::PortablePath;
use platform_io::fs_safety::{EntryName, PrivateDirectory};
use platform_io::payload::Expectation;
use std::io::{Cursor, Read};
use storage_contract::{ContentHash, FileId, VersionId};
use storage_contract::working_tree::{SnapshotNodeId, SnapshotRepositoryId};

struct Fixture {
    temp: tempfile::TempDir,
    repository: DiskRoot,
    source: HeldSnapshotNode,
    file: SnapshotFile,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("repository");
        PrivateDirectory::create_all(&path).unwrap();
        let repository = DiskRoot::open(&path).unwrap();
        let file = SnapshotFile {
            file_id: FileId::from_random([1; 16]),
            version_id: VersionId::from_random([2; 16]),
            size: 8,
            content_hash: ContentHash::from_digest(blake3::hash(b"original")),
        };
        let manifest = AdmittedSnapshotManifest::prepare(
            SnapshotRepositoryId::from_random([3; 16]),
            SnapshotNodeId::from_random([4; 16]),
            OperationId::from_random([5; 16]),
            vec![
                SnapshotEntry {
                    path: PortablePath::parse("A.txt").unwrap(),
                    content: content(file),
                },
                SnapshotEntry {
                    path: PortablePath::parse("Empty").unwrap(),
                    content: SnapshotEntryContent::Directory {},
                },
            ],
        ).unwrap();
        let source = HeldSnapshotNode::prepare(&repository, manifest, |_| {
            Ok(Cursor::new(b"original"))
        }).unwrap();
        Self { temp, repository, source, file }
    }
    fn create(&self, id: u8) -> HeldSparseWorktree {
        HeldSparseWorktree::prepare(
            &self.source,
            WorktreeId::from_random([id; 16]),
            OperationId::from_random([id; 16]),
        ).unwrap()
    }
}
fn names(directory: &DiskRoot) -> Vec<EntryName> {
    directory.root_entries().unwrap().map(Result::unwrap).collect()
}
fn edit(worktree: &HeldSparseWorktree, name: &EntryName, bytes: &[u8]) {
    worktree.directory().copy_new_entry_verified_synced(
        name,
        &mut Cursor::new(bytes),
        Expectation { bytes: bytes.len() as u64, blake3: Some(blake3::hash(bytes)) },
    ).unwrap();
}

#[test]
fn creation_is_physically_empty_even_when_original_snapshot_has_files_and_empty_directories() {
    let fixture = Fixture::new();
    let first = fixture.create(6);
    assert!(names(first.directory()).is_empty());
    assert_eq!(first.binding().source(), fixture.source.manifest().reference());
    assert_eq!(fixture.source.manifest().entries().len(), 2);
    let mut bytes = Vec::new();
    fixture.source.open_payload(fixture.file.version_id).unwrap().read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"original");
}

#[test]
fn same_node_instances_are_distinct_and_never_inherit_other_instances_sparse_edits() {
    let fixture = Fixture::new();
    let first = fixture.create(6);
    let changed = EntryName::new("edited-file").unwrap();
    edit(&first, &changed, b"first instance modification");
    let second = fixture.create(7);
    assert_ne!(first.binding().worktree(), second.binding().worktree());
    assert_ne!(first.binding().worktree_entity, second.binding().worktree_entity);
    assert_eq!(first.binding().source(), second.binding().source());
    assert!(names(second.directory()).is_empty());
    assert_eq!(first.directory().read_entry_bytes(&changed, 1024).unwrap(),
        b"first instance modification");
}

#[test]
fn cold_reopen_preserves_edits_using_the_independently_retained_binding() {
    let fixture = Fixture::new();
    let worktree = fixture.create(6);
    let changed = EntryName::new("edited-file").unwrap();
    edit(&worktree, &changed, b"survives restart");
    let source_binding: SnapshotNodeBinding = fixture.source.binding();
    let bytes = storage_contract::canonical_json(&worktree.binding()).unwrap();
    let binding: SparseWorktreeBinding = storage_contract::decode_strict(&bytes).unwrap();
    assert_eq!(storage_contract::canonical_json(&binding).unwrap(), bytes);
    drop(worktree);
    let Fixture { temp, repository, source, .. } = fixture;
    drop(source);
    drop(repository);
    let repository = DiskRoot::open(&temp.path().join("repository")).unwrap();
    let source = HeldSnapshotNode::reopen(&repository, source_binding).unwrap();
    let restored = HeldSparseWorktree::reopen(&source, binding).unwrap();
    assert_eq!(restored.directory().read_entry_bytes(&changed, 1024).unwrap(), b"survives restart");
    let new = HeldSparseWorktree::prepare(
        &source, WorktreeId::from_random([7; 16]), OperationId::from_random([7; 16]),
    ).unwrap();
    assert!(names(new.directory()).is_empty());
}

#[test]
fn repeated_creation_never_adopts_or_resets_an_existing_instance() {
    let fixture = Fixture::new();
    let existing = fixture.create(6);
    let changed = EntryName::new("kept").unwrap();
    edit(&existing, &changed, b"retain me");
    assert!(matches!(
        HeldSparseWorktree::prepare(
            &fixture.source, existing.binding().worktree(), OperationId::from_random([6; 16]),
        ),
        Err(WorktreePreparationError::Rejected(_)),
    ));
    assert_eq!(existing.directory().read_entry_bytes(&changed, 1024).unwrap(), b"retain me");
}

#[test]
fn substituted_instance_identity_is_rejected_without_destroying_either_directory() {
    let fixture = Fixture::new();
    let original = fixture.create(6);
    let binding = original.binding();
    let path = original.directory().path().to_owned();
    let displaced = path.with_file_name("retained-original");
    std::fs::rename(&path, &displaced).unwrap();
    PrivateDirectory::create_all(&path).unwrap();
    assert!(matches!(
        HeldSparseWorktree::reopen(&fixture.source, binding),
        Err(WorktreeStoreError::Directory(_)),
    ));
    assert!(path.is_dir());
    assert!(displaced.is_dir());
}

#[test]
fn another_snapshot_cannot_authorize_reopening_this_worktree() {
    let first = Fixture::new();
    let second = Fixture::new();
    let worktree = first.create(6);
    assert!(matches!(
        HeldSparseWorktree::reopen(&second.source, worktree.binding()),
        Err(WorktreeStoreError::Source),
    ));
    assert!(names(worktree.directory()).is_empty());
}

fn content(material: SnapshotFile) -> SnapshotEntryContent {
    SnapshotEntryContent::File {
        material,
        source: crate::working_tree::snapshot::SnapshotFileSource::SnapshotOnly,
    }
}
