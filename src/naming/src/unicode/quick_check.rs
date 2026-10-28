use crate::unicode::UnicodeNormalization;
use crate::unicode::lookups::canonical_combining_class;
use crate::unicode::tables;

/// QuickCheck quickly determines if a string is normalized, it can return
/// `Maybe`
///
/// The QuickCheck algorithm can quickly determine if a text is or isn't
/// normalized without any allocations in many cases, but it has to be able to
/// return `Maybe` when a full decomposition and recomposition is necessary.
#[derive(Debug, Eq, PartialEq)]
pub enum IsNormalized {
    /// The text is definitely normalized.
    Yes,
    /// The text is definitely not normalized.
    No,
    /// The text may be normalized.
    Maybe,
}

// https://unicode.org/reports/tr15/#Detecting_Normalization_Forms
#[inline]
fn quick_check<F, I>(s: I, is_allowed: F) -> IsNormalized
where
    I: Iterator<Item = char>,
    F: Fn(char) -> IsNormalized,
{
    let mut last_cc = 0u8;
    let mut result = IsNormalized::Yes;
    for ch in s {
        // For ASCII we know it's always allowed and a starter
        if ch <= '\x7f' {
            last_cc = 0;
            continue;
        }

        // Otherwise, lookup the combining class and QC property
        let cc = canonical_combining_class(ch);
        if last_cc > cc && cc != 0 {
            return IsNormalized::No;
        }
        match is_allowed(ch) {
            IsNormalized::Yes => (),
            IsNormalized::No => return IsNormalized::No,
            IsNormalized::Maybe => {
                result = IsNormalized::Maybe;
            }
        }
        last_cc = cc;
    }
    result
}

/// Quickly check if a string is in NFC, potentially returning
/// `IsNormalized::Maybe` if further checks are necessary.  In this case a check
/// like `s.chars().nfc().eq(s.chars())` should suffice.
#[inline]
pub fn is_nfc_quick<I: Iterator<Item = char>>(s: I) -> IsNormalized {
    quick_check(s, tables::qc_nfc)
}

/// Authoritatively check if a string is in NFC.
#[inline]
pub fn is_nfc(s: &str) -> bool {
    match is_nfc_quick(s.chars()) {
        IsNormalized::Yes => true,
        IsNormalized::No => false,
        IsNormalized::Maybe => s.chars().eq(s.chars().nfc()),
    }
}

#[cfg(test)]
#[path = "../../../../test/naming/unit/unicode/quick_check/tests.rs"]
mod tests;
