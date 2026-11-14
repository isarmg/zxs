use super::*;
use crate::directory_worktree::ledger::{EditLeaf, EditRecord, Event};
use crate::directory_worktree::lifecycle::{Mount, Publication};
use crate::directory_worktree::projection::FileResult;
use crate::directory_worktree::projection::namespace::NamespaceChange;
use crate::disk::DiskRoot;
use crate::persist::layout::worktree as layout;
use crate::working_tree::records::local_entity;
use crate::working_tree::snapshot::{AdmittedSnapshotManifest, SnapshotEntry, SnapshotFileSource};
use name_policy::LookupName;
use platform_io::{fs_safety::PrivateDirectory, payload::Expectation};
use std::{fs, io::{Cursor as Bytes, Read}};
use storage_contract::{ContentHash, FileId, OperationId, VersionId};
use storage_contract::working_tree::{
    DirectoryId, SnapshotBranchId, SnapshotNodeId, SnapshotRepositoryId,
};

fn operation(id: u8) -> OperationId { OperationId::from_random([id; 16]) }
fn lookup(path: &str) -> LookupPath { LookupPath::parse(path).unwrap() }
fn file(id: u8, bytes: &[u8]) -> SnapshotFile {
    SnapshotFile {
        file_id: FileId::from_random([id; 16]), version_id: VersionId::from_random([id; 16]),
        size: bytes.len() as u64, content_hash: ContentHash::from_digest(blake3::hash(bytes)),
    }
}
fn trusted(_: &Publication, _: &[FileResult]) -> Result<(), LedgerError> { Ok(()) }
struct Fixture {
    temp: tempfile::TempDir,
    repository: DiskRoot,
    source: HeldSnapshotNode,
    instance: HeldSparseWorktree,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("repository");
        PrivateDirectory::create(&path).unwrap();
        let repository = DiskRoot::open(&path).unwrap();
        let entries = vec![
            SnapshotEntry {
                path: PortablePath::parse("Empty").unwrap(),
                content: SnapshotEntryContent::Directory {},
            },
            SnapshotEntry {
                path: PortablePath::parse("Folder").unwrap(),
                content: SnapshotEntryContent::Directory {},
            },
            SnapshotEntry {
                path: PortablePath::parse("Folder/file.txt").unwrap(),
                content: SnapshotEntryContent::File {
                    material: file(3, b"nested"), source: SnapshotFileSource::SnapshotOnly,
                },
            },
            SnapshotEntry {
                path: PortablePath::parse("Root.txt").unwrap(),
                content: SnapshotEntryContent::File {
                    material: file(4, b"root"), source: SnapshotFileSource::SnapshotOnly,
                },
            },
        ];
        let manifest = AdmittedSnapshotManifest::prepare(
            SnapshotRepositoryId::from_random([1; 16]), SnapshotNodeId::from_random([2; 16]),
            operation(2), entries,
        ).unwrap();
        let source = HeldSnapshotNode::prepare(&repository, manifest, |file| {
            Ok(Bytes::new(if file.version_id == VersionId::from_random([3; 16]) {
                b"nested".to_vec()
            } else { b"root".to_vec() }))
        }).unwrap();
        let instance = HeldSparseWorktree::prepare(
            &source, WorktreeId::from_random([6; 16]), operation(6),
        ).unwrap();
        Self { temp, repository, source, instance }
    }
    fn ledger(&self) -> HeldWorktreeLedger<'_> {
        HeldWorktreeLedger::prepare(
            &self.source, self.instance.binding(), SnapshotBranchId::from_random([7; 16]),
            Mount {
                parent: DirectoryId::from_random([9; 16]),
                name: LookupName::parse("worktree-main-001").unwrap(),
            },
        ).unwrap()
    }
    fn edit(&self, ledger: &mut HeldWorktreeLedger<'_>) -> EditRecord {
        let path = PortablePath::parse("Root.txt").unwrap();
        let mut material = file(4, b"edited");
        material.version_id = VersionId::from_random([8; 16]);
        let leaf = layout::edited_file(material.version_id, &path);
        let receipt = self.instance.directory().copy_new_entry_verified_synced(
            &leaf, &mut Bytes::new(b"edited"),
            Expectation { bytes: 6, blake3: Some(blake3::hash(b"edited")) },
        ).unwrap();
        let edit = EditRecord {
            path, material, entity: local_entity(receipt.identity),
            leaf: EditLeaf::parse(leaf.as_os_str().to_str().unwrap()).unwrap(),
        };
        ledger.append(Event::Edits {
            operation: operation(10), expected: ledger.worktree().revision(),
            edits: vec![edit.clone()],
        }, trusted).unwrap();
        edit
    }
}
fn read(view: &HeldReadView<'_>, path: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    view.open_file(&lookup(path)).unwrap().read_to_end(&mut bytes).unwrap();
    bytes
}

#[test]
fn physically_empty_worktree_reads_original_bytes_and_preserves_logical_empty_directories() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let view = HeldReadView::sparse(&fixture.source, &ledger).unwrap();
    assert_eq!(read(&view, "root.TXT"), b"root");
    assert_eq!(read(&view, "folder/FILE.txt"), b"nested");
    assert_eq!(fixture.instance.directory().root_entries().unwrap().count(), 0);
    let page = view.directory(None, None, NonZeroU16::new(10).unwrap()).unwrap();
    assert_eq!(page.entries.iter().map(|row| row.path.as_str()).collect::<Vec<_>>(),
        ["Empty", "Folder", "Root.txt"]);
    assert!(view.directory(Some(&lookup("empty")), None, NonZeroU16::new(10).unwrap())
        .unwrap().entries.is_empty());
    assert!(matches!(view.open_file(&lookup("Empty")), Err(ViewError::IsDirectory)));
    assert!(matches!(view.open_file(&lookup("Missing")), Err(ViewError::Missing)));
}

#[test]
fn edited_read_uses_exact_sparse_inode_and_does_not_change_original_snapshot() {
    let fixture = Fixture::new();
    let mut ledger = fixture.ledger();
    let edit = fixture.edit(&mut ledger);
    let view = HeldReadView::sparse(&fixture.source, &ledger).unwrap();
    assert_eq!(read(&view, "Root.txt"), b"edited");
    let mut original = Vec::new();
    fixture.source.open_payload(VersionId::from_random([4; 16])).unwrap()
        .read_to_end(&mut original).unwrap();
    assert_eq!(original, b"root");
    let path = fixture.instance.directory().path().join(edit.leaf.as_str());
    fs::rename(&path, fixture.temp.path().join("retained-edit")).unwrap();
    fs::write(&path, b"edited").unwrap();
    assert!(matches!(view.open_file(&lookup("Root.txt")),
        Err(ViewError::Worktree(WorktreeStoreError::MaterialIdentity))));
}

#[test]
fn bounded_directory_pages_bind_parent_and_revision_without_listing_physical_management_names() {
    let fixture = Fixture::new();
    let mut ledger = fixture.ledger();
    let first = {
        let view = HeldReadView::sparse(&fixture.source, &ledger).unwrap();
        let page = view.directory(None, None, NonZeroU16::new(1).unwrap()).unwrap();
        assert_eq!(page.entries[0].path.as_str(), "Empty");
        let next = page.next.unwrap();
        let second = view.directory(None, Some(&next), NonZeroU16::new(1).unwrap()).unwrap();
        assert_eq!(second.entries[0].path.as_str(), "Folder");
        assert!(matches!(view.directory(
            Some(&lookup("Folder")), Some(&next), NonZeroU16::new(1).unwrap(),
        ), Err(ViewError::Cursor)));
        next
    };
    fixture.edit(&mut ledger);
    let view = HeldReadView::sparse(&fixture.source, &ledger).unwrap();
    assert!(matches!(view.directory(None, Some(&first), NonZeroU16::new(1).unwrap()),
        Err(ViewError::Cursor)));
    assert!(matches!(view.directory(None, None, NonZeroU16::new(257).unwrap()),
        Err(ViewError::Budget)));
    assert!(matches!(view.directory(Some(&lookup("Root.txt")), None, NonZeroU16::new(1).unwrap()),
        Err(ViewError::NotDirectory)));
}

#[test]
fn publishing_and_deleted_instances_cannot_become_live_smb_read_views() {
    let fixture = Fixture::new();
    let mut ledger = fixture.ledger();
    ledger.append(Event::Delete {
        operation: operation(10), expected: ledger.worktree().revision(),
    }, trusted).unwrap();
    assert!(matches!(HeldReadView::sparse(&fixture.source, &ledger), Err(ViewError::Phase)));
    let Phase::Publishing { intent, .. } = ledger.worktree().phase() else {
        panic!("expected pending delete")
    };
    ledger.append(Event::Completed {
        publication: Publication { intent, scope: PublicationScope::FilesOnly }, files: vec![],
    }, trusted).unwrap();
    assert!(matches!(HeldReadView::sparse(&fixture.source, &ledger), Err(ViewError::Phase)));
}

#[test]
fn completed_directory_view_reads_new_node_without_refilling_moved_worktree_bodies() {
    let fixture = Fixture::new();
    let mut ledger = fixture.ledger();
    let edit = fixture.edit(&mut ledger);
    ledger.append(Event::Commit {
        operation: operation(11), expected: ledger.worktree().revision(),
        current_head: fixture.source.manifest().node_id(),
        fork_branch: SnapshotBranchId::from_random([12; 16]),
    }, trusted).unwrap();
    let mut entries = fixture.source.manifest().entries().to_vec();
    entries[3].content = SnapshotEntryContent::File {
        material: edit.material, source: SnapshotFileSource::SnapshotOnly,
    };
    let node = HeldSnapshotNode::prepare(&fixture.repository, AdmittedSnapshotManifest::prepare(
        fixture.source.manifest().repository_id(), SnapshotNodeId::from_random([20; 16]),
        operation(11), entries,
    ).unwrap(), |file| {
        Ok(Bytes::new(if file.version_id == edit.material.version_id {
            b"edited".to_vec()
        } else { b"nested".to_vec() }))
    }).unwrap();
    fs::rename(
        fixture.instance.directory().path().join(edit.leaf.as_str()),
        fixture.temp.path().join("retained-transfer-fixture"),
    ).unwrap();
    let Phase::Publishing { intent, .. } = ledger.worktree().phase() else {
        panic!("expected frozen publication")
    };
    ledger.append(Event::Completed {
        publication: Publication {
            intent, scope: PublicationScope::Directory {
                branch: intent.source.branch, node: node.manifest().node_id(),
            },
        },
        files: vec![FileResult {
            path: edit.path, material: edit.material, source: SnapshotFileSource::SnapshotOnly,
        }],
    }, |publication, files| {
        assert_eq!(publication.scope, PublicationScope::Directory {
            branch: intent.source.branch, node: node.manifest().node_id(),
        });
        assert_eq!(files[0].material, edit.material);
        Ok(())
    }).unwrap();
    assert!(matches!(HeldReadView::sparse(&fixture.source, &ledger), Err(ViewError::Phase)));
    let view = HeldReadView::published(&node, &ledger).unwrap();
    assert_eq!(view.stamp().material, node.manifest().reference());
    assert_eq!(read(&view, "Root.txt"), b"edited");
    assert_eq!(read(&view, "Folder/file.txt"), b"nested");
    assert_eq!(fixture.instance.directory().root_entries().unwrap().count(), 0);
    assert!(matches!(HeldReadView::published(&fixture.source, &ledger), Err(ViewError::Binding)));
}

#[test]
fn uncertain_or_replaced_ledger_cannot_expose_cached_active_reads() {
    let fixture = Fixture::new();
    let mut ledger = fixture.ledger();
    {
        let view = HeldReadView::sparse(&fixture.source, &ledger).unwrap();
        let ledger_path = fixture.source.worktrees_directory().path()
            .join(layout::ledger(fixture.instance.binding().worktree()).as_path());
        fs::rename(&ledger_path, fixture.temp.path().join("retained-ledger")).unwrap();
        PrivateDirectory::create(&ledger_path).unwrap();
        assert!(matches!(view.open_file(&lookup("Root.txt")), Err(ViewError::Ledger(_))));
        assert!(matches!(view.directory(None, None, NonZeroU16::new(1).unwrap()),
            Err(ViewError::Ledger(_))));
    }
    assert!(ledger.append(Event::Delete {
        operation: operation(10), expected: ledger.worktree().revision(),
    }, trusted).is_err());
    assert!(matches!(HeldReadView::sparse(&fixture.source, &ledger),
        Err(ViewError::Ledger(LedgerError::Uncertain))));
}

#[test]
fn another_writer_advancing_the_checkpoint_fences_an_already_open_read_view() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let mut writer = HeldWorktreeLedger::reopen(
        &fixture.source, ledger.binding(), ledger.checkpoint(), trusted,
    ).unwrap();
    let view = HeldReadView::sparse(&fixture.source, &ledger).unwrap();
    assert_eq!(read(&view, "Root.txt"), b"root");
    writer.append(Event::Delete {
        operation: operation(10), expected: writer.worktree().revision(),
    }, trusted).unwrap();
    assert!(matches!(view.open_file(&lookup("Root.txt")),
        Err(ViewError::Ledger(LedgerError::Chain))));
}


#[test]
fn durable_namespace_reads_and_pages_reopen_the_exact_original_and_sparse_occurrences() {
    let fixture = Fixture::new();
    let mut ledger = fixture.ledger();
    let edit = fixture.edit(&mut ledger);
    let prior = ledger.checkpoint();
    let cursor = HeldReadView::sparse(&fixture.source, &ledger).unwrap()
        .directory(None, None, NonZeroU16::new(1).unwrap()).unwrap().next.unwrap();
    let changes = vec![
        NamespaceChange::Rename {
            source: lookup("Folder"), target: PortablePath::parse("Moved").unwrap(),
        },
        NamespaceChange::Rename {
            source: lookup("Root.txt"), target: PortablePath::parse("Moved/Edited.txt").unwrap(),
        },
        NamespaceChange::AddDirectory { path: PortablePath::parse("Moved/New").unwrap() },
        NamespaceChange::RemoveSubtree { path: lookup("Empty") },
        NamespaceChange::Rename {
            source: lookup("Moved"), target: PortablePath::parse("moved").unwrap(),
        },
    ];
    ledger.append(Event::Namespace {
        operation: operation(11), expected: ledger.worktree().revision(), changes,
    }, trusted).unwrap();
    let binding = ledger.binding();
    let checkpoint = ledger.checkpoint();
    let state = ledger.worktree().clone();
    drop(ledger);
    assert!(HeldWorktreeLedger::reopen(&fixture.source, binding, prior, trusted).is_err());
    let recovered = HeldWorktreeLedger::reopen(
        &fixture.source, binding, checkpoint, trusted,
    ).unwrap();
    assert_eq!(recovered.worktree(), &state);
    assert_eq!(recovered.edits(), [edit.clone()]);
    let view = HeldReadView::sparse(&fixture.source, &recovered).unwrap();
    assert_eq!(read(&view, "MOVED/edited.TXT"), b"edited");
    assert_eq!(read(&view, "moved/FILE.txt"), b"nested");
    for missing in ["Root.txt", "Folder/file.txt", "Empty"] {
        assert!(matches!(view.open_file(&lookup(missing)), Err(ViewError::Missing)));
    }
    assert!(matches!(view.directory(None, Some(&cursor), NonZeroU16::new(10).unwrap()),
        Err(ViewError::Cursor)));
    let root = view.directory(None, None, NonZeroU16::new(10).unwrap()).unwrap();
    assert_eq!(root.entries.iter().map(|entry| entry.path.as_str()).collect::<Vec<_>>(), ["moved"]);
    let children = view.directory(Some(&lookup("moved")), None, NonZeroU16::new(10).unwrap())
        .unwrap();
    assert_eq!(children.entries.iter().map(|entry| entry.path.as_str()).collect::<Vec<_>>(),
        ["moved/Edited.txt", "moved/file.txt", "moved/New"]);
    assert!(view.directory(Some(&lookup("moved/New")), None, NonZeroU16::new(10).unwrap())
        .unwrap().entries.is_empty());
    assert_eq!(fixture.instance.directory().root_entries().unwrap().count(), 1);
    assert_eq!(fs::read(fixture.instance.directory().path().join(edit.leaf.as_str())).unwrap(),
        b"edited");
    let fresh = HeldSparseWorktree::prepare(
        &fixture.source, WorktreeId::from_random([16; 16]), operation(16),
    ).unwrap();
    let fresh_ledger = HeldWorktreeLedger::prepare(
        &fixture.source, fresh.binding(), SnapshotBranchId::from_random([7; 16]),
        Mount {
            parent: DirectoryId::from_random([9; 16]),
            name: LookupName::parse("worktree-main-002").unwrap(),
        },
    ).unwrap();
    let fresh_view = HeldReadView::sparse(&fixture.source, &fresh_ledger).unwrap();
    assert_eq!(read(&fresh_view, "Root.txt"), b"root");
    assert_eq!(read(&fresh_view, "Folder/file.txt"), b"nested");
    assert!(matches!(fresh_view.open_file(&lookup("moved/Edited.txt")), Err(ViewError::Missing)));
    assert_eq!(fresh.directory().root_entries().unwrap().count(), 0);
}
