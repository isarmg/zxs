use super::*;
use storage_engine::{CommitRequest, Config, new_id};

#[test]
fn actual_primary_and_backup_projection_is_page_bounded_and_selected_is_independent() {
    let root = tempfile::tempdir().unwrap();
    let config = Config::development([
        root.path().join("d1"),
        root.path().join("d2"),
        root.path().join("d3"),
    ]);
    let mut engine = Engine::initialize(&root.path().join("system"), config).unwrap();
    let repo = engine
        .create_repository("Files", "P01", 3, "owner", &new_id("op"))
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "owner")
        .unwrap();
    for name in [
        "alpha.txt",
        "beta.txt",
        "gamma.txt",
        "theta.txt",
        "zeta.txt",
        "folder/child.txt",
    ] {
        engine
            .stage_bytes(&workspace.workspace_id, name, b"body")
            .unwrap();
    }
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: new_id("op"),
            principal: "owner".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "paged fixture".into(),
            request_digest: None,
        })
        .unwrap();
    for (slot, path) in [("P01", "Files"), ("B01", "P01/Files")] {
        let all =
            listing::entries(&engine, &json!({"slot_id":slot,"query":{"path":path}})).unwrap();
        let selected = all["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["name"] == "zeta.txt")
            .unwrap();
        LISTED_ENTRY_PROJECTIONS.set(0);
        let first=listing::entries(&engine,&json!({"slot_id":slot,"query":{"path":path,"limit":"2","selected_path":selected["path"],"selected_entry_id":selected["entry_id"]}})).unwrap();
        assert_eq!(first["entries"].as_array().unwrap().len(), 2);
        assert_eq!(first["total_entries"], "6");
        assert_eq!(first["directory_file_count"], "6");
        assert_eq!(first["selected_entry"], *selected);
        assert!(LISTED_ENTRY_PROJECTIONS.get() <= 3);
        assert_eq!(first["entries"][0]["kind"], "directory");
        let query = json!({"slot_id":slot,"query":{"path":path,"limit":"2","query":"alpha","selected_path":selected["path"],"selected_entry_id":selected["entry_id"]}});
        let filtered = listing::entries(&engine, &query).unwrap();
        assert_eq!(filtered["total_entries"], "1");
        assert_eq!(filtered["selected_entry"], *selected);
        let mut missing = query.clone();
        missing["query"]["selected_entry_id"] = new_id("entry").into();
        assert!(listing::entries(&engine, &missing).unwrap()["selected_entry"].is_null());
        let mut malformed = query;
        malformed["query"]
            .as_object_mut()
            .unwrap()
            .remove("selected_path");
        assert_eq!(
            listing::entries(&engine, &malformed).unwrap_err().status,
            400
        );
        let next=listing::entries(&engine,&json!({"slot_id":slot,"query":{"path":path,"limit":"2","cursor":first["next_cursor"]}})).unwrap();
        let back=listing::entries(&engine,&json!({"slot_id":slot,"query":{"path":path,"limit":"2","cursor":next["previous_cursor"]}})).unwrap();
        assert_eq!(back["entries"], first["entries"]);
        assert_eq!(back["snapshot"], first["snapshot"]);
    }
    engine
        .initialize_directory_scope(&repo.repository_id, "", "owner", &new_id("op"))
        .unwrap();
    let initial =
        listing::entries(&engine, &json!({"slot_id":"P01","query":{"path":"Files"}})).unwrap();
    let branch = initial["directory_branches"]
        .as_array()
        .unwrap()
        .iter()
        .find(|branch| branch["current"] == true)
        .unwrap();
    let chosen = initial["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "zeta.txt")
        .unwrap();
    let request = json!({"repository_id":repo.repository_id,"path":"","sources":[{"directory_branch_id":branch["directory_branch_id"],"expected_tree_id":branch["head_tree_id"]}],"entries":[{"entry_id":chosen["entry_id"],"path":"zeta.txt"}]});
    let before = (engine.state().sequence, engine.state().operations.len());
    let facts = scope_query::read(&engine, &request).unwrap();
    assert_eq!(
        facts["directory_branches"][0]["entries"],
        json!([{"entry_id":chosen["entry_id"],"path":"zeta.txt","selected_path":"zeta.txt"}])
    );
    assert_eq!(
        (engine.state().sequence, engine.state().operations.len()),
        before
    );
    let mut stale = request.clone();
    stale["sources"][0]["expected_tree_id"] = new_id("tree").into();
    assert_eq!(
        scope_query::read(&engine, &stale).unwrap_err().code,
        storage_contract::ErrorCode::ListingChanged
    );
    for path in [Value::Null, json!(12), json!([])] {
        let mut malformed = request.clone();
        malformed["path"] = path;
        assert_eq!(
            scope_query::read(&engine, &malformed).unwrap_err().status,
            400
        );
    }
    let mut traversal = request.clone();
    traversal["entries"][0]["path"] = "../zeta.txt".into();
    assert_eq!(
        scope_query::read(&engine, &traversal).unwrap_err().status,
        400
    );
    let mut nested = request;
    nested["entries"][0]["path"] = "folder/child.txt".into();
    assert_eq!(scope_query::read(&engine, &nested).unwrap_err().status, 400);
}

#[test]
fn selected_display_path_may_include_a_repository_prefix_beyond_stored_path_limit() {
    let root = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &root.path().join("system"),
        Config::development([root.path().join("d1")]),
    )
    .unwrap();
    let repo = engine
        .create_repository("Repository", "P01", 1, "owner", &new_id("op"))
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "owner")
        .unwrap();
    let name = "x".repeat(195);
    engine
        .stage_bytes(&workspace.workspace_id, &name, b"body")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: new_id("op"),
            principal: "owner".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "long display fixture".into(),
            request_digest: None,
        })
        .unwrap();
    let first = listing::entries(
        &engine,
        &json!({"slot_id":"P01","query":{"path":"Repository"}}),
    )
    .unwrap();
    let selected = &first["entries"][0];
    assert!(selected["path"].as_str().unwrap().len() > 200);
    let response=listing::entries(&engine,&json!({"slot_id":"P01","query":{"path":"Repository","selected_path":selected["path"],"selected_entry_id":selected["entry_id"]}})).unwrap();
    assert_eq!(response["selected_entry"], *selected);
}
