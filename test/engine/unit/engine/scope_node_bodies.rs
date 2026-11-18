use super::*;
use crate::working_tree::snapshot::SnapshotFileSource;
use name_policy::PortablePath;
use storage_contract::{BranchId, FileId, FileNodeId, working_tree::{SnapshotNodeId, SnapshotRepositoryId}};

fn material(id: u8) -> SnapshotFile {
    SnapshotFile {
        file_id: FileId::from_random([3; 16]), version_id: VersionId::from_random([id; 16]),
        size: 3, content_hash: ContentHash::from_digest(blake3::hash(b"one")),
    }
}
fn occurrence(path: &str, file: SnapshotFile, branch: u8) -> SnapshotEntry {
    SnapshotEntry {
        path: PortablePath::parse(path).unwrap(),
        content: SnapshotEntryContent::File {
            material: file,
            source: SnapshotFileSource::Container {
                branch: BranchId::from_random([branch; 16]), node: FileNodeId::Version(file.version_id),
            },
        },
    }
}
fn manifest(node: u8, entries: Vec<SnapshotEntry>) -> AdmittedSnapshotManifest {
    AdmittedSnapshotManifest::prepare(
        SnapshotRepositoryId::from_random([1; 16]), SnapshotNodeId::from_random([node; 16]),
        OperationId::from_random([9; 16]), entries,
    ).unwrap()
}
fn observed(m: &AdmittedSnapshotManifest) -> BTreeMap<VersionId, BodyLeaf> {
    m.files().iter().enumerate().map(|(i, (&version, &file))| (version, BodyLeaf {
        logical: file, operation: m.operation_id(), name: layout::file(&m.payloads()[&version]),
        physical: OriginalLeaf::Present { entity: LocalEntityIdentity::new(3, i as u64 + 4) },
    })).collect()
}

#[test]
fn alias_occurrences_and_original_node_are_not_replaced_by_one_version_or_one_digest() {
    let file = material(4);
    let entries = vec![occurrence("a", file, 10), occurrence("b", file, 11), SnapshotEntry {
        path: PortablePath::parse("empty").unwrap(), content: SnapshotEntryContent::Directory {},
    }];
    let first = manifest(2, entries.clone());
    let other = manifest(7, entries.clone());
    let source = NodeBodies::from_owner(&first, observed(&first)).unwrap();
    let rebound = NodeBodies::from_owner(&other, observed(&other)).unwrap();
    assert_eq!(source.leaves.len(), 1);
    assert_eq!(source.pages.entries(), 2);
    assert_ne!(source.pages.root(), rebound.pages.root());
    let changed = manifest(2, vec![occurrence("a", file, 10), occurrence("b", file, 12), entries[2].clone()]);
    assert_ne!(source.pages.root(), NodeBodies::from_owner(&changed, observed(&changed)).unwrap().pages.root());
}

#[test]
fn missing_wrong_logical_and_hardlinked_distinct_version_leaves_are_not_node_closure() {
    let m = manifest(2, vec![occurrence("a", material(4), 10), occurrence("b", material(5), 11)]);
    let leaves = observed(&m);
    let a = material(4).version_id;
    let b = material(5).version_id;
    let mut missing = leaves.clone();
    missing.remove(&a);
    assert!(NodeBodies::from_owner(&m, missing).is_err());
    let mut wrong = leaves.clone();
    wrong.get_mut(&a).unwrap().logical.size += 1;
    assert!(NodeBodies::from_owner(&m, wrong).is_err());
    let mut alias = leaves.clone();
    alias.get_mut(&b).unwrap().physical = leaves[&a].physical.clone();
    assert!(NodeBodies::from_owner(&m, alias).is_err());
    let mut present = leaves;
    present.get_mut(&b).unwrap().physical = OriginalLeaf::ReservedMove {
        entity: LocalEntityIdentity::new(3, 8), source_parent: LocalEntityIdentity::new(3, 0),
        source_name: EntryName::new("only-a-name").unwrap(),
    };
    assert!(NodeBodies::from_owner(&m, present).is_err());
}
