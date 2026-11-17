//! Wire descriptions only. These fixtures never claim a live source or receipt.
use super::*;
use crate::file_draft::DraftPayloadId;
use crate::{
    BranchId, ContentHash, EntryId, FileId, FileNodeId, Generation, RepositoryId, VersionId,
};
use crate::working_tree::{SnapshotBranchId, SnapshotNodeId, SnapshotRepositoryId};
use name_policy::PortablePath;
use std::collections::BTreeSet;

#[test]
fn born_selection_wire_is_strict_and_public_outcome_has_no_native_material() {
    let version = VersionId::from_random([1; 16]);
    let source = SnapshotHandle {
        publication: OperationId::from_random([2; 16]),
        repository_id: SnapshotRepositoryId::from_random([3; 16]),
        branch_id: SnapshotBranchId::from_random([4; 16]),
        node_id: SnapshotNodeId::from_random([5; 16]),
    };
    let selected = SelectedDirectoryDraft {
        target: DraftLocation {
            repository: RepositoryId::from_random([6; 16]),
            path: PortablePath::parse("named.txt").unwrap(),
            entry: EntryId::from_random([7; 16]), file: FileId::from_random([8; 16]),
        },
        expected: DirectoryDraftExpectation::Born {
            revision: WorkingRevision::new(Generation::new(9)),
            base: DraftBase::new(BranchId::from_random([10; 16]), FileNodeId::Version(version),
                version, Generation::new(11)),
            payload: DraftPayload::new(DraftPayloadId::from_random([12; 16]),
                ContentHash::from_digest(blake3::hash(b"bounded body")), 12),
        },
    };
    let request = CommitDirectoryDrafts {
        source, selected: vec![selected.clone()], message: "explicit".into(),
        operation_id: OperationId::from_random([13; 16]),
    };
    let bytes = crate::canonical_json(&request).unwrap();
    let decoded: CommitDirectoryDrafts = crate::decode_strict(&bytes).unwrap();
    assert_eq!(decoded, request);
    assert_eq!(decoded.selected[0].selection(), selected.selection());
    let mut value: json_codec::Value = json_codec::from_slice(&bytes).unwrap();
    value["principal"] = json_codec::json!("untrusted-author");
    assert!(crate::decode_strict::<CommitDirectoryDrafts>(
        &json_codec::to_vec(&value).unwrap()).is_err());
    let mut value: json_codec::Value = json_codec::from_slice(&bytes).unwrap();
    value["selected"][0]["expected"] = json_codec::json!({
        "revision":"9", "content":{"state":"empty"},
    });
    let decoded = crate::decode_strict::<CommitDirectoryDrafts>(
        &json_codec::to_vec(&value).unwrap()).unwrap();
    assert!(matches!(decoded.selected[0].selection().expected.content,
        WorkingContentExpectation::UnbornEmpty));
    value["selected"][0]["expected"]["base"] = json_codec::json!({});
    assert!(crate::decode_strict::<CommitDirectoryDrafts>(
        &json_codec::to_vec(&value).unwrap()).is_err());
    let mut value: json_codec::Value = json_codec::from_slice(&bytes).unwrap();
    value["selected"][0]["expected"]["payload"]["inode"] = json_codec::json!("123");
    assert!(crate::decode_strict::<CommitDirectoryDrafts>(
        &json_codec::to_vec(&value).unwrap()).is_err());

    for result in [
        DirectoryDraftOutcome::Changed { snapshot: source },
        DirectoryDraftOutcome::Unchanged { snapshot: source },
    ] {
        // This is a response-shape test, not a fabricated publication oracle.
        let projection = DirectoryDraftReceipt {
            operation_id: request.operation_id, publication_sequence: Sequence::new(20),
            commit_state: CommitState::Published,
            protection_at_publish: ProtectionAtPublish::NotApplicable, result,
        };
        let value = json_codec::to_value(&projection).unwrap();
        let keys: BTreeSet<_> = value.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, BTreeSet::from([
            "commit_state", "operation_id", "protection_at_publish",
            "publication_sequence", "result",
        ]));
        assert_eq!(value["result"].as_object().unwrap().len(), 2);
        assert_eq!(value["result"]["snapshot"].as_object().unwrap().len(), 4);
        assert_eq!(crate::decode_strict::<DirectoryDraftReceipt>(
            &crate::canonical_json(&projection).unwrap()).unwrap(), projection);
    }
}
