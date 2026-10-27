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

#[test]
fn parsed_notes_keep_empty_spaces_and_non_nfc_text_without_implicit_normalization() {
    for value in [
        String::new(),
        "  ".into(),
        "Cafe\u{301}".into(),
        "汉".repeat(42) + "ab",
    ] {
        assert_eq!(Note::parse(value.clone()).unwrap().as_str(), value);
    }
    for value in [
        "x".repeat(129),
        "汉".repeat(43),
        "bad\ntext".into(),
        "\u{0085}".into(),
    ] {
        assert_eq!(
            Note::parse(value).unwrap_err().code(),
            crate::ErrorCode::InvalidRequest
        );
    }
}
