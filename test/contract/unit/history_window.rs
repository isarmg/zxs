//! Strict syntax and bounded wire shapes. Live root membership, authorization,
//! page selection and byte-limited response assembly belong to core/server tests.
use super::*;
use crate::{decode_strict, validate_note, validate_tag};
use json_codec::{Value, json};

fn id(prefix: &str, ordinal: u64) -> String {
    format!("{prefix}_{ordinal:032x}")
}

fn file() -> HistoryFileId {
    HistoryFileId::parse(&id("file", 1)).unwrap()
}

fn node(ordinal: u64) -> HistoryNodeId {
    HistoryNodeId::parse(&id("fmeta", ordinal)).unwrap()
}

fn version() -> HistoryVersionId {
    HistoryVersionId::parse(&id("ver", 2)).unwrap()
}

fn branch(ordinal: u64) -> HistoryBranchId {
    HistoryBranchId::parse(&id("br", ordinal)).unwrap()
}

fn merge() -> HistoryMergeId {
    HistoryMergeId::parse(&id("fmerge", 3)).unwrap()
}

fn root() -> HistoryRoot {
    HistoryRoot::from_digest(blake3::hash(b"history-manifest fixture"))
}

fn context() -> HistoryResolvedContext {
    HistoryResolvedContext {
        repository_id: HistoryRepositoryId::parse(&id("repo", 4)).unwrap(),
        history_tree_id: HistoryTreeId::parse(&id("history", 5)).unwrap(),
        path: Some(name_policy::PortablePath::parse("Straße/Café.txt").unwrap()),
        current_node_id: Some(node(6)),
        selected_branch_id: Some(branch(7)),
    }
}

fn payload(collection: HistoryCollection, position: HistoryPosition) -> HistoryCursorPayload {
    let limit = HistoryPageLimit::default_for(&collection);
    HistoryCursorPayload {
        format: HistoryCursorFormat::HistoryWindowV1,
        history_root: root(),
        history_tree_id: context().history_tree_id,
        file_id: file(),
        collection,
        query_hash: HistoryQueryHash::from_digest(blake3::hash(b"bound query")),
        limit,
        direction: HistoryDirection::Forward,
        position,
        anchor_node_id: None,
    }
}

fn cursor(collection: HistoryCollection, position: HistoryPosition) -> HistoryCursor {
    HistoryCursor::from_payload(payload(collection, position)).unwrap()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn identity(collection: HistoryCollection) -> HistoryPageIdentity {
    HistoryPageIdentity {
        history_root: root(),
        file_id: file(),
        context: context(),
        query_hash: HistoryQueryHash::from_digest(blake3::hash(b"bound query")),
        limit: HistoryPageLimit::default_for(&collection),
        action_max_entries: u64::MAX,
        collection,
    }
}

fn fields() -> HistoryNodeFields {
    HistoryNodeFields {
        message: HistoryFieldInfo {
            byte_length: u64::MAX,
        },
        author: HistoryFieldInfo {
            byte_length: u64::MAX,
        },
        created_at: HistoryFieldInfo { byte_length: 20 },
    }
}

fn summary() -> HistoryNodeSummary {
    HistoryNodeSummary {
        node_id: node(6),
        version_id: version(),
        kind: HistoryNodeKind::Tags,
        sequence: u64::MAX,
        current: true,
        branch_id: Some(branch(7)),
        path: "Straße/Café.txt".into(),
        previous_path: None,
        note: "review".into(),
        tags: vec!["review".into()],
        size: u64::MAX,
        content_hash: ContentHash::from_digest(blake3::hash(b"body")),
        content_state: Some(ContentState::Purged),
        download_name: "Café.txt".into(),
        storage_name: "v000001_20261030T000000Z_fixture.txt".into(),
        parents: HistoryParentSet::Complete,
        fields: fields(),
    }
}

fn version_summary() -> HistoryVersionSummary {
    HistoryVersionSummary {
        version_id: version(),
        version_sequence: u64::MAX,
        size: u64::MAX,
        content_hash: ContentHash::from_digest(blake3::hash(b"body")),
        content_state: Some(ContentState::Migrated),
        note: String::new(),
        download_name: "body.txt".into(),
        storage_name: "stored.txt".into(),
        source_count: u64::MAX,
        fields: fields(),
    }
}

fn branch_summary() -> HistoryBranchSummary {
    HistoryBranchSummary {
        branch_id: branch(3),
        name: "审阅".into(),
        head_commit_id: HistoryCommitId::parse(&id("commit", 4)).unwrap(),
        fork_commit_id: HistoryCommitId::parse(&id("commit", 5)).unwrap(),
        node_id: Some(node(6)),
        state: crate::BranchState::Merged,
        branch_kind: crate::BranchKind::File,
        generation: u64::MAX,
        current: false,
        read_only: true,
        alias_of: Some(branch(7)),
        content: HistoryContentDisposition {
            version_id: Some(version()),
            source: HistoryContentSource::Inherited {
                source_version_id: Some(version()),
            },
            stored_state: None,
        },
    }
}

fn merge_summary() -> HistoryMergeSummary {
    HistoryMergeSummary {
        merge_id: merge(),
        sequence: u64::MAX,
        chosen_node_id: node(1),
        result_node_id: node(2),
        surviving_branch_id: branch(3),
        reused_node: true,
        operation_id: id("op", 4).parse().unwrap(),
        input_count: u64::MAX,
        parent_count: u64::MAX,
    }
}

fn keys(value: &Value) -> Vec<&str> {
    let mut keys = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    keys.sort_unstable();
    keys
}

#[test]
fn typed_history_identifiers_keep_node_body_and_merge_domains_distinct() {
    for prefix in ["ver", "fnode", "fmeta"] {
        let text = id(prefix, 9);
        let parsed = HistoryNodeId::parse(&text).unwrap();
        assert_eq!(parsed.as_str(), text);
        assert_eq!(json_codec::to_value(&parsed).unwrap(), text);
        assert_eq!(
            json_codec::from_value::<HistoryNodeId>(json!(text)).unwrap(),
            parsed
        );
    }
    assert!(HistoryVersionId::parse(&id("ver", 9)).is_ok());
    for prefix in ["file", "repo", "history", "br", "commit", "fmerge"] {
        assert!(HistoryNodeId::parse(&id(prefix, 9)).is_err());
    }
    for prefix in ["fnode", "fmeta", "file", "fmerge"] {
        assert!(HistoryVersionId::parse(&id(prefix, 9)).is_err());
    }
    for prefix in ["ver", "fnode", "fmeta", "file", "br"] {
        assert!(HistoryMergeId::parse(&id(prefix, 9)).is_err());
    }
    for text in [
        String::new(),
        id("ver", 9).to_uppercase(),
        format!("ver_{:032X}", 0xab),
        format!(" {}", id("ver", 9)),
        format!("{}\n", id("ver", 9)),
        "ver_0000000000000000000000000000000g".into(),
        "ver_0000000000000000000000000000000".into(),
        "ver_000000000000000000000000000000000".into(),
    ] {
        assert!(HistoryNodeId::parse(&text).is_err(), "accepted {text:?}");
        assert!(json_codec::from_value::<HistoryNodeId>(json!(text)).is_err());
    }
    for shape in [
        json!(null),
        json!(17),
        json!([]),
        json!({"node_id":id("ver", 9)}),
    ] {
        assert!(json_codec::from_value::<HistoryNodeId>(shape).is_err());
    }
}

#[test]
fn history_roots_and_query_hashes_require_canonical_fixed_digest_text() {
    let digest = blake3::hash(b"exact manifest");
    let text = digest.to_hex().to_string();
    let root = HistoryRoot::from_digest(digest);
    assert_eq!(std::mem::size_of::<HistoryRoot>(), 32);
    assert_eq!(root.to_string(), text);
    assert_eq!(HistoryRoot::parse(&text).unwrap(), root);
    assert_eq!(json_codec::to_value(root).unwrap(), text);
    assert_eq!(HistoryQueryHash::from_digest(digest).to_string(), text);
    // Syntactic acceptance does not make a digest an engine-issued binding.
    for text in ["0".repeat(64), "f".repeat(64)] {
        assert!(HistoryRoot::parse(&text).is_ok());
        assert!(HistoryQueryHash::parse(&text).is_ok());
    }
    for text in [
        String::new(),
        "0".repeat(63),
        "0".repeat(65),
        "g".repeat(64),
        digest.to_hex().to_string().to_uppercase(),
        format!(" {text}"),
        format!("{text}\n"),
    ] {
        assert!(HistoryRoot::parse(&text).is_err());
        assert!(HistoryQueryHash::parse(&text).is_err());
        assert!(json_codec::from_value::<HistoryRoot>(json!(text)).is_err());
    }
}

#[test]
fn history_limits_are_collection_specific_without_becoming_retention_limits() {
    let collections = [
        HistoryCollection::Nodes,
        HistoryCollection::Versions,
        HistoryCollection::Branches,
        HistoryCollection::Merges,
        HistoryCollection::NodeParents { node_id: node(1) },
        HistoryCollection::VersionSources {
            version_id: version(),
        },
        HistoryCollection::MergeInputs { merge_id: merge() },
        HistoryCollection::MergeParents { merge_id: merge() },
        HistoryCollection::NodeField {
            node_id: node(1),
            field: HistoryField::Message,
        },
    ];
    for collection in collections {
        let maximum = collection.maximum_limit();
        assert!(HistoryPageLimit::for_collection(0, &collection).is_err());
        assert_eq!(
            HistoryPageLimit::for_collection(maximum, &collection)
                .unwrap()
                .get(),
            maximum
        );
        assert!(HistoryPageLimit::for_collection(maximum + 1, &collection).is_err());
        let default = HistoryPageLimit::default_for(&collection);
        assert!(default.get() > 0 && default.get() <= maximum);
        assert_eq!(json_codec::to_value(default).unwrap(), json!(default.get()));
        assert_eq!(
            collection.requires_root(),
            !matches!(
                collection,
                HistoryCollection::Nodes | HistoryCollection::Versions
            )
        );
    }
    assert_eq!(
        MAX_HISTORY_RESPONSE_BYTES,
        crate::client_protocol::MAX_RESPONSE_BYTES
    );
    for bad in [
        json!(0),
        json!(-1),
        json!(MAX_HISTORY_FIELD_BYTES + 1),
        json!("1"),
        json!(null),
    ] {
        assert!(json_codec::from_value::<HistoryPageLimit>(bad).is_err());
    }
}

#[test]
fn cursor_positions_roundtrip_with_full_width_keys_and_reject_cross_collection_keys() {
    let pairs = [
        (
            HistoryCollection::Nodes,
            HistoryPosition::Node {
                sequence: u64::MAX,
                node_id: node(1),
            },
        ),
        (
            HistoryCollection::Versions,
            HistoryPosition::Version {
                sequence: u64::MAX,
                version_id: version(),
            },
        ),
        (
            HistoryCollection::Branches,
            HistoryPosition::Branch {
                branch_id: branch(1),
            },
        ),
        (
            HistoryCollection::Merges,
            HistoryPosition::Merge {
                sequence: u64::MAX,
                merge_id: merge(),
            },
        ),
        (
            HistoryCollection::NodeParents { node_id: node(1) },
            HistoryPosition::Parent {
                parent_node_id: node(2),
            },
        ),
        (
            HistoryCollection::VersionSources {
                version_id: version(),
            },
            HistoryPosition::Member { ordinal: u64::MAX },
        ),
        (
            HistoryCollection::MergeInputs { merge_id: merge() },
            HistoryPosition::Member { ordinal: u64::MAX },
        ),
        (
            HistoryCollection::MergeParents { merge_id: merge() },
            HistoryPosition::Member { ordinal: u64::MAX },
        ),
        (
            HistoryCollection::NodeField {
                node_id: node(1),
                field: HistoryField::Author,
            },
            HistoryPosition::Field {
                byte_offset: u64::MAX,
            },
        ),
    ];
    for (collection, position) in &pairs {
        for direction in [HistoryDirection::Forward, HistoryDirection::Backward] {
            let mut payload = payload(collection.clone(), position.clone());
            payload.direction = direction;
            let encoded = HistoryCursor::from_payload(payload.clone()).unwrap();
            assert!(encoded.as_str().len() <= MAX_HISTORY_CURSOR_BYTES);
            assert!(
                encoded
                    .as_str()
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            );
            assert_eq!(
                HistoryCursor::parse(encoded.as_str()).unwrap().payload(),
                &payload
            );
            assert_eq!(
                json_codec::from_value::<HistoryCursor>(json!(encoded.as_str())).unwrap(),
                encoded
            );
        }
        let mut wrong_key = payload(collection.clone(), position.clone());
        wrong_key.position = if matches!(collection, HistoryCollection::Nodes) {
            HistoryPosition::Member { ordinal: 0 }
        } else {
            HistoryPosition::Node {
                sequence: 0,
                node_id: node(1),
            }
        };
        assert!(wrong_key.validate().is_err());
        assert!(HistoryCursor::from_payload(wrong_key).is_err());
    }
    let mut bad_limit = payload(
        HistoryCollection::Nodes,
        HistoryPosition::Node {
            sequence: 0,
            node_id: node(1),
        },
    );
    bad_limit.limit = HistoryPageLimit::new(HistoryCollection::Nodes.maximum_limit() + 1).unwrap();
    assert!(bad_limit.validate().is_err());
    let mut anchor = payload(
        HistoryCollection::Nodes,
        HistoryPosition::Node {
            sequence: 0,
            node_id: node(1),
        },
    );
    anchor.anchor_node_id = Some(node(8));
    assert!(anchor.validate().is_ok());
    anchor.collection = HistoryCollection::Versions;
    anchor.position = HistoryPosition::Version {
        sequence: 0,
        version_id: version(),
    };
    assert!(anchor.validate().is_err());
    let mut start = payload(
        HistoryCollection::NodeParents { node_id: node(1) },
        HistoryPosition::Start,
    );
    assert!(start.validate().is_ok());
    let encoded = HistoryCursor::from_payload(start.clone()).unwrap();
    assert_eq!(
        HistoryCursor::parse(encoded.as_str()).unwrap().payload(),
        &start
    );
    start.direction = HistoryDirection::Backward;
    assert!(start.validate().is_err());
    start.direction = HistoryDirection::Forward;
    for (collection, _) in pairs {
        if matches!(collection, HistoryCollection::NodeParents { .. }) {
            continue;
        }
        start.limit = HistoryPageLimit::default_for(&collection);
        start.collection = collection;
        assert!(start.validate().is_err());
    }
}

#[test]
fn cursor_decoder_rejects_noncurrent_shapes_duplicates_noncanonical_integers_and_oversize() {
    let payload = payload(
        HistoryCollection::Nodes,
        HistoryPosition::Node {
            sequence: u64::MAX,
            node_id: node(1),
        },
    );
    let value = json_codec::to_value(&payload).unwrap();
    for (field, bad) in [
        ("format", json!("history_window_v0")),
        ("format", json!({"history_window_v1":null})),
        ("direction", json!("sideways")),
        ("direction", json!({"forward":null})),
        ("history_root", json!("unbound")),
        ("file_id", json!(id("fmerge", 1))),
        ("extra", json!(true)),
    ] {
        let mut bad_value = value.clone();
        bad_value[field] = bad;
        assert!(
            HistoryCursor::parse(&hex(&json_codec::to_vec(&bad_value).unwrap())).is_err(),
            "accepted {field}"
        );
    }
    for field in [
        "format",
        "history_root",
        "history_tree_id",
        "file_id",
        "collection",
        "query_hash",
        "limit",
        "direction",
        "position",
    ] {
        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(
            HistoryCursor::parse(&hex(&json_codec::to_vec(&missing).unwrap())).is_err(),
            "accepted missing {field}"
        );
    }
    for bad in [
        json!(u64::MAX),
        json!("01"),
        json!("+1"),
        json!("18446744073709551616"),
    ] {
        let mut bad_value = value.clone();
        bad_value["position"]["sequence"] = bad;
        assert!(HistoryCursor::parse(&hex(&json_codec::to_vec(&bad_value).unwrap())).is_err());
    }
    let raw = json_codec::to_string(&payload).unwrap();
    let duplicate = format!("{{\"format\":\"history_window_v1\",{}", &raw[1..]);
    assert!(HistoryCursor::parse(&hex(duplicate.as_bytes())).is_err());
    let duplicate_nested = raw.replace(
        r#""collection":{"kind":"nodes"}"#,
        r#""collection":{"kind":"nodes","kind":"nodes"}"#,
    );
    assert_ne!(duplicate_nested, raw);
    assert!(HistoryCursor::parse(&hex(duplicate_nested.as_bytes())).is_err());
    let mut at_limit = raw.into_bytes();
    assert!(at_limit.len() < MAX_HISTORY_CURSOR_BYTES / 2);
    at_limit.resize(MAX_HISTORY_CURSOR_BYTES / 2, b' ');
    assert_eq!(
        HistoryCursor::parse(&hex(&at_limit)).unwrap().payload(),
        &payload
    );
    for bad in [
        String::new(),
        "a".into(),
        "GG".into(),
        "é".into(),
        "00".into(),
        "0".repeat(MAX_HISTORY_CURSOR_BYTES + 2),
    ] {
        assert!(HistoryCursor::parse(&bad).is_err());
    }
}

#[test]
fn query_hash_normalizes_requested_and_resolved_paths_and_binds_every_scope_dimension() {
    let resolved = context();
    let mut request = HistoryWindowRequest {
        file_id: file(),
        collection: HistoryCollection::Nodes,
        location: Some(HistoryLocation::new(
            resolved.repository_id.clone(),
            name_policy::LookupPath::parse("STRASSE/CAFE\u{301}.TXT").unwrap(),
        )),
        history_root: None,
        cursor: None,
        anchor_node_id: None,
        limit: HistoryPageLimit::default_for(&HistoryCollection::Nodes),
    };
    let expected = request.query_hash(&resolved, None);
    request.location.as_mut().unwrap().path =
        name_policy::LookupPath::parse("straße/Café.txt").unwrap();
    let mut equivalent = resolved.clone();
    equivalent.path = Some(name_policy::PortablePath::parse("STRASSE/CAFÉ.TXT").unwrap());
    assert_eq!(request.query_hash(&equivalent, None), expected);
    let mut unscoped = request.clone();
    unscoped.location = None;
    assert_ne!(
        unscoped.query_hash(&resolved, None),
        expected,
        "absence is part of query identity"
    );
    let mut changed = request.clone();
    changed.file_id = HistoryFileId::parse(&id("file", 2)).unwrap();
    assert_ne!(changed.query_hash(&resolved, None), expected);
    changed = request.clone();
    changed.collection = HistoryCollection::Merges;
    assert_ne!(changed.query_hash(&resolved, None), expected);
    changed = request.clone();
    changed.limit = HistoryPageLimit::for_collection(1, &HistoryCollection::Nodes).unwrap();
    assert_ne!(changed.query_hash(&resolved, None), expected);
    changed = request.clone();
    changed.location.as_mut().unwrap().repository_id =
        HistoryRepositoryId::parse(&id("repo", 9)).unwrap();
    assert_ne!(changed.query_hash(&resolved, None), expected);
    changed = request.clone();
    changed.location.as_mut().unwrap().path = name_policy::LookupPath::parse("other.txt").unwrap();
    assert_ne!(changed.query_hash(&resolved, None), expected);
    assert_ne!(request.query_hash(&resolved, Some(&node(99))), expected);
    for changed in [
        HistoryResolvedContext {
            repository_id: HistoryRepositoryId::parse(&id("repo", 9)).unwrap(),
            ..resolved.clone()
        },
        HistoryResolvedContext {
            history_tree_id: HistoryTreeId::parse(&id("history", 9)).unwrap(),
            ..resolved.clone()
        },
        HistoryResolvedContext {
            path: None,
            ..resolved.clone()
        },
        HistoryResolvedContext {
            path: Some(name_policy::PortablePath::parse("other.txt").unwrap()),
            ..resolved.clone()
        },
        HistoryResolvedContext {
            current_node_id: None,
            ..resolved.clone()
        },
        HistoryResolvedContext {
            selected_branch_id: None,
            ..resolved.clone()
        },
    ] {
        assert_ne!(request.query_hash(&changed, None), expected);
    }
}

#[test]
fn tagged_history_unit_variants_reject_surplus_fields_without_changing_valid_wire() {
    fn closed_variant<T>(variant: T)
    where
        T: data_serialization::Serialize
            + data_serialization::de::DeserializeOwned
            + std::fmt::Debug
            + PartialEq,
    {
        let wire = json_codec::to_value(&variant).unwrap();
        assert_eq!(json_codec::from_value::<T>(wire.clone()).unwrap(), variant);
        for field in ["cursor", "node_id", "source_version_id", "future"] {
            for value in [json!(null), json!("unexpected")] {
                let mut bad = wire.clone();
                bad[field] = value;
                assert!(
                    json_codec::from_value::<T>(bad.clone()).is_err(),
                    "surplus field accepted: {bad}"
                );
            }
        }
    }

    for collection in [
        HistoryCollection::Nodes,
        HistoryCollection::Versions,
        HistoryCollection::Branches,
        HistoryCollection::Merges,
    ] {
        closed_variant(collection);
    }
    closed_variant(HistoryPosition::Start);
    closed_variant(HistoryContentSource::Owned);
    closed_variant(HistoryParentSet::Complete);

    let valid = cursor(
        HistoryCollection::NodeParents { node_id: node(6) },
        HistoryPosition::Start,
    );
    let mut payload = json_codec::to_value(valid.payload()).unwrap();
    payload["position"]["parent_node_id"] = json!(node(2));
    assert!(HistoryCursor::parse(&hex(&json_codec::to_vec(&payload).unwrap())).is_err());
}

#[test]
fn parent_preview_and_endpoint_tokens_do_not_invent_loaded_nodes_or_complete_unread_sets() {
    let parent_cursor = cursor(
        HistoryCollection::NodeParents { node_id: node(6) },
        HistoryPosition::Parent {
            parent_node_id: node(2),
        },
    );
    assert_eq!(
        json_codec::to_value(HistoryParentSet::Complete).unwrap(),
        json!({"state":"complete"})
    );
    let more = HistoryParentSet::More {
        cursor: parent_cursor.clone(),
    };
    let encoded = json_codec::to_value(&more).unwrap();
    assert_eq!(
        encoded,
        json!({"state":"more","cursor":parent_cursor.as_str()})
    );
    assert_eq!(
        json_codec::from_value::<HistoryParentSet>(encoded).unwrap(),
        more
    );
    for bad in [
        json!({"state":"more"}),
        json!({"state":"complete","cursor":parent_cursor.as_str()}),
        json!({"state":"unknown"}),
    ] {
        assert!(json_codec::from_value::<HistoryParentSet>(bad).is_err());
    }
    let mut child = summary();
    child.parents = more;
    let page = HistoryWindowPage::Nodes {
        identity: identity(HistoryCollection::Nodes),
        context: HistoryContextSummary {
            note: String::new(),
            content: None,
        },
        nodes: vec![child],
        edges: vec![],
        page: HistoryPageCursors {
            previous_cursor: None,
            next_cursor: None,
        },
    };
    let encoded = json_codec::to_value(page).unwrap();
    assert_eq!(encoded["nodes"][0]["parents"]["state"], "more");
    assert_eq!(encoded["edges"], json!([]));
    let edge = HistoryGraphEdge {
        node_id: node(1),
        parent_node_id: node(2),
        endpoint: HistoryEndpoint::OutsideWindow,
    };
    let encoded = json_codec::to_value(edge).unwrap();
    assert_eq!(keys(&encoded), ["endpoint", "node_id", "parent_node_id"]);
    assert_eq!(encoded["endpoint"], "outside_window");
    for endpoint in [json!("missing"), json!({"outside_window":null})] {
        let mut malformed = encoded.clone();
        malformed["endpoint"] = endpoint;
        assert!(json_codec::from_value::<HistoryGraphEdge>(malformed).is_err());
    }
}

#[test]
fn summaries_externalize_large_scalars_and_relationships_without_parallel_input_arrays() {
    let encoded = json_codec::to_value(summary()).unwrap();
    assert_eq!(
        keys(&encoded),
        [
            "branch_id",
            "content_hash",
            "content_state",
            "current",
            "download_name",
            "fields",
            "kind",
            "node_id",
            "note",
            "parents",
            "path",
            "previous_path",
            "sequence",
            "size",
            "storage_name",
            "tags",
            "version_id"
        ]
    );
    assert_eq!(encoded["sequence"], u64::MAX.to_string());
    assert_eq!(encoded["size"], u64::MAX.to_string());
    assert_eq!(
        encoded["fields"]["message"]["byte_length"],
        u64::MAX.to_string()
    );
    assert_eq!(encoded["content_state"], "PURGED");
    for field in [
        "message",
        "author",
        "created_at",
        "parent_node_ids",
        "source_version_ids",
    ] {
        assert!(encoded.get(field).is_none());
        let mut malformed = encoded.clone();
        malformed[field] = json!("not inline");
        assert!(json_codec::from_value::<HistoryNodeSummary>(malformed).is_err());
    }
    let version = json_codec::to_value(version_summary()).unwrap();
    assert_eq!(version["source_count"], u64::MAX.to_string());
    assert!(version.get("source_version_ids").is_none());
    let merge = json_codec::to_value(merge_summary()).unwrap();
    assert_eq!(
        keys(&merge),
        [
            "chosen_node_id",
            "input_count",
            "merge_id",
            "operation_id",
            "parent_count",
            "result_node_id",
            "reused_node",
            "sequence",
            "surviving_branch_id"
        ]
    );
    let input = HistoryMergeInputMember {
        ordinal: u64::MAX,
        branch_id: branch(3),
        node_id: node(2),
    };
    let input = json_codec::to_value(input).unwrap();
    assert_eq!(keys(&input), ["branch_id", "node_id", "ordinal"]);
    assert_eq!(input["ordinal"], u64::MAX.to_string());
    for field in ["branch_id", "node_id"] {
        let mut incomplete = input.clone();
        incomplete.as_object_mut().unwrap().remove(field);
        assert!(json_codec::from_value::<HistoryMergeInputMember>(incomplete).is_err());
    }
    let mut bad_kind = encoded;
    bad_kind["kind"] = json!({"tags":null});
    assert!(json_codec::from_value::<HistoryNodeSummary>(bad_kind).is_err());
    let branch = json_codec::to_value(branch_summary()).unwrap();
    assert_eq!(
        keys(&branch),
        [
            "alias_of",
            "branch_id",
            "branch_kind",
            "content",
            "current",
            "fork_commit_id",
            "generation",
            "head_commit_id",
            "name",
            "node_id",
            "read_only",
            "state"
        ]
    );
    assert_eq!(branch["generation"], u64::MAX.to_string());
    assert_eq!(branch["content"]["source"]["kind"], "inherited");
    assert_eq!(
        branch["content"]["source"]["source_version_id"],
        self::version().as_str()
    );
    assert!(branch["content"]["stored_state"].is_null());
    assert_eq!(branch["read_only"], true);
    assert!(branch.get("pending_file_versions").is_none());
    let mut empty = branch_summary();
    empty.node_id = None;
    empty.state = crate::BranchState::Active;
    empty.alias_of = None;
    empty.read_only = false;
    empty.content = HistoryContentDisposition {
        version_id: None,
        source: HistoryContentSource::Inherited {
            source_version_id: None,
        },
        stored_state: None,
    };
    let encoded = json_codec::to_value(&empty).unwrap();
    assert!(encoded["node_id"].is_null());
    assert!(encoded["content"]["version_id"].is_null());
    assert_eq!(encoded["content"]["source"]["kind"], "inherited");
    assert!(encoded["content"]["source"]["source_version_id"].is_null());
    assert!(encoded["content"]["stored_state"].is_null());
    assert_eq!(
        json_codec::from_value::<HistoryBranchSummary>(encoded).unwrap(),
        empty
    );
}

#[test]
fn all_page_kinds_keep_root_identity_and_exact_auxiliary_wire_shapes() {
    let terminal = || HistoryPageCursors {
        previous_cursor: None,
        next_cursor: None,
    };
    let pages = [
        HistoryWindowPage::Nodes {
            identity: identity(HistoryCollection::Nodes),
            context: HistoryContextSummary {
                note: String::new(),
                content: None,
            },
            nodes: vec![summary()],
            edges: vec![],
            page: terminal(),
        },
        HistoryWindowPage::Versions {
            identity: identity(HistoryCollection::Versions),
            versions: vec![version_summary()],
            page: terminal(),
        },
        HistoryWindowPage::Branches {
            identity: identity(HistoryCollection::Branches),
            branches: vec![branch_summary()],
            page: terminal(),
        },
        HistoryWindowPage::Merges {
            identity: identity(HistoryCollection::Merges),
            merges: vec![merge_summary()],
            page: terminal(),
        },
        HistoryWindowPage::NodeParents {
            identity: identity(HistoryCollection::NodeParents { node_id: node(1) }),
            parents: vec![HistoryParentMember {
                parent_node_id: node(2),
            }],
            page: terminal(),
        },
        HistoryWindowPage::VersionSources {
            identity: identity(HistoryCollection::VersionSources {
                version_id: version(),
            }),
            sources: vec![HistorySourceMember {
                ordinal: u64::MAX,
                version_id: version(),
            }],
            page: terminal(),
        },
        HistoryWindowPage::MergeInputs {
            identity: identity(HistoryCollection::MergeInputs { merge_id: merge() }),
            inputs: vec![HistoryMergeInputMember {
                ordinal: u64::MAX,
                branch_id: branch(3),
                node_id: node(2),
            }],
            page: terminal(),
        },
        HistoryWindowPage::MergeParents {
            identity: identity(HistoryCollection::MergeParents { merge_id: merge() }),
            parents: vec![HistoryMergeParentMember {
                ordinal: u64::MAX,
                parent_node_id: node(2),
            }],
            page: terminal(),
        },
        HistoryWindowPage::NodeField {
            identity: identity(HistoryCollection::NodeField {
                node_id: node(1),
                field: HistoryField::Author,
            }),
            field: HistoryField::Author,
            total_bytes: 0,
            byte_offset: 0,
            text: String::new(),
            end_of_field: true,
            next_cursor: None,
        },
    ];
    for page in pages {
        let bytes = json_codec::to_vec(&page).unwrap();
        assert!(bytes.len() <= MAX_HISTORY_RESPONSE_BYTES);
        assert_eq!(decode_strict::<HistoryWindowPage>(&bytes).unwrap(), page);
        let encoded = json_codec::to_value(page).unwrap();
        assert_eq!(
            keys(&encoded["identity"]),
            [
                "action_max_entries",
                "collection",
                "context",
                "file_id",
                "history_root",
                "limit",
                "query_hash"
            ]
        );
        assert_eq!(encoded["identity"]["history_root"], root().to_string());
        assert_eq!(encoded["identity"]["file_id"], file().as_str());
        assert_eq!(
            encoded["identity"]["action_max_entries"],
            u64::MAX.to_string()
        );
        for bad in [
            json!(u64::MAX),
            json!("01"),
            json!("18446744073709551616"),
            json!(null),
        ] {
            let mut malformed = encoded.clone();
            malformed["identity"]["action_max_entries"] = bad;
            assert!(json_codec::from_value::<HistoryWindowPage>(malformed).is_err());
        }
        let mut missing_action_budget = encoded.clone();
        missing_action_budget["identity"]
            .as_object_mut()
            .unwrap()
            .remove("action_max_entries");
        assert!(json_codec::from_value::<HistoryWindowPage>(missing_action_budget).is_err());
        let mut extra = encoded.clone();
        extra["all"] = json!(true);
        assert!(json_codec::from_value::<HistoryWindowPage>(extra).is_err());
        let mut changed_root_shape = encoded;
        changed_root_shape["identity"]["history_root"] = json!(17);
        assert!(json_codec::from_value::<HistoryWindowPage>(changed_root_shape).is_err());
    }
}

#[test]
fn field_fragments_preserve_utf8_bytes_empty_values_and_end_markers_without_claiming_complete_history()
 {
    let collection = HistoryCollection::NodeField {
        node_id: node(1),
        field: HistoryField::Message,
    };
    let parts = ["汉", "😀", "\"\n"];
    let whole = parts.concat();
    let mut offset = 0;
    let mut observed = String::new();
    for (index, text) in parts.iter().enumerate() {
        let next_offset = offset + text.len();
        let next_cursor = (index + 1 < parts.len()).then(|| {
            cursor(
                collection.clone(),
                HistoryPosition::Field {
                    byte_offset: next_offset as u64,
                },
            )
        });
        let page = HistoryWindowPage::NodeField {
            identity: identity(collection.clone()),
            field: HistoryField::Message,
            total_bytes: whole.len() as u64,
            byte_offset: offset as u64,
            text: (*text).into(),
            end_of_field: next_cursor.is_none(),
            next_cursor,
        };
        let bytes = json_codec::to_vec(&page).unwrap();
        let decoded: HistoryWindowPage = decode_strict(&bytes).unwrap();
        assert_eq!(decoded, page);
        if *text == "😀" {
            let mut malformed = bytes.clone();
            let index = malformed
                .windows(4)
                .position(|bytes| bytes == "😀".as_bytes())
                .unwrap();
            malformed[index + 1] = 0xff;
            assert!(decode_strict::<HistoryWindowPage>(&malformed).is_err());
        }
        let encoded = json_codec::to_value(page).unwrap();
        assert_eq!(encoded["byte_offset"], offset.to_string());
        assert_eq!(encoded["total_bytes"], whole.len().to_string());
        assert_eq!(encoded["text"], *text);
        assert_eq!(encoded["end_of_field"], index + 1 == parts.len());
        assert!(encoded.get("complete").is_none());
        observed.push_str(encoded["text"].as_str().unwrap());
        offset = next_offset;
    }
    assert_eq!(observed, whole);
    let empty = HistoryWindowPage::NodeField {
        identity: identity(collection),
        field: HistoryField::Message,
        total_bytes: 0,
        byte_offset: 0,
        text: String::new(),
        end_of_field: true,
        next_cursor: None,
    };
    let encoded = json_codec::to_value(empty).unwrap();
    assert_eq!(encoded["text"], "");
    assert_eq!(encoded["total_bytes"], "0");
    assert!(encoded["next_cursor"].is_null());
    for (field, bad) in [
        ("field", json!("unknown")),
        ("field", json!({"message":null})),
        ("byte_offset", json!(0)),
        ("total_bytes", json!("00")),
        ("complete", json!(true)),
    ] {
        let mut malformed = encoded.clone();
        malformed[field] = bad;
        assert!(json_codec::from_value::<HistoryWindowPage>(malformed).is_err());
    }
}

#[test]
fn declared_node_and_field_budgets_include_json_escaping_and_shared_response_ceiling() {
    let mut node = summary();
    node.note = "\"".repeat(crate::MAX_NOTE_BYTES);
    validate_note(&node.note).unwrap();
    node.tags = (0..crate::MAX_FILE_TAGS)
        .map(|index| {
            let tag = format!("{index:02}{}", "\"".repeat(crate::MAX_TAG_BYTES - 2));
            validate_tag(&tag).unwrap();
            tag
        })
        .collect();
    node.parents = HistoryParentSet::More {
        cursor: cursor(
            HistoryCollection::NodeParents {
                node_id: node.node_id.clone(),
            },
            HistoryPosition::Parent {
                parent_node_id: self::node(2),
            },
        ),
    };
    let encoded = json_codec::to_vec(&node).unwrap();
    assert!(encoded.len() <= MAX_HISTORY_NODE_ENCODED_BYTES);
    let maximum_nodes = HistoryCollection::Nodes.maximum_limit();
    let edge_bytes = json_codec::to_vec(&HistoryGraphEdge {
        node_id: self::node(1),
        parent_node_id: self::node(2),
        endpoint: HistoryEndpoint::OutsideWindow,
    })
    .unwrap()
    .len();
    assert!(
        MAX_HISTORY_ENVELOPE_BYTES
            + maximum_nodes * (MAX_HISTORY_NODE_ENCODED_BYTES + 1)
            + MAX_HISTORY_PREVIEW_EDGES * (edge_bytes + 1)
            <= MAX_HISTORY_RESPONSE_BYTES
    );
    // A field may contain controls and needs up to six JSON bytes per raw byte.
    let text = "\u{0001}".repeat(MAX_HISTORY_FIELD_BYTES);
    let bytes = json_codec::to_vec(&text).unwrap();
    assert_eq!(bytes.len(), 6 * MAX_HISTORY_FIELD_BYTES + 2);
    assert!(MAX_HISTORY_ENVELOPE_BYTES + bytes.len() <= MAX_HISTORY_RESPONSE_BYTES);
}

#[test]
fn cancellation_latched_while_queued_survives_request_drop_before_worker_execution() {
    use std::{sync::mpsc, time::Duration};
    let request = HistoryWindowCancellation::new();
    let (enqueue, queued) = mpsc::sync_channel(1);
    enqueue.send(request.clone()).unwrap();
    let (ready, ready_receiver) = mpsc::sync_channel(1);
    let (execute, execution) = mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        ready.send(()).unwrap();
        execution.recv_timeout(Duration::from_secs(10)).unwrap();
        let signal = queued.recv_timeout(Duration::from_secs(10)).unwrap();
        signal.is_cancelled()
    });
    ready_receiver
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    request.cancel();
    request.cancel();
    drop(request);
    // The simulated backend job remains queued until cancellation has completed
    // and the caller has gone away; no scheduling race or sleep establishes it.
    execute.send(()).unwrap();
    assert!(worker.join().unwrap());
}

#[test]
fn cancellation_clones_share_a_monotone_signal_without_affecting_independent_reads() {
    use std::{sync::mpsc, time::Duration};
    let request = HistoryWindowCancellation::new();
    let worker_signal = request.clone();
    let independent_new = HistoryWindowCancellation::new();
    let independent_default = HistoryWindowCancellation::default();
    assert!(!request.is_cancelled());
    assert!(!worker_signal.is_cancelled());
    assert!(!independent_new.is_cancelled());
    assert!(!independent_default.is_cancelled());
    let (cancelled, observed) = mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        worker_signal.cancel();
        worker_signal.cancel();
        cancelled.send(()).unwrap();
    });
    observed.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(request.is_cancelled());
    let late_observer = request.clone();
    request.cancel();
    drop(request);
    worker.join().unwrap();
    assert!(late_observer.is_cancelled());
    assert!(!independent_new.is_cancelled());
    assert!(!independent_default.is_cancelled());
    independent_default.cancel();
    independent_default.cancel();
    assert!(independent_default.is_cancelled());
    assert!(!independent_new.is_cancelled());
}
