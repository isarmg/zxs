use super::*;
use crate::directory_worktree::io::EntryContent;
use crate::directory_worktree::ledger::Event;
use crate::directory_worktree::lifecycle::Phase;
use crate::directory_worktree::projection::namespace::NamespaceChange;
use crate::directory_worktree::store::WorktreeStoreError;
use crate::disk::DiskRoot;
use crate::persist::layout::worktree as layout;
use crate::working_tree::snapshot::{
    AdmittedSnapshotManifest, SnapshotEntry, SnapshotEntryContent, SnapshotFile,
    SnapshotFileSource,
};
use name_policy::{LookupName, LookupPath, PortablePath};
use platform_io::fs_safety::PrivateDirectory;
use std::fs;
use std::io::{Cursor as Bytes, Read};
use std::num::NonZeroU16;
use std::path::Path;
use storage_contract::working_tree::{DirectoryId, SnapshotNodeId, SnapshotRepositoryId};
use storage_contract::{ContentHash, FileId, VersionId};

// These component tests retain the actual preparation outputs, not a fabricated
// publication capability. No test accepts a FILE/directory publication receipt.
fn no_publication(_: &Publication, _: &[FileResult]) -> Result<(), LedgerError> {
    panic!("this fixture has no independently published FILE/directory receipt")
}

fn operation(byte: u8) -> OperationId {
    OperationId::from_random([byte; 16])
}

fn worktree(byte: u8) -> WorktreeId {
    WorktreeId::from_random([byte; 16])
}

fn mount() -> Mount {
    Mount {
        parent: DirectoryId::from_random([90; 16]),
        name: LookupName::parse("worktree-main-001").unwrap(),
    }
}

fn lookup(path: &str) -> LookupPath {
    LookupPath::parse(path).unwrap()
}

fn page_size(value: u16) -> NonZeroU16 {
    NonZeroU16::new(value).unwrap()
}

fn prepare_source(repository: &DiskRoot, node: u8) -> HeldSnapshotNode {
    let material = SnapshotFile {
        file_id: FileId::from_random([4; 16]),
        version_id: VersionId::from_random([5; 16]),
        size: 8,
        content_hash: ContentHash::from_digest(blake3::hash(b"original")),
    };
    let manifest = AdmittedSnapshotManifest::prepare(
        SnapshotRepositoryId::from_random([1; 16]),
        SnapshotNodeId::from_random([node; 16]),
        operation(node),
        vec![
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
                    material,
                    source: SnapshotFileSource::SnapshotOnly,
                },
            },
            SnapshotEntry {
                path: PortablePath::parse("Root.txt").unwrap(),
                content: SnapshotEntryContent::File {
                    material,
                    source: SnapshotFileSource::SnapshotOnly,
                },
            },
        ],
    ).unwrap();
    HeldSnapshotNode::prepare(repository, manifest, |_| Ok(Bytes::new(b"original"))).unwrap()
}

struct Fixture {
    temp: tempfile::TempDir,
    source: HeldSnapshotNode,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("repository");
        PrivateDirectory::create(&path).unwrap();
        let repository = DiskRoot::open(&path).unwrap();
        let source = prepare_source(&repository, 2);
        Self { temp, source }
    }

    fn prepare(&self, id: u8) -> PreparedWorktree {
        PreparedWorktree::prepare(
            &self.source,
            worktree(id),
            operation(id),
            SnapshotBranchId::from_random([7; 16]),
            mount(),
        ).unwrap()
    }
}

fn ready<'a>(source: &'a HeldSnapshotNode, prepared: &PreparedWorktree) -> WorktreeSession<'a> {
    reopen(source, prepared.binding(), prepared.checkpoint())
}

fn reopen(
    source: &HeldSnapshotNode,
    binding: Binding,
    checkpoint: Checkpoint,
) -> WorktreeSession<'_> {
    match WorktreeSession::reopen(source, binding, checkpoint, no_publication).unwrap() {
        ReopenedWorktree::Ready(session) => session,
        ReopenedWorktree::Publishing(_) => panic!("expected a locally readable session"),
    }
}

fn read(view: &HeldReadView<'_>, path: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    view.open_file(&lookup(path)).unwrap().read_to_end(&mut bytes).unwrap();
    bytes
}

fn empty(path: &Path) {
    assert_eq!(fs::read_dir(path).unwrap().count(), 0);
}

#[test]
fn preparation_reopens_exact_empty_instance_and_lists_the_immutable_source() {
    let fixture = Fixture::new();
    let prepared = fixture.prepare(6);
    let source_binding = fixture.source.binding();
    let binding = prepared.binding();
    let checkpoint = prepared.checkpoint();
    let instance = fixture.source.worktrees_directory().path()
        .join(layout::instance(worktree(6)).as_path());
    empty(&instance);
    let first = ready(&fixture.source, &prepared);
    assert_eq!(first.worktree().id(), worktree(6));
    assert_eq!(first.worktree().mount(), &mount());
    let view = first.read(ReadMaterial::Original).unwrap();
    let stamp = view.stamp();
    assert_eq!(read(&view, "ROOT.txt"), b"original");
    assert_eq!(read(&view, "folder/FILE.txt"), b"original");
    let page = view.directory(None, None, page_size(10)).unwrap();
    assert_eq!(page.entries.iter().map(|entry| entry.path.as_str()).collect::<Vec<_>>(),
        ["Empty", "Folder", "Root.txt"]);
    assert!(view.directory(Some(&lookup("empty")), None, page_size(10))
        .unwrap().entries.is_empty());
    assert!(matches!(view.content(&lookup("Root.txt")), Ok(EntryContent::File { .. })));
    empty(&instance);
    drop(view);
    drop(first);
    drop(prepared);
    let Fixture { temp, source } = fixture;
    drop(source);

    let repository = DiskRoot::open(&temp.path().join("repository")).unwrap();
    let source = HeldSnapshotNode::reopen(&repository, source_binding).unwrap();
    let session = reopen(&source, binding, checkpoint);
    let view = session.read(ReadMaterial::Original).unwrap();
    assert_eq!(view.stamp(), stamp);
    assert_eq!(read(&view, "Root.txt"), b"original");
    empty(&instance);
}

#[test]
fn ledger_creation_failure_retains_the_exact_empty_instance_without_adoption_or_cleanup() {
    let fixture = Fixture::new();
    let parent = fixture.source.worktrees_directory().path();
    let ledger_path = parent.join(layout::ledger(worktree(6)).as_path());
    PrivateDirectory::create(&ledger_path).unwrap();
    fs::write(ledger_path.join("foreign"), b"not a worktree ledger").unwrap();
    let error = PreparedWorktree::prepare(
        &fixture.source,
        worktree(6),
        operation(6),
        SnapshotBranchId::from_random([7; 16]),
        mount(),
    ).err().unwrap();
    let PreparationError::Ledger { instance, source } = error else {
        panic!("the empty instance was created before the ledger collision")
    };
    assert!(matches!(source, LedgerPreparationError::Rejected(_)));
    assert_eq!(instance.worktree(), worktree(6));
    assert_eq!(instance.operation(), operation(6));
    let retained = HeldSparseWorktree::reopen(&fixture.source, instance).unwrap();
    empty(retained.directory().path());
    let entity = retained.directory().held_identity().unwrap();
    assert!(matches!(PreparedWorktree::prepare(
        &fixture.source,
        worktree(6),
        operation(6),
        SnapshotBranchId::from_random([7; 16]),
        mount(),
    ), Err(PreparationError::Instance(WorktreePreparationError::Rejected(_)))));
    assert_eq!(HeldSparseWorktree::reopen(&fixture.source, instance).unwrap()
        .directory().held_identity().unwrap(), entity);
    assert_eq!(fs::read(ledger_path.join("foreign")).unwrap(), b"not a worktree ledger");
    empty(retained.directory().path());
}

#[test]
fn namespace_edit_reuses_the_same_ledger_and_rejects_old_checkpoints_and_cursors() {
    let fixture = Fixture::new();
    let prepared = fixture.prepare(6);
    let session = ready(&fixture.source, &prepared);
    let cursor = session.read(ReadMaterial::Original).unwrap()
        .directory(None, None, page_size(1)).unwrap().next.unwrap();
    let mut ledger = session.into_ledger();
    let checkpoint = ledger.append(Event::Namespace {
        operation: operation(10),
        expected: ledger.worktree().revision(),
        changes: vec![NamespaceChange::Rename {
            source: lookup("Folder"),
            target: PortablePath::parse("Moved").unwrap(),
        }],
    }, no_publication).unwrap();
    assert_eq!(ledger.binding(), prepared.binding());
    drop(ledger);
    assert!(matches!(WorktreeSession::reopen(
        &fixture.source, prepared.binding(), prepared.checkpoint(), no_publication,
    ), Err(LedgerError::Chain)));
    let session = reopen(&fixture.source, prepared.binding(), checkpoint);
    let view = session.read(ReadMaterial::Original).unwrap();
    assert!(matches!(view.directory(None, Some(&cursor), page_size(1)), Err(ViewError::Cursor)));
    assert!(matches!(view.open_file(&lookup("Folder/file.txt")), Err(ViewError::Missing)));
    assert_eq!(read(&view, "Moved/file.txt"), b"original");
    let page = view.directory(Some(&lookup("Moved")), None, page_size(10)).unwrap();
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].path.as_str(), "Moved/file.txt");
    empty(&fixture.source.worktrees_directory().path()
        .join(layout::instance(worktree(6)).as_path()));
}

#[test]
fn wrong_source_or_replaced_instance_cannot_be_reopened_or_serve_cached_reads() {
    let fixture = Fixture::new();
    let prepared = fixture.prepare(6);
    let repository = DiskRoot::open(&fixture.temp.path().join("repository")).unwrap();
    let other = prepare_source(&repository, 3);
    assert!(matches!(WorktreeSession::reopen(
        &other, prepared.binding(), prepared.checkpoint(), no_publication,
    ), Err(LedgerError::Binding)));
    let session = ready(&fixture.source, &prepared);
    assert!(matches!(session.read(ReadMaterial::Published(&other)), Err(ViewError::Phase)));
    let path = fixture.source.worktrees_directory().path()
        .join(layout::instance(worktree(6)).as_path());
    let parked = fixture.temp.path().join("retained-instance");
    fs::rename(&path, &parked).unwrap();
    PrivateDirectory::create(&path).unwrap();
    assert!(matches!(WorktreeSession::reopen(
        &fixture.source, prepared.binding(), prepared.checkpoint(), no_publication,
    ), Err(LedgerError::Instance(WorktreeStoreError::Directory(_)))));
    assert!(matches!(session.read(ReadMaterial::Original), Err(ViewError::Worktree(_))));
    empty(&path);
    empty(&parked);
    fs::remove_dir(&path).unwrap();
    fs::rename(&parked, &path).unwrap();
    let restored = ready(&fixture.source, &prepared);
    assert_eq!(read(&restored.read(ReadMaterial::Original).unwrap(), "Root.txt"), b"original");
    empty(&path);
}

#[test]
fn pending_publication_reopens_only_as_the_original_publishers_resume_state() {
    let fixture = Fixture::new();
    let prepared = fixture.prepare(6);
    let session = ready(&fixture.source, &prepared);
    let mut ledger = session.into_ledger();
    let checkpoint = ledger.append(Event::Delete {
        operation: operation(10),
        expected: ledger.worktree().revision(),
    }, no_publication).unwrap();
    let expected = *ledger.worktree().phase();
    drop(ledger);
    let recovered = WorktreeSession::reopen(
        &fixture.source, prepared.binding(), checkpoint, no_publication,
    ).unwrap();
    let ReopenedWorktree::Publishing(pending) = recovered else {
        panic!("an uncompleted publication must not return an ordinary session")
    };
    assert_eq!(pending.worktree().phase(), &expected);
    assert!(matches!(expected, Phase::Publishing { .. }));
    assert!(pending.edits().is_empty());
    empty(&fixture.source.worktrees_directory().path()
        .join(layout::instance(worktree(6)).as_path()));
}
