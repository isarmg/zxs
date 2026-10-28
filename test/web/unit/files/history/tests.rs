use super::*;
use crate::snapshot::Snapshot;
use json_codec::json;

pub(crate) fn branch_wire(overrides: Value) -> Value {
    let mut value = json!({
        "branch_id":"branch", "history_tree_id":"history", "name":"main",
        "head_commit_id":"head", "fork_commit_id":"fork", "branch_kind":"file",
        "file_id":"file", "pending_file_versions":{}, "generation":"1", "state":"active",
        "materialized":true, "inherited":false, "content_state":"LIVE",
        "version_id":"version", "own_version_id":"version", "source_version_id":null,
        "visible_content_state":"LIVE", "read_only":false, "alias_of":null,
        "node_id":"version", "current":false
    });
    value
        .as_object_mut()
        .unwrap()
        .extend(overrides.as_object().unwrap().clone());
    value
}
pub(crate) fn branch(overrides: Value) -> FileBranch {
    wire::branch(branch_wire(overrides), "file", None).unwrap()
}
pub(crate) fn response(branches: Vec<Value>, selected: Option<&str>) -> Value {
    json!({"file_id":"file", "selected_branch_id":selected, "branches":branches})
}

/// Focused consumers provide unrelated history metadata without duplicating the wire fixture.
pub(crate) fn history(mut metadata: Value) -> Snapshot<FileHistory> {
    if metadata.is_null() {
        return Default::default();
    }
    let fields = metadata.as_object_mut().unwrap();
    fields.entry("file_id").or_insert_with(|| "file".into());
    fields.entry("selected_branch_id").or_insert(Value::Null);
    fields.entry("branches").or_insert_with(|| json!([]));
    let file = metadata["file_id"].as_str().unwrap().to_owned();
    FileHistory::parse(metadata, &file).unwrap().into()
}
pub(crate) fn update_metadata(
    history: &mut Snapshot<FileHistory>,
    update: impl FnOnce(&mut Value),
) {
    update(&mut history.metadata);
}

#[test]
fn state_and_alias_are_admitted_once_as_write_disposition() {
    for state in ["active", "archived", "migrated", "merged"] {
        for alias in [None, Some("survivor")] {
            let readonly = state != "active" || alias.is_some();
            let value = branch_wire(json!({"state":state, "alias_of":alias, "read_only":readonly}));
            let parsed = FileHistory::parse(response(vec![value.clone()], None), "file").unwrap();
            assert_eq!(parsed.branches()[0].writable(), !readonly);
            assert!(parsed.get("branches").is_none());
            assert!(parsed.get("selected_branch_id").is_none());
            let mut contradictory = value;
            contradictory["read_only"] = (!readonly).into();
            assert!(FileHistory::parse(response(vec![contradictory], None), "file").is_err());
        }
    }
}

#[test]
fn current_uses_the_single_selected_identity_without_requiring_a_file_branch() {
    for selected in [None, Some("full-branch"), Some("migrated-branch")] {
        let parsed = FileHistory::parse(response(vec![], selected), "file").unwrap();
        assert_eq!(parsed.selected(), selected);
    }
    let current = branch_wire(json!({"current":true}));
    let parsed =
        FileHistory::parse(response(vec![current.clone()], Some("branch")), "file").unwrap();
    assert_eq!(parsed.selected(), Some("branch"));
    assert!(FileHistory::parse(response(vec![current], None), "file").is_err());
    assert!(
        FileHistory::parse(
            response(vec![branch_wire(json!({}))], Some("branch")),
            "file"
        )
        .is_err()
    );
    let duplicate = branch_wire(json!({}));
    assert!(
        FileHistory::parse(response(vec![duplicate.clone(), duplicate], None), "file").is_err()
    );
}

#[test]
fn owned_pending_empty_and_unavailable_content_keep_the_current_projection() {
    let explicit =
        branch(json!({"file_node_id":"node","node_id":"node","generation":u64::MAX.to_string()}));
    assert_eq!(explicit.head(), Some("node"));
    for visible in ["LIVE", "AVAILABLE", "PURGED", "MIGRATED"] {
        let owned = branch(json!({"visible_content_state":visible,"content_state":visible}));
        assert!(!owned.pending());
        assert_eq!(owned.head(), Some("version"));
        for source in ["source", ""] {
            let version = (!source.is_empty()).then_some(source);
            let pending = branch(json!({
                "materialized":false, "inherited":true, "content_state":"PENDING",
                "visible_content_state":if source.is_empty() { "LIVE" } else { visible }, "source_version_id":source,
                "version_id":version, "own_version_id":null, "node_id":null,
                "pending_file_versions":{"file":source}
            }));
            assert!(pending.pending());
            assert_eq!(
                pending.head(),
                version,
                "real visible version is the fallback baseline"
            );
        }
    }
    let empty = branch(
        json!({"materialized":false, "version_id":null, "own_version_id":null, "node_id":null}),
    );
    assert!(empty.pending());
    assert!(empty.head().is_none());
    let pending = branch(
        json!({"materialized":false, "inherited":true, "content_state":"PENDING", "node_id":"",
        "version_id":"source", "own_version_id":null, "source_version_id":"source", "pending_file_versions":{"file":"source"}}),
    );
    assert_eq!(pending.head(), Some("source"));
}

#[test]
fn backup_pinning_changes_content_markers_without_reinterpreting_branches() {
    let branch = branch_wire(json!({"current":true}));
    let mut value = response(vec![branch], Some("branch"));
    value["nodes"] =
        json!([{"node_id":"stored","current":false},{"node_id":"primary","current":true}]);
    value["versions"] =
        json!([{"version_id":"stored","current":false},{"version_id":"primary","current":true}]);
    let admitted = FileHistory::parse(value, "file").unwrap();
    let branches = admitted.branches().to_vec();
    let entry = crate::files::test_entries::entry(
        json!({"backup":true,"version_id":"stored","node_id":"stored"}),
    );
    let pinned = admitted.for_entry(&entry);
    assert_eq!(pinned.branches(), branches);
    assert_eq!(pinned.selected(), Some("branch"));
    assert_eq!(pinned["node_id"], "stored");
    assert_eq!(pinned["nodes"][0]["current"], true);
    assert_eq!(pinned["nodes"][1]["current"], false);
    assert_eq!(pinned["read_only"], true);
}

#[test]
fn malformed_or_unknown_branch_fields_are_rejected_at_ingress() {
    let valid = branch_wire(json!({}));
    for field in valid.as_object().unwrap().keys() {
        let mut missing = valid.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(
            FileHistory::parse(response(vec![missing], None), "file").is_err(),
            "{field}"
        );
    }
    for extra in [
        "status",
        "is_alias",
        "alias",
        "selected",
        "head_node_id",
        "future",
    ] {
        let mut value = valid.clone();
        value[extra] = true.into();
        assert!(
            FileHistory::parse(response(vec![value], None), "file").is_err(),
            "{extra}"
        );
    }
    for (field, malformed) in [
        ("state", json!("ARCHIVED")),
        ("state", json!({"active":null})),
        ("branch_kind", json!("full")),
        ("file_id", json!("other")),
        ("branch_id", json!("")),
        ("name", json!("")),
        ("read_only", json!("false")),
        ("alias_of", json!("")),
        ("current", Value::Null),
        ("generation", json!(1)),
        ("generation", json!("01")),
        ("generation", json!("18446744073709551616")),
        ("file_node_id", Value::Null),
        ("file_node_id", json!("different")),
        ("node_id", json!("x".repeat(257))),
        ("version_id", json!("")),
        ("materialized", json!(false)),
        ("inherited", json!(true)),
        ("content_state", json!("PENDING")),
        ("visible_content_state", json!("pending")),
        ("source_version_id", json!("source")),
        ("own_version_id", Value::Null),
    ] {
        let mut value = valid.clone();
        value[field] = malformed;
        assert!(
            FileHistory::parse(response(vec![value], None), "file").is_err(),
            "{field}"
        );
    }
    for value in [
        Value::Null,
        json!({}),
        json!({"file_id":"file","branches":[]}),
        json!({"file_id":"file","branches":null,"selected_branch_id":null}),
        response(vec![], Some("")),
        response(vec![json!([])], None),
    ] {
        assert!(FileHistory::parse(value, "file").is_err());
    }
    assert!(FileHistory::parse(response(vec![], None), "other").is_err());
}

#[test]
fn directory_and_file_branch_contracts_are_not_interchangeable() {
    let directory = crate::files::test_entries::branch_wire(json!({}));
    assert!(FileHistory::parse(response(vec![directory], None), "file").is_err());
    assert!(crate::files::page::DirectoryBranch::parse(branch_wire(json!({}))).is_err());
}

#[test]
fn decoding_keeps_the_concrete_error_source() {
    use std::error::Error;
    let error = FileHistory::parse(
        response(vec![branch_wire(json!({"read_only":0}))], None),
        "file",
    )
    .unwrap_err();
    assert_eq!(error.status, 200);
    assert_eq!(error.code, storage_contract::ErrorCode::InvalidResponse);
    assert!(error.source().unwrap().source().unwrap().source().is_some());
}

#[test]
fn actual_server_branch_fixtures_enter_the_web_model_without_shape_reconstruction() {
    // The server test compares actual capture + response serialization with
    // these same bytes. Empty nodes/versions do not claim whole-HTTP coverage.
    let fixtures = storage_contract::strict_json(include_bytes!(
        "../../../../fixtures/file-history-branches.json"
    ))
    .unwrap();
    let fixtures = fixtures.as_object().unwrap();
    assert_eq!(fixtures.len(), 6);
    for (case, selected, branch) in [
        (
            "explicit-node",
            Some("branch"),
            Some(("file-node", false, true)),
        ),
        ("visible-node", None, Some(("body", false, true))),
        ("merged-pending", Some("branch"), Some(("body", true, true))),
        (
            "empty-source",
            Some("full-branch"),
            Some(("body", true, false)),
        ),
        ("no-selection", None, None),
        ("full-selection", Some("full-branch"), None),
    ] {
        let history = FileHistory::parse(fixtures[case].clone(), "file").unwrap();
        assert_eq!(history.selected(), selected, "{case}");
        if let Some((head, pending, writable)) = branch {
            assert_eq!(history.branches().len(), 1, "{case}");
            let branch = &history.branches()[0];
            assert_eq!(branch.id(), "branch", "{case}");
            assert_eq!(branch.name(), "main", "{case}");
            assert_eq!(branch.head(), Some(head), "{case}");
            assert_eq!(branch.pending(), pending, "{case}");
            assert_eq!(branch.writable(), writable, "{case}");
        } else {
            assert!(history.branches().is_empty(), "{case}");
        }
    }
}
