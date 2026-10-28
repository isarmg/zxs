//! Persisted namespace keys must remain byte-for-byte compatible with the
//! original materialized NFC -> Unicode16 full case fold -> NFC pipeline.
use super::*;
use crate::profiling::materialized_key;

#[test]
fn all_unicode_scalars_preserve_persisted_keys() {
    let mut text = String::new();
    for scalar in 0..=0x10ffff {
        if let Some(c) = char::from_u32(scalar) {
            text.clear();
            text.push(c);
            assert_eq!(lookup_key(&text), materialized_key(&text), "U+{scalar:04X}");
        }
    }
}

#[test]
fn fold_expansions_and_neighboring_marks_preserve_keys() {
    // Exercise every table mapping in context, including reordering across a
    // fold boundary and composition after multi-scalar expansions.
    for &(scalar, _) in unicode16::FOLD {
        let c = char::from_u32(scalar).unwrap();
        for before in ["", "A", "\u{301}", "\u{1100}", "\u{345}"] {
            for after in ["", "\u{301}", "\u{323}\u{301}", "\u{1161}\u{11a8}", "Z"] {
                let input = format!("{before}{c}{after}");
                let key = lookup_key(&input);
                assert_eq!(key, materialized_key(&input), "{input:?}");
                assert!(unicode::is_nfc(&key), "{input:?}");
                assert_eq!(lookup_key(&key), key, "key is not idempotent: {input:?}");
            }
        }
    }
}

#[test]
fn canonical_equivalence_and_long_sequences_preserve_keys() {
    for input in [
        "",
        "Report-2026.TXT",
        "报告（终稿）.docx",
        "Straße.Σςσ.İı.\u{fb03}",
        "\u{212b}\u{2126}\u{212a}",
        "\u{1100}\u{1161}\u{11a8}.각",
        "\u{1f88}\u{345}\u{301}",
        "a\u{315}\u{300}\u{5ae}\u{301}",
    ] {
        let expected = materialized_key(input);
        assert_eq!(lookup_key(input), expected, "{input:?}");
        // Replace each scalar by its full canonical decomposition: a
        // canonically equivalent spelling of the same name.
        let mut decomposed = String::new();
        for c in input.chars() {
            unicode::char::decompose_canonical(c, |d| decomposed.push(d));
        }
        assert_eq!(lookup_key(&decomposed), expected);
        assert_eq!(lookup_key(&input.nfc().collect::<String>()), expected);
    }
    // Do not silently insert stream-safe CGJ or truncate long sequences. Public
    // lookup_key remains compatible for callers that already own validated data.
    let input = format!("A{}\u{323}", "\u{301}".repeat(1024));
    assert_eq!(lookup_key(&input), materialized_key(&input));
    assert!(!lookup_key(&input).contains('\u{034f}'));
}

#[test]
fn normalization_check_keeps_original_error_and_suggestion() {
    for input in ["e\u{301}", "A\u{30a}", "\u{1100}\u{1161}\u{11a8}"] {
        let error = validate_name(input).unwrap_err();
        assert!(matches!(error, crate::Error::NotNormalized(_)));
        assert!(error.to_string().contains(&input.nfc().collect::<String>()));
    }
}

fn component_wise_path_key(path: &str) -> String {
    path.split('/')
        .map(lookup_key)
        .collect::<Vec<_>>()
        .join("/")
}

#[test]
fn path_keys_preserve_components_and_empty_segments() {
    for path in [
        "",
        "/",
        "//",
        "/A//B/",
        "Projects/Report-2026.TXT",
        "项目/报告（终稿）.docx",
        "Straße/Σςσ/İı/각",
        "A/\u{301}/Z",
        "A\u{301}//\u{323}\u{301}/Z",
        "\u{1100}/\u{1161}/\u{11a8}",
        "\u{212b}/\u{2126}/\u{212a}",
    ] {
        assert_eq!(path_key(path), component_wise_path_key(path), "{path:?}");
        assert_eq!(path_key(&path_key(path)), path_key(path), "{path:?}");
    }
    let deep_path = ["A"; 32].join("/");
    let long_combining = format!("A{}/\u{323}/Z", "\u{301}".repeat(1024));
    for path in [&deep_path, &long_combining] {
        assert_eq!(path_key(path), component_wise_path_key(path), "{path:?}");
    }
}

#[test]
fn path_keys_preserve_all_fold_expansions_at_component_boundaries() {
    for &(scalar, _) in unicode16::FOLD {
        let c = char::from_u32(scalar).unwrap();
        for path in [
            format!("ASCII/{c}/\u{301}/Z"),
            format!("/{c}\u{323}\u{301}//\u{345}/"),
            format!("\u{1100}/{c}/\u{1161}\u{11a8}"),
        ] {
            assert_eq!(path_key(&path), component_wise_path_key(&path), "{path:?}");
        }
    }
}

#[test]
fn lookup_suffix_preserves_parsed_display_and_cached_component_keys() {
    let path = LookupPath::parse("Repo/Cafe\u{301}/FILE.txt").unwrap();
    assert_eq!(path.strip_components(0).unwrap(), path);
    let suffix = path.strip_components(1).unwrap();
    assert_eq!(suffix.as_str(), "Cafe\u{301}/FILE.txt");
    assert_eq!(suffix.key(), "café/file.txt");
    assert_eq!(suffix.strip_components(1).unwrap().as_str(), "FILE.txt");
    assert!(path.strip_components(3).is_none());
}
