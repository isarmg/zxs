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
    let snapshot = listing::entries(engine, request)?;
    Ok(json_codec::to_value(crate::backend_listing::ListingResponse::from(snapshot)).unwrap())
}

#[test]
fn paged_namespace_listing_preserves_selection_and_cursors_across_source_chunks() {
    let root = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &root.path().join("system"),
        Config::development([root.path().join("d1")]),
    )
    .unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Files").unwrap(),
            "P01",
            1,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    // Exercise real immutable namespace pages without publishing hundreds of
    // unrelated mutations. This checks response semantics, not allocation or
    // source-read counts; the existing seam exposes neither measurement.
    let mut names = (0..storage_contract::NAMESPACE_PAGE_ENTRIES * 2 + 5)
        .map(|index| format!("item-{index:04}"))
        .collect::<Vec<_>>();
    names.extend(["Straße".into(), "STRASSZ".into(), "zzz-selected".into()]);
    let mut entries = names
        .iter()
        .map(|name| format!("nested/{name}"))
        .chain(
            (0..storage_contract::NAMESPACE_PAGE_ENTRIES)
                .map(|index| format!("exact/item-{index:04}")),
        )
        .chain(["nested".into(), "exact".into(), "empty".into()])
        .map(|path| storage_contract::Entry {
            entry_id: new_id("entry"),
            path: name_policy::PortablePath::parse(&path).unwrap(),
            kind: storage_contract::NamespaceEntryKind::Directory,
            file_id: String::new(),
            version_id: String::new(),
            size: 0,
        })
        .collect::<Vec<_>>();
    let namespace = storage_engine::test_support::plant_namespace(&engine, &entries).unwrap();
    let mut state = engine.state().clone();
    let head = state.branches[&repo.main_branch_id].head_commit_id.clone();
    state.commits.get_mut(&head).unwrap().namespace = namespace;
    storage_engine::test_support::replace_state(&mut engine, state);
    let engine = AdmissionEngine::new(engine);
    entries.retain(|entry| entry.path().starts_with("nested/"));
    entries.sort_by_key(|entry| {
        crate::backend_listing::SortKey::for_entry(entry, &format!("Files/{}", entry.path))
    });
    let expected = entries.iter().map(|entry| entry.name()).collect::<Vec<_>>();
    let selected = entries
        .iter()
        .find(|entry| entry.name() == "zzz-selected")
        .unwrap();
    let selected_path = format!("Files/{}", selected.path);
    let request = |cursor: Option<&Value>, query: &str| {
        let mut input = json!({"slot_id":"P01","query":{
            "path":"Files/nested", "limit":"137", "query":query,
            "selected_path":selected_path, "selected_entry_id":selected.entry_id
        }});
        if let Some(cursor) = cursor {
            input["query"]["cursor"] = cursor.clone();
        }
        listing_json(&engine, &input).unwrap()
    };
    let first = request(None, "");
    assert_eq!(first["entries"].as_array().unwrap().len(), 137);
    assert_eq!(first["total_entries"], names.len().to_string());
    assert_eq!(first["selected_entry"]["path"], selected_path);
    assert!(first["previous_cursor"].is_null());
    let second = request(Some(&first["next_cursor"]), "");
    let back = request(Some(&second["previous_cursor"]), "");
    assert_eq!(back["entries"], first["entries"]);
    let actual = first["entries"]
        .as_array()
        .unwrap()
        .iter()
        .chain(second["entries"].as_array().unwrap())
        .map(|entry| entry["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
    assert!(second["next_cursor"].is_null());
    let filtered = request(None, "item-02");
    assert_eq!(filtered["total_entries"], "61");
    assert_eq!(filtered["selected_entry"], first["selected_entry"]);
    assert!(
        filtered["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["name"].as_str().unwrap().starts_with("item-02"))
    );
    for (path, count) in [("Files/exact", 128), ("Files/empty", 0)] {
        let page = listing_json(
            &engine,
            &json!({"slot_id":"P01","query":{"path":path,"limit":"500"}}),
        )
        .unwrap();
        assert_eq!(page["entries"].as_array().unwrap().len(), count);
        assert!(page["next_cursor"].is_null());
    }
}

#[test]
fn actual_primary_and_backup_projection_is_page_bounded_and_selected_is_independent() {
    let root = tempfile::tempdir().unwrap();
    let config = Config::development([
        root.path().join("d1"),
        root.path().join("d2"),
        root.path().join("d3"),
    ]);
    let mut engine =
        AdmissionEngine::new(Engine::initialize(&root.path().join("system"), config).unwrap());
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Files").unwrap(),
            "P01",
            3,
            "owner",
            storage_contract::OperationId::generate(),
        )
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
            operation_id: storage_contract::OperationId::generate(),
            principal: "owner".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "paged fixture".into(),
            request_digest: None,
        })
        .unwrap();
    for (slot, path) in [("P01", "Files"), ("B01", "P01/Files")] {
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
        missing["query"]["selected_entry_id"] = new_id("entry").into();
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
    engine
        .initialize_directory_scope(
            &repo.repository_id,
            None,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let initial =
        listing_json(&engine, &json!({"slot_id":"P01","query":{"path":"Files"}})).unwrap();
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
    let selection = |head: String, path: &str| {
        crate::ReadDirectorySelection::parse(
            repo.repository_id.clone(),
            "",
            vec![crate::DirectorySelectionSourceInput {
                directory_branch_id: branch["directory_branch_id"].as_str().unwrap().into(),
                expected_tree_id: head,
            }],
            vec![crate::DirectorySelectionEntryInput {
                entry_id: chosen["entry_id"].as_str().unwrap().into(),
                path: path.into(),
            }],
        )
    };
    let before = (engine.state().sequence, engine.state().operations.len());
    let request = selection(branch["head_tree_id"].as_str().unwrap().into(), "zeta.txt").unwrap();
    let facts = crate::engine_backend::directory_selection::read(&engine, &request).unwrap();
    let facts = &facts.branches[0].entries;
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].entry_id, chosen["entry_id"]);
    assert_eq!(facts[0].path, "zeta.txt");
    assert_eq!(facts[0].selected_path, "zeta.txt");
    assert_eq!(
        (engine.state().sequence, engine.state().operations.len()),
        before
    );
    let stale = selection(new_id("tree"), "zeta.txt").unwrap();
    assert_eq!(
        crate::engine_backend::directory_selection::read(&engine, &stale)
            .err()
            .unwrap()
            .code,
        storage_contract::ErrorCode::ListingChanged
    );
    for path in ["../zeta.txt", "folder/child.txt"] {
        assert_eq!(
            selection(branch["head_tree_id"].as_str().unwrap().into(), path)
                .err()
                .unwrap()
                .status,
            http::StatusCode::BAD_REQUEST
        );
    }
}

#[test]
fn selected_display_path_may_include_a_repository_prefix_beyond_stored_path_limit() {
    let root = tempfile::tempdir().unwrap();
    let engine = Engine::initialize(
        &root.path().join("system"),
        Config::development([root.path().join("d1")]),
    )
    .unwrap();
    let mut engine = AdmissionEngine::new(engine);
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Repository").unwrap(),
            "P01",
            1,
            "owner",
            storage_contract::OperationId::generate(),
        )
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
            operation_id: storage_contract::OperationId::generate(),
            principal: "owner".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "long display fixture".into(),
            request_digest: None,
        })
        .unwrap();
    let first = listing_json(
        &engine,
        &json!({"slot_id":"P01","query":{"path":"Repository"}}),
    )
    .unwrap();
    let selected = &first["entries"][0];
    assert!(selected["path"].as_str().unwrap().len() > 200);
    let response=listing_json(&engine,&json!({"slot_id":"P01","query":{"path":"Repository","selected_path":selected["path"],"selected_entry_id":selected["entry_id"]}})).unwrap();
    assert_eq!(response["selected_entry"], *selected);
}

#[test]
fn virtual_root_snapshots_outlive_engine_and_generation_invalidates_cursors_without_creating_root()
{
    let root = tempfile::tempdir().unwrap();
    let engine = Engine::initialize(
        &root.path().join("system"),
        Config::development([root.path().join("d1")]),
    )
    .unwrap();
    let mut engine = AdmissionEngine::new(engine);
    let empty = listing_json(&engine, &json!({"slot_id":"P01"})).unwrap();
    assert_eq!(empty["entries"], json!([]));
    assert!(empty["context"].is_null());
    assert!(engine.state().repositories.is_empty());
    for name in ["Alpha", "Beta", "Gamma"] {
        engine
            .create_repository(
                &storage_contract::RepositoryName::parse(name).unwrap(),
                "P01",
                1,
                "owner",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
    }
    let first = listing_json(&engine, &json!({"slot_id":"P01","query":{"limit":"1"}})).unwrap();
    let expected_snapshot=blake3::hash(&json_codec::to_vec(&json!({"configuration":engine.config(),"generation":engine.state().business_generation.to_string()})).unwrap()).to_hex().to_string();
    assert_eq!(first["snapshot"], expected_snapshot);
    assert_eq!(first["entries"][0]["name"], "Alpha");
    assert_eq!(first["total_entries"], "3");
    assert_eq!(first["directory_branches_supported"], false);
    let row = &first["entries"][0];
    assert_eq!(row.as_object().unwrap().len(), 7);
    assert!(row.get("note").is_none());
    assert!(row.get("container").is_none());
    assert!(first["context"].is_null());
    assert!(
        engine
            .state()
            .repositories
            .values()
            .all(|repo| !repo.file_manager_root)
    );
    let cursor = first["next_cursor"].as_str().unwrap();
    engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Delta").unwrap(),
            "P01",
            1,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let stale = listing_json(
        &engine,
        &json!({"slot_id":"P01","query":{"limit":"1","cursor":cursor}}),
    )
    .unwrap_err();
    assert_eq!(stale.code, storage_contract::ErrorCode::ListingChanged);
    let snapshot = listing::entries(
        &engine,
        crate::ReadListing::parse("P01".into(), Default::default()).unwrap(),
    )
    .unwrap();
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
        Config::development([
            root.path().join("d1"),
            root.path().join("d2"),
            root.path().join("d3"),
        ]),
    )
    .unwrap();
    let mut engine = AdmissionEngine::new(engine);
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Files").unwrap(),
            "P01",
            3,
            "owner",
            storage_contract::OperationId::generate(),
        )
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
            operation_id: storage_contract::OperationId::generate(),
            principal: "owner".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "backup history fixture".into(),
            request_digest: None,
        })
        .unwrap();
    let entry = engine.list_tree(&repo.main_branch_id).unwrap()[0].clone();
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
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let listing = listing_json(
        &engine,
        &json!({"slot_id":"B01","query":{"path":"P01/Files"}}),
    )
    .unwrap();
    assert_eq!(listing["backup"], true);
    assert_eq!(listing["writable"], false);
    assert_eq!(listing["directory_branches"], json!([]));
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
