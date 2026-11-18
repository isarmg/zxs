use super::{DirectoryId, DiskRegistrationIdentity, LocalDirectoryBinding, LocalEntityIdentity, RootId};

#[test]
fn ordinary_directory_entity_has_one_strict_location_free_encoding() {
    let id = DirectoryId::from_random([17; 16]);
    let binding = LocalDirectoryBinding { directory_id: id, entity: LocalEntityIdentity::new(12, 34) };
    let bytes = concat!(
        r#"{"directory_id":"directory_11111111111111111111111111111111","#,
        r#""entity":{"device":"12","inode":"34"}}"#,
    );
    assert_eq!(crate::canonical_json(&binding).unwrap(), bytes.as_bytes());
    assert_eq!(crate::decode_strict::<LocalDirectoryBinding>(bytes.as_bytes()).unwrap(), binding);
    assert!(id.to_string().parse::<RootId>().is_err());

    for malformed in [
        bytes.replace(r#""device":"12""#, r#""device":12"#),
        bytes.replace(r#""inode":"34""#, r#""inode":"034""#),
        bytes.replace(r#""entity":{"#, r#""snapshot_repositories":[],"entity":{"#),
        bytes.replace(r#""entity":{"#, r#""entity":{"path":"/disk","#),
    ] {
        assert!(crate::decode_strict::<LocalDirectoryBinding>(malformed.as_bytes()).is_err());
    }
}

#[test]
fn registration_does_not_turn_a_missing_or_extra_disk_binding_into_a_default() {
    let bytes = r#"{"binding_generation":"1","disk_id":"disk","pool_id":"pool","slot_id":"slot"}"#;
    let actual = crate::decode_strict::<DiskRegistrationIdentity>(bytes.as_bytes()).unwrap();
    assert_eq!(actual.binding_generation, 1);
    assert_eq!(crate::canonical_json(&actual).unwrap(), bytes.as_bytes());
    for malformed in [
        bytes.replace(r#""binding_generation":"1","#, ""),
        bytes.replace(r#""disk_id":"disk""#, r#""disk_id":"disk","replicas":[]"#),
    ] {
        assert!(crate::decode_strict::<DiskRegistrationIdentity>(malformed.as_bytes()).is_err());
    }
}
