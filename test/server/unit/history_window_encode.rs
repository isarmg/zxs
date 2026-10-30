use super::*;
use storage_contract::{BranchKind, BranchState, ContentHash, OperationId};

fn identity(collection: HistoryCollection, limit: usize) -> HistoryPageIdentity {
    HistoryPageIdentity {
        history_root: HistoryRoot::parse(&"a".repeat(64)).unwrap(),
        file_id: HistoryFileId::parse(&format!("file_{}", "0".repeat(32))).unwrap(),
        context: HistoryResolvedContext {
            repository_id: HistoryRepositoryId::parse(&format!("repo_{}", "0".repeat(32))).unwrap(),
            history_tree_id: HistoryTreeId::parse(&format!("history_{}", "0".repeat(32))).unwrap(),
            path: Some(name_policy::PortablePath::parse("report.txt").unwrap()),
            current_node_id: None,
            selected_branch_id: None,
        },
        query_hash: HistoryQueryHash::parse(&"b".repeat(64)).unwrap(),
        action_max_entries: u64::MAX,
        limit: HistoryPageLimit::new(limit).unwrap(),
        collection,
    }
}

fn node_id(index: usize) -> HistoryNodeId {
    HistoryNodeId::parse(&format!("ver_{index:032x}")).unwrap()
}
fn version_id(index: usize) -> HistoryVersionId {
    HistoryVersionId::parse(&format!("ver_{index:032x}")).unwrap()
}
fn branch_id(index: usize) -> HistoryBranchId {
    HistoryBranchId::parse(&format!("br_{index:032x}")).unwrap()
}
fn merge_id(index: usize) -> HistoryMergeId {
    HistoryMergeId::parse(&format!("fmerge_{index:032x}")).unwrap()
}
fn cursors_empty() -> HistoryPageCursors {
    HistoryPageCursors {
        previous_cursor: None,
        next_cursor: None,
    }
}
fn fields() -> HistoryNodeFields {
    HistoryNodeFields {
        message: HistoryFieldInfo {
            byte_length: u64::MAX,
        },
        author: HistoryFieldInfo { byte_length: 0 },
        created_at: HistoryFieldInfo { byte_length: 20 },
    }
}

fn summary(index: usize) -> HistoryNodeSummary {
    HistoryNodeSummary {
        node_id: node_id(index),
        version_id: version_id(index),
        kind: HistoryNodeKind::Content,
        sequence: u64::MAX,
        current: false,
        branch_id: None,
        path: "report.txt".into(),
        previous_path: None,
        note: String::new(),
        tags: Vec::new(),
        size: u64::MAX,
        content_hash: ContentHash::from_digest(blake3::hash(b"body")),
        content_state: Some(storage_contract::ContentState::Purged),
        download_name: "report.txt".into(),
        storage_name: "version.txt".into(),
        parents: HistoryParentSet::Complete,
        fields: fields(),
    }
}

fn nodes_page(
    nodes: Vec<HistoryNodeSummary>,
    edges: Vec<HistoryGraphEdge>,
    limit: usize,
) -> HistoryWindowPage {
    HistoryWindowPage::Nodes {
        identity: identity(HistoryCollection::Nodes, limit),
        context: HistoryContextSummary {
            note: String::new(),
            content: None,
        },
        nodes,
        edges,
        page: cursors_empty(),
    }
}

fn cursor(
    identity: &HistoryPageIdentity,
    collection: HistoryCollection,
    position: HistoryPosition,
) -> HistoryCursor {
    let limit = if collection == identity.collection {
        identity.limit
    } else {
        HistoryPageLimit::default_for(&collection)
    };
    HistoryCursor::from_payload(HistoryCursorPayload {
        format: HistoryCursorFormat::HistoryWindowV1,
        history_root: identity.history_root,
        history_tree_id: identity.context.history_tree_id.clone(),
        file_id: identity.file_id.clone(),
        collection,
        query_hash: identity.query_hash,
        limit,
        direction: HistoryDirection::Forward,
        position,
        anchor_node_id: None,
    })
    .unwrap()
}

#[test]
fn hard_json_writer_rejects_before_extension_at_exact_8mib() {
    assert_eq!(
        MAX_HISTORY_RESPONSE_BYTES,
        storage_contract::client_protocol::MAX_RESPONSE_BYTES
    );
    let mut buffer = PageBuffer::new(MAX_HISTORY_RESPONSE_BYTES);
    let block = vec![b'x'; MAX_HISTORY_RESPONSE_BYTES];
    buffer.write_all(&block).unwrap();
    assert_eq!(buffer.bytes.len(), MAX_HISTORY_RESPONSE_BYTES);
    assert!(buffer.write_all(b"!").is_err());
    assert_eq!(
        buffer.bytes, block,
        "a rejected write must not append a partial chunk"
    );
}

#[test]
fn escaped_json_bytes_are_counted_before_the_buffer_grows() {
    let text = "\0".repeat(8);
    let mut small = PageBuffer::new(text.len() + 2);
    assert!(json_codec::to_writer(&mut small, &text).is_err());
    assert!(small.bytes.len() <= text.len() + 2);
    let mut count = CountBytes {
        used: 0,
        limit: text.len() + 2,
    };
    assert!(json_codec::to_writer(&mut count, &text).is_err());
    assert!(count.used <= text.len() + 2);
    let mut exact = PageBuffer::new(8 * 6 + 2);
    json_codec::to_writer(&mut exact, &text).unwrap();
    assert_eq!(exact.bytes.len(), 8 * 6 + 2);
}

#[test]
fn maximum_utf8_field_fragments_preserve_all_bytes_and_include_escaping_and_cursors() {
    let text = "\0界".repeat(MAX_HISTORY_FIELD_BYTES / 4);
    assert_eq!(text.len(), MAX_HISTORY_FIELD_BYTES);
    let collection = HistoryCollection::NodeField {
        node_id: node_id(1),
        field: HistoryField::Message,
    };
    let identity = identity(collection, MAX_HISTORY_FIELD_BYTES);
    let next = cursor(
        &identity,
        identity.collection.clone(),
        HistoryPosition::Field {
            byte_offset: text.len() as u64,
        },
    );
    let page = HistoryWindowPage::NodeField {
        identity: identity.clone(),
        field: HistoryField::Message,
        total_bytes: (text.len() * 2) as u64,
        byte_offset: 0,
        text: text.clone(),
        end_of_field: false,
        next_cursor: Some(next),
    };
    let encoded = encode(&page).unwrap();
    assert!(encoded.len() > text.len());
    assert!(encoded.len() <= MAX_HISTORY_RESPONSE_BYTES);
    let decoded: HistoryWindowPage = storage_contract::decode_strict(&encoded).unwrap();
    assert_eq!(decoded, page);
    let tail = HistoryWindowPage::NodeField {
        identity,
        field: HistoryField::Message,
        total_bytes: (text.len() * 2) as u64,
        byte_offset: text.len() as u64,
        text,
        end_of_field: true,
        next_cursor: None,
    };
    let encoded = encode(&tail).unwrap();
    let wire: json_codec::Value = storage_contract::strict_json(&encoded).unwrap();
    assert_eq!(wire["end_of_field"], true);
    assert!(
        wire.get("complete").is_none(),
        "a tail does not claim ownership of earlier fragments"
    );
    assert_eq!(
        storage_contract::decode_strict::<HistoryWindowPage>(&encoded).unwrap(),
        tail
    );
}

#[test]
fn empty_fields_are_explicit_and_nonempty_fragments_must_advance() {
    let collection = HistoryCollection::NodeField {
        node_id: node_id(1),
        field: HistoryField::Author,
    };
    let identity = identity(collection, 4);
    let empty = HistoryWindowPage::NodeField {
        identity: identity.clone(),
        field: HistoryField::Author,
        total_bytes: 0,
        byte_offset: 0,
        text: String::new(),
        end_of_field: true,
        next_cursor: None,
    };
    assert!(encode(&empty).is_ok());
    for (total_bytes, byte_offset, text, end_of_field) in [
        (1, 0, "", false),
        (1, 1, "", true),
        (1, 0, "a", false),
        (1, 0, "aa", true),
        (u64::MAX, u64::MAX, "a", true),
    ] {
        let page = HistoryWindowPage::NodeField {
            identity: identity.clone(),
            field: HistoryField::Author,
            total_bytes,
            byte_offset,
            text: text.into(),
            end_of_field,
            next_cursor: None,
        };
        assert!(encode(&page).is_err());
    }
    let incorrect = cursor(
        &identity,
        identity.collection.clone(),
        HistoryPosition::Field { byte_offset: 2 },
    );
    let page = HistoryWindowPage::NodeField {
        identity,
        field: HistoryField::Author,
        total_bytes: 2,
        byte_offset: 0,
        text: "a".into(),
        end_of_field: false,
        next_cursor: Some(incorrect),
    };
    assert!(encode(&page).is_err());
}

#[test]
fn every_collection_enforces_its_record_limit_before_encoding() {
    let content = HistoryContentDisposition {
        version_id: Some(version_id(1)),
        source: HistoryContentSource::Owned,
        stored_state: None,
    };
    let branch = HistoryBranchSummary {
        branch_id: branch_id(1),
        name: "main".into(),
        head_commit_id: HistoryCommitId::parse(&format!("commit_{}", "1".repeat(32))).unwrap(),
        fork_commit_id: HistoryCommitId::parse(&format!("commit_{}", "0".repeat(32))).unwrap(),
        node_id: Some(node_id(1)),
        state: BranchState::Active,
        branch_kind: BranchKind::Full,
        generation: 1,
        current: true,
        read_only: false,
        alias_of: None,
        content,
    };
    let version = HistoryVersionSummary {
        version_id: version_id(1),
        version_sequence: 1,
        size: 1,
        content_hash: ContentHash::from_digest(blake3::hash(b"body")),
        content_state: None,
        note: String::new(),
        download_name: "file.txt".into(),
        storage_name: "stored.txt".into(),
        source_count: 0,
        fields: fields(),
    };
    let merge = HistoryMergeSummary {
        merge_id: merge_id(1),
        sequence: 1,
        chosen_node_id: node_id(1),
        result_node_id: node_id(1),
        surviving_branch_id: branch_id(1),
        reused_node: true,
        operation_id: OperationId::generate(),
        input_count: u64::MAX,
        parent_count: u64::MAX,
    };
    let pages = [
        nodes_page(vec![summary(1); 2], Vec::new(), 1),
        HistoryWindowPage::Versions {
            identity: identity(HistoryCollection::Versions, 1),
            versions: vec![version; 2],
            page: cursors_empty(),
        },
        HistoryWindowPage::Branches {
            identity: identity(HistoryCollection::Branches, 1),
            branches: vec![branch; 2],
            page: cursors_empty(),
        },
        HistoryWindowPage::Merges {
            identity: identity(HistoryCollection::Merges, 1),
            merges: vec![merge; 2],
            page: cursors_empty(),
        },
        HistoryWindowPage::NodeParents {
            identity: identity(
                HistoryCollection::NodeParents {
                    node_id: node_id(1),
                },
                1,
            ),
            parents: vec![
                HistoryParentMember {
                    parent_node_id: node_id(2)
                };
                2
            ],
            page: cursors_empty(),
        },
        HistoryWindowPage::VersionSources {
            identity: identity(
                HistoryCollection::VersionSources {
                    version_id: version_id(1),
                },
                1,
            ),
            sources: vec![
                HistorySourceMember {
                    ordinal: 0,
                    version_id: version_id(2)
                };
                2
            ],
            page: cursors_empty(),
        },
        HistoryWindowPage::MergeInputs {
            identity: identity(
                HistoryCollection::MergeInputs {
                    merge_id: merge_id(1),
                },
                1,
            ),
            inputs: vec![
                HistoryMergeInputMember {
                    ordinal: 0,
                    branch_id: branch_id(1),
                    node_id: node_id(1)
                };
                2
            ],
            page: cursors_empty(),
        },
        HistoryWindowPage::MergeParents {
            identity: identity(
                HistoryCollection::MergeParents {
                    merge_id: merge_id(1),
                },
                1,
            ),
            parents: vec![
                HistoryMergeParentMember {
                    ordinal: 0,
                    parent_node_id: node_id(1)
                };
                2
            ],
            page: cursors_empty(),
        },
    ];
    for page in pages {
        assert!(encode(&page).is_err());
    }
    assert!(
        encode(&nodes_page(
            Vec::new(),
            Vec::new(),
            HistoryCollection::Nodes.maximum_limit() + 1
        ))
        .is_err()
    );
}

#[test]
fn pending_branch_without_a_node_or_inherited_body_remains_readable() {
    let branch = HistoryBranchSummary {
        branch_id: branch_id(1),
        name: "pending".into(),
        head_commit_id: HistoryCommitId::parse(&format!("commit_{}", "1".repeat(32))).unwrap(),
        fork_commit_id: HistoryCommitId::parse(&format!("commit_{}", "0".repeat(32))).unwrap(),
        node_id: None,
        state: BranchState::Active,
        branch_kind: BranchKind::Full,
        generation: 1,
        current: true,
        read_only: false,
        alias_of: None,
        content: HistoryContentDisposition {
            version_id: None,
            source: HistoryContentSource::Inherited {
                source_version_id: None,
            },
            stored_state: None,
        },
    };
    let page = HistoryWindowPage::Branches {
        identity: identity(HistoryCollection::Branches, 1),
        branches: vec![branch],
        page: cursors_empty(),
    };
    let encoded = encode(&page).unwrap();
    let wire = storage_contract::strict_json(&encoded).unwrap();
    assert!(wire["branches"][0]["node_id"].is_null());
    assert!(wire["branches"][0]["content"]["source"]["source_version_id"].is_null());
    assert_eq!(
        storage_contract::decode_strict::<HistoryWindowPage>(&encoded).unwrap(),
        page
    );
}

#[test]
fn global_preview_budget_and_endpoint_window_state_are_enforced() {
    let edge = HistoryGraphEdge {
        node_id: node_id(1),
        parent_node_id: node_id(2),
        endpoint: HistoryEndpoint::OutsideWindow,
    };
    assert!(
        encode(&nodes_page(
            vec![summary(1)],
            vec![edge.clone(); MAX_HISTORY_PREVIEW_EDGES + 1],
            1
        ))
        .is_err()
    );
    let mut wrong_endpoint = edge.clone();
    wrong_endpoint.endpoint = HistoryEndpoint::InWindow;
    assert!(encode(&nodes_page(vec![summary(1)], vec![wrong_endpoint], 1)).is_err());
    let mut wrong_child = edge.clone();
    wrong_child.node_id = node_id(3);
    assert!(encode(&nodes_page(vec![summary(1)], vec![wrong_child], 1)).is_err());
    assert!(encode(&nodes_page(vec![summary(1)], vec![edge], 1)).is_ok());
}

#[test]
fn zero_preview_more_has_a_node_scoped_start_continuation() {
    let page_identity = identity(HistoryCollection::Nodes, 1);
    let continuation = cursor(
        &page_identity,
        HistoryCollection::NodeParents {
            node_id: node_id(1),
        },
        HistoryPosition::Start,
    );
    let mut value = summary(1);
    value.parents = HistoryParentSet::More {
        cursor: continuation,
    };
    assert!(encode(&nodes_page(vec![value.clone()], Vec::new(), 1)).is_ok());
    value.parents = HistoryParentSet::More {
        cursor: cursor(
            &page_identity,
            HistoryCollection::NodeParents {
                node_id: node_id(2),
            },
            HistoryPosition::Start,
        ),
    };
    assert!(encode(&nodes_page(vec![value], Vec::new(), 1)).is_err());
}

#[test]
fn maximum_inline_fields_and_node_rows_remain_within_encoded_budget() {
    let mut value = summary(1);
    value.path = format!("{}/{}", "界".repeat(80), "界".repeat(80));
    value.previous_path = Some(value.path.clone());
    value.note = "\\\"".repeat(storage_contract::MAX_NOTE_BYTES / 2);
    value.tags = (0..storage_contract::MAX_FILE_TAGS)
        .map(|index| {
            format!(
                "{index:02}{}",
                "\"".repeat(storage_contract::MAX_TAG_BYTES - 2)
            )
        })
        .collect();
    value.download_name = "界".repeat(80);
    value.storage_name = "s".repeat(240);
    let count = HistoryCollection::Nodes.maximum_limit();
    let page = nodes_page(
        (0..count)
            .map(|index| {
                let mut node = value.clone();
                node.node_id = node_id(index);
                node.version_id = version_id(index);
                node
            })
            .collect(),
        Vec::new(),
        count,
    );
    let encoded = encode(&page).unwrap();
    assert!(encoded.len() <= MAX_HISTORY_RESPONSE_BYTES);
    assert_eq!(
        storage_contract::decode_strict::<HistoryWindowPage>(&encoded).unwrap(),
        page
    );
}

#[test]
fn malformed_inline_fields_and_collection_mismatches_fail_instead_of_truncating() {
    let mut value = summary(1);
    value.note = "x".repeat(storage_contract::MAX_NOTE_BYTES + 1);
    assert!(encode(&nodes_page(vec![value], Vec::new(), 1)).is_err());
    let mut value = summary(1);
    value.tags = vec!["tag".into(); storage_contract::MAX_FILE_TAGS + 1];
    assert!(encode(&nodes_page(vec![value], Vec::new(), 1)).is_err());
    let mut value = summary(1);
    value.path = "x".repeat(769);
    assert!(encode(&nodes_page(vec![value], Vec::new(), 1)).is_err());
    let mut page = nodes_page(Vec::new(), Vec::new(), 1);
    let HistoryWindowPage::Nodes { identity, .. } = &mut page else {
        unreachable!()
    };
    identity.collection = HistoryCollection::Versions;
    assert!(encode(&page).is_err());
}

#[test]
fn current_action_allowance_does_not_shrink_the_history_read_window() {
    let mut page = nodes_page(vec![summary(1), summary(2)], Vec::new(), 2);
    let HistoryWindowPage::Nodes { identity, .. } = &mut page else {
        unreachable!()
    };
    identity.action_max_entries = 1;
    let encoded = encode(&page).unwrap();
    let wire = storage_contract::strict_json(&encoded).unwrap();
    assert_eq!(wire["identity"]["action_max_entries"], "1");
    assert_eq!(wire["nodes"].as_array().unwrap().len(), 2);
}

#[test]
fn page_continuations_include_the_exact_page_scope_and_query() {
    let mut page = nodes_page(vec![summary(1)], Vec::new(), 1);
    let HistoryWindowPage::Nodes {
        identity,
        page: cursors,
        ..
    } = &mut page
    else {
        unreachable!()
    };
    let mut payload = cursor(
        identity,
        HistoryCollection::Nodes,
        HistoryPosition::Node {
            sequence: 1,
            node_id: node_id(1),
        },
    )
    .payload()
    .clone();
    payload.history_root = HistoryRoot::parse(&"c".repeat(64)).unwrap();
    cursors.next_cursor = Some(HistoryCursor::from_payload(payload).unwrap());
    assert!(encode(&page).is_err());
}
