use super::*;
use json_codec::{Value, json};

fn snapshot(detail: ActivityDetailSnapshot) -> ActivityEventSnapshot {
    ActivityEventSnapshot {
        sequence: u64::MAX,
        event_id: "opaque:entry/path:0".into(),
        timestamp: Some(1_792_803_600),
        slot_id: "P01".into(),
        path: "Café/file.txt".into(),
        action: "future.persisted.kind".into(),
        principal: Some("owner".into()),
        file_id: "file".into(),
        message: String::new(),
        detail,
    }
}
fn wire(event: ActivityEventSnapshot) -> Value {
    let response = ActivityHistoryResponse::try_from(ActivityHistorySnapshot {
        events: vec![event],
    })
    .unwrap();
    storage_contract::strict_json(&json_codec::to_vec(&response).unwrap()).unwrap()
}

#[test]
fn activity_wire_preserves_all_row_shapes_and_null_versus_absent_fields() {
    let cases = [
        (
            ActivityDetailSnapshot::Plain { version_id: None },
            json!({"version_id":null}),
        ),
        (
            ActivityDetailSnapshot::Plain {
                version_id: Some("directory-entry-version".into()),
            },
            json!({"version_id":"directory-entry-version"}),
        ),
        (
            ActivityDetailSnapshot::Content(ContentDispositionSnapshot {
                version_id: Some("visible".into()),
                source_version_id: Some("source".into()),
                stored_state: Some(storage_contract::ContentState::Available),
            }),
            json!({"version_id":"visible","materialized":false,"inherited":true,"own_version_id":null,"source_version_id":"source","content_state":"PENDING","visible_content_state":"AVAILABLE"}),
        ),
        (
            ActivityDetailSnapshot::FileOperation {
                version_id: None,
                node_id: None,
                branch_id: None,
            },
            json!({"version_id":null,"node_id":null,"branch_id":null}),
        ),
        (
            ActivityDetailSnapshot::DirectoryOperation {
                version_id: None,
                directory_branch_id: "directory".into(),
                head_tree_id: None,
            },
            json!({"version_id":null,"directory_branch_id":"directory","head_tree_id":null}),
        ),
        (
            ActivityDetailSnapshot::Backup {
                version_id: None,
                target_copies: Some(3),
            },
            json!({"version_id":null,"target_copies":3}),
        ),
        (
            ActivityDetailSnapshot::Path {
                version_id: "version".into(),
                node_id: "path-node".into(),
                previous_path: "old.txt".into(),
                branch_id: "branch".into(),
            },
            json!({"version_id":"version","node_id":"path-node","previous_path":"old.txt","branch_id":"branch"}),
        ),
        (
            ActivityDetailSnapshot::Tree {
                version_id: None,
                directory_branch_id: "directory".into(),
                head_tree_id: "tree".into(),
                parent_tree_id: None,
            },
            json!({"version_id":null,"directory_branch_id":"directory","head_tree_id":"tree","parent_tree_id":null}),
        ),
    ];
    for (detail, expected_detail) in cases {
        let response = wire(snapshot(detail));
        assert_eq!(response["limit"], 200);
        let mut expected = json!({
            "sequence":u64::MAX.to_string(),"event_id":"opaque:entry/path:0",
            "timestamp":"2026-10-24T01:00:00Z","slot_id":"P01","path":"Café/file.txt",
            "action":"future.persisted.kind","principal":"owner","file_id":"file","message":"",
        });
        expected
            .as_object_mut()
            .unwrap()
            .extend(expected_detail.as_object().unwrap().clone());
        assert_eq!(response["events"], json!([expected]));
    }
}

#[test]
fn activity_wire_keeps_missing_timestamp_and_reports_unrepresentable_stored_seconds() {
    use std::error::Error;
    let mut event = snapshot(ActivityDetailSnapshot::Plain { version_id: None });
    event.timestamp = None;
    event.principal = None;
    let row = wire(event);
    assert!(row["events"][0]["timestamp"].is_null());
    assert!(row["events"][0]["principal"].is_null());
    let mut event = snapshot(ActivityDetailSnapshot::Plain { version_id: None });
    event.timestamp = Some(u64::MAX);
    let error = ActivityHistoryResponse::try_from(ActivityHistorySnapshot {
        events: vec![event],
    })
    .unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::StateInvalid);
    assert!(error.source().is_some());
}
