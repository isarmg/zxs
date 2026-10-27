use super::*;
fn sources(count: usize) -> Vec<DirectorySelectionSourceInput> {
    (0..count)
        .map(|n| DirectorySelectionSourceInput {
            directory_branch_id: format!("branch{n}"),
            expected_tree_id: "tree".into(),
        })
        .collect()
}
fn entries(count: usize) -> Vec<DirectorySelectionEntryInput> {
    (0..count)
        .map(|n| DirectorySelectionEntryInput {
            entry_id: format!("entry{n}"),
            path: format!("file{n}.txt"),
        })
        .collect()
}
#[test]
fn selection_constructor_bounds_work_before_accepting_owned_input() {
    for (sources_count, entries_count) in [(500, 0), (0, 500), (100, 100)] {
        assert!(
            ReadDirectorySelection::parse(
                "repo".into(),
                "",
                sources(sources_count),
                entries(entries_count)
            )
            .is_ok()
        );
    }
    for (sources_count, entries_count) in [(501, 0), (0, 501), (100, 101), (21, 500)] {
        let error = ReadDirectorySelection::parse(
            "repo".into(),
            "../invalid",
            sources(sources_count),
            entries(entries_count),
        )
        .err()
        .unwrap();
        assert_eq!(
            error.code,
            storage_contract::ErrorCode::ResourceLimitExceeded
        );
        assert_eq!(error.status, http::StatusCode::TOO_MANY_REQUESTS);
    }
}
#[test]
fn selection_constructor_preserves_direct_children_unique_keys_and_input_source_order() {
    let entry = |id: &str, path: &str| DirectorySelectionEntryInput {
        entry_id: id.into(),
        path: path.into(),
    };
    let query = ReadDirectorySelection::parse(
        "repo".into(),
        "CAFE\u{301}",
        vec![
            DirectorySelectionSourceInput {
                directory_branch_id: "z".into(),
                expected_tree_id: "one".into(),
            },
            DirectorySelectionSourceInput {
                directory_branch_id: "a".into(),
                expected_tree_id: "two".into(),
            },
        ],
        vec![
            entry("z", "Café/z.txt"),
            entry("a", "Cafe\u{301}/A\u{30a}.txt"),
        ],
    )
    .unwrap();
    assert_eq!(query.scope().unwrap().key(), "café");
    assert_eq!(query.sources()[0].directory_branch_id, "z");
    assert_eq!(query.sources()[1].directory_branch_id, "a");
    assert_eq!(query.entries()[0].entry_id(), "a");
    assert_eq!(
        query.entries()[0].path().as_str(),
        "Cafe\u{301}/A\u{30a}.txt"
    );
    assert_eq!(query.entries()[0].path().key(), "café/å.txt");
    for selected in [
        vec![
            entry("x", "Café/one.txt"),
            entry("x", "CAFE\u{301}/ONE.txt"),
        ],
        vec![entry("x", "Café/nested/file.txt")],
        vec![entry("x", "Other/one.txt")],
        vec![entry("", "Café/one.txt")],
        vec![entry("x", "")],
        vec![entry(&"x".repeat(257), "Café/one.txt")],
    ] {
        assert!(ReadDirectorySelection::parse("repo".into(), "Café", vec![], selected).is_err());
    }
    assert!(
        ReadDirectorySelection::parse(
            "repo".into(),
            "Café",
            vec![],
            vec![entry(&"x".repeat(256), "Café/one.txt")]
        )
        .is_ok()
    );
    let mut duplicated = sources(2);
    duplicated[1].directory_branch_id = duplicated[0].directory_branch_id.clone();
    assert_eq!(
        ReadDirectorySelection::parse("repo".into(), "", duplicated, vec![])
            .err()
            .unwrap()
            .message,
        "duplicate source scope"
    );
    for field in ["directory_branch_id", "expected_tree_id"] {
        let mut invalid = sources(1);
        if field == "directory_branch_id" {
            invalid[0].directory_branch_id.clear();
        } else {
            invalid[0].expected_tree_id.clear();
        }
        assert_eq!(
            ReadDirectorySelection::parse("repo".into(), "", invalid, vec![])
                .err()
                .unwrap()
                .message,
            format!("missing string field {field}")
        );
    }
}
