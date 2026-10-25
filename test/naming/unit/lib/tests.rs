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
    assert_eq!(
        validate_name("e\u{301}").unwrap_err().code(),
        storage_contract::ErrorCode::NameNotNormalized
    );
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
fn lookup_paths_accept_canonical_equivalence_without_relaxing_new_names() {
    for (stored, query) in [
        ("Café/é.txt", "Cafe\u{301}/e\u{301}.txt"),
        ("Å/Å.txt", "A\u{30a}/A\u{30a}.txt"),
        ("각/가.txt", "\u{1100}\u{1161}\u{11a8}/\u{1100}\u{1161}.txt"),
    ] {
        validate_path(stored).unwrap();
        validate_lookup_path(query).unwrap();
        assert_eq!(path_key(query), path_key(stored));
        assert_eq!(
            validate_path(query).unwrap_err().code(),
            storage_contract::ErrorCode::NameNotNormalized
        );
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
        assert_eq!(lookup.code(), strict.code(), "{path:?}");
    }
    validate_lookup_path(&"a".repeat(200)).unwrap();
    validate_lookup_path(&vec!["a"; 32].join("/")).unwrap();
}

#[test]
fn lookup_leaf_names_keep_name_budget_separate_from_repository_path_budget() {
    validate_lookup_name(&"a".repeat(240)).unwrap();
    assert_eq!(
        validate_lookup_name(&"a".repeat(241)).unwrap_err().code(),
        storage_contract::ErrorCode::NameTooLong
    );
    for (stored, query) in [
        ("é", "e\u{301}"),
        ("Å", "A\u{30a}"),
        ("가", "\u{1100}\u{1161}"),
    ] {
        validate_lookup_name(query).unwrap();
        assert_eq!(lookup_key(query), lookup_key(stored));
        assert_eq!(
            validate_name(query).unwrap_err().code(),
            storage_contract::ErrorCode::NameNotNormalized
        );
    }
    for name in [".", "..", "CON", "a/b", "a\\b", "a\u{200d}b"] {
        assert!(validate_lookup_name(name).is_err(), "{name:?}");
    }
}
