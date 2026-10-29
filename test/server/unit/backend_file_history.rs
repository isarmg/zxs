use super::*;
use json_codec::json;

fn query(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

fn root() -> String {
    "ef".repeat(32)
}

fn node_cursor(root: &str, backward: bool) -> String {
    NodeCursor::encode(
        &HistoryCursorSeed::new(CursorKind::Nodes, root, "history", "file", &json!({})),
        backward,
        NodeCursorKey {
            sequence: 3,
            node_id: "node".into(),
        },
    )
    .unwrap()
}

#[test]
fn file_history_request_is_strict_about_fields_limits_cursors_and_anchors() {
    let request = ReadFileHistory::parse("file".into(), BTreeMap::new()).unwrap();
    assert_eq!(request.limit, limits::NODES_DEFAULT);
    assert!(matches!(request.position, NodePosition::First));
    assert!(request.root.is_none() && request.location.is_none());
    for (key, value, message) in [
        ("unused", "1", "unknown field unused"),
        ("limit", "0", "limit must be between 1 and 200"),
        ("limit", "201", "limit must be between 1 and 200"),
        ("limit", "-1", "canonical unsigned decimal"),
        ("limit", "01", "canonical unsigned decimal"),
        (
            "history_root",
            "abc",
            "history_root must be 64 lowercase hexadecimal digits",
        ),
        (
            "repository_id",
            "repo",
            "repository_id and path must be supplied together",
        ),
        (
            "path",
            "x.txt",
            "repository_id and path must be supplied together",
        ),
    ] {
        let error = ReadFileHistory::parse("file".into(), query(&[(key, value)])).unwrap_err();
        assert_eq!(
            error.status,
            ::http::StatusCode::BAD_REQUEST,
            "{key}={value}"
        );
        assert!(
            error.message.contains(message),
            "{key}={value}: {}",
            error.message
        );
    }
    assert!(ReadFileHistory::parse(String::new(), BTreeMap::new()).is_err());
    assert!(ReadFileHistory::parse("f".repeat(257), BTreeMap::new()).is_err());
    let cursor = node_cursor(&root(), true);
    let conflict = ReadFileHistory::parse(
        "file".into(),
        query(&[("cursor", &cursor), ("anchor_node_id", "node")]),
    )
    .unwrap_err();
    assert_eq!(
        conflict.message,
        "cursor and anchor_node_id are mutually exclusive"
    );
    let different_root = ReadFileHistory::parse(
        "file".into(),
        query(&[("cursor", &cursor), ("history_root", &"01".repeat(32))]),
    )
    .unwrap_err();
    assert_eq!(
        different_root.message,
        "history_root differs from the cursor's history root"
    );
    let same_root = ReadFileHistory::parse(
        "file".into(),
        query(&[("cursor", &cursor), ("history_root", &root())]),
    )
    .unwrap();
    assert_eq!(same_root.root.unwrap().as_str(), root());
    let from_cursor = ReadFileHistory::parse("file".into(), query(&[("cursor", &cursor)])).unwrap();
    assert_eq!(from_cursor.root.as_ref().unwrap().as_str(), root());
    assert!(matches!(from_cursor.position, NodePosition::Cursor(cursor) if cursor.backward));
    let anchored = ReadFileHistory::parse(
        "file".into(),
        query(&[
            ("anchor_node_id", "node"),
            ("limit", "200"),
            ("history_root", &root()),
        ]),
    )
    .unwrap();
    assert!(matches!(anchored.position, NodePosition::Anchor(id) if id.as_str() == "node"));
    assert_eq!(anchored.limit, 200);
    let bad_cursor = ReadFileHistory::parse("file".into(), query(&[("cursor", "zz")])).unwrap_err();
    assert_eq!(bad_cursor.code, storage_contract::ErrorCode::InvalidCursor);
    let located = ReadFileHistory::parse(
        "file".into(),
        query(&[("repository_id", "repo"), ("path", "Cafe\u{301}/FILE.txt")]),
    )
    .unwrap();
    assert_eq!(located.location().unwrap().1.key(), "café/file.txt");
    assert!(
        ReadFileHistory::parse(
            "file".into(),
            query(&[("repository_id", "repo"), ("path", "../file.txt")])
        )
        .is_err()
    );
}

#[test]
fn sub_collection_requests_require_a_root_and_bound_their_limits_and_offsets() {
    let error = ReadHistoryBranches::parse("file".into(), BTreeMap::new()).unwrap_err();
    assert_eq!(error.message, "history_root is required (query or cursor)");
    let branches =
        ReadHistoryBranches::parse("file".into(), query(&[("history_root", &root())])).unwrap();
    assert_eq!(branches.0.limit, limits::BRANCHES_DEFAULT);
    assert!(
        ReadHistoryBranches::parse(
            "file".into(),
            query(&[("history_root", &root()), ("limit", "501")])
        )
        .is_err()
    );
    let merges = ReadHistoryMerges::parse(
        "file".into(),
        query(&[("history_root", &root()), ("limit", "200")]),
    )
    .unwrap();
    assert_eq!(merges.0.limit, 200);
    assert!(
        ReadHistoryMerges::parse(
            "file".into(),
            query(&[("history_root", &root()), ("limit", "201")])
        )
        .is_err()
    );
    let parents = ReadNodeParents::parse(
        "file".into(),
        "node".into(),
        query(&[("history_root", &root())]),
    )
    .unwrap();
    assert_eq!(parents.collection.limit, limits::PARENTS_DEFAULT);
    assert_eq!(parents.node_id.as_str(), "node");
    assert!(
        ReadNodeParents::parse(
            "file".into(),
            String::new(),
            query(&[("history_root", &root())])
        )
        .is_err()
    );
    let slice = ReadIndexedSlice::parse(
        "file".into(),
        "merge_id",
        "merge".into(),
        query(&[
            ("history_root", &root()),
            ("offset", "7"),
            ("limit", "1000"),
        ]),
    )
    .unwrap();
    assert_eq!((slice.offset, slice.limit), (7, 1000));
    assert!(
        ReadIndexedSlice::parse(
            "file".into(),
            "merge_id",
            "merge".into(),
            query(&[("history_root", &root()), ("limit", "1001")])
        )
        .is_err()
    );
    assert!(
        ReadIndexedSlice::parse(
            "file".into(),
            "merge_id",
            "merge".into(),
            query(&[("history_root", &root()), ("offset", "-1")])
        )
        .is_err()
    );
    assert!(
        ReadIndexedSlice::parse(
            "file".into(),
            "merge_id",
            "merge".into(),
            query(&[("history_root", &root()), ("cursor", "00")])
        )
        .is_err()
    );
    let field = ReadNodeField::parse(
        "file".into(),
        "node".into(),
        "author".into(),
        query(&[("history_root", &root())]),
    )
    .unwrap();
    assert_eq!(field.max_bytes, limits::FIELD_DEFAULT_BYTES);
    assert_eq!(field.field, NodeFieldName::Author);
    let unknown = ReadNodeField::parse(
        "file".into(),
        "node".into(),
        "path".into(),
        query(&[("history_root", &root())]),
    )
    .unwrap_err();
    assert_eq!(unknown.status, ::http::StatusCode::NOT_FOUND);
    assert_eq!(unknown.message, "unknown field");
    assert!(
        ReadNodeField::parse(
            "file".into(),
            "node".into(),
            "message".into(),
            query(&[("history_root", &root()), ("max_bytes", "0")])
        )
        .is_err()
    );
    assert!(
        ReadNodeField::parse(
            "file".into(),
            "node".into(),
            "message".into(),
            query(&[("history_root", &root()), ("max_bytes", "1048577")])
        )
        .is_err()
    );
    let versions = ReadVersionPage::parse("file".into(), BTreeMap::new()).unwrap();
    assert!(versions.root().is_none());
    assert_eq!(versions.limit, limits::VERSIONS_DEFAULT);
    assert!(ReadVersionPage::parse("file".into(), query(&[("limit", "501")])).is_err());
    assert!(ReadVersionPage::parse("file".into(), query(&[("unused", "yes")])).is_err());
    let rooted =
        ReadVersionPage::parse("file".into(), query(&[("history_root", &root())])).unwrap();
    assert_eq!(rooted.root().unwrap().as_str(), root());
}

#[test]
fn unrooted_anchor_is_rejected_and_first_version_page_has_no_fake_root() {
    let error =
        ReadFileHistory::parse("file".into(), query(&[("anchor_node_id", "node")])).unwrap_err();
    assert_eq!(error.message, "anchor_node_id requires history_root");
    let first = ReadVersionPage::parse("file".into(), BTreeMap::new()).unwrap();
    assert!(first.root.is_none());
}
