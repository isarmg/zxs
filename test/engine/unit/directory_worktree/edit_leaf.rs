use super::*;

#[test]
fn physical_edit_leaf_keeps_239_utf8_user_bytes_and_the_full_prefix() {
    let user = format!("{}ab", "界".repeat(79));
    let spelling = format!("0123456789abcde-{user}");
    assert_eq!(user.len(), 239);
    assert_eq!(spelling.len(), 255);
    assert!(LookupName::parse(&spelling).is_err());
    let leaf = EditLeaf::parse(&spelling).unwrap();
    assert_eq!(leaf.as_str(), spelling);
    assert_eq!(leaf.entry().as_os_str(), spelling.as_str());
    assert_eq!(leaf.key(), name_policy::lookup_key(&spelling));
    let expected = storage_contract::canonical_json(&spelling).unwrap();
    assert_eq!(storage_contract::canonical_json(&leaf).unwrap(), expected);
    let reopened: EditLeaf = storage_contract::decode_strict(&expected).unwrap();
    assert_eq!(reopened, leaf);
    assert_eq!(storage_contract::canonical_json(&reopened).unwrap(), expected);
}

#[test]
fn physical_edit_leaf_preserves_existing_scalar_bytes_and_rejects_invalid_components() {
    for spelling in [
        "0123456789abcde-file.txt",
        "0123456789abcde-e\u{301}.txt",
        "0123456789abcde-界.txt",
    ] {
        let original = LookupName::parse(spelling).unwrap();
        let leaf = EditLeaf::parse(spelling).unwrap();
        assert_eq!(
            storage_contract::canonical_json(&leaf).unwrap(),
            storage_contract::canonical_json(&original).unwrap()
        );
        assert_eq!(leaf.key(), original.key());
    }
    for spelling in [
        "file.txt",
        "0123456789abcd-file.txt",
        "0123456789abcdE-file.txt",
        "0123456789abcde-",
        "0123456789abcde-../escape",
        "0123456789abcde-nul\0byte",
    ] {
        assert!(EditLeaf::parse(spelling).is_err());
        let encoded = storage_contract::canonical_json(&spelling).unwrap();
        assert!(storage_contract::decode_strict::<EditLeaf>(&encoded).is_err());
    }
    let excessive = format!("0123456789abcde-{}", "界".repeat(81));
    assert!(EditLeaf::parse(&excessive).is_err());
}

#[test]
fn full_length_edit_leaf_appends_and_reopens_without_relaxing_binding_or_user_budget() {
    use crate::directory_worktree::ledger::{EditRecord, Event, HeldWorktreeLedger, LedgerError};
    use crate::directory_worktree::lifecycle::{Mount, Publication, WorktreeId};
    use crate::directory_worktree::projection::{FileResult, ReadRoute};
    use crate::directory_worktree::store::HeldSparseWorktree;
    use crate::disk::DiskRoot;
    use crate::persist::layout::worktree as layout;
    use crate::working_tree::records::local_entity;
    use crate::working_tree::snapshot::{
        AdmittedSnapshotManifest, HeldSnapshotNode, SnapshotEntry, SnapshotEntryContent,
        SnapshotFile, SnapshotFileSource,
    };
    use name_policy::{LookupPath, PortablePath};
    use platform_io::{fs_safety::PrivateDirectory, payload::Expectation};
    use std::{fs, io::{Cursor, Read}};
    use storage_contract::{ContentHash, FileId, OperationId, Sequence, VersionId};
    use storage_contract::working_tree::{
        DirectoryId, SnapshotBranchId, SnapshotNodeId, SnapshotRepositoryId,
    };

    fn trusted(_: &Publication, _: &[FileResult]) -> Result<(), LedgerError> { Ok(()) }
    let operation = |byte| OperationId::from_random([byte; 16]);
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repository");
    PrivateDirectory::create_all(&root).unwrap();
    let repository = DiskRoot::open(&root).unwrap();
    let user = format!("{}ab", "界".repeat(79));
    let path = PortablePath::parse(&user).unwrap();
    let original = SnapshotFile {
        file_id: FileId::from_random([4; 16]),
        version_id: VersionId::from_random([5; 16]),
        size: 8, content_hash: ContentHash::from_digest(blake3::hash(b"original")),
    };
    let source = HeldSnapshotNode::prepare(&repository, AdmittedSnapshotManifest::prepare(
        SnapshotRepositoryId::from_random([1; 16]), SnapshotNodeId::from_random([2; 16]),
        operation(3), vec![SnapshotEntry {
            path: path.clone(),
            content: SnapshotEntryContent::File {
                material: original, source: SnapshotFileSource::SnapshotOnly,
            },
        }],
    ).unwrap(), |_| Ok(Cursor::new(b"original"))).unwrap();
    let instance = HeldSparseWorktree::prepare(
        &source, WorktreeId::from_random([6; 16]), operation(6),
    ).unwrap();
    let mut ledger = HeldWorktreeLedger::prepare(
        &source, instance.binding(), SnapshotBranchId::from_random([7; 16]),
        Mount {
            parent: DirectoryId::from_random([9; 16]),
            name: LookupName::parse("worktree-main-001").unwrap(),
        },
    ).unwrap();
    let material = SnapshotFile {
        file_id: original.file_id, version_id: VersionId::from_random([8; 16]),
        size: 6, content_hash: ContentHash::from_digest(blake3::hash(b"edited")),
    };
    let physical = layout::edited_file(material.version_id, &path);
    assert_eq!(physical.as_os_str().as_encoded_bytes().len(), 255);
    let receipt = instance.directory().copy_new_entry_verified_synced(
        &physical, &mut Cursor::new(b"edited"),
        Expectation { bytes: 6, blake3: Some(blake3::hash(b"edited")) },
    ).unwrap();
    let edit = EditRecord {
        path, material, leaf: EditLeaf::parse(physical.as_os_str().to_str().unwrap()).unwrap(),
        entity: local_entity(receipt.identity),
    };
    let before = ledger.worktree().clone();
    let checkpoint = ledger.checkpoint();
    let directory = source.worktrees_directory().path()
        .join(layout::ledger(instance.binding().worktree()).as_path());
    let genesis = fs::read(directory.join(layout::frame(Sequence::new(0)).as_path())).unwrap();
    let event = |edit| Event::Edits {
        operation: operation(10), expected: before.revision(), edits: vec![edit],
    };
    let mut different_leaf = edit.clone();
    let unbound = layout::edited_file(VersionId::from_random([80; 16]), &edit.path);
    different_leaf.leaf = EditLeaf::parse(unbound.as_os_str().to_str().unwrap()).unwrap();
    assert!(matches!(ledger.append(event(different_leaf), trusted), Err(LedgerError::Binding)));
    let mut excessive = edit.clone();
    excessive.path = PortablePath::parse(&"界".repeat(80)).unwrap();
    let unbudgeted = layout::edited_file(material.version_id, &excessive.path);
    excessive.leaf = EditLeaf::parse(unbudgeted.as_os_str().to_str().unwrap()).unwrap();
    assert!(matches!(ledger.append(event(excessive), trusted), Err(LedgerError::Budget)));
    assert_eq!(ledger.worktree(), &before);
    assert_eq!(ledger.checkpoint(), checkpoint);
    assert!(ledger.edits().is_empty());
    assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
    assert_eq!(fs::read(directory.join(layout::frame(Sequence::new(0)).as_path())).unwrap(), genesis);
    let intended = event(edit.clone());
    ledger.append(intended.clone(), trusted).unwrap();
    let binding = ledger.binding();
    let checkpoint = ledger.checkpoint();
    let state = ledger.worktree().clone();
    let frame = fs::read(directory.join(layout::frame(checkpoint.sequence).as_path())).unwrap();
    drop(ledger);
    let mut reopened = HeldWorktreeLedger::reopen(&source, binding, checkpoint, trusted).unwrap();
    assert_eq!(reopened.worktree(), &state);
    assert_eq!(reopened.edits(), [edit.clone()]);
    assert_eq!(reopened.append(intended, trusted).unwrap(), checkpoint);
    assert_eq!(fs::read(directory.join(layout::frame(checkpoint.sequence).as_path())).unwrap(), frame);
    assert_eq!(fs::read_dir(&directory).unwrap().count(), 2);
    let projection = reopened.projection().unwrap();
    let Some(ReadRoute::Working { material }) = projection.route(&LookupPath::from(&edit.path))
        else { panic!("the retained occurrence must route to its exact edited body") };
    assert_eq!(material.entity, edit.entity);
    assert_eq!(material.leaf, physical);
    let mut contents = Vec::new();
    instance.open_material(material).unwrap().read_to_end(&mut contents).unwrap();
    assert_eq!(contents, b"edited");
    let mut original_bytes = Vec::new();
    source.open_payload(original.version_id).unwrap().read_to_end(&mut original_bytes).unwrap();
    assert_eq!(original_bytes, b"original");
}

#[test]
fn edit_leaf_retains_the_previous_complete_frame_bytes_and_commitment() {
    use super::super::record::{Body, EditRecord, Event, Format, Frame, commitment};
    use crate::directory_worktree::lifecycle::Revision;
    use crate::persist::layout::worktree as layout;
    use crate::working_tree::snapshot::SnapshotFile;
    use name_policy::PortablePath;
    use storage_contract::{ContentHash, FileId, Generation, OperationId, Sequence, VersionId};
    use storage_contract::working_tree::{LocalEntityIdentity, MaterialCommitment};

    #[derive(Serialize)]
    struct PreviousEditRecord {
        path: PortablePath,
        material: SnapshotFile,
        leaf: LookupName,
        entity: LocalEntityIdentity,
    }
    let path = PortablePath::parse("file.txt").unwrap();
    let material = SnapshotFile {
        file_id: FileId::from_random([1; 16]), version_id: VersionId::from_random([2; 16]),
        size: 6, content_hash: ContentHash::from_digest(blake3::hash(b"edited")),
    };
    let leaf = layout::edited_file(material.version_id, &path);
    let spelling = leaf.as_os_str().to_str().unwrap();
    let entity = LocalEntityIdentity::new(11, 22);
    let previous_edit = PreviousEditRecord {
        path: path.clone(), material, leaf: LookupName::parse(spelling).unwrap(), entity,
    };
    let edit = EditRecord { path, material, leaf: EditLeaf::parse(spelling).unwrap(), entity };
    let operation = OperationId::from_random([3; 16]);
    let expected = Revision {
        generation: Generation::new(4),
        manifest: MaterialCommitment::from_digest(blake3::hash(b"prior revision")),
    };
    let previous = MaterialCommitment::from_digest(blake3::hash(b"previous frame"));
    let frame = Frame {
        format: Format::WorktreeTransitions, sequence: Sequence::new(5), previous: Some(previous),
        body: Body::Event(Event::Edits { operation, expected, edits: vec![edit] }),
    };
    let previous_frame = json_codec::json!({
        "format": "worktree_transitions", "sequence": Sequence::new(5), "previous": previous,
        "body": {
            "kind": "event",
            "record": {
                "kind": "edits", "operation": operation, "expected": expected,
                "edits": [previous_edit],
            },
        },
    });
    let previous_bytes = storage_contract::canonical_json(&previous_frame).unwrap();
    assert_eq!(frame.bytes().unwrap(), previous_bytes);
    assert_eq!(frame.checkpoint(&frame.bytes().unwrap()).commitment, commitment(&previous_bytes));
    let decoded: Frame = storage_contract::decode_strict(&previous_bytes).unwrap();
    assert_eq!(decoded, frame);
    assert_eq!(decoded.bytes().unwrap(), previous_bytes);
}
