use super::*;
use json_codec::{Value, json};

fn root() -> String {
    "ab".repeat(32)
}

fn seed(kind: CursorKind) -> HistoryCursorSeed {
    HistoryCursorSeed::new(kind, &root(), "history_x", "file_y", &json!({"limit":50}))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

#[test]
fn every_cursor_kind_round_trips_and_rejects_foreign_or_tampered_cursors() {
    let node = NodeCursor::encode(
        &seed(CursorKind::Nodes),
        false,
        NodeCursorKey {
            sequence: u64::MAX,
            node_id: "ver_1".into(),
        },
    )
    .unwrap();
    let decoded: NodeCursor = decode(&node).unwrap();
    decoded.validate().unwrap();
    assert_eq!(decoded.key.sequence, u64::MAX);
    assert!(!decoded.backward);
    assert_eq!(decoded.root(), root());
    decoded.seed.admit(&seed(CursorKind::Nodes)).unwrap();
    // A different limit changes the request fingerprint.
    let other = HistoryCursorSeed::new(
        CursorKind::Nodes,
        &root(),
        "history_x",
        "file_y",
        &json!({"limit":51}),
    );
    let mismatch = decoded.seed.admit(&other).unwrap_err();
    assert_eq!(mismatch.message, "cursor does not match the history query");
    assert_eq!(mismatch.status, ::http::StatusCode::BAD_REQUEST);
    // Another file or history is a different scope even with the same limit.
    for (history, file) in [("history_z", "file_y"), ("history_x", "file_z")] {
        let scope = HistoryCursorSeed::new(
            CursorKind::Nodes,
            &root(),
            history,
            file,
            &json!({"limit":50}),
        );
        assert!(decoded.seed.admit_scope(&scope).is_err());
    }
    // The JSON is strict: an unknown field, a wrong kind and a bad root fail.
    let mut value: Value = json_codec::from_slice(&unhex(&node)).unwrap();
    assert_eq!(value["v"], 1);
    assert_eq!(value["kind"], "nodes");
    assert_eq!(value["key"]["sequence"], u64::MAX.to_string());
    value["extra"] = json!(true);
    let error = decode::<NodeCursor>(&hex(&json_codec::to_vec(&value).unwrap())).unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::InvalidCursor);
    // A node cursor cannot decode as another collection's cursor (the key
    // shape differs); even a shape-compatible foreign cursor fails validate.
    assert!(decode::<ParentCursor>(&node).is_err());
    let mut value: Value = json_codec::from_slice(&unhex(&node)).unwrap();
    value["kind"] = json!("merges");
    value["key"] = json!({"sequence":"1","merge_id":"m"});
    let foreign: MergeCursor = decode(&hex(&json_codec::to_vec(&value).unwrap())).unwrap();
    foreign.validate().unwrap();
    assert!(
        foreign.seed.admit(&seed(CursorKind::Nodes)).is_err(),
        "kind differs"
    );
    // The request digest is kind-independent by construction; the same
    // normalized query under the merges kind is the same continuation scope.
    foreign.seed.admit(&seed(CursorKind::Merges)).unwrap();
    let mut value: Value = json_codec::from_slice(&unhex(&node)).unwrap();
    value["root"] = json!("ZZ");
    let bad_root: NodeCursor = decode(&hex(&json_codec::to_vec(&value).unwrap())).unwrap();
    assert_eq!(
        bad_root.validate().unwrap_err().code,
        storage_contract::ErrorCode::InvalidCursor
    );
    let mut value: Value = json_codec::from_slice(&unhex(&node)).unwrap();
    value["v"] = json!(2);
    let bad_version: NodeCursor = decode(&hex(&json_codec::to_vec(&value).unwrap())).unwrap();
    assert!(bad_version.validate().is_err());
    let mut value: Value = json_codec::from_slice(&unhex(&node)).unwrap();
    value["key"]["node_id"] = json!("x".repeat(257));
    let long_id: NodeCursor = decode(&hex(&json_codec::to_vec(&value).unwrap())).unwrap();
    assert!(long_id.validate().is_err());
    for digest in ["Z".repeat(64), "0".repeat(63), "é".repeat(32)] {
        let mut value: Value = json_codec::from_slice(&unhex(&node)).unwrap();
        value["request"] = json!(digest);
        let error =
            decode_bound::<NodeCursor>(&hex(&json_codec::to_vec(&value).unwrap()), "file_y")
                .unwrap_err();
        assert_eq!(error.code, storage_contract::ErrorCode::InvalidCursor);
    }
    // Numbers must be canonical u64 text, never JSON numbers.
    let mut value: Value = json_codec::from_slice(&unhex(&node)).unwrap();
    value["key"]["sequence"] = json!(7);
    assert!(decode::<NodeCursor>(&hex(&json_codec::to_vec(&value).unwrap())).is_err());

    let parents = ParentCursor::encode(
        &seed(CursorKind::Parents),
        false,
        ParentCursorKey {
            node_id: "node".into(),
            after: Some("parent".into()),
        },
    )
    .unwrap();
    let decoded: ParentCursor = decode(&parents).unwrap();
    decoded.validate().unwrap();
    assert_eq!(decoded.key.after.as_deref(), Some("parent"));
    let restart = ParentCursor::encode(
        &seed(CursorKind::Parents),
        false,
        ParentCursorKey {
            node_id: "node".into(),
            after: None,
        },
    )
    .unwrap();
    let decoded: ParentCursor = decode(&restart).unwrap();
    decoded.validate().unwrap();
    assert!(decoded.key.after.is_none());

    let branches = BranchCursor::encode(
        &seed(CursorKind::Branches),
        false,
        BranchCursorKey {
            after: "br_1".into(),
        },
    )
    .unwrap();
    decode::<BranchCursor>(&branches)
        .unwrap()
        .validate()
        .unwrap();
    let merges = MergeCursor::encode(
        &seed(CursorKind::Merges),
        true,
        MergeCursorKey {
            sequence: 3,
            merge_id: "fmerge_1".into(),
        },
    )
    .unwrap();
    let decoded: MergeCursor = decode(&merges).unwrap();
    decoded.validate().unwrap();
    assert!(decoded.backward);
    let versions = VersionCursor::encode(
        &seed(CursorKind::Versions),
        false,
        VersionCursorKey {
            version_sequence: 9,
            version_id: "ver_9".into(),
        },
    )
    .unwrap();
    decode::<VersionCursor>(&versions)
        .unwrap()
        .validate()
        .unwrap();
    for cursor in [&node, &parents, &branches, &merges, &versions] {
        assert!(
            cursor
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        );
        assert!(cursor.len() <= super::super::limits::MAX_CURSOR_CHARS);
    }
}

#[test]
fn malformed_cursor_text_is_rejected_as_invalid_cursor() {
    for text in [
        String::new(),
        "a".into(),
        "zz".into(),
        "AB".into(),
        "00".into(),
        "7b7d".into(),
        "a".repeat(super::super::limits::MAX_CURSOR_CHARS + 2),
    ] {
        let error = decode::<NodeCursor>(&text).unwrap_err();
        assert_eq!(
            error.code,
            storage_contract::ErrorCode::InvalidCursor,
            "{text:.8}"
        );
        assert_eq!(error.status, ::http::StatusCode::BAD_REQUEST);
    }
}

#[test]
fn a_cursor_is_bound_to_the_request_file_before_any_state_lookup() {
    let cursor = NodeCursor::encode(
        &seed(CursorKind::Nodes),
        false,
        NodeCursorKey {
            sequence: 7,
            node_id: "ver_1".into(),
        },
    )
    .unwrap();
    let bound: NodeCursor = decode_bound(&cursor, "file_y").unwrap();
    assert_eq!(bound.key.sequence, 7);
    // The same bytes presented on another file's route are a malformed
    // request (400 INVALID_REQUEST), not a stale page (409 HISTORY_CHANGED).
    let foreign = decode_bound::<NodeCursor>(&cursor, "file_z").unwrap_err();
    assert_eq!(foreign.status, ::http::StatusCode::BAD_REQUEST);
    assert_eq!(foreign.code, storage_contract::ErrorCode::InvalidRequest);
    // A cursor of another collection kind fails its own validation first.
    let branch = BranchCursor::encode(
        &seed(CursorKind::Branches),
        false,
        BranchCursorKey {
            after: "br_1".into(),
        },
    )
    .unwrap();
    let kind = decode_bound::<NodeCursor>(&branch, "file_y").unwrap_err();
    assert_eq!(kind.status, ::http::StatusCode::BAD_REQUEST);
}
