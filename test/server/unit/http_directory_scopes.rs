use super::*;
#[test]
fn directory_query_has_distinct_all_root_and_lookup_filters() {
    let parse_query = |pairs: &[(&str, &str)]| {
        parse(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    };
    let (filter, projection) = parse_query(&[]).unwrap();
    assert!(matches!(filter, DirectoryBranchFilter::All));
    assert!(matches!(projection, DirectoryBranchProjection::Full));
    let (filter, projection) = parse_query(&[("path", ""), ("summary", "true")]).unwrap();
    assert!(matches!(filter, DirectoryBranchFilter::Root));
    assert!(matches!(projection, DirectoryBranchProjection::Summary));
    let (filter, _) = parse_query(&[("path", "Cafe\u{301}")]).unwrap();
    let DirectoryBranchFilter::Directory(path) = filter else {
        panic!("named filter lost lookup path")
    };
    assert_eq!(path.as_str(), "Cafe\u{301}");
    assert_eq!(path.key(), "café");
    for summary in ["", "false", "TRUE", "1"] {
        assert_eq!(
            parse_query(&[("summary", summary), ("path", "../bad")])
                .err()
                .unwrap()
                .message,
            "summary must be true when supplied"
        );
    }
    assert_eq!(
        parse_query(&[("extra", "value"), ("summary", "false")])
            .err()
            .unwrap()
            .message,
        "unknown field extra"
    );
    let bad = parse_query(&[("path", "../bad")]).err().unwrap();
    assert!(std::error::Error::source(&bad).is_some());
}

#[test]
fn owning_main_query_retains_root_and_original_lookup_path() {
    for raw in ["", "Cafe\u{301}/Child"] {
        let query = [
            ("summary".into(), "true".into()),
            ("owner_main".into(), "true".into()),
            ("path".into(), raw.into()),
        ]
        .into_iter()
        .collect();
        let (filter, projection) = parse(query).unwrap();
        assert!(matches!(projection, DirectoryBranchProjection::Summary));
        let DirectoryBranchFilter::OwningMain(path) = filter else {
            panic!("owning-main query must retain its distinct filter")
        };
        assert_eq!(
            path.as_ref().map(name_policy::LookupPath::as_str),
            if raw.is_empty() { None } else { Some(raw) }
        );
        if let Some(path) = path {
            assert_eq!(path.key(), "café/child");
        }
    }
    for pairs in [
        vec![("owner_main", "true"), ("path", "")],
        vec![("owner_main", "true"), ("summary", "true")],
        vec![("owner_main", "false"), ("summary", "true"), ("path", "")],
        vec![("owner_main", "true"), ("summary", "true"), ("path", "../bad")],
    ] {
        let error = parse(pairs.into_iter().map(|(k, v)| (k.into(), v.into())).collect())
            .err()
            .unwrap();
        assert_eq!(error.status, http::StatusCode::BAD_REQUEST);
    }
}
