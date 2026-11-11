use crate::{EmptyPrincipal, Principal, canonical_json};

#[test]
fn principal_preserves_nonempty_identity_text() {
    for (text, json) in [
        ("admin", r#""admin""#),
        (" ", r#"" ""#),
        (" \t\n ", r#"" \t\n ""#),
        ("用户/É/e\u{301}/😀", "\"用户/É/e\u{301}/😀\""),
        ("\0", r#""\u0000""#),
        ("quote\"\\", r#""quote\"\\""#),
    ] {
        let principal: Principal = text.parse().unwrap();
        assert_eq!(principal.as_str(), text);
        assert_eq!(principal.to_string(), text);
        assert_eq!(Principal::new(text.into()).unwrap(), principal);
        assert_eq!(canonical_json(&principal).unwrap(), json.as_bytes());
        assert_eq!(json_codec::from_str::<Principal>(json).unwrap(), principal);
    }
    assert_ne!(
        "é".parse::<Principal>().unwrap(),
        "e\u{301}".parse::<Principal>().unwrap()
    );
}

#[test]
fn principal_requires_nonempty_string_input() {
    assert_eq!(Principal::new("".into()), Err(EmptyPrincipal));
    assert_eq!("".parse::<Principal>(), Err(EmptyPrincipal));
    for bytes in [r#""""#, "null", "0", "true", "[]", "{}"] {
        assert!(json_codec::from_str::<Principal>(bytes).is_err());
    }
}
