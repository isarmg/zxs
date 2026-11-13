use crate::{VersionId, VersionStorageId};

#[test]
fn storage_identity_is_strict_ascii_and_keeps_its_canonical_wire_bytes() {
    for valid in ["0123456789abcde", "abcdefghijklmno", "000000000000000"] {
        let id: VersionStorageId = valid.parse().unwrap();
        assert_eq!(id.as_str(), valid);
        let bytes = crate::canonical_json(&id).unwrap();
        assert_eq!(bytes, format!("\"{valid}\"").as_bytes());
        assert_eq!(crate::decode_strict::<VersionStorageId>(&bytes).unwrap(), id);
    }
    for invalid in ["", "0123456789abcd", "0123456789abcdef", "0123456789ABCD5",
        "0123456789abcd-", "0123456789abcd/", "0123456789abcé"]
    {
        assert!(invalid.parse::<VersionStorageId>().is_err());
    }
}

#[test]
fn storage_identity_is_stable_for_a_version_and_does_not_encode_a_filename() {
    let first = VersionId::from_random([1; 16]);
    let second = VersionId::from_random([2; 16]);
    assert_eq!(VersionStorageId::from_version(first), VersionStorageId::from_version(first));
    assert_ne!(VersionStorageId::from_version(first), VersionStorageId::from_version(second));
    assert_eq!(VersionStorageId::from_version(first).as_str().len(), 15);
}

#[test]
fn installed_leaf_binds_strict_identity_to_current_name_without_spending_prefix_budget() {
    use crate::validate_installed_version_name;
    for name in ["a-b.txt".to_string(), "x".repeat(240), "界".repeat(80)] {
        assert!(validate_installed_version_name(&format!("000000000000001-{name}"), &name).is_ok());
    }
    for stored in [
        "v000001_old.txt", "00000000000001-file.txt", "0000000000000001-file.txt",
        "00000000000000A-file.txt", "000000000000001-old.txt", "000000000000001-../file.txt",
    ] {
        assert!(validate_installed_version_name(stored, "file.txt").is_err());
    }
    let oversized = "x".repeat(241);
    assert!(validate_installed_version_name(&format!("000000000000001-{oversized}"), &oversized).is_err());
}
