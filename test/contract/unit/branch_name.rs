use super::*;
#[test]
fn branch_names_bind_nfc_display_to_the_fixed_collision_key() {
    for (name, key) in [("Main", "main"), ("Café", "café"), ("Straße", "strasse")] {
        let parsed = BranchName::parse(name).unwrap();
        assert_eq!(parsed.as_str(), name);
        assert_eq!(parsed.key(), key);
        assert!(validate_branch_name(name).is_ok());
    }
    assert!(BranchName::parse(&"é".repeat(64)).is_ok());
    assert!(BranchName::parse(&"x".repeat(128)).is_ok());
    for name in [
        "x".repeat(129),
        "é".repeat(65),
        format!("{}e\u{301}", "x".repeat(128)),
    ] {
        let error = BranchName::parse(&name).unwrap_err();
        assert_eq!(error.code(), ErrorCode::MetadataTooLong);
        assert_eq!(
            error.message(),
            "branch names must not exceed 128 UTF-8 bytes"
        );
    }
    for (name, code) in [
        ("", ErrorCode::NameInvalidCharacter),
        ("bad/label", ErrorCode::NameInvalidCharacter),
        ("name ", ErrorCode::NameInvalidCharacter),
        ("CON.txt", ErrorCode::NameReserved),
        ("Cafe\u{301}", ErrorCode::NameNotNormalized),
    ] {
        let error = BranchName::parse(name).unwrap_err();
        assert_eq!(error.code(), code);
        assert!(std::error::Error::source(&error).is_some());
        assert_eq!(validate_branch_name(name).unwrap_err().code(), code);
    }
}
