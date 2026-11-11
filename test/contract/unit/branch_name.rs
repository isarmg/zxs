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

#[test]
fn branch_name_wire_preserves_exact_string_bytes_and_collision_key() {
    for (wire, name, key) in [
        ("\"Main\"", "Main", "main"),
        ("\"Café\"", "Café", "café"),
        ("\"Straße\"", "Straße", "strasse"),
    ] {
        let decoded: BranchName = json_codec::from_str(wire).unwrap();
        assert_eq!(decoded.as_str(), name);
        assert_eq!(decoded.key(), key);
        assert_eq!(json_codec::to_string(&decoded).unwrap().as_bytes(), wire.as_bytes());
    }
}

#[test]
fn branch_name_wire_rejects_nonstring_and_invalid_creation_labels() {
    for wire in [
        "null", "true", "1", "{}", "[]", "\"\"", "\"CON.txt\"",
        "\"bad/label\"", "\"name \"", "\"Cafe\\u0301\"",
    ] {
        assert!(json_codec::from_str::<BranchName>(wire).is_err(), "{wire}");
    }
    let oversized = json_codec::to_string(&"é".repeat(65)).unwrap();
    assert!(json_codec::from_str::<BranchName>(&oversized).is_err());
}
