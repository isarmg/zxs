use crate::disk::DiskRoot;
use crate::persist::layout::snapshot as layout;
use crate::working_tree::snapshot::*;
use name_policy::PortablePath;
use platform_io::fs_safety::PrivateDirectory;
use std::{fs, io::{Cursor, Read}, os::unix::fs::MetadataExt};
use storage_contract::{ContentHash, FileId, OperationId, VersionId};
use storage_contract::working_tree::{SnapshotNodeId, SnapshotRepositoryId};

const BODY: &[u8] = b"complete original snapshot";
fn manifest() -> AdmittedSnapshotManifest {
    AdmittedSnapshotManifest::prepare(
        SnapshotRepositoryId::from_random([1; 16]),
        SnapshotNodeId::from_random([2; 16]),
        OperationId::from_random([3; 16]),
        vec![
            SnapshotEntry {
                path: PortablePath::parse("empty").unwrap(),
                content: SnapshotEntryContent::Directory {},
            },
            SnapshotEntry {
                path: PortablePath::parse("original.txt").unwrap(),
                content: SnapshotEntryContent::File(SnapshotFile {
                    file_id: FileId::from_random([4; 16]),
                    version_id: VersionId::from_random([5; 16]),
                    size: BODY.len() as u64,
                    content_hash: ContentHash::from_digest(blake3::hash(BODY)),
                }),
            },
        ],
    ).unwrap()
}
fn root() -> (tempfile::TempDir, DiskRoot) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("repository");
    PrivateDirectory::create(&path).unwrap();
    let root = DiskRoot::open(&path).unwrap();
    (temp, root)
}
fn prepared(root: &DiskRoot) -> HeldSnapshotNode {
    HeldSnapshotNode::prepare(root, manifest(), |_| Ok(Cursor::new(BODY))).unwrap()
}
fn version() -> VersionId {
    VersionId::from_random([5; 16])
}
fn bytes(node: &HeldSnapshotNode) -> Vec<u8> {
    let mut source = node.open_payload(version()).unwrap();
    let mut bytes = Vec::new();
    source.read_to_end(&mut bytes).unwrap();
    bytes
}

#[test]
fn reopen_uses_external_identity_and_ignores_unregistered_worktree_material() {
    let (_temp, root) = root();
    let node = prepared(&root);
    let expected = node.binding();
    assert_eq!(expected.reference(), manifest().reference());
    let worktrees = node.worktrees_directory();
    let soft_deleted = platform_io::fs_safety::EntryName::new("deleted-instance").unwrap();
    worktrees.write_relative_atomic(&soft_deleted.as_relative(), b"edited body", false).unwrap();
    let reopened = HeldSnapshotNode::reopen(&root, expected).unwrap();
    assert_eq!(bytes(&reopened), BODY);
    assert_eq!(reopened.manifest().entries(), manifest().entries());
    assert!(reopened.open_payload(VersionId::from_random([9; 16])).is_err());
    assert_eq!(worktrees.read_entry_bytes(&soft_deleted, 100).unwrap(), b"edited body");
}

#[test]
fn prepare_never_adopts_or_overwrites_existing_node() {
    let (_temp, root) = root();
    let original = prepared(&root);
    assert!(matches!(HeldSnapshotNode::prepare(&root, manifest(), |_| Ok(Cursor::new(BODY))),
        Err(SnapshotPreparationError::Rejected(_))));
    assert_eq!(bytes(&original), BODY);
    assert_eq!(bytes(&HeldSnapshotNode::reopen(&root, original.binding()).unwrap()), BODY);
}

#[test]
fn incomplete_payload_preserves_node_and_cannot_be_retried_as_a_fresh_preparation() {
    let (_temp, root) = root();
    let reference = manifest().reference();
    let error = HeldSnapshotNode::prepare(&root, manifest(), |_| Ok(Cursor::new(b"wrong")))
        .err().unwrap();
    match error {
        SnapshotPreparationError::Incomplete { reference: actual, entity, source } => {
            assert_eq!(actual, reference);
            assert!(entity.is_some());
            assert!(matches!(source, SnapshotStoreError::Content));
        }
        _ => panic!("creation must remain visible as incomplete"),
    }
    assert!(root.path().join(layout::node(reference.node).as_path()).is_dir());
    assert!(!root.path().join(layout::node(reference.node).as_path())
        .join(layout::MANIFEST.as_path()).exists());
    assert!(matches!(HeldSnapshotNode::prepare(&root, manifest(), |_| Ok(Cursor::new(BODY))),
        Err(SnapshotPreparationError::Rejected(_))));
}

#[test]
fn recovery_rejects_corrupt_missing_and_extra_payloads_without_repairing_them() {
    let (_temp, root) = root();
    let node = prepared(&root);
    let binding = node.binding();
    let payload = node.node.path().join(layout::FILES.as_path())
        .join(layout::file(version()).as_path());
    fs::write(&payload, b"modified snapshot").unwrap();
    assert!(HeldSnapshotNode::reopen(&root, binding).is_err());
    assert_eq!(fs::read(&payload).unwrap(), b"modified snapshot");
    fs::write(&payload, BODY).unwrap();
    let unknown = payload.parent().unwrap().join("unregistered");
    fs::write(&unknown, b"retain evidence").unwrap();
    assert!(matches!(HeldSnapshotNode::reopen(&root, binding), Err(SnapshotStoreError::Entries)));
    assert_eq!(fs::read(&unknown).unwrap(), b"retain evidence");
    fs::remove_file(&unknown).unwrap();
    fs::remove_file(&payload).unwrap();
    assert!(matches!(HeldSnapshotNode::reopen(&root, binding), Err(SnapshotStoreError::Entries)));
}

#[test]
fn manifest_replacement_and_same_bytes_at_replaced_node_do_not_rebind_recovery() {
    let (_temp, root) = root();
    let node = prepared(&root);
    let binding = node.binding();
    let path = node.node.path();
    let manifest_path = path.join(layout::MANIFEST.as_path());
    let original = fs::read(&manifest_path).unwrap();
    fs::write(&manifest_path, b"{}").unwrap();
    assert!(matches!(HeldSnapshotNode::reopen(&root, binding),
        Err(SnapshotStoreError::Manifest(_))));
    fs::write(&manifest_path, original).unwrap();
    let moved = path.with_extension("retained");
    fs::rename(path, &moved).unwrap();
    let replacement = prepared(&root);
    assert_ne!(replacement.node.held_identity().unwrap(),
        node.node.held_identity().unwrap());
    assert!(HeldSnapshotNode::reopen(&root, binding).is_err());
    assert_eq!(bytes(&node), BODY);
    assert_eq!(bytes(&replacement), BODY);
    let (_other, wrong_root) = self::root();
    assert!(matches!(HeldSnapshotNode::reopen(&wrong_root, binding),
        Err(SnapshotStoreError::Repository)));
}

#[test]
fn payloads_are_independent_single_link_copies_and_new_reads_verify_every_time() {
    let (_temp, root) = root();
    let node = prepared(&root);
    let mut first = node.open_payload(version()).unwrap();
    assert_eq!(first.metadata().unwrap().nlink(), 1);
    let mut body = Vec::new();
    first.read_to_end(&mut body).unwrap();
    assert_eq!(body, BODY);
    let payload = node.node.path().join(layout::FILES.as_path())
        .join(layout::file(version()).as_path());
    fs::write(&payload, b"changed after admission").unwrap();
    assert!(matches!(node.open_payload(version()), Err(SnapshotStoreError::Payload(_))));
}
