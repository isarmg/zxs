use super::*;

#[test]
fn history_wire_keeps_numeric_facts_sparse_disposition_and_field_whitelist() {
    let data = Arc::new(HistoryVersionData {
        version_id: "version".into(),
        file_id: "file".into(),
        source_version_ids: vec![],
        branch_id: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        message: String::new(),
        size: u64::MAX,
        content_state: None,
        content_hash: storage_contract::ContentHash::from_digest(blake3::hash(b"body")),
        note: String::new(),
        download_name: "report.txt".into(),
        storage_name: "stored".into(),
    });
    let snapshot = FileHistorySnapshot {
        file_id: "file".into(),
        repository_id: "repository".into(),
        path: None,
        note: String::new(),
        current_node_id: None,
        selected_branch_id: None,
        versions: vec![HistoryVersion {
            data: Arc::clone(&data),
            sequence: u64::MAX,
            current: false,
        }],
        nodes: vec![HistoryNode::Content {
            version: HistoryVersion {
                data,
                sequence: 7,
                current: true,
            },
            node_id: "version".into(),
            kind: ContentNodeKind::Content,
            tags: BTreeSet::new(),
            parent_node_ids: vec![],
            path: "report.txt".into(),
            previous_path: None,
        }],
        branches: vec![],
        merges: vec![],
        content: None,
    };
    let wire = storage_contract::strict_json(
        &json_codec::to_vec(&FileHistoryResponse::from(snapshot)).unwrap(),
    )
    .unwrap();
    assert!(wire["selected_branch_id"].is_null());
    assert!(wire["current_node_id"].is_null());
    assert!(wire.get("version_id").is_none());
    // The old Value projection overwrote sequence/current for its content node.
    assert_eq!(wire["nodes"][0]["sequence"], "7");
    assert_eq!(wire["nodes"][0]["current"], true);
    assert_eq!(wire["versions"][0]["current"], false);
    let version = &wire["versions"][0];
    assert_eq!(version["sequence"], u64::MAX.to_string());
    assert_eq!(version["size"], u64::MAX.to_string());
    assert_eq!(version["content_state"], "LIVE");
    assert_eq!(version["message"], "");
    assert!(version["branch_id"].is_null());
    assert_eq!(
        version
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec![
            "branch_id",
            "content_hash",
            "content_state",
            "created_at",
            "current",
            "download_name",
            "file_id",
            "message",
            "note",
            "sequence",
            "size",
            "source_version_ids",
            "storage_name",
            "version_id",
        ]
    );
}

#[test]
fn file_history_context_requires_a_paired_lookup_path() {
    assert!(
        ReadFileHistory::parse("file".into(), None, None)
            .unwrap()
            .location
            .is_none()
    );
    for (repository, path) in [(Some("repo".into()), None), (None, Some("file.txt".into()))] {
        assert!(ReadFileHistory::parse("file".into(), repository, path).is_err());
    }
    let request = ReadFileHistory::parse(
        "file".into(),
        Some("repo".into()),
        Some("Cafe\u{301}/FILE.txt".into()),
    )
    .unwrap();
    assert_eq!(request.location().unwrap().1.key(), "café/file.txt");
    assert!(
        ReadFileHistory::parse(
            "file".into(),
            Some("repo".into()),
            Some("../file.txt".into())
        )
        .is_err()
    );
}
