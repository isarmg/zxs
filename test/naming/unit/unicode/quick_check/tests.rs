use super::{IsNormalized, is_nfc, is_nfc_quick};

#[test]
fn quick_check_classifies_nfc_yes_no_and_maybe() {
    for (input, expected) in [
        ("abc", IsNormalized::Yes),
        ("\u{e9}\u{1e0d}\u{4e2d}", IsNormalized::Yes),
        // Out-of-order combining marks (ccc 230 before 220).
        ("a\u{301}\u{323}", IsNormalized::No),
        // Singleton and composition-excluded characters never occur in NFC.
        ("\u{2126}", IsNormalized::No),
        ("\u{958}", IsNormalized::No),
        // Combining marks that may compose with the preceding starter.
        ("a\u{301}", IsNormalized::Maybe),
        ("\u{1161}", IsNormalized::Maybe),
        // Unicode 16 Tulu-Tigalari vowel sign with NFC_QC=Maybe.
        ("\u{113c9}", IsNormalized::Maybe),
    ] {
        assert_eq!(is_nfc_quick(input.chars()), expected, "{input:?}");
    }
}

#[test]
fn maybe_results_are_resolved_by_full_composition() {
    assert!(!is_nfc("a\u{301}"));
    assert!(is_nfc("\u{301}"));
    assert!(!is_nfc("\u{1100}\u{1161}"));
    assert!(!is_nfc("\u{113c2}\u{113c9}"));
    assert!(is_nfc("\u{113c8}"));
}
