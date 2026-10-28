//! Naming semantics that the Unicode table toolchain must preserve.
//!
//! The digests were captured on the implementation before the unreachable
//! normalization tables were trimmed (docs/tools/unicode-build.md). They cover
//! persisted keys, accept/reject results with their error values, the NFC
//! quick-check outcome and NFC output. The conformance tests use the official
//! NormalizationTest-16.0.0 vectors, which share no table with the
//! implementation.
use super::*;

const NORMALIZATION_TEST: &str = include_str!("../data/normalization-test-16.0.0.txt");
const SCALAR_DIGEST: u64 = 0x1133_85b8_b69b_4cfd;
const NORMALIZATION_TEST_DIGEST: u64 = 0x8396_6791_e017_e27c;

/// FNV-1a over length-delimited fields: a regression fingerprint, not a
/// security boundary.
struct Fingerprint(u64);
impl Fingerprint {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
    fn field(&mut self, bytes: &[u8]) {
        for byte in (bytes.len() as u64).to_le_bytes().iter().chain(bytes) {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    fn observe(&mut self, input: &str) {
        self.field(input.as_bytes());
        self.field(lookup_key(input).as_bytes());
        self.field(format!("{:?}", validate_name(input)).as_bytes());
        self.field(format!("{:?}", validate_lookup_name(input)).as_bytes());
        self.field(&[u8::from(unicode::is_nfc(input))]);
        self.field(input.nfc().collect::<String>().as_bytes());
    }
}

/// Parsed NormalizationTest rows: the five columns of each line.
fn normalization_rows() -> impl Iterator<Item = [String; 5]> {
    NORMALIZATION_TEST.lines().filter_map(|line| {
        let data = line.split('#').next().unwrap_or_default();
        if data.starts_with('@') || !data.contains(';') {
            return None;
        }
        let mut columns = data.split(';').map(|column| {
            column
                .split_whitespace()
                .map(|scalar| char::from_u32(u32::from_str_radix(scalar, 16).unwrap()).unwrap())
                .collect::<String>()
        });
        Some(std::array::from_fn(|_| columns.next().unwrap()))
    })
}

fn nfc(input: &str) -> String {
    input.nfc().collect()
}

#[test]
fn every_scalar_keeps_its_key_validation_and_nfc_results() {
    let mut fingerprint = Fingerprint::new();
    let mut text = String::new();
    for scalar in 0..=0x10ffff {
        if let Some(c) = char::from_u32(scalar) {
            text.clear();
            text.push(c);
            fingerprint.observe(&text);
        }
    }
    assert_eq!(fingerprint.0, SCALAR_DIGEST, "{:#018x}", fingerprint.0);
}

#[test]
fn normalization_test_inputs_keep_their_key_validation_and_nfc_results() {
    let mut fingerprint = Fingerprint::new();
    let mut rows = 0;
    for row in normalization_rows() {
        for column in &row {
            fingerprint.observe(column);
        }
        rows += 1;
    }
    assert_eq!(rows, 19_965);
    assert_eq!(
        fingerprint.0, NORMALIZATION_TEST_DIGEST,
        "{:#018x}",
        fingerprint.0
    );
}

#[test]
fn nfc_conforms_to_normalization_test_16() {
    for [
        source,
        composed,
        decomposed,
        compatibility,
        compatibility_decomposed,
    ] in normalization_rows()
    {
        // UAX #15 NormalizationTest invariants for toNFC.
        assert_eq!(nfc(&source), composed, "{source:?}");
        assert_eq!(nfc(&composed), composed, "{source:?}");
        assert_eq!(nfc(&decomposed), composed, "{source:?}");
        assert_eq!(nfc(&compatibility), compatibility, "{source:?}");
        assert_eq!(nfc(&compatibility_decomposed), compatibility, "{source:?}");
        assert!(unicode::is_nfc(&composed), "{source:?}");
        assert!(unicode::is_nfc(&compatibility), "{source:?}");
        assert_eq!(unicode::is_nfc(&source), source == composed, "{source:?}");
    }
}

#[test]
fn scalars_outside_normalization_test_part_one_are_nfc_invariant() {
    let mut listed = std::collections::BTreeSet::new();
    let mut part_one = false;
    for line in NORMALIZATION_TEST.lines() {
        if line.starts_with("@Part") {
            part_one = line.starts_with("@Part1");
            continue;
        }
        let data = line.split('#').next().unwrap_or_default();
        if part_one && let Some(source) = data.split(';').next().filter(|s| !s.is_empty()) {
            listed.insert(u32::from_str_radix(source.trim(), 16).unwrap());
        }
    }
    assert!(listed.len() > 10_000);
    let mut text = String::new();
    for scalar in 0..=0x10ffff {
        if listed.contains(&scalar) {
            continue;
        }
        if let Some(c) = char::from_u32(scalar) {
            text.clear();
            text.push(c);
            assert_eq!(nfc(&text), text, "U+{scalar:04X}");
            assert!(unicode::is_nfc(&text), "U+{scalar:04X}");
        }
    }
}

#[test]
fn reordering_supplementary_composition_and_folding_have_fixed_keys() {
    for (input, key) in [
        // Canonical reordering of marks before composition (ccc 220 then 230).
        ("a\u{301}\u{323}", "\u{1ea1}\u{301}"),
        // Supplementary-plane composition (Kaithi) and decomposition of the composite.
        ("\u{11099}\u{110ba}", "\u{1109a}"),
        ("\u{1109a}", "\u{1109a}"),
        // Unicode 16 additions: Tulu-Tigalari and Gurung Khema chained compositions.
        ("\u{113c2}\u{113c9}", "\u{113c8}"),
        ("\u{1611e}\u{1611e}\u{1611f}", "\u{16126}"),
        // Full case folding expands and recomposes.
        ("Stra\u{df}e", "strasse"),
        ("\u{1f88}", "\u{1f00}\u{3b9}"),
        ("\u{130}", "i\u{307}"),
        ("\u{3a3}\u{3c2}", "\u{3c3}\u{3c3}"),
        ("\u{fb03}", "ffi"),
        // Hangul jamo compose algorithmically.
        ("\u{1100}\u{1161}\u{11a8}", "\u{ac01}"),
    ] {
        assert_eq!(lookup_key(input), key, "{input:?}");
    }
}

#[test]
fn rejection_boundaries_are_unchanged() {
    for (input, accepted) in [
        ("\u{34f}", false),    // CGJ: Default_Ignorable
        ("a\u{fe0f}", false),  // variation selector
        ("\u{200d}", false),   // ZWJ
        ("\u{e000}", false),   // private use
        ("\u{10ffff}", false), // noncharacter
        ("\u{2fffe}", false),  // unassigned
        ("\u{1f600}", false),  // emoji (So)
        ("\u{10400}", true),   // supplementary letter
        ("\u{1e900}", true),   // Adlam letter
        ("e\u{301}", false),   // valid lookup, but not NFC for creation
        ("\u{e9}", true),
    ] {
        assert_eq!(validate_name(input).is_ok(), accepted, "{input:?}");
    }
    assert!(validate_lookup_name("e\u{301}").is_ok());
}
