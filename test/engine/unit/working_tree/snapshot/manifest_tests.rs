use crate::working_tree::snapshot::manifest::*;

fn repository() -> SnapshotRepositoryId {
    SnapshotRepositoryId::from_random([1; 16])
}
fn node() -> SnapshotNodeId {
    SnapshotNodeId::from_random([2; 16])
}
fn file(body: &[u8]) -> SnapshotFile {
    SnapshotFile {
        file_id: FileId::from_random([4; 16]),
        version_id: VersionId::from_random([5; 16]),
        size: body.len() as u64,
        content_hash: ContentHash::from_digest(blake3::hash(body)),
    }
}
fn entry(path: &str, content: SnapshotEntryContent) -> SnapshotEntry {
    SnapshotEntry { path: PortablePath::parse(path).unwrap(), content }
}
fn prepare(entries: Vec<SnapshotEntry>) -> Result<AdmittedSnapshotManifest, SnapshotManifestError> {
    AdmittedSnapshotManifest::prepare(
        repository(), node(), OperationId::from_random([3; 16]), entries,
    )
}
fn entries() -> Vec<SnapshotEntry> {
    vec![
        entry("empty", SnapshotEntryContent::Directory {}),
        entry("folder", SnapshotEntryContent::Directory {}),
        entry("folder/file.txt", SnapshotEntryContent::File(file(b"snapshot"))),
    ]
}

#[test]
fn complete_manifest_preserves_empty_directories_ids_bytes_and_exact_spelling() {
    let admitted = prepare(entries()).unwrap();
    let decoded = AdmittedSnapshotManifest::decode(admitted.bytes(), admitted.reference()).unwrap();
    assert_eq!(decoded.entries(), entries());
    assert_eq!(decoded.repository_id(), repository());
    assert_eq!(decoded.node_id(), node());
    assert_eq!(decoded.bytes(), admitted.bytes());
    assert_eq!(decoded.commitment(), admitted.commitment());
    assert_eq!(decoded.files().len(), 1);
    assert_eq!(decoded.files()[&file(b"snapshot").version_id], file(b"snapshot"));
    let empty = prepare(vec![]).unwrap();
    assert!(empty.entries().is_empty());
    assert!(empty.files().is_empty());
}

#[test]
fn independent_commitment_and_selected_node_identity_cannot_be_self_attested() {
    let admitted = prepare(entries()).unwrap();
    let mut expected = admitted.reference();
    expected.manifest = MaterialCommitment::from_digest(blake3::hash(b"forged"));
    assert!(matches!(AdmittedSnapshotManifest::decode(admitted.bytes(), expected),
        Err(SnapshotManifestError::Commitment)));
    expected = admitted.reference();
    expected.repository = SnapshotRepositoryId::from_random([8; 16]);
    assert!(matches!(AdmittedSnapshotManifest::decode(admitted.bytes(), expected),
        Err(SnapshotManifestError::Identity)));
    expected = admitted.reference();
    expected.node = SnapshotNodeId::from_random([8; 16]);
    assert!(matches!(AdmittedSnapshotManifest::decode(admitted.bytes(), expected),
        Err(SnapshotManifestError::Identity)));
}

#[test]
fn complete_parent_closure_rejects_omission_files_and_case_aliases() {
    let item = entry("folder/file.txt", SnapshotEntryContent::File(file(b"data")));
    assert!(matches!(prepare(vec![item.clone()]), Err(SnapshotManifestError::Parent)));
    assert!(matches!(prepare(vec![
        entry("folder", SnapshotEntryContent::File(file(b"data"))), item.clone(),
    ]), Err(SnapshotManifestError::Parent)));
    assert!(matches!(prepare(vec![
        entry("FOLDER", SnapshotEntryContent::Directory {}), item,
    ]), Err(SnapshotManifestError::Parent)));
    assert!(matches!(prepare(vec![
        entry("a", SnapshotEntryContent::Directory {}),
        entry("A", SnapshotEntryContent::Directory {}),
    ]), Err(SnapshotManifestError::Order)));
    let mut reversed = entries();
    reversed.reverse();
    assert!(prepare(reversed).is_err());
}

#[test]
fn protected_management_never_becomes_snapshot_user_content() {
    for path in [".zxs", ".ZxS"] {
        assert!(matches!(prepare(vec![entry(path, SnapshotEntryContent::Directory {})]),
            Err(SnapshotManifestError::Management)));
    }
    assert!(matches!(prepare(vec![
        entry("a", SnapshotEntryContent::Directory {}),
        entry("a/.ZXS", SnapshotEntryContent::Directory {}),
    ]), Err(SnapshotManifestError::Management)));
}

#[test]
fn version_aliases_share_one_full_payload_but_count_every_logical_occurrence() {
    let common = file(b"same version");
    let admitted = prepare(vec![
        entry("a", SnapshotEntryContent::File(common)),
        entry("b", SnapshotEntryContent::File(common)),
    ]).unwrap();
    assert_eq!(admitted.entries().len(), 2);
    assert_eq!(admitted.files().len(), 1);
    let mut conflicting = common;
    conflicting.file_id = FileId::from_random([9; 16]);
    assert!(matches!(prepare(vec![
        entry("a", SnapshotEntryContent::File(common)),
        entry("b", SnapshotEntryContent::File(conflicting)),
    ]), Err(SnapshotManifestError::Version)));
    let mut large = common;
    large.size = MAX_LOGICAL_BYTES;
    assert!(matches!(prepare(vec![
        entry("a", SnapshotEntryContent::File(large)),
        entry("b", SnapshotEntryContent::File(large)),
    ]), Err(SnapshotManifestError::Budget)));
    large.size = u64::MAX;
    assert!(matches!(prepare(vec![entry("a", SnapshotEntryContent::File(large))]),
        Err(SnapshotManifestError::Budget)));
}

#[test]
fn strict_manifest_has_no_old_format_missing_field_or_noncanonical_fallback() {
    let admitted = prepare(entries()).unwrap();
    let text = String::from_utf8(admitted.bytes().to_vec()).unwrap();
    for bytes in [
        text.replace("full_directory_snapshot", "directory_git").into_bytes(),
        text.replace("\"full_directory_snapshot\"", "{\"full_directory_snapshot\":null}")
            .into_bytes(),
        format!("{text} ").into_bytes(),
        format!("{{\"unknown\":true,{}", &text[1..]).into_bytes(),
        format!("{{\"entries\":[],{}", &text[1..]).into_bytes(),
        b"{}".to_vec(),
    ] {
        let expected = SnapshotReference { manifest: commitment(&bytes), ..admitted.reference() };
        assert!(AdmittedSnapshotManifest::decode(&bytes, expected).is_err());
    }
    assert!(matches!(AdmittedSnapshotManifest::decode(
        &vec![b' '; MAX_MANIFEST_BYTES + 1], admitted.reference(),
    ), Err(SnapshotManifestError::Budget)));
}

#[test]
fn short_ids_are_bound_addresses_and_collision_cannot_merge_full_version_identity() {
    let first = file(b"one");
    let mut second = file(b"two");
    second.version_id = VersionId::from_random([6; 16]);
    let admitted = prepare(vec![
        entry("one.txt", SnapshotEntryContent::File(first)),
        entry("two.txt", SnapshotEntryContent::File(second)),
    ]).unwrap();
    let one = &admitted.payloads()[&first.version_id];
    assert_eq!(one.storage_id.as_str().len(), 15);
    assert_eq!(layout::snapshot::file(one).as_os_str().to_str().unwrap(),
        format!("{}-one.txt", one.storage_id));
    let mut record = admitted.record.clone();
    record.payloads[1].storage_id = record.payloads[0].storage_id;
    let bytes = storage_contract::canonical_json(&record).unwrap();
    let expected = SnapshotReference { manifest: commitment(&bytes), ..admitted.reference() };
    assert!(matches!(AdmittedSnapshotManifest::decode(&bytes, expected),
        Err(SnapshotManifestError::StorageIdentity)));
}

#[test]
fn payload_mapping_requires_full_inventory_and_original_alias_spelling() {
    let common = file(b"shared");
    let admitted = prepare(vec![
        entry("first.txt", SnapshotEntryContent::File(common)),
        entry("second.txt", SnapshotEntryContent::File(common)),
    ]).unwrap();
    assert_eq!(admitted.payloads().len(), 1);
    assert_eq!(admitted.payloads()[&common.version_id].source_path.as_str(), "first.txt");
    let mut record = admitted.record.clone();
    record.payloads.clear();
    assert!(matches!(
        AdmittedSnapshotManifest::admit(record), Err(SnapshotManifestError::Payloads),
    ));
    let mut record = admitted.record.clone();
    record.payloads[0].source_path = PortablePath::parse("second.txt").unwrap();
    assert!(matches!(
        AdmittedSnapshotManifest::admit(record), Err(SnapshotManifestError::Payloads),
    ));
    let long = "a".repeat(240);
    assert!(matches!(prepare(vec![entry(&long, SnapshotEntryContent::File(common))]),
        Err(SnapshotManifestError::Filename)));
}
