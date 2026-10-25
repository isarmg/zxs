use super::*;

#[test]
fn notes_share_byte_control_and_clear_semantics() {
    for note in [String::new(), "a".repeat(128), "汉".repeat(42) + "ab"] {
        validate_note(&note).unwrap();
    }
    for note in [
        "a".repeat(129),
        "汉".repeat(43),
        "line\nbreak".into(),
        "tab\there".into(),
        "\u{007f}".into(),
        "\u{0085}".into(),
    ] {
        assert_eq!(
            validate_note(&note).unwrap_err().code(),
            crate::ErrorCode::InvalidRequest
        );
    }
}
