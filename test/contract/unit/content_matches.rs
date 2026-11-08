use super::*;
fn id(prefix: &str, n: u8) -> String {
    format!("{prefix}_{n:032x}")
}
fn request(limit: usize) -> ContentMatchRequest {
    ContentMatchRequest::parse(&id("file", 1), &id("ver", 1), &"a".repeat(64), None, limit).unwrap()
}
fn page() -> ContentMatchPage {
    let request = request(1);
    ContentMatchPage {
        file_id: request.file_id,
        source_node_id: request.node_id,
        source_version_id: HistoryVersionId::parse(&id("ver", 1)).unwrap(),
        history_tree_id: HistoryTreeId::parse(&id("history", 1)).unwrap(),
        history_root: request.history_root,
        logical_size: 3,
        content_hash: ContentHash::from_digest(blake3::hash(b"abc")),
        source_content_state: Some(ContentState::Available),
        matches: vec![ContentMatch {
            creation_name: "other.txt".into(),
            file_id: HistoryFileId::parse(&id("file", 2)).unwrap(),
            version_id: HistoryVersionId::parse(&id("ver", 2)).unwrap(),
            content_state: Some(ContentState::Purged),
        }],
        next_after_version_id: Some(HistoryVersionId::parse(&id("ver", 2)).unwrap()),
    }
}
#[test]
fn content_matches_parse_exact_bounded_request() {
    let req = request(64);
    crate::validate_api_path(&req.route()).unwrap();
    assert_eq!(req.query_fields()["limit"], "64");
    assert_eq!(req.history_request().history_root, Some(req.history_root));
    for limit in [0, 65, usize::MAX] {
        assert!(
            ContentMatchRequest::parse(&id("file", 1), &id("ver", 1), &"a".repeat(64), None, limit)
                .is_err()
        );
    }
    for after in ["bad", "fnode_00000000000000000000000000000000"] {
        assert!(
            ContentMatchRequest::parse(
                &id("file", 1),
                &id("ver", 1),
                &"a".repeat(64),
                Some(after),
                1
            )
            .is_err()
        );
    }
}
#[test]
fn content_matches_response_rejects_crossed_focus_duplicates_and_bad_cursor() {
    let req = request(1);
    let good = page();
    good.validate(&req).unwrap();
    let mut bad = good.clone();
    bad.file_id = HistoryFileId::parse(&id("file", 3)).unwrap();
    assert!(bad.validate(&req).is_err());
    let mut bad = good.clone();
    bad.matches[0].file_id = req.file_id.clone();
    assert!(bad.validate(&req).is_err());
    let mut bad = good.clone();
    bad.next_after_version_id = Some(HistoryVersionId::parse(&id("ver", 3)).unwrap());
    assert!(bad.validate(&req).is_err());
    let mut bad = good.clone();
    bad.logical_size = 0;
    assert!(bad.validate(&req).is_err());
    let mut bad = good.clone();
    bad.matches.push(bad.matches[0].clone());
    assert!(bad.validate(&request(2)).is_err());
    let continued = ContentMatchRequest::parse(
        &id("file", 1),
        &id("ver", 1),
        &"a".repeat(64),
        Some(&id("ver", 2)),
        1,
    )
    .unwrap();
    assert!(good.validate(&continued).is_err());
}
#[test]
fn content_matches_wire_is_strict_and_preserves_purged_metadata() {
    let page = page();
    let mut wire = json_codec::to_value(&page).unwrap();
    let decoded: ContentMatchPage = json_codec::from_value(wire.clone()).unwrap();
    assert_eq!(decoded, page);
    assert_eq!(wire["logical_size"], "3");
    wire["automatic_association"] = true.into();
    assert!(json_codec::from_value::<ContentMatchPage>(wire).is_err());
}

#[test]
fn content_matches_nullable_fields_are_required_and_source_version_is_exact() {
    let request = request(1);
    let original = page();
    for field in ["source_content_state", "next_after_version_id"] {
        let mut wire = json_codec::to_value(&original).unwrap();
        wire.as_object_mut().unwrap().remove(field);
        assert!(json_codec::from_value::<ContentMatchPage>(wire).is_err());
    }
    let mut wire = json_codec::to_value(&original).unwrap();
    wire["matches"][0]
        .as_object_mut()
        .unwrap()
        .remove("content_state");
    assert!(json_codec::from_value::<ContentMatchPage>(wire).is_err());
    let mut bad = original;
    bad.source_version_id = HistoryVersionId::parse(&id("ver", 3)).unwrap();
    assert!(bad.validate(&request).is_err());
}
