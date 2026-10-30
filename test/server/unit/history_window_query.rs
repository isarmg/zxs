use super::*;
use storage_contract::history_window::{
    HistoryCursorFormat, HistoryCursorPayload, HistoryDirection, HistoryMergeId, HistoryPosition,
    HistoryQueryHash, HistoryTreeId, HistoryVersionId, MAX_HISTORY_CURSOR_BYTES,
};

const FILE: &str = "file_00000000000000000000000000000001";
const REPOSITORY: &str = "repo_00000000000000000000000000000002";
const NODE: &str = "ver_00000000000000000000000000000003";
const MERGE: &str = "fmerge_00000000000000000000000000000004";

fn query(values: &[(&str, &str)]) -> BTreeMap<String, String> {
    values
        .iter()
        .map(|(key, value)| ((*key).into(), (*value).into()))
        .collect()
}

fn root() -> String {
    "a".repeat(64)
}

fn node() -> HistoryNodeId {
    HistoryNodeId::parse(NODE).unwrap()
}

fn auxiliary() -> Vec<HistoryCollection> {
    vec![
        HistoryCollection::Branches,
        HistoryCollection::Merges,
        HistoryCollection::NodeParents { node_id: node() },
        HistoryCollection::VersionSources {
            version_id: HistoryVersionId::parse(NODE).unwrap(),
        },
        HistoryCollection::MergeInputs {
            merge_id: HistoryMergeId::parse(MERGE).unwrap(),
        },
        HistoryCollection::MergeParents {
            merge_id: HistoryMergeId::parse(MERGE).unwrap(),
        },
        HistoryCollection::NodeField {
            node_id: node(),
            field: HistoryField::Message,
        },
    ]
}

fn make_cursor(
    collection: HistoryCollection,
    position: HistoryPosition,
    limit: usize,
) -> HistoryCursor {
    HistoryCursor::from_payload(HistoryCursorPayload {
        format: HistoryCursorFormat::HistoryWindowV1,
        history_root: HistoryRoot::parse(&root()).unwrap(),
        history_tree_id: HistoryTreeId::parse("history_00000000000000000000000000000005").unwrap(),
        file_id: HistoryFileId::parse(FILE).unwrap(),
        limit: HistoryPageLimit::for_collection(limit, &collection).unwrap(),
        collection,
        query_hash: HistoryQueryHash::parse(&"b".repeat(64)).unwrap(),
        direction: HistoryDirection::Forward,
        position,
        anchor_node_id: None,
    })
    .unwrap()
}

#[test]
fn main_history_routes_default_to_root_optional_first_pages() {
    for (collection, expected_limit) in [
        (HistoryCollection::Nodes, 32),
        (HistoryCollection::Versions, 64),
    ] {
        let request = parse(FILE, collection.clone(), BTreeMap::new()).unwrap();
        assert_eq!(request.file_id.as_str(), FILE);
        assert_eq!(request.collection, collection);
        assert_eq!(request.limit.get(), expected_limit);
        assert!(request.history_root.is_none());
        assert!(request.cursor.is_none());
        assert!(request.anchor_node_id.is_none());
        assert!(request.location.is_none());
    }
}

#[test]
fn auxiliary_collections_require_an_observation_root_or_a_cursor() {
    for collection in auxiliary() {
        let error = parse(FILE, collection.clone(), BTreeMap::new()).unwrap_err();
        assert_eq!(error.status, http::StatusCode::BAD_REQUEST);
        assert_eq!(error.code, storage_contract::ErrorCode::InvalidRequest);
        let request = parse(
            FILE,
            collection.clone(),
            query(&[("history_root", &root())]),
        )
        .unwrap();
        assert_eq!(request.limit.get(), collection.maximum_limit() / 2);
    }
    let cursor = make_cursor(
        HistoryCollection::NodeParents { node_id: node() },
        HistoryPosition::Parent {
            parent_node_id: node(),
        },
        7,
    );
    let request = parse(
        FILE,
        HistoryCollection::NodeParents { node_id: node() },
        query(&[("cursor", cursor.as_str())]),
    )
    .unwrap();
    assert!(request.history_root.is_none());
    assert_eq!(request.limit.get(), 7);
}

#[test]
fn history_queries_reject_unknown_all_and_noncanonical_or_out_of_budget_limits() {
    for key in [
        "all",
        "offset",
        "after",
        "before",
        "direction",
        "query",
        "summary",
        "field",
    ] {
        let error = parse(FILE, HistoryCollection::Nodes, query(&[(key, "true")])).unwrap_err();
        assert_eq!(error.code, storage_contract::ErrorCode::InvalidRequest);
        assert_eq!(error.message, format!("unknown field {key}"));
    }
    for limit in [
        "",
        "0",
        "-1",
        "+1",
        "01",
        "1.0",
        " 1",
        "1 ",
        "18446744073709551616",
    ] {
        let error = parse(FILE, HistoryCollection::Nodes, query(&[("limit", limit)])).unwrap_err();
        assert_eq!(error.status, http::StatusCode::BAD_REQUEST, "{limit}");
        assert_eq!(
            error.code,
            storage_contract::ErrorCode::InvalidRequest,
            "{limit}"
        );
        assert!(error.source.is_some(), "{limit}");
    }
    for collection in [HistoryCollection::Nodes, HistoryCollection::Versions]
        .into_iter()
        .chain(auxiliary())
    {
        let maximum = collection.maximum_limit();
        assert_eq!(
            parse(
                FILE,
                collection.clone(),
                query(&[("history_root", &root()), ("limit", &maximum.to_string()),])
            )
            .unwrap()
            .limit
            .get(),
            maximum
        );
        assert!(
            parse(
                FILE,
                collection,
                query(&[
                    ("history_root", &root()),
                    ("limit", &(maximum + 1).to_string()),
                ])
            )
            .is_err()
        );
    }
}

#[test]
fn history_location_is_an_absent_or_typed_normalized_pair() {
    let plain = parse(FILE, HistoryCollection::Nodes, BTreeMap::new()).unwrap();
    assert!(plain.location.is_none());
    for path in ["Café/File.txt", "CAFE\u{301}/FILE.TXT"] {
        let request = parse(
            FILE,
            HistoryCollection::Nodes,
            query(&[("repository_id", REPOSITORY), ("path", path)]),
        )
        .unwrap();
        let location = request.location.unwrap();
        assert_eq!(location.repository_id.as_str(), REPOSITORY);
        assert_eq!(location.path.as_str(), path);
        assert_eq!(location.path.key(), "café/file.txt");
    }
    for values in [
        query(&[("repository_id", REPOSITORY)]),
        query(&[("path", "file.txt")]),
        query(&[("repository_id", FILE), ("path", "file.txt")]),
        query(&[("repository_id", REPOSITORY), ("path", "")]),
        query(&[("repository_id", REPOSITORY), ("path", "../file.txt")]),
    ] {
        assert_eq!(
            parse(FILE, HistoryCollection::Nodes, values)
                .unwrap_err()
                .status,
            http::StatusCode::BAD_REQUEST
        );
    }
}

#[test]
fn node_anchor_is_typed_root_bound_and_exclusive_to_initial_node_windows() {
    let valid = query(&[("history_root", &root()), ("anchor_node_id", NODE)]);
    let request = parse(FILE, HistoryCollection::Nodes, valid.clone()).unwrap();
    assert_eq!(request.anchor_node_id.unwrap().as_str(), NODE);
    assert!(request.cursor.is_none());
    assert!(
        parse(
            FILE,
            HistoryCollection::Nodes,
            query(&[("anchor_node_id", NODE)])
        )
        .is_err()
    );
    for collection in [HistoryCollection::Versions].into_iter().chain(auxiliary()) {
        assert!(parse(FILE, collection, valid.clone()).is_err());
    }
    for id in [FILE, MERGE, "ver_0000000000000000000000000000000g"] {
        assert!(
            parse(
                FILE,
                HistoryCollection::Nodes,
                query(&[("history_root", &root()), ("anchor_node_id", id),])
            )
            .is_err()
        );
    }
    let cursor = make_cursor(
        HistoryCollection::Nodes,
        HistoryPosition::Node {
            sequence: 1,
            node_id: node(),
        },
        4,
    );
    assert!(
        parse(
            FILE,
            HistoryCollection::Nodes,
            query(&[
                ("history_root", &root()),
                ("anchor_node_id", NODE),
                ("cursor", cursor.as_str()),
            ])
        )
        .is_err()
    );
}

#[test]
fn cursor_root_query_and_limit_matching_are_left_for_authorized_backend_checks() {
    let cursor = make_cursor(
        HistoryCollection::Nodes,
        HistoryPosition::Node {
            sequence: u64::MAX,
            node_id: node(),
        },
        7,
    );
    let changed_root = "c".repeat(64);
    let request = parse(
        FILE,
        HistoryCollection::Nodes,
        query(&[("history_root", &changed_root), ("cursor", cursor.as_str())]),
    )
    .unwrap();
    assert_ne!(request.history_root, Some(cursor.payload().history_root));
    assert_eq!(request.limit.get(), 7);
    let request = parse(
        FILE,
        HistoryCollection::Nodes,
        query(&[
            ("cursor", cursor.as_str()),
            ("limit", "5"),
            ("repository_id", REPOSITORY),
            ("path", "another.txt"),
        ]),
    )
    .unwrap();
    assert_eq!(request.limit.get(), 5);
    assert!(request.location.is_some());
    let field_cursor = make_cursor(
        HistoryCollection::NodeField {
            node_id: node(),
            field: HistoryField::Message,
        },
        HistoryPosition::Field { byte_offset: 4 },
        32_768,
    );
    let changed_collection = parse(
        FILE,
        HistoryCollection::Nodes,
        query(&[("cursor", field_cursor.as_str())]),
    )
    .unwrap();
    assert_eq!(changed_collection.limit.get(), 32_768);
}

#[test]
fn malformed_typed_identity_root_cursor_and_field_selectors_fail_as_invalid_requests() {
    for id in [
        "",
        REPOSITORY,
        NODE,
        MERGE,
        "file_0000000000000000000000000000000G",
    ] {
        let error = parse(id, HistoryCollection::Nodes, BTreeMap::new()).unwrap_err();
        assert_eq!(error.code, storage_contract::ErrorCode::InvalidRequest);
    }
    for value in [
        String::new(),
        "a".repeat(63),
        "A".repeat(64),
        "z".repeat(64),
    ] {
        assert!(
            parse(
                FILE,
                HistoryCollection::Nodes,
                query(&[("history_root", &value)])
            )
            .is_err()
        );
    }
    for value in [
        String::new(),
        "00".into(),
        "gg".into(),
        "a".into(),
        "a".repeat(MAX_HISTORY_CURSOR_BYTES + 2),
    ] {
        let error =
            parse(FILE, HistoryCollection::Nodes, query(&[("cursor", &value)])).unwrap_err();
        assert_eq!(error.status, http::StatusCode::BAD_REQUEST);
        assert_eq!(error.code, storage_contract::ErrorCode::InvalidRequest);
        assert!(error.source.is_some());
    }
    for (name, expected) in [
        ("message", HistoryField::Message),
        ("author", HistoryField::Author),
        ("created_at", HistoryField::CreatedAt),
    ] {
        assert_eq!(field(name).unwrap(), expected);
    }
    for name in [
        "",
        "Message",
        "author.name",
        "path",
        "parents",
        "source_version_ids",
        "__proto__",
    ] {
        assert_eq!(
            field(name).unwrap_err().code,
            storage_contract::ErrorCode::InvalidRequest
        );
    }
}

#[tokio::test]
async fn actual_query_extraction_rejects_duplicate_decoded_history_parameters() {
    use crate::http_boundary::ApiQuery;
    use http_routing::{body::Body, extract::FromRequestParts};
    for query in [
        "limit=1&limit=2",
        "history_root=a&history_root=b",
        "anchor_node_id=a&%61nchor_node_id=b",
        "repository_id=a&repository_id=b",
        "path=a&path=a",
        "cursor=a&cursor=b",
    ] {
        let (mut parts, _) = http::Request::builder()
            .uri(format!("/api/file-manager/files/{FILE}/history?{query}"))
            .body(Body::empty())
            .unwrap()
            .into_parts();
        let error =
            match ApiQuery::<BTreeMap<String, String>>::from_request_parts(&mut parts, &()).await {
                Ok(_) => panic!("duplicate decoded history query field accepted"),
                Err(error) => error,
            };
        assert_eq!(error.status, http::StatusCode::BAD_REQUEST);
        assert_eq!(error.code, storage_contract::ErrorCode::InvalidRequest);
    }
}
