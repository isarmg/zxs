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
