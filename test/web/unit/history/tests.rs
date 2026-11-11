use super::*;

#[test]
fn current_directory_git_operations_have_specific_labels() {
    for (action, expected) in [
        ("directory.git.initialize", "Initialized folder repository"),
        (
            "directory.git.stage-owned",
            "Staged this repository's changes",
        ),
        (
            "directory.git.stage-child",
            "Staged child repository reference",
        ),
        ("directory.git.checkout", "Switched folder branch"),
        ("directory.git.branch-create", "Created folder branch"),
        ("directory.git.branch-create-history", "Created folder branch from history"),
        ("directory.changes.commit", "Committed folder changes"),
    ] {
        assert_eq!(action_label(action), expected, "{action}");
    }
}

#[test]
fn branch_registration_is_distinct_from_legacy_content_creation() {
    assert_eq!(action_label("file_branch_pending"), "Registered branch");
    assert_eq!(action_label("branch_pending"), "Registered branch");
    assert_eq!(action_label("file_branch"), "Created file branch");
    assert_eq!(
        action_label("directory.branch.rename"),
        "Renamed folder branch"
    );
    assert_eq!(
        action_label("directory.default.select"),
        "Switched SMB default folder branch"
    );
    assert_eq!(
        action_label("directory.branches.create"),
        "Registered folder branch"
    );
    assert_eq!(
        action_label("directory.branches.select"),
        "Switched folder branch"
    );
    assert_eq!(action_label("directory.tree.write"), "Wrote folder subtree");
}

thread_local! {
    static PROJECTIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
pub(super) fn record_projection() {
    PROJECTIONS.set(PROJECTIONS.get() + 1);
}

#[test]
fn unrelated_updates_do_not_reproject_history_and_filters_preserve_order() {
    Owner::new().with(|| {
        PROJECTIONS.set(0);
        let state = RwSignal::new(State::default());
        state.update(|s| {
            s.activity.events = (0..200)
                .map(|i| event(if i % 2 == 0 { "P01" } else { "P02" }, &i.to_string()))
                .collect::<Vec<_>>()
                .into();
        });
        let events = visible_events(state);
        assert_eq!(events.with_untracked(Vec::len), 200);
        for i in 0..100 {
            state.update(|s| {
                s.management.loading = i % 2 == 0;
                s.session_notice = i.to_string();
                s.activity.loading = i % 2 == 0;
            });
            assert_eq!(events.with_untracked(Vec::len), 200);
        }
        assert_eq!(PROJECTIONS.get(), 1);

        state.update(|s| s.activity.disk = "P02".into());
        assert_eq!(
            events.with_untracked(|events| events
                .iter()
                .map(|event| event.path.clone())
                .collect::<Vec<_>>()),
            (0..200)
                .filter(|i| i % 2 == 1)
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
        );
        assert_eq!(PROJECTIONS.get(), 2);

        state.update(|s| {
            s.activity.events =
                vec![event("P02", "z"), event("P01", "hidden"), event("P02", "a")].into();
        });
        assert_eq!(
            events.with_untracked(|events| events
                .iter()
                .map(|event| event.path.clone())
                .collect::<Vec<_>>()),
            ["z", "a"]
        );
        assert_eq!(PROJECTIONS.get(), 3);

        state.update(|s| s.activity.disk = "missing".into());
        assert!(events.with_untracked(Vec::is_empty));
        assert_eq!(PROJECTIONS.get(), 4);
        state.update(|s| s.activity.disk.clear());
        assert_eq!(events.with_untracked(Vec::len), 3);
        assert_eq!(PROJECTIONS.get(), 5);
        state.update(|s| s.activity.events = Default::default());
        assert!(events.with_untracked(Vec::is_empty));
        assert_eq!(PROJECTIONS.get(), 6);
    });
}

fn event(slot: &str, path: &str) -> Event {
    Event {
        timestamp: None,
        slot_id: slot.into(),
        path: path.into(),
        action: "file.path.rename".into(),
        principal: None,
    }
}

fn response() -> Value {
    json_codec::json!({"events": [{
        "sequence": "1", "event_id": "commit-1", "timestamp": null,
        "slot_id": "P01", "path": "a", "action": "open-action-name",
        "principal": null, "file_id": "file-1", "message": "",
        "version_id": null
    }], "limit": 200})
}

#[test]
fn current_activity_response_accepts_empty_and_explicit_null() {
    assert!(
        decode(json_codec::json!({"events": [], "limit": 200}))
            .unwrap()
            .is_empty()
    );
    let rows = decode(response()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].timestamp, None);
    assert_eq!(rows[0].principal, None);
    assert_eq!(rows[0].path, "a");
    let mut populated = response();
    populated["events"][0]["timestamp"] = json_codec::json!("2026-11-01T02:00:00Z");
    populated["events"][0]["principal"] = json_codec::json!("admin");
    populated["events"][0]["sequence"] = json_codec::json!("18446744073709551615");
    let rows = decode(populated).unwrap();
    assert_eq!(rows[0].timestamp.as_deref(), Some("2026-11-01T02:00:00Z"));
    assert_eq!(rows[0].principal.as_deref(), Some("admin"));
}

#[test]
fn activity_response_rejects_old_wrappers_missing_fields_and_wrong_types() {
    for bad in [
        Value::Null,
        json_codec::json!([]),
        json_codec::json!([[], 200]),
        json_codec::json!({}),
        json_codec::json!({"events": []}),
        json_codec::json!({"events": null, "limit": 200}),
        json_codec::json!({"events": "bad", "limit": 200}),
        json_codec::json!({"events": [], "limit": "200"}),
        json_codec::json!({"events": [], "limit": 200, "extra": true}),
    ] {
        let error = decode(bad).unwrap_err();
        assert_eq!(error.code, storage_contract::ErrorCode::InvalidResponse);
        assert!(std::error::Error::source(&error).is_some());
    }
    let fields: Vec<_> = response()["events"][0]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    for field in fields {
        let mut bad = response();
        bad["events"][0].as_object_mut().unwrap().remove(&field);
        assert!(decode(bad).is_err(), "missing {field}");
        let mut bad = response();
        bad["events"][0][&field] = json_codec::json!({});
        assert!(decode(bad).is_err(), "wrong type {field}");
    }
    for (key, value) in [
        ("extra", Value::Bool(true)),
        ("display_path", Value::String("old".into())),
    ] {
        let mut bad = response();
        bad["events"][0][key] = value;
        assert!(decode(bad).is_err(), "unexpected {key}");
    }
    for sequence in [
        json_codec::json!(1),
        json_codec::json!("01"),
        json_codec::json!("18446744073709551616"),
    ] {
        let mut bad = response();
        bad["events"][0]["sequence"] = sequence;
        assert!(decode(bad).is_err());
    }
}

#[test]
fn every_current_activity_detail_shape_is_accepted_and_closed() {
    let variants = [
        json_codec::json!({"version_id":null}),
        json_codec::json!({"version_id":null,"node_id":null,"branch_id":null}),
        json_codec::json!({"version_id":null,"directory_branch_id":"b","head_tree_id":null}),
        json_codec::json!({"version_id":null,"target_copies":null}),
        json_codec::json!({"version_id":"v","node_id":"n","previous_path":"old","branch_id":"b"}),
        json_codec::json!({"version_id":null,"directory_branch_id":"b","head_tree_id":"t","parent_tree_id":null}),
        json_codec::json!({"version_id":null,"materialized":false,"inherited":false,"own_version_id":null,"source_version_id":null,"content_state":"LIVE","visible_content_state":"LIVE"}),
    ];
    for detail in variants {
        let mut value = response();
        let row = value["events"][0].as_object_mut().unwrap();
        row.remove("version_id");
        row.extend(detail.as_object().unwrap().clone());
        assert!(decode(value.clone()).is_ok(), "{detail}");
        value["events"][0]["unexpected"] = Value::Bool(true);
        assert!(decode(value).is_err(), "{detail}");
    }
}

#[test]
fn malformed_refresh_preserves_snapshot_and_reports_failure_instead_of_successful_empty() {
    Owner::new().with(|| {
        let state = RwSignal::new(State {
            authenticated: true,
            session_epoch: 7,
            ..Default::default()
        });
        complete_load(state, 7, decode(json_codec::json!({})));
        state.with_untracked(|s| {
            assert!(!s.activity.loaded);
            assert!(s.activity.failure.is_some());
        });
        complete_load(state, 7, decode(response()));
        let snapshot = state.with_untracked(|s| s.activity.events.clone());
        complete_load(
            state,
            7,
            decode(json_codec::json!({"events":"wrong","limit":200})),
        );
        state.with_untracked(|s| {
            assert!(s.activity.loaded);
            assert_eq!(s.activity.events, snapshot);
            assert!(s.activity.failure.is_some());
            assert!(!s.activity.loading);
        });
        complete_load(state, 6, Ok(vec![]));
        assert_eq!(
            state.with_untracked(|s| s.activity.events.clone()),
            snapshot
        );
        complete_load(
            state,
            7,
            decode(json_codec::json!({"events":[],"limit":200})),
        );
        state.with_untracked(|s| {
            assert!(s.activity.loaded);
            assert!(s.activity.events.is_empty());
            assert!(s.activity.failure.is_none());
        });
    });
}

#[test]
fn activity_history_keeps_current_response_budget() {
    for limit in [0, 199, 201] {
        assert!(decode(json_codec::json!({"events": [], "limit": limit})).is_err());
    }
    let row = response()["events"][0].clone();
    assert!(decode(json_codec::json!({"events": vec![row.clone(); 200], "limit": 200})).is_ok());
    assert!(decode(json_codec::json!({"events": vec![row; 201], "limit": 200})).is_err());
}

#[test]
fn file_lifecycle_operations_have_specific_labels() {
    for action in ["file.delete", "file.delete.batch"] {
        assert_eq!(action_label(action), "Soft-deleted file");
    }
    for action in ["file.delete.undo", "file.delete.batch.undo"] {
        assert_eq!(action_label(action), "Restored file");
    }
}
