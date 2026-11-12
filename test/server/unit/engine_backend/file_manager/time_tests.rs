use super::*;
use storage_contract::{CommitRequest, Config};

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
        Config::development([root.path().join("disk")]).unwrap(),
    )
    .unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("UTC").unwrap(),
            "P01",
            1,
            "owner",
            storage_contract::OperationId::generate().unwrap(),
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
            operation_id: storage_contract::OperationId::generate().unwrap(),
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
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    let changed = engine
        .replace_container_tags(
            &crate::container_tags_tests::command(
                &entry.file_id,
                &repo.repository_id,
                "after.txt",
                0,
                &["utc"],
            ),
            &"owner".parse::<storage_contract::Principal>().unwrap(),
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    let unchanged = engine.replace_container_tags(
        &crate::container_tags_tests::command(
            &entry.file_id, &repo.repository_id, "after.txt", 1, &["utc"],
        ),
        &"owner".parse::<storage_contract::Principal>().unwrap(),
        storage_contract::OperationId::generate().unwrap(),
    ).unwrap();
    assert_eq!(unchanged.result["changed"], false);
    let before = json_codec::to_value(engine.state()).unwrap();
    let history = storage_contract::strict_json(
        &json_codec::to_vec(&crate::backend_file_history::FileHistoryResponse::from(
            FileHistoryRead::capture(
                &engine,
                crate::ReadFileHistory::parse(entry.file_id.clone(), None, None).unwrap(),
            )
            .unwrap()
            .project()
            .unwrap(),
        ))
        .unwrap(),
    )
    .unwrap();
    let nodes = history["nodes"].as_array().unwrap();
    for kind in ["content", "rename"] {
        assert!(nodes.iter().any(|node| node["kind"] == kind));
    }
    for node in nodes {
        let id = node["node_id"].as_str().unwrap();
        let expected = match node["kind"].as_str().unwrap() {
            "content" => engine.state().versions[id].created_at.clone(),
            "rename" => utc_timestamp(engine.state().file_path_nodes[id].created_at).unwrap(),
            _ => unreachable!(),
        };
        assert_eq!(node["created_at"], expected);
        assert_eq!(expected.len(), 20);
        assert!(expected.ends_with('Z'));
    }
    let snapshot = activity::read(&engine).unwrap();
    let activity = json_codec::to_value(
        crate::backend_activity::ActivityHistoryResponse::try_from(snapshot).unwrap(),
    )
    .unwrap();
    let events = activity["events"].as_array().unwrap();
    assert!(events.iter().any(|event| {
        event["action"] == storage_contract::file_container_tags::KIND
            && event["file_id"] == entry.file_id
    }));
    assert!(events.iter().any(|event| event["event_id"] == changed.operation_id.to_string()));
    assert!(events.iter().all(|event| event["event_id"] != unchanged.operation_id.to_string()));
    assert!(nodes.iter().all(|node| node.get("tags").is_none()));
    assert!(events.iter().all(|event| {
        event["timestamp"]
            .as_str()
            .is_some_and(|time| time.len() == 20 && time.ends_with('Z'))
    }));
    assert_eq!(json_codec::to_value(engine.state()).unwrap(), before);
    let snapshot = activity::read(&engine).unwrap();
    engine
        .set_file_note(
            &entry.file_id,
            None,
            &storage_contract::Note::parse("later note".into()).unwrap(),
            "owner",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    let pinned = json_codec::to_value(
        crate::backend_activity::ActivityHistoryResponse::try_from(snapshot).unwrap(),
    )
    .unwrap();
    assert_eq!(
        pinned, activity,
        "snapshot is owned and does not observe later publication"
    );
    let next = activity::read(&engine).unwrap();
    assert!(
        next.events
            .iter()
            .any(|event| event.action == "file.metadata")
    );
}
