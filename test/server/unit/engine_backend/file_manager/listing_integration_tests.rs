use super::*;
use storage_engine::{CommitRequest, Config, new_id};

pub(super) fn listing_json(engine: &AdmissionEngine, input: &Value) -> Result<Value, ApiError> {
    let mut query: BTreeMap<String, String> = input
        .get("query")
        .map(|query| json_codec::from_value(query.clone()).map_err(ApiError::invalid_source))
        .transpose()?
        .unwrap_or_default();
    if let Some(path) = input.get("path") {
        query.insert(
            "path".into(),
            json_codec::from_value(path.clone()).map_err(ApiError::invalid_source)?,
        );
    }
    let request = crate::ReadListing::parse(input["slot_id"].as_str().unwrap().into(), query)?;
    let snapshot = match listing::capture(engine, request)? {
        listing::ListingRead::Backup(read) => {
            let selected = read.select()?;
            let page = selected.project(engine)?;
            selected.finish(page)
        }
        listing::ListingRead::Primary(read) => {
            let selected = read.select()?;
            let page = selected.project(engine)?;
            selected.finish(page)
        }
    };
    Ok(json_codec::to_value(crate::backend_listing::ListingResponse::from(snapshot)).unwrap())
}

#[test]
fn actual_primary_and_backup_projection_is_page_bounded_and_selected_is_independent() {
    let root = tempfile::tempdir().unwrap();
    let config = Config::development(crate::test_disks([
        root.path().join("d1"),
        root.path().join("d2"),
        root.path().join("d3"),
    ]))
    .unwrap();
    let mut engine =
        AdmissionEngine::new(Engine::initialize(&root.path().join("system"), config).unwrap());
    let repo = engine
        .create_disk_root("P01", "owner", storage_contract::OperationId::generate().unwrap())
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
            operation_id: storage_contract::OperationId::generate().unwrap(),
            principal: "owner".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "paged fixture".into(),
            request_digest: None,
        })
        .unwrap();
    for (slot, path) in [("P01", ""), ("B01", "P01")] {
        let all = listing_json(&engine, &json!({"slot_id":slot,"query":{"path":path}})).unwrap();
        let selected = all["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["name"] == "zeta.txt")
            .unwrap();
        LISTED_ENTRY_PROJECTIONS.set(0);
        let first=listing_json(&engine,&json!({"slot_id":slot,"query":{"path":path,"limit":"2","selected_path":selected["path"],"selected_entry_id":selected["entry_id"]}})).unwrap();
        assert_eq!(first["entries"].as_array().unwrap().len(), 2);
        assert_eq!(first["total_entries"], "6");
        assert_eq!(first["directory_file_count"], "6");
        assert_eq!(first["selected_entry"], *selected);
        assert!(LISTED_ENTRY_PROJECTIONS.get() <= 3);
        assert_eq!(first["entries"][0]["kind"], "directory");
        let query = json!({"slot_id":slot,"query":{"path":path,"limit":"2","query":"alpha","selected_path":selected["path"],"selected_entry_id":selected["entry_id"]}});
        let filtered = listing_json(&engine, &query).unwrap();
        assert_eq!(filtered["total_entries"], "1");
        assert_eq!(filtered["selected_entry"], *selected);
        let mut missing = query.clone();
        missing["query"]["selected_entry_id"] = new_id("entry").unwrap().into();
        assert!(listing_json(&engine, &missing).unwrap()["selected_entry"].is_null());
        let mut malformed = query;
        malformed["query"]
            .as_object_mut()
            .unwrap()
            .remove("selected_path");
        assert_eq!(
            listing_json(&engine, &malformed).unwrap_err().status,
            http::StatusCode::BAD_REQUEST
        );
        let next=listing_json(&engine,&json!({"slot_id":slot,"query":{"path":path,"limit":"2","cursor":first["next_cursor"]}})).unwrap();
        let back=listing_json(&engine,&json!({"slot_id":slot,"query":{"path":path,"limit":"2","cursor":next["previous_cursor"]}})).unwrap();
        assert_eq!(back["entries"], first["entries"]);
        assert_eq!(back["snapshot"], first["snapshot"]);
    }
}

#[test]
fn long_registered_path_and_stable_selected_entry_are_not_rebased() {
    let root = tempfile::tempdir().unwrap();
    let engine = Engine::initialize(
        &root.path().join("system"),
        Config::development(crate::test_disks([root.path().join("d1")])).unwrap(),
    )
    .unwrap();
    let mut engine = AdmissionEngine::new(engine);
    let repo = engine
        .create_disk_root("P01", "owner", storage_contract::OperationId::generate().unwrap())
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "owner")
        .unwrap();
    let name = format!("Reports/{}", "x".repeat(105));
    engine
        .stage_bytes(&workspace.workspace_id, &name, b"body")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate().unwrap(),
            principal: "owner".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "long display fixture".into(),
            request_digest: None,
        })
        .unwrap();
    let first = listing_json(
        &engine,
        &json!({"slot_id":"P01","query":{"path":"Reports"}}),
    )
    .unwrap();
    let selected = &first["entries"][0];
    assert_eq!(selected["path"], name);
    assert_eq!(selected["repository_path"], name);
    let response=listing_json(&engine,&json!({"slot_id":"P01","query":{"path":"Reports","selected_path":selected["path"],"selected_entry_id":selected["entry_id"]}})).unwrap();
    assert_eq!(response["selected_entry"], *selected);
}

#[test]
fn listing_never_registers_an_empty_root_and_fences_new_ordinary_entries()
{
    let root = tempfile::tempdir().unwrap();
    let engine = Engine::initialize(
        &root.path().join("system"),
        Config::development(crate::test_disks([root.path().join("d1")])).unwrap(),
    )
    .unwrap();
    let mut engine = AdmissionEngine::new(engine);
    let empty = listing_json(&engine, &json!({"slot_id":"P01"})).unwrap();
    assert_eq!(empty["entries"], json!([]));
    assert!(empty["context"].is_null());
    assert!(engine.state().repositories.is_empty());
    let repo = engine.create_disk_root("P01", "owner", storage_contract::OperationId::generate().unwrap()).unwrap();
    let mkdir = |engine: &mut AdmissionEngine, name: &str| {
        engine.file_namespace(&repo.repository_id, NamespaceOperation::parse(
            storage_contract::NamespaceOperationKind::Mkdir, name, None,
        ).unwrap(), "owner", storage_contract::OperationId::generate().unwrap()).unwrap();
    };
    for name in ["Alpha", "Beta", "Gamma"] { mkdir(&mut engine, name); }
    let first = listing_json(&engine, &json!({"slot_id":"P01","query":{"limit":"1"}})).unwrap();
    let expected_snapshot=blake3::hash(&json_codec::to_vec(&json!({"configuration":engine.config(),"generation":engine.state().business_generation.to_string()})).unwrap()).to_hex().to_string();
    assert_eq!(first["snapshot"], expected_snapshot);
    assert_eq!(first["entries"][0]["name"], "Alpha");
    assert_eq!(first["total_entries"], "3");
    let row = &first["entries"][0];
    assert_eq!(row["repository_id"], repo.repository_id);
    assert_eq!(row["path"], row["repository_path"]);
    assert_eq!(first["context"]["repository_id"], repo.repository_id);
    let cursor = first["next_cursor"].as_str().unwrap();
    mkdir(&mut engine, "Delta");
    let stale = listing_json(
        &engine,
        &json!({"slot_id":"P01","query":{"limit":"1","cursor":cursor}}),
    )
    .unwrap_err();
    assert_eq!(stale.code, storage_contract::ErrorCode::ListingChanged);
    let listing::ListingRead::Primary(read) = listing::capture(
        &engine,
        crate::ReadListing::parse("P01".into(), Default::default()).unwrap(),
    )
    .unwrap() else {
        panic!("primary disk must select primary enumeration");
    };
    let selected = read.select().unwrap();
    let page = selected.project(&engine).unwrap();
    let snapshot = selected.finish(page);
    drop(engine);
    let wire =
        json_codec::to_value(crate::backend_listing::ListingResponse::from(snapshot)).unwrap();
    assert_eq!(
        wire["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["Alpha", "Beta", "Delta", "Gamma"]
    );
}

#[test]
fn backup_deleted_file_retains_historical_namespace_without_fabricated_current_context() {
    let root = tempfile::tempdir().unwrap();
    let engine = Engine::initialize(
        &root.path().join("system"),
        Config::development(crate::test_disks([
            root.path().join("d1"),
            root.path().join("d2"),
            root.path().join("d3"),
        ]))
        .unwrap(),
    )
    .unwrap();
    let mut engine = AdmissionEngine::new(engine);
    let repo = engine
        .create_disk_root("P01", "owner", storage_contract::OperationId::generate().unwrap())
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "owner")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "lost.txt", b"body")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate().unwrap(),
            principal: "owner".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "backup history fixture".into(),
            request_digest: None,
        })
        .unwrap();
    let entry = engine.list_tree_ref(&repo.main_branch_id).unwrap()[0].clone();
    engine
        .file_namespace(
            &repo.repository_id,
            NamespaceOperation::parse(
                storage_contract::NamespaceOperationKind::Delete,
                "lost.txt",
                None,
            )
            .unwrap(),
            "owner",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    let listing = listing_json(
        &engine,
        &json!({"slot_id":"B01","query":{"path":"P01"}}),
    )
    .unwrap();
    assert_eq!(listing["backup"], true);
    assert_eq!(listing["writable"], false);
    assert!(listing.get("directory_branches").is_none());
    assert!(listing["context"].is_null());
    let row = &listing["entries"][0];
    assert_eq!(row["entry_id"], entry.file_id);
    assert_eq!(row["file_id"], entry.file_id);
    assert_eq!(row["version_id"], entry.version_id);
    assert_eq!(row["node_id"], entry.version_id);
    assert_eq!(row["size"], "4");
    assert_eq!(row["historical_only"], true);
    assert_eq!(row["read_only"], true);
    assert!(row["repository_path"].is_null());
    assert!(row["selected_branch_id"].is_null());
}
