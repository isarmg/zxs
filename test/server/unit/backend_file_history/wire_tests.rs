use super::super::wire;
use super::*;
use crate::backend_file_history::{
    BoundedText, Continuation, CursorKind, FileHistoryPage, HistoryContext, HistoryCursorSeed,
    HistoryNodeKind, HistoryNodeSummary, NodeCursor, NodeFieldName, NodeParentsPage, ParentCursor,
    ParentCursorKey, ParentEdge, ParentPreview, cursors,
};
use json_codec::{Value, json};
use std::collections::BTreeSet;
use storage_contract::client_protocol::{MAX_HISTORY_PAGE_BYTES, MAX_RESPONSE_BYTES};

fn root() -> String {
    "cd".repeat(32)
}

fn continuation(kind: CursorKind, more_before: bool, more_after: bool) -> Continuation {
    Continuation {
        direction: crate::backend_file_history::PageDirection::Forward,
        seed: HistoryCursorSeed::new(kind, &root(), "history", "file", &json!({"limit":50})),
        more_before,
        more_after,
    }
}

fn text(bytes: usize) -> BoundedText {
    let inline = (bytes <= storage_contract::client_protocol::MAX_INLINE_TEXT_BYTES)
        .then(|| "m".repeat(bytes));
    BoundedText { bytes, inline }
}

/// The largest legal node record: maximum path (768 bytes of multibyte text
/// escaped to its JSON form), a maximal note, 16 maximal tags, maximal inline
/// texts, 32 preview edges with 256-byte ids and a maximal-length cursor.
fn maximal_node(index: usize) -> HistoryNodeSummary {
    let path = "\u{10FFFF}".repeat(192); // 768 UTF-8 bytes
    let note = "\u{00E9}".repeat(64); // 128 UTF-8 bytes
    let tags = (0..storage_contract::MAX_FILE_TAGS)
        .map(|tag| format!("{tag:02}{}", "\u{4E2D}".repeat(42)))
        .collect::<BTreeSet<_>>();
    let id = |prefix: &str| format!("{prefix}{}", "i".repeat(256 - prefix.len()));
    HistoryNodeSummary {
        node_id: id(&format!("node{index}-")),
        kind: HistoryNodeKind::Merge,
        version_id: id("version-"),
        sequence: u64::MAX,
        created_at: "2026-10-28T00:00:00Z".into(),
        branch_id: Some(id("branch-")),
        path: path.clone(),
        previous_path: Some(path),
        note,
        tags,
        size: u64::MAX,
        content_hash: storage_contract::ContentHash::from_digest(blake3::hash(b"max")),
        content_state: Some(storage_contract::ContentState::Migrated),
        download_name: "\u{10FFFF}".repeat(63),
        storage_name: "v999999_20261028T000000Z_ffffffff.txt".into(),
        operation_id: Some(storage_contract::OperationId::generate()),
        author: text(storage_contract::client_protocol::MAX_INLINE_TEXT_BYTES),
        message: text(storage_contract::client_protocol::MAX_INLINE_TEXT_BYTES),
        parents: ParentPreview::More {
            edges: (0..limits::MAX_PARENT_PREVIEW_PER_NODE)
                .map(|edge| ParentEdge {
                    node_id: id(&format!("parent{edge:02}-")),
                    in_window: edge % 2 == 0,
                })
                .collect(),
            cursor: "f".repeat(limits::MAX_CURSOR_CHARS),
        },
    }
}

fn page(nodes: Vec<HistoryNodeSummary>, more_before: bool, more_after: bool) -> FileHistoryPage {
    FileHistoryPage {
        file_id: "file".into(),
        repository_id: "repo".into(),
        history_tree_id: "history".into(),
        history_root: root(),
        context: HistoryContext {
            path: Some("\u{10FFFF}".repeat(192)),
            note: "n".repeat(128),
            current_node_id: Some("x".repeat(256)),
            selected_branch_id: Some("b".repeat(256)),
            content: Some(crate::ContentDispositionSnapshot {
                version_id: Some("v".repeat(256)),
                source_version_id: Some("s".repeat(256)),
                stored_state: None,
            }),
        },
        nodes,
        continuation: continuation(CursorKind::Nodes, more_before, more_after),
        anchor: None,
        limit: limits::NODES_MAX,
    }
}

fn decode(page: &EncodedPage) -> Value {
    storage_contract::strict_json(page.body()).unwrap()
}

#[test]
fn the_largest_legal_node_record_fits_the_minimal_page_and_the_full_page_the_response_cap() {
    let single = page(vec![maximal_node(0)], true, true);
    let record =
        json_codec::to_vec(&wire::HistoryNodeResponse::new(&single.nodes[0], None)).unwrap();
    let encoded = single.encode(PageBudget::PROTOCOL).unwrap();
    assert_eq!(encoded.admitted(), 1);
    assert!(record.len() + 2 * limits::MAX_CURSOR_CHARS + 8192 < MAX_HISTORY_PAGE_BYTES);
    assert!(encoded.body().len() < MAX_HISTORY_PAGE_BYTES);
    // Every record of a full page at the protocol maximum still fits the
    // page budget, and the page budget is below the client receive limit.
    let full = page(
        (0..limits::NODES_MAX).map(maximal_node).collect(),
        true,
        true,
    );
    let encoded = full.encode(PageBudget::PROTOCOL).unwrap();
    assert!(MAX_HISTORY_PAGE_BYTES < MAX_RESPONSE_BYTES);
    assert!(
        encoded.body().len() <= MAX_HISTORY_PAGE_BYTES,
        "{}",
        encoded.body().len()
    );
    assert!(encoded.admitted() >= 1);
    let value = decode(&encoded);
    assert_eq!(value["nodes"].as_array().unwrap().len(), encoded.admitted());
    assert_eq!(
        value["budget"],
        json!({"limit":limits::NODES_MAX,"parent_edges":limits::MAX_PAGE_PARENT_EDGES})
    );
}

#[test]
fn byte_budget_truncates_from_the_tail_and_keeps_a_forward_cursor_from_the_last_admitted_record() {
    let nodes = (0..6)
        .map(|index| HistoryNodeSummary {
            node_id: format!("node-{index}"),
            kind: HistoryNodeKind::Content,
            version_id: format!("node-{index}"),
            sequence: 100 - index as u64,
            created_at: "2026-10-28T00:00:00Z".into(),
            branch_id: None,
            path: "p.txt".into(),
            previous_path: None,
            note: String::new(),
            tags: BTreeSet::new(),
            size: 1,
            content_hash: storage_contract::ContentHash::from_digest(blake3::hash(b"n")),
            content_state: None,
            download_name: "p.txt".into(),
            storage_name: "s".into(),
            operation_id: None,
            author: text(5),
            message: text(600),
            parents: ParentPreview::Complete(vec![]),
        })
        .collect::<Vec<_>>();
    let records = nodes
        .iter()
        .map(|node| {
            json_codec::to_vec(&wire::HistoryNodeResponse::new(node, None))
                .unwrap()
                .len()
        })
        .collect::<Vec<_>>();
    let mut full = page(nodes, false, false);
    let head_bytes = {
        let encoded = page(vec![], false, false)
            .encode(PageBudget::PROTOCOL)
            .unwrap();
        encoded.body().len()
    };
    // Budget for exactly three records plus the shell reserve.
    // The node page also reserves one byte per possible edge flag flip.
    let reserve = 2 * limits::MAX_CURSOR_CHARS + head_bytes + 1024 + limits::MAX_PAGE_PARENT_EDGES;
    let three = reserve + records[..3].iter().sum::<usize>() + 2;
    let encoded = full.encode(PageBudget::with_max_bytes(three)).unwrap();
    assert_eq!(encoded.admitted(), 3);
    let value = decode(&encoded);
    assert_eq!(
        value["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|node| node["node_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["node-0", "node-1", "node-2"]
    );
    assert!(value["previous_cursor"].is_null());
    let next = value["next_cursor"]
        .as_str()
        .expect("truncated pages continue forward");
    let cursor: NodeCursor = cursors::decode(next).unwrap();
    assert_eq!(cursor.key.node_id, "node-2");
    assert_eq!(cursor.key.sequence, 98);
    assert!(!cursor.backward);
    // A budget below one record rejects, rather than violating the cap.
    assert!(full.encode(PageBudget::with_max_bytes(1)).is_err());
    // The whole page fits: no truncation, no spurious cursors.
    let whole = full.encode(PageBudget::PROTOCOL).unwrap();
    assert_eq!(whole.admitted(), 6);
    let value = decode(&whole);
    assert!(value["next_cursor"].is_null() && value["previous_cursor"].is_null());
    assert_eq!(value["context"]["path"], "\u{10FFFF}".repeat(192));
    assert_eq!(value["context"]["visible_content_state"], "LIVE");
    assert!(value["anchor"].is_null());

    // Reverse traversal must take the nearest suffix, not the same prefix:
    // otherwise following only previous_cursor would skip node-3..node-5.
    full.continuation.direction = crate::PageDirection::Backward;
    let reverse = decode(&full.encode(PageBudget::with_max_bytes(three)).unwrap());
    assert_eq!(
        reverse["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|node| node["node_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["node-3", "node-4", "node-5"]
    );
    let previous: NodeCursor =
        cursors::decode(reverse["previous_cursor"].as_str().unwrap()).unwrap();
    assert!(previous.backward);
    assert_eq!(previous.key.node_id, "node-3");
    assert!(reverse["next_cursor"].is_null());
}

#[test]
fn shell_fields_are_exact_and_context_without_an_entry_omits_content_fields() {
    let mut empty = page(vec![], true, true);
    empty.context = HistoryContext {
        path: None,
        note: String::new(),
        current_node_id: None,
        selected_branch_id: None,
        content: None,
    };
    let value = decode(&empty.encode(PageBudget::PROTOCOL).unwrap());
    assert_eq!(
        value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        [
            "anchor",
            "budget",
            "context",
            "file_id",
            "history_root",
            "history_tree_id",
            "next_cursor",
            "nodes",
            "previous_cursor",
            "repository_id"
        ]
    );
    assert_eq!(
        value["context"],
        json!({"path":null,"note":"","current_node_id":null,"selected_branch_id":null})
    );
    // Empty selections never mint cursors even when the selector reported more.
    assert!(value["previous_cursor"].is_null() && value["next_cursor"].is_null());
}

#[test]
fn parent_pages_and_slices_encode_exclusive_cursors_and_next_offsets() {
    let parents = NodeParentsPage {
        file_id: "file".into(),
        history_tree_id: "history".into(),
        history_root: root(),
        node_id: "node".into(),
        parents: vec!["a".into(), "b".into(), "c".into()],
        continuation: continuation(CursorKind::Parents, true, true),
    };
    let value = decode(&parents.encode(PageBudget::PROTOCOL).unwrap());
    assert_eq!(
        value["parents"],
        json!([{"node_id":"a"},{"node_id":"b"},{"node_id":"c"}])
    );
    let next: ParentCursor = cursors::decode(value["next_cursor"].as_str().unwrap()).unwrap();
    assert_eq!(
        next.key,
        ParentCursorKey {
            node_id: "node".into(),
            after: Some("c".into())
        }
    );
    let previous: ParentCursor =
        cursors::decode(value["previous_cursor"].as_str().unwrap()).unwrap();
    assert!(previous.backward);
    assert_eq!(previous.key.after.as_deref(), Some("a"));
    let slice = IndexedSlice {
        file_id: "file".into(),
        history_tree_id: "history".into(),
        history_root: root(),
        subject_id: "merge".into(),
        offset: 2,
        total: 5,
        items: vec![
            ("branch-a".to_owned(), "node-a".to_owned()),
            ("branch-b".to_owned(), "node-b".to_owned()),
        ],
    };
    let value = decode(&slice.encode_inputs(PageBudget::PROTOCOL).unwrap());
    assert_eq!(
        value["inputs"],
        json!([{"index":2,"branch_id":"branch-a","node_id":"node-a"},{"index":3,"branch_id":"branch-b","node_id":"node-b"}])
    );
    assert_eq!(value["next_offset"], 4);
    assert_eq!(value["merge_id"], "merge");
    assert!(value.get("version_id").is_none());
    let last = IndexedSlice {
        items: vec!["z".to_owned()],
        offset: 4,
        ..slice_sources()
    };
    let value = decode(&last.encode_sources(PageBudget::PROTOCOL).unwrap());
    assert_eq!(value["sources"], json!([{"index":4,"version_id":"z"}]));
    assert!(value["next_offset"].is_null());
    assert_eq!(value["version_id"], "version");
    let beyond = IndexedSlice {
        items: vec![],
        offset: 9,
        ..slice_sources()
    };
    let value = decode(&beyond.encode_sources(PageBudget::PROTOCOL).unwrap());
    assert_eq!(value["sources"], json!([]));
    assert!(value["next_offset"].is_null());
}

fn slice_sources() -> IndexedSlice<String> {
    IndexedSlice {
        file_id: "file".into(),
        history_tree_id: "history".into(),
        history_root: root(),
        subject_id: "version".into(),
        offset: 0,
        total: 5,
        items: vec![],
    }
}

#[test]
fn field_slices_report_next_offset_only_while_incomplete() {
    let slice = NodeFieldSlice {
        file_id: "file".into(),
        history_tree_id: "history".into(),
        history_root: root(),
        node_id: "node".into(),
        field: NodeFieldName::Message,
        total_bytes: 10,
        offset: 2,
        text: "abc".into(),
        complete: false,
    };
    let value: Value = decode(&slice.encode().unwrap());
    assert_eq!(value["next_offset"], 5);
    assert_eq!(value["field"], "message");
    let done = NodeFieldSlice {
        complete: true,
        offset: 0,
        total_bytes: 0,
        text: String::new(),
        ..slice
    };
    let value: Value = decode(&done.encode().unwrap());
    assert!(value["next_offset"].is_null());
    assert_eq!(value["text"], "");
}

#[test]
fn budget_truncation_moves_edges_into_dropped_nodes_outside_the_window() {
    // node-0's parent is node-2; both are selected, so the projection marked
    // the edge in_window. A budget admitting only two records drops node-2.
    let node = |index: u64, parents: Vec<ParentEdge>| HistoryNodeSummary {
        node_id: format!("node-{index}"),
        kind: HistoryNodeKind::Content,
        version_id: format!("node-{index}"),
        sequence: 100 - index,
        created_at: "2026-10-28T00:00:00Z".into(),
        branch_id: None,
        path: "p.txt".into(),
        previous_path: None,
        note: String::new(),
        tags: BTreeSet::new(),
        size: 1,
        content_hash: storage_contract::ContentHash::from_digest(blake3::hash(b"n")),
        content_state: None,
        download_name: "p.txt".into(),
        storage_name: "s".into(),
        operation_id: None,
        author: text(5),
        message: text(600),
        parents: ParentPreview::Complete(parents),
    };
    let nodes = vec![
        node(
            0,
            vec![
                ParentEdge {
                    node_id: "node-1".into(),
                    in_window: true,
                },
                ParentEdge {
                    node_id: "node-2".into(),
                    in_window: true,
                },
            ],
        ),
        node(1, vec![]),
        node(2, vec![]),
    ];
    let lengths = nodes
        .iter()
        .map(|node| {
            json_codec::to_vec(&wire::HistoryNodeResponse::new(node, None))
                .unwrap()
                .len()
        })
        .collect::<Vec<_>>();
    let mut full = page(nodes, false, false);
    let head_bytes = page(vec![], false, false)
        .encode(PageBudget::PROTOCOL)
        .unwrap()
        .body()
        .len();
    let reserve = 2 * limits::MAX_CURSOR_CHARS + head_bytes + 1024 + limits::MAX_PAGE_PARENT_EDGES;
    let two = reserve + lengths[0] + lengths[1] + 1;
    let encoded = full.encode(PageBudget::with_max_bytes(two)).unwrap();
    assert_eq!(encoded.admitted(), 2);
    let value = decode(&encoded);
    let edges = value["nodes"][0]["parents"]["edges"].as_array().unwrap();
    assert_eq!(edges[0], json!({"node_id":"node-1","in_window":true}));
    assert_eq!(edges[1], json!({"node_id":"node-2","in_window":false}));
    // The page still respects its byte budget after the flag flip.
    assert!(
        encoded.body().len() <= two,
        "{} > {two}",
        encoded.body().len()
    );
    // `current` follows the context's current node only.
    for node in value["nodes"].as_array().unwrap() {
        assert_eq!(node["current"], json!(false));
    }
    full.nodes[2].parents = ParentPreview::Complete(vec![
        ParentEdge {
            node_id: "node-0".into(),
            in_window: true,
        },
        ParentEdge {
            node_id: "node-1".into(),
            in_window: true,
        },
    ]);
    full.continuation.direction = crate::PageDirection::Backward;
    full.anchor = Some(AnchorPlacement {
        node_id: "node-0".into(),
        in_page: true,
    });
    let last_length = json_codec::to_vec(&wire::HistoryNodeResponse::new(&full.nodes[2], None))
        .unwrap()
        .len();
    let reverse_budget = reserve + lengths[1] + last_length + 1;
    let reverse = full
        .encode(PageBudget::with_max_bytes(reverse_budget))
        .unwrap();
    assert_eq!(reverse.admitted(), 2);
    let value = decode(&reverse);
    assert_eq!(value["nodes"][0]["node_id"], "node-1");
    assert_eq!(
        value["nodes"][1]["parents"]["edges"],
        json!([
            {"node_id":"node-0","in_window":false},
            {"node_id":"node-1","in_window":true}
        ])
    );
    assert_eq!(value["anchor"]["in_page"], false);
    assert!(reverse.body().len() <= reverse_budget);
}

/// Representative pages shared with the Web decoder through
/// test/fixtures/file-history-window.json: the server's real encoder output
/// must equal these bytes, and the Web test decodes the same file.
fn shared_window_pages() -> Vec<(&'static str, FileHistoryPage)> {
    let hash = |seed: &[u8]| storage_contract::ContentHash::from_digest(blake3::hash(seed));
    let operation = |hex: &str| hex.parse::<storage_contract::OperationId>().unwrap();
    let content = HistoryNodeSummary {
        node_id: "ver_00000000000000000000000000000003".into(),
        kind: HistoryNodeKind::Content,
        version_id: "ver_00000000000000000000000000000003".into(),
        sequence: 9,
        created_at: "2026-10-28T00:00:09Z".into(),
        branch_id: Some("br_00000000000000000000000000000001".into()),
        path: "docs/报告.txt".into(),
        previous_path: None,
        note: "最新".into(),
        tags: BTreeSet::from(["release".to_owned()]),
        size: 18_446_744_073_709_551_615,
        content_hash: hash(b"third"),
        content_state: None,
        download_name: "报告.txt".into(),
        storage_name: "v3_20261028T000009Z_00000003.txt".into(),
        operation_id: None,
        author: BoundedText {
            bytes: 5,
            inline: Some("admin".into()),
        },
        message: BoundedText {
            bytes: 300,
            inline: None,
        },
        parents: ParentPreview::Complete(vec![
            ParentEdge {
                node_id: "fnode_00000000000000000000000000000002".into(),
                in_window: true,
            },
            ParentEdge {
                node_id: "ver_00000000000000000000000000000001".into(),
                in_window: false,
            },
        ]),
    };
    let rename = HistoryNodeSummary {
        node_id: "fnode_00000000000000000000000000000002".into(),
        kind: HistoryNodeKind::Rename,
        version_id: "ver_00000000000000000000000000000002".into(),
        sequence: 7,
        created_at: "2026-10-28T00:00:07Z".into(),
        branch_id: Some("br_00000000000000000000000000000001".into()),
        path: "docs/报告.txt".into(),
        previous_path: Some("报告.txt".into()),
        note: String::new(),
        tags: BTreeSet::new(),
        size: 4,
        content_hash: hash(b"second"),
        content_state: Some(storage_contract::ContentState::Purged),
        download_name: "报告.txt".into(),
        storage_name: "v2_20261028T000005Z_00000002.txt".into(),
        operation_id: Some(operation("op_0000000000000000000000000000000a")),
        author: BoundedText {
            bytes: 5,
            inline: Some("admin".into()),
        },
        message: BoundedText {
            bytes: 0,
            inline: Some(String::new()),
        },
        parents: ParentPreview::More {
            edges: vec![],
            cursor: ParentCursor::encode(
                &continuation(CursorKind::Parents, false, false).seed,
                false,
                ParentCursorKey {
                    node_id: "fnode_00000000000000000000000000000002".into(),
                    after: None,
                },
            )
            .unwrap(),
        },
    };
    let live = FileHistoryPage {
        file_id: "file".into(),
        repository_id: "repo".into(),
        history_tree_id: "history".into(),
        history_root: root(),
        context: HistoryContext {
            path: Some("docs/报告.txt".into()),
            note: "文件备注".into(),
            current_node_id: Some("ver_00000000000000000000000000000003".into()),
            selected_branch_id: Some("br_00000000000000000000000000000001".into()),
            content: Some(crate::ContentDispositionSnapshot {
                version_id: Some("ver_00000000000000000000000000000003".into()),
                source_version_id: None,
                stored_state: None,
            }),
        },
        nodes: vec![content, rename],
        continuation: continuation(CursorKind::Nodes, false, true),
        anchor: Some(crate::backend_file_history::AnchorPlacement {
            node_id: "fnode_00000000000000000000000000000002".into(),
            in_page: true,
        }),
        limit: 2,
    };
    let deleted = FileHistoryPage {
        context: HistoryContext {
            path: None,
            note: String::new(),
            current_node_id: None,
            selected_branch_id: None,
            content: None,
        },
        nodes: vec![],
        continuation: continuation(CursorKind::Nodes, false, false),
        anchor: None,
        limit: 50,
        ..page(vec![], false, false)
    };
    vec![("live", live), ("deleted", deleted)]
}

#[test]
fn encoded_node_pages_match_the_shared_web_window_fixtures() {
    let fixtures =
        storage_contract::strict_json(include_bytes!("../../../fixtures/file-history-window.json"))
            .unwrap();
    let fixtures = fixtures.as_object().unwrap();
    let pages = shared_window_pages();
    assert_eq!(fixtures.len(), pages.len());
    for (case, page) in pages {
        let encoded = page.encode(PageBudget::PROTOCOL).unwrap();
        assert_eq!(&decode(&encoded), &fixtures[case], "{case}");
    }
}

#[test]
fn lazy_wire_budget_stops_before_later_candidates_and_caps_first_record_allocation() {
    use std::cell::Cell;
    struct Counted<'a> {
        visits: &'a Cell<usize>,
        text: &'a str,
    }
    impl data_serialization::Serialize for Counted<'_> {
        fn serialize<S: data_serialization::Serializer>(
            &self,
            serializer: S,
        ) -> Result<S::Ok, S::Error> {
            self.visits.set(self.visits.get() + 1);
            serializer.serialize_str(self.text)
        }
    }
    let visits = Cell::new(0);
    let candidates = (0..500).map(|_| Counted {
        visits: &visits,
        text: "12345678",
    });
    let admitted = prepare_records(
        PageBudget::with_max_bytes(21),
        0,
        candidates,
        crate::backend_file_history::PageDirection::Forward,
    )
    .unwrap();
    assert_eq!(admitted.len(), 2);
    assert_eq!(
        visits.get(),
        3,
        "stop immediately after the first candidate that cannot fit"
    );
    let mut writer = BoundedWriter {
        bytes: Vec::new(),
        limit: 127,
        exceeded: false,
    };
    assert!(json_codec::to_writer(&mut writer, &"\0".repeat(1024 * 1024)).is_err());
    assert!(writer.exceeded);
    assert!(writer.bytes.len() <= 127 && writer.bytes.capacity() <= 127);
    assert!(
        prepare_records(
            PageBudget::with_max_bytes(1),
            0,
            ["large"],
            crate::backend_file_history::PageDirection::Forward
        )
        .is_err()
    );
}

#[tokio::test]
async fn response_chunks_retain_admission_after_body_eof_until_transport_owners_drop() {
    use futures::StreamExt;
    let slots = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
    let permit = slots.clone().try_acquire_owned().unwrap();
    let limits = crate::TransferLimits::default();
    let response = EncodedPage {
        body: vec![b'x'; 100_000],
        admitted: 1,
    }
    .into_leased_response(
        permit,
        &limits,
        tokio::time::Instant::now() + limits.total_timeout,
        None,
    );
    let mut stream = response.into_body().into_data_stream();
    let mut transport_queue = Vec::new();
    while let Some(chunk) = stream.next().await {
        transport_queue.push(chunk.unwrap());
    }
    drop(stream);
    assert_eq!(
        slots.available_permits(),
        0,
        "EOF must not release queued response buffers"
    );
    assert!(transport_queue.iter().all(|chunk| chunk.len() <= 64 * 1024));
    let last = transport_queue.pop().unwrap();
    drop(transport_queue);
    assert_eq!(
        slots.available_permits(),
        0,
        "the last queued chunk owns the same response lease"
    );
    drop(last);
    assert_eq!(slots.available_permits(), 1);
}
