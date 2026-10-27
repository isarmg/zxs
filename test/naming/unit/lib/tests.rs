use super::*;
#[test]
fn portable_names() {
    for s in [
        "报告-2026.docx",
        ".gitignore",
        "~$报告.docx",
        "报告（终稿）.docx",
        "项目、预算.xlsx",
    ] {
        validate_name(s).unwrap();
    }
    for s in [
        "a:b.txt",
        "CON",
        "nul.txt",
        "COM¹.txt",
        "LPT².txt",
        "CON .txt",
        " bad",
        "bad.",
        ".",
        "..",
        "a\u{200d}b",
        "🚀",
    ] {
        assert!(validate_name(s).is_err(), "{s}");
    }
}
#[test]
fn normalized_and_folded() {
    assert_eq!(lookup_key("Straße.txt"), "strasse.txt");
    assert_eq!(lookup_key("Σ"), lookup_key("ς"));
    assert_eq!(lookup_key("e\u{301}"), lookup_key("é"));
    assert!(matches!(
        validate_name("e\u{301}").unwrap_err(),
        crate::Error::NotNormalized(_)
    ));
    assert_eq!(unicode::UNICODE_VERSION, (16, 0, 0));
}
#[test]
fn path_budgets() {
    assert!(validate_path("../escape").is_err());
    assert!(validate_path("a//b").is_err());
    assert!(validate_path("/root").is_err());
    assert!(validate_path(&vec!["a"; 33].join("/")).is_err());
}

#[test]
fn name_byte_budget_covers_ascii_cjk_and_supplementary_letters() {
    for (letter, count, utf16_units) in [("a", 240, 240), ("界", 80, 80), ("𐐀", 60, 120)] {
        let boundary = letter.repeat(count);
        assert_eq!(boundary.len(), 240);
        assert_eq!(boundary.encode_utf16().count(), utf16_units);
        assert_eq!(validate_name(&boundary), Ok(()));
        assert_eq!(validate_lookup_name(&boundary), Ok(()));

        let oversized = letter.repeat(count + 1);
        assert_eq!(oversized.len(), 240 + letter.len());
        assert_eq!(validate_name(&oversized), Err(Error::NameTooLong));
        assert_eq!(validate_lookup_name(&oversized), Err(Error::NameTooLong));
    }
}

#[test]
fn name_byte_budget_precedes_other_name_errors_and_normalization() {
    for name in [
        format!(" {}", "a".repeat(240)),
        format!("{}:", "a".repeat(240)),
        "e\u{301}".repeat(81),
    ] {
        assert_eq!(validate_name(&name), Err(Error::NameTooLong));
        assert_eq!(validate_lookup_name(&name), Err(Error::NameTooLong));
    }
}

#[test]
fn path_utf16_budget_still_limits_valid_name_components() {
    let first = "a".repeat(100);
    let last = "b".repeat(99);
    let boundary = format!("{first}/{last}");
    assert_eq!(boundary.len(), 200);
    assert_eq!(boundary.encode_utf16().count(), 200);
    assert_eq!(validate_path(&boundary), Ok(()));
    assert_eq!(validate_lookup_path(&boundary), Ok(()));
    assert_eq!(PortablePath::parse(&boundary).unwrap().as_str(), boundary);
    assert_eq!(LookupPath::parse(&boundary).unwrap().as_str(), boundary);

    let oversized = format!("{boundary}b");
    assert_eq!(oversized.len(), 201);
    assert_eq!(oversized.encode_utf16().count(), 201);
    for component in oversized.split('/') {
        assert_eq!(validate_name(component), Ok(()));
        assert_eq!(validate_lookup_name(component), Ok(()));
    }
    assert_eq!(validate_path(&oversized), Err(Error::InvalidPath));
    assert_eq!(validate_lookup_path(&oversized), Err(Error::InvalidPath));
    assert_eq!(
        PortablePath::parse(&oversized).unwrap_err(),
        Error::InvalidPath
    );
    assert_eq!(
        LookupPath::parse(&oversized).unwrap_err(),
        Error::InvalidPath
    );
}

#[test]
fn lookup_paths_accept_canonical_equivalence_without_relaxing_new_names() {
    for (stored, query) in [
        ("Café/é.txt", "Cafe\u{301}/e\u{301}.txt"),
        ("Å/Å.txt", "A\u{30a}/A\u{30a}.txt"),
        ("각/가.txt", "\u{1100}\u{1161}\u{11a8}/\u{1100}\u{1161}.txt"),
    ] {
        validate_path(stored).unwrap();
        validate_lookup_path(query).unwrap();
        assert_eq!(path_key(query), path_key(stored));
        assert!(matches!(
            validate_path(query).unwrap_err(),
            crate::Error::NotNormalized(_)
        ));
    }
    validate_lookup_path("STRASSE/ς.txt").unwrap();
    assert_eq!(path_key("Straße/Σ.txt"), path_key("STRASSE/ς.txt"));
}

#[test]
fn lookup_paths_keep_raw_budgets_and_portability_guards() {
    let invalid = [
        "".to_owned(),
        "../escape".into(),
        "a/../b".into(),
        "a/./b".into(),
        "/root".into(),
        "a\\b".into(),
        "a//b".into(),
        "a/".into(),
        "CON.txt".into(),
        "COM¹.txt".into(),
        "dir/LPT².txt".into(),
        "a\u{200d}b".into(),
        "a\u{feff}b".into(),
        " bad".into(),
        "bad.".into(),
        "a:b".into(),
        "🚀".into(),
        vec!["a"; 33].join("/"),
        "a".repeat(201),
        "é".repeat(121),
        // These normalize below a budget: validate the submitted form, not its key.
        "e\u{301}".repeat(81),
        format!("{}/{}", "e\u{301}".repeat(50), "e\u{301}".repeat(50)),
    ];
    for path in invalid {
        let strict = validate_path(&path).unwrap_err();
        let lookup = validate_lookup_path(&path).unwrap_err();
        assert_eq!(lookup, strict, "{path:?}");
    }
    validate_lookup_path(&"a".repeat(200)).unwrap();
    validate_lookup_path(&vec!["a"; 32].join("/")).unwrap();
}

#[test]
fn lookup_leaf_names_keep_name_budget_separate_from_repository_path_budget() {
    validate_lookup_name(&"a".repeat(240)).unwrap();
    assert_eq!(
        validate_lookup_name(&"a".repeat(241)).unwrap_err(),
        crate::Error::NameTooLong
    );
    for (stored, query) in [
        ("é", "e\u{301}"),
        ("Å", "A\u{30a}"),
        ("가", "\u{1100}\u{1161}"),
    ] {
        validate_lookup_name(query).unwrap();
        assert_eq!(lookup_key(query), lookup_key(stored));
        assert!(matches!(
            validate_name(query).unwrap_err(),
            crate::Error::NotNormalized(_)
        ));
    }
    for name in [".", "..", "CON", "a/b", "a\\b", "a\u{200d}b"] {
        assert!(validate_lookup_name(name).is_err(), "{name:?}");
    }
}

#[test]
fn parsed_paths_retain_exact_display_and_fixed_unicode_keys() {
    for display in ["Straße/Café.txt", "Σ/报告.txt", "𐐀.txt", "Ａ/A.txt"] {
        let path = PortablePath::parse(display).unwrap();
        assert_eq!(path.as_str(), display);
        assert_eq!(path.key(), path_key(display));
        assert_eq!(path.file_name(), display.rsplit('/').next().unwrap());
        let lookup = LookupPath::parse(display).unwrap();
        assert_eq!(lookup.as_str(), display);
        assert_eq!(lookup.key(), path.key());
        let borrowed = LookupPath::from(&path);
        assert_eq!(borrowed.as_str(), display);
        assert_eq!(borrowed.key(), lookup.key());
        let owned = LookupPath::from(path);
        assert_eq!(owned.as_str(), display);
        assert_eq!(owned.key(), lookup.key());
    }
    let lookup = LookupPath::parse("Cafe\u{301}/STRASSE.TXT").unwrap();
    assert_eq!(
        lookup.key(),
        PortablePath::parse("Café/Straße.txt").unwrap().key()
    );
    assert!(matches!(
        PortablePath::parse(lookup.as_str()),
        Err(Error::NotNormalized(_))
    ));
}

#[test]
fn parsed_path_composition_preserves_cached_component_keys_and_checks_new_budgets() {
    let original = PortablePath::parse("STRASSE/Café/报告.txt").unwrap();
    let parent = original.parent().unwrap();
    assert_eq!(parent.as_str(), "STRASSE/Café");
    assert_eq!(parent.key(), "strasse/café");
    let target = PortablePath::parse("Σ").unwrap();
    let moved = original.rebase(1, &target).unwrap();
    assert_eq!(moved.as_str(), "Σ/Café/报告.txt");
    assert_eq!(moved.key(), path_key(moved.as_str()));
    let long_target = PortablePath::parse(&"a".repeat(199)).unwrap();
    assert_eq!(
        original.rebase(2, &long_target).unwrap_err(),
        Error::InvalidPath
    );
    assert!(original.strip_components(usize::MAX).is_none());
    assert_eq!(
        original.rebase(usize::MAX, &target).unwrap_err(),
        Error::InvalidPath
    );
    let deep_target = PortablePath::parse(&vec!["a"; 32].join("/")).unwrap();
    assert_eq!(
        original.rebase(2, &deep_target).unwrap_err(),
        Error::PathTooDeep
    );
}
