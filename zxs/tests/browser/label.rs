use super::{Label, MAX};

#[test]
fn exact_text_and_canonical_route_bytes() {
    for name in ["热处理", "Nickel", "nickel", "A/B & C", "加 工", " ends "] {
        let value = Label::parse(name).unwrap();
        assert_eq!(value.text(), name);
        assert_eq!(Label::from_hex(&value.encoded()), Some(value));
    }
    assert_ne!(Label::parse("Nickel"), Label::parse("nickel"));
    assert_ne!(Label::parse("é"), Label::parse("e\u{301}"));
    assert!(Label::parse(&"a".repeat(MAX)).is_some());
    assert!(Label::parse(&"a".repeat(MAX + 1)).is_none());
    for s in ["", " ", "line\nbreak", "nul\0", "break\u{2028}here", "break\u{2029}here"] {
        assert!(Label::parse(s).is_none());
    }
    for s in ["1", "aA", "e8ac", "zz", "00"] {
        assert!(Label::from_hex(s).is_none());
    }
}
