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
