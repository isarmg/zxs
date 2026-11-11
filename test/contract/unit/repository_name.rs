use super::*;
#[test]
fn repository_names_keep_full_portable_limits_and_cached_unicode_keys() {
    for (name, key) in [("Café", "café"), ("Straße", "strasse")] {
        let parsed = RepositoryName::parse(name).unwrap();
        assert_eq!(parsed.as_str(), name);
        assert_eq!(parsed.key(), key);
    }
    assert!(RepositoryName::parse(&"a".repeat(240)).is_ok());
    assert!(RepositoryName::parse(&"é".repeat(120)).is_ok());
    for invalid in [
        "a".repeat(241),
        "é".repeat(121),
        "Cafe\u{301}".into(),
        "CON.txt".into(),
        "../bad".into(),
    ] {
        let source = name_policy::validate_name(&invalid).unwrap_err();
        let error = RepositoryName::parse(&invalid).unwrap_err();
        assert_eq!(error.code(), crate::Error::from(source).code());
        assert!(std::error::Error::source(&error).is_some());
    }
}

#[test]
fn naming_scalars_encode_original_bytes_and_reject_invalid_wire_values() {
    for raw in ["Café", "Straße", "报告-2026.docx"] {
        let bytes = json_codec::to_vec(raw).unwrap();
        let name: name_policy::Name = json_codec::from_slice(&bytes).unwrap();
        assert_eq!(json_codec::to_vec(&name).unwrap(), bytes);
        let lookup: name_policy::LookupName = json_codec::from_slice(&bytes).unwrap();
        assert_eq!(json_codec::to_vec(&lookup).unwrap(), bytes);
    }
    let bytes = json_codec::to_vec("Cafe\u{301}").unwrap();
    let lookup: name_policy::LookupName = json_codec::from_slice(&bytes).unwrap();
    assert_eq!(json_codec::to_vec(&lookup).unwrap(), bytes);
    assert!(json_codec::from_slice::<name_policy::Name>(&bytes).is_err());
    for raw in ["A", "Disk_01-X"] {
        let bytes = json_codec::to_vec(raw).unwrap();
        let slot: name_policy::SlotId = json_codec::from_slice(&bytes).unwrap();
        assert_eq!(json_codec::to_vec(&slot).unwrap(), bytes);
    }
    for raw in ["", "CON.txt", "a/b"] {
        let bytes = json_codec::to_vec(raw).unwrap();
        assert!(json_codec::from_slice::<name_policy::Name>(&bytes).is_err());
        assert!(json_codec::from_slice::<name_policy::LookupName>(&bytes).is_err());
    }
    assert!(json_codec::from_str::<name_policy::SlotId>("\"0disk\"").is_err());
}
