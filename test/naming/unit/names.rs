use super::*;

#[test]
fn creation_and_lookup_names_retain_admitted_spelling() {
    for raw in ["报告-2026.docx", ".gitignore", "Straße", "Café"] {
        let name = Name::parse(raw).unwrap();
        let lookup = LookupName::from(&name);
        assert_eq!(name.as_str(), raw);
        assert_eq!(name.to_string(), raw);
        assert_eq!(lookup.as_str(), raw);
        assert_eq!(lookup.key(), name.key());
        assert_eq!(Name::try_from(lookup).unwrap(), name);
    }
    let raw = "Cafe\u{301}";
    let lookup = LookupName::parse(raw).unwrap();
    assert_eq!(lookup.as_str(), raw);
    assert_eq!(lookup.key(), "café");
    assert_eq!(
        Name::try_from(lookup),
        Err(Error::NotNormalized("Café".into()))
    );
}

#[test]
fn name_constructors_preserve_limits_and_error_precedence() {
    for (letter, count) in [("a", 240), ("界", 80), ("𐐀", 60)] {
        let raw = letter.repeat(count);
        assert_eq!(Name::parse(&raw).unwrap().as_str(), raw);
        assert_eq!(LookupName::parse(&raw).unwrap().as_str(), raw);
        let oversized = letter.repeat(count + 1);
        assert_eq!(Name::parse(&oversized), Err(Error::NameTooLong));
        assert_eq!(LookupName::parse(&oversized), Err(Error::NameTooLong));
    }
    for (raw, expected) in [
        ("", Error::EmptyOrDot),
        ("..", Error::EmptyOrDot),
        (" name", Error::NameEdge),
        ("name.", Error::NameEdge),
        ("CON.txt", Error::ReservedName),
        ("COM¹.txt", Error::ReservedName),
        ("a/b", Error::UnsupportedCharacter('/' as u32)),
    ] {
        assert_eq!(Name::parse(raw).unwrap_err(), expected);
        assert_eq!(LookupName::parse(raw).unwrap_err(), expected);
    }
    let oversized = format!(" {}", "a".repeat(240));
    assert_eq!(Name::parse(&oversized), Err(Error::NameTooLong));
    assert_eq!(LookupName::parse(&oversized), Err(Error::NameTooLong));
}

#[test]
fn slot_constructor_preserves_ascii_case_and_exact_budget() {
    for raw in ["A", "a_0-Z", &format!("A{}", "z".repeat(63))] {
        let slot: SlotId = raw.parse().unwrap();
        assert_eq!(slot.as_str(), raw);
        assert_eq!(slot.to_string(), raw);
    }
    for raw in ["", "0a", "_a", "-a", "a/b", "é", "a b", &"A".repeat(65)] {
        assert_eq!(SlotId::parse(raw), Err(Error::InvalidSlot));
    }
    assert_ne!(SlotId::parse("A").unwrap(), SlotId::parse("a").unwrap());
}

#[test]
fn typed_path_components_keep_display_keys_and_aggregate_limits() {
    let name = Name::parse("Straße").unwrap();
    let root = PortablePath::from_name(&name).unwrap();
    assert_eq!(root.as_str(), "Straße");
    assert_eq!(root.key(), "strasse");
    let parent = PortablePath::parse("Café").unwrap();
    let child = parent.join_name(&name).unwrap();
    assert_eq!(child.as_str(), "Café/Straße");
    assert_eq!(child.key(), "café/strasse");

    let at_limit = Name::parse(&"a".repeat(200)).unwrap();
    assert_eq!(PortablePath::from_name(&at_limit).unwrap().as_str().len(), 200);
    let too_long = Name::parse(&"a".repeat(201)).unwrap();
    assert_eq!(PortablePath::from_name(&too_long), Err(Error::InvalidPath));
    let parent = PortablePath::parse(&"a".repeat(198)).unwrap();
    let one = Name::parse("b").unwrap();
    assert_eq!(parent.join_name(&one).unwrap().as_str().len(), 200);
    assert_eq!(
        parent.join_name(&Name::parse("bb").unwrap()),
        Err(Error::InvalidPath)
    );
    let parent = PortablePath::parse(&vec!["a"; 31].join("/")).unwrap();
    let deepest = parent.join_name(&one).unwrap();
    assert_eq!(deepest.as_str().split('/').count(), 32);
    assert_eq!(deepest.join_name(&one), Err(Error::PathTooDeep));
}
