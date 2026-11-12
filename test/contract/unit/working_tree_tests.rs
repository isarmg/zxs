use crate::working_tree::{
    DirectoryId, DirectoryRelativePath, DirectoryTargetPath, ReplicaId, ReplicaPurpose,
    RootId, SnapshotRepositoryId, StorageLayout, WorkingTreeCatalog,
};

#[test]
fn working_tree_identity_domains_preserve_full_identity_and_order() {
    let first = RootId::from_random([1; 16]);
    let second = RootId::from_random([2; 16]);
    assert!(first < second);
    assert!(first.to_string() < second.to_string());
    assert_eq!(first.to_string().parse::<RootId>().unwrap(), first);
    assert!(first.to_string().parse::<DirectoryId>().is_err());
    assert!(first.to_string().parse::<ReplicaId>().is_err());
    assert!(first.to_string().parse::<SnapshotRepositoryId>().is_err());
    assert_eq!(
        json_codec::from_slice::<RootId>(&crate::canonical_json(&first).unwrap()).unwrap(),
        first,
    );
    assert!("directory_name".parse::<DirectoryId>().is_err());
    assert!("directory_00112233445566778899AABBCCDDEEFF".parse::<DirectoryId>().is_err());
}

#[test]
fn working_tree_layout_and_replica_purpose_require_exact_string_tokens() {
    assert_eq!(
        json_codec::from_str::<StorageLayout>("\"unified_working_tree_v1\"").unwrap(),
        StorageLayout::UnifiedWorkingTreeV1,
    );
    for bytes in [
        "null", "{}", "0", "\"managed_directory\"", "\"membership_generation\"",
        "{\"unified_working_tree_v1\":null}",
    ] {
        assert!(json_codec::from_str::<StorageLayout>(bytes).is_err(), "{bytes}");
    }
    for bytes in ["{\"current\":null}", "{\"backup\":null}", "null", "\"owner\""] {
        assert!(json_codec::from_str::<ReplicaPurpose>(bytes).is_err(), "{bytes}");
    }
}

#[test]
fn working_tree_catalog_requires_every_table_and_format_even_when_empty() {
    let empty = WorkingTreeCatalog {
        storage_layout: StorageLayout::UnifiedWorkingTreeV1,
        roots: vec![],
        directories: vec![],
        containers: vec![],
        snapshot_repositories: vec![],
        replicas: vec![],
    };
    let encoded = crate::canonical_json(&empty).unwrap();
    assert_eq!(json_codec::from_slice::<WorkingTreeCatalog>(&encoded).unwrap(), empty);
    for bytes in [
        "{}",
        concat!(
            r#"{"roots":[],"directories":[],"containers":[],"#,
            r#""snapshot_repositories":[],"replicas":[]}"#,
        ),
        concat!(
            r#"{"storage_layout":"unified_working_tree_v1","roots":[],"directories":[],"#,
            r#""containers":[],"replicas":[]}"#,
        ),
        concat!(
            r#"{"storage_layout":"unified_working_tree_v1","roots":[],"directories":[],"#,
            r#""containers":[],"snapshot_repositories":[],"replicas":[],"skip":true}"#,
        ),
    ] {
        assert!(json_codec::from_str::<WorkingTreeCatalog>(bytes).is_err());
    }
}

#[test]
fn directory_target_is_relative_bounded_and_does_not_require_directory_identity() {
    let path = DirectoryRelativePath::parse("Projects/notes.txt").unwrap();
    assert_eq!(path.components()[0].as_str(), "Projects");
    assert_eq!(path.components()[1].as_str(), "notes.txt");
    let target = DirectoryTargetPath::Relative(path);
    assert_eq!(
        json_codec::from_slice::<DirectoryTargetPath>(&crate::canonical_json(&target).unwrap())
            .unwrap(),
        target,
    );
    assert_eq!(
        json_codec::from_str::<DirectoryTargetPath>(r#"{"kind":"root"}"#).unwrap(),
        DirectoryTargetPath::Root,
    );
    for path in ["", "/a", "a/", "a//b", ".", "..", "a/../b", "a\\b", "a\0b"] {
        assert!(DirectoryRelativePath::parse(path).is_err(), "{path:?}");
    }
    assert!(DirectoryRelativePath::parse(&vec!["a"; 65].join("/")).is_err());
    assert!(DirectoryRelativePath::parse(&vec!["a".repeat(200); 21].join("/")).is_err());
    assert!(json_codec::from_str::<DirectoryRelativePath>("[]").is_err());
}

#[test]
fn working_tree_registration_promotion_preserves_existing_canonical_wire() {
    use crate::working_tree::DiskRegistrationIdentity;

    // The pre-existing disk_catalog_directory_tests identity, in canonical form.
    let bytes = concat!(
        r#"{"binding_generation":"1","disk_id":"disk_22222222222222222222222222222222","#,
        r#""pool_id":"pool_11111111111111111111111111111111","slot_id":"B01"}"#,
    ).as_bytes();
    let identity = crate::decode_strict::<DiskRegistrationIdentity>(bytes).unwrap();
    assert_eq!(identity.binding_generation, 1);
    assert_eq!(identity.slot_id, "B01");
    assert_eq!(crate::canonical_json(&identity).unwrap(), bytes);

    // The tuple remains a wire record, not a new ID admission boundary.
    let maximum = concat!(
        r#"{"binding_generation":"18446744073709551615","disk_id":"primary","#,
        r#""pool_id":"pool","slot_id":"primary"}"#,
    ).as_bytes();
    let identity = crate::decode_strict::<DiskRegistrationIdentity>(maximum).unwrap();
    assert_eq!(identity.binding_generation, u64::MAX);
    assert_eq!(crate::canonical_json(&identity).unwrap(), maximum);
}

#[test]
fn working_tree_registration_promotion_preserves_strict_fields_and_decimal_encoding() {
    use crate::working_tree::DiskRegistrationIdentity;

    for generation in ["0", "1", "18446744073709551615"] {
        let bytes = format!(
            "{{\"binding_generation\":\"{generation}\",\"disk_id\":\"disk\",\
            \"pool_id\":\"pool\",\"slot_id\":\"P01\"}}",
        );
        let identity = crate::decode_strict::<DiskRegistrationIdentity>(bytes.as_bytes()).unwrap();
        assert_eq!(crate::canonical_json(&identity).unwrap(), bytes.as_bytes());
    }
    for generation in ["1", "null", "\"01\"", "\"-1\"", "\"18446744073709551616\""] {
        let bytes = format!(
            "{{\"binding_generation\":{generation},\"disk_id\":\"disk\",\
            \"pool_id\":\"pool\",\"slot_id\":\"P01\"}}",
        );
        assert!(crate::decode_strict::<DiskRegistrationIdentity>(bytes.as_bytes()).is_err());
    }
    for bytes in [
        r#"{"pool_id":"pool","disk_id":"disk","slot_id":"P01"}"#,
        concat!(
            r#"{"pool_id":"pool","disk_id":"disk","slot_id":"P01","#,
            r#""binding_generation":"1","unknown":true}"#,
        ),
        concat!(
            r#"{"pool_id":"pool","disk_id":"disk","slot_id":"P01","#,
            r#""binding_generation":"1","binding_generation":"1"}"#,
        ),
    ] {
        assert!(crate::decode_strict::<DiskRegistrationIdentity>(bytes.as_bytes()).is_err());
    }
}
