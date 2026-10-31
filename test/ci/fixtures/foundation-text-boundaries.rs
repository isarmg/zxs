use writeable::Writeable;

#[test]
fn tiny_ascii_rejects_non_ascii_null_and_overflow() {
    use tinystr::TinyAsciiStr;
    let value = TinyAsciiStr::<8>::try_from_str("en-US").unwrap();
    assert_eq!(value.as_str(), "en-US");
    assert_eq!(value.to_ascii_lowercase().as_str(), "en-us");
    assert!(TinyAsciiStr::<2>::try_from_str("abc").is_err());
    assert!(TinyAsciiStr::<8>::try_from_str("中").is_err());
    assert!(TinyAsciiStr::<8>::try_from_str("a\0b").is_err());
}
#[test]
fn locale_parse_preserves_canonical_subtags_and_rejects_invalid_input() {
    let locale = icu_locale_core::Locale::try_from_utf8_with_single_variant_single_keyword_unicode_extension(b"zh-Hant-TW-u-ca-chinese").unwrap();
    assert_eq!(locale.1.unwrap().to_string(), "Hant");
    assert_eq!(locale.4.unwrap().0.to_string(), "ca");
    assert_eq!(locale.4.unwrap().1.unwrap().to_string(), "chinese");
    assert_eq!(locale.0.to_string(), "zh");
    assert_eq!(locale.2.unwrap().to_string(), "TW");
    assert!(icu_locale_core::Locale::try_from_utf8_with_single_variant_single_keyword_unicode_extension(b"en-@").is_err());
}
#[test]
fn provider_request_borrows_locale_and_keeps_error_context() {
    use icu_provider::{DataErrorKind, DataIdentifierBorrowed, DataLocale};
    let locale: DataLocale = "fr-CA".parse().unwrap();
    let id = DataIdentifierBorrowed::for_locale(&locale);
    assert!(std::ptr::eq(id.locale, &locale));
    assert_eq!(id.to_string(), "fr-CA");
    let error = DataErrorKind::IdentifierNotFound.with_str_context("localized-title");
    assert_eq!(error.kind, DataErrorKind::IdentifierNotFound);
    assert_eq!(error.str_context, Some("localized-title"));
}
#[test]
fn writeable_reports_exact_length_and_sink_failure() {
    let value = 12345_u32;
    let mut output = String::new();
    value.write_to(&mut output).unwrap();
    assert_eq!(output, "12345");
    assert_eq!(
        value.writeable_length_hint(),
        writeable::LengthHint::exact(5)
    );
    struct Reject;
    impl std::fmt::Write for Reject {
        fn write_str(&mut self, _: &str) -> std::fmt::Result {
            Err(std::fmt::Error)
        }
    }
    assert!(value.write_to(&mut Reject).is_err());
}
#[test]
fn zerovec_reads_unaligned_little_endian_and_rejects_partial_element() {
    let bytes = [99, 1, 0, 0, 0, 2, 0, 0, 0];
    let values = zerovec::ZeroVec::<u32>::parse_bytes(&bytes[1..]).unwrap();
    assert_eq!(values.get(0), Some(1));
    assert_eq!(values.get(1), Some(2));
    assert_eq!(values.get(2), None);
    assert!(zerovec::ZeroVec::<u32>::parse_bytes(&bytes[1..8]).is_err());
}
#[test]
fn potential_utf_preserves_bytes_and_rejects_invalid_scalar() {
    use potential_utf::{PotentialCodePoint, PotentialUtf8};
    let bytes = "版本".as_bytes();
    let text = PotentialUtf8::from_bytes(bytes);
    assert_eq!(text.try_as_str().unwrap(), "版本");
    assert_eq!(text.as_bytes().as_ptr(), bytes.as_ptr());
    assert!(PotentialUtf8::from_bytes(&[0xff]).try_as_str().is_err());
    assert!(PotentialCodePoint::from_u24(0xd800).try_to_char().is_err());
    assert_eq!(
        PotentialCodePoint::from_u24(0xd800).to_char_lossy(),
        '\u{fffd}'
    );
}
#[test]
fn unicode_identifier_boundary_classifies_start_and_continue() {
    assert!(unicode_ident::is_xid_start('版'));
    assert!(!unicode_ident::is_xid_start('9'));
    assert!(unicode_ident::is_xid_continue('9'));
    assert!(unicode_ident::is_xid_continue('\u{301}'));
    assert!(!unicode_ident::is_xid_start('\u{301}'));
    assert!(!unicode_ident::is_xid_continue(' '));
}
#[test]
fn grapheme_iteration_keeps_combining_marks_and_emoji_clusters() {
    use unicode_segmentation::UnicodeSegmentation;
    let source = "a\u{301}👩‍💻中";
    assert_eq!(
        source.graphemes(true).collect::<Vec<_>>(),
        ["a\u{301}", "👩‍💻", "中"]
    );
    assert_eq!(
        source
            .grapheme_indices(true)
            .map(|(n, _)| n)
            .collect::<Vec<_>>(),
        [0, 3, 14]
    );
}
#[test]
fn case_conversion_preserves_word_boundaries() {
    use convert_case::{Case, Casing};
    assert_eq!("HTTPServer2Name".to_case(Case::Snake), "http_server_2_name");
    assert_eq!("history_window".to_case(Case::Pascal), "HistoryWindow");
}
