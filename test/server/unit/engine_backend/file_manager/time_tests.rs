use super::*;
use crate::BackendAction;
use storage_contract::{CommitRequest, Config, FileTagTarget};

#[test]
fn utc_history_timestamp_formats_seconds() {
    for (seconds, expected) in [
        (0, "1970-01-01T00:00:00Z"),
        (1_792_803_600, "2026-10-24T01:00:00Z"),
    ] {
        assert_eq!(utc_timestamp(seconds).unwrap(), expected);
    }
}

#[test]
fn file_history_and_activity_emit_one_utc_form_without_changing_stored_facts() {
    let root = crate::tests::private_tempdir();
    let mut engine = Engine::initialize(
        &root.path().join("system"),
        Config::development([root.path().join("disk")]),
    )
    .unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("UTC").unwrap(),
            "P01",
            1,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "owner")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "before.txt", b"body")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "owner".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: String::new(),
            request_digest: None,
        })
        .unwrap();
    let entry = engine
        .list_tree(&repo.main_branch_id)
        .unwrap()
        .into_iter()
        .find(|entry| entry.kind == storage_contract::NamespaceEntryKind::File)
        .unwrap();
    engine
        .file_namespace(
            &repo.repository_id,
            NamespaceOperation::parse(
                storage_contract::NamespaceOperationKind::Rename,
                "before.txt",
                Some("after.txt"),
            )
            .unwrap(),
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let context = engine
        .file_context(&repo.repository_id, "after.txt")
        .unwrap();
    let head = engine.file_branch_head(&context.branch_id).unwrap();
    engine
        .update_file_node_tags(storage_engine::FileNodeTagsUpdateRequest {
            file_id: &entry.file_id,
            targets: &[FileTagTarget {
                branch_id: context.branch_id,
                source_node_id: head.clone(),
                expected_node_id: head,
            }],
            add: &["utc".into()],
            remove: &[],
            location: None,
            principal: "owner",
            operation_id: storage_contract::OperationId::generate(),
        })
        .unwrap();
    let before = json_codec::to_value(engine.state()).unwrap();
    let history = file_history(&engine, &json!({"file_id":entry.file_id})).unwrap();
    let nodes = history["nodes"].as_array().unwrap();
    for kind in ["content", "rename", "tags"] {
        assert!(nodes.iter().any(|node| node["kind"] == kind));
    }
    for node in nodes {
        let id = node["node_id"].as_str().unwrap();
        let expected = match node["kind"].as_str().unwrap() {
            "content" => engine.state().versions[id].created_at.clone(),
            "rename" => utc_timestamp(engine.state().file_path_nodes[id].created_at).unwrap(),
            "tags" => utc_timestamp(engine.state().file_metadata_nodes[id].created_at).unwrap(),
            _ => unreachable!(),
        };
        assert_eq!(node["created_at"], expected);
        assert_eq!(expected.len(), 20);
        assert!(expected.ends_with('Z'));
    }
    let activity = read(&engine, BackendAction::FileManagerHistory, json!({})).unwrap();
    let events = activity["events"].as_array().unwrap();
    assert!(!events.is_empty());
    assert!(events.iter().all(|event| {
        event["timestamp"]
            .as_str()
            .is_some_and(|time| time.len() == 20 && time.ends_with('Z'))
    }));
    assert_eq!(json_codec::to_value(engine.state()).unwrap(), before);
}
