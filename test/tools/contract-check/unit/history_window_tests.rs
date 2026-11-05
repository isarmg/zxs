use super::*;
use storage_contract::history_window::{
    HistoryCollection, HistoryCursor, HistoryDirection, HistoryPageLimit, HistoryPosition,
    HistoryWindowPage, MAX_HISTORY_CURSOR_BYTES, MAX_HISTORY_FIELD_BYTES,
    MAX_HISTORY_PREVIEW_EDGES, MAX_HISTORY_RESPONSE_BYTES,
};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture(name: &str) -> Value {
    read(&root().join(format!(
        "test/tools/contract-check/fixtures/history-window/{name}.json"
    )))
    .unwrap()
}

#[test]
fn history_window_fixed_vectors_match_both_schemas_and_shared_wire() {
    let mut core = read(&root().join("docs/contracts/core.schema.json")).unwrap();
    let mut api = read(&root().join("docs/contracts/openapi.json")).unwrap();
    let vectors = read(&root().join("docs/contracts/vectors.json")).unwrap();
    assert_eq!(api["components"]["schemas"], bundle(&core["$defs"]));
    let mut cases = 0;
    for group in ["positive", "negative"] {
        for case in array(&vectors[group]).unwrap().iter().filter(|case| {
            case["name"].as_str().is_some_and(|name| {
                name.starts_with("history-window-") || name.starts_with("history-cursor-")
            })
        }) {
            cases += 1;
            let schema_name = case["schema"].as_str().unwrap();
            core["$ref"] = format!("#/$defs/{schema_name}").into();
            api["$ref"] = format!("#/components/schemas/{schema_name}").into();
            let shape_valid = group == "positive" || case["layer"] == "semantic";
            assert_eq!(
                validator(&core).unwrap().is_valid(&case["value"]),
                shape_valid,
                "{}",
                case["name"]
            );
            assert_eq!(
                validator(&api).unwrap().is_valid(&case["value"]),
                shape_valid,
                "{}",
                case["name"]
            );
            if shape_valid {
                assert_eq!(
                    semantics(schema_name, &case["value"]).is_ok(),
                    group == "positive",
                    "{}",
                    case["name"]
                );
            }
        }
    }
    assert!(cases >= 54);
    for name in [
        "nodes",
        "nodes-zero-preview",
        "versions",
        "branches",
        "branches-pending-empty",
        "merges",
        "node-parents",
        "version-sources",
        "merge-inputs",
        "merge-parents",
        "node-field-empty",
        "node-field-first",
        "node-field-tail",
    ] {
        let value = fixture(name);
        let page: HistoryWindowPage =
            storage_contract::decode_strict(&json_codec::to_vec(&value).unwrap()).unwrap();
        let roundtrip = read_wire(&json_codec::to_vec(&page).unwrap());
        assert_eq!(roundtrip, value, "{name}");
        semantics("HistoryWindowPage", &value).unwrap();
    }
}

fn read_wire(bytes: &[u8]) -> Value {
    storage_contract::strict_json(bytes).unwrap()
}

#[test]
fn history_window_routes_require_strict_queries_and_current_root_auxiliary_reads() {
    let api = read(&root().join("docs/contracts/openapi.json")).unwrap();
    let routes = [
        (
            "/api/file-manager/files/{file}/history",
            "HistoryNodesPage",
            HistoryCollection::Nodes,
        ),
        (
            "/api/files/{file}/history",
            "HistoryVersionsPage",
            HistoryCollection::Versions,
        ),
        (
            "/api/file-manager/files/{file}/history/branches",
            "HistoryBranchesPage",
            HistoryCollection::Branches,
        ),
        (
            "/api/file-manager/files/{file}/history/merges",
            "HistoryMergesPage",
            HistoryCollection::Merges,
        ),
    ];
    for (path, schema, collection) in routes {
        let operation = &api["paths"][path]["get"];
        assert_eq!(
            operation["responses"]["200"]["content"]["application/json"]["schema"]["$ref"],
            format!("#/components/schemas/{schema}")
        );
        let limit = array(&operation["parameters"])
            .unwrap()
            .iter()
            .find(|parameter| parameter["name"] == "limit")
            .unwrap();
        assert_eq!(limit["schema"]["maximum"], collection.maximum_limit());
        assert_eq!(
            limit["schema"]["default"],
            HistoryPageLimit::default_for(&collection).get()
        );
    }
    let mut count = 0;
    for (path, item) in object(&api["paths"]).unwrap() {
        let operation = &item["get"];
        if operation.get("x-historyWindowBudget").is_none() {
            continue;
        }
        count += 1;
        assert_eq!(operation["x-rejectUnknownQueryParameters"], true, "{path}");
        assert_eq!(
            operation["x-rejectDuplicateQueryParameters"], true,
            "{path}"
        );
        assert_eq!(
            operation["x-historyRootOrCursorRequired"],
            !path.ends_with("/history"),
            "{path}"
        );
        assert_eq!(
            operation["x-historyWindowBudget"]["max_response_bytes"],
            MAX_HISTORY_RESPONSE_BYTES
        );
        assert_eq!(
            operation["x-historyWindowBudget"]["cursor_ascii_bytes"],
            MAX_HISTORY_CURSOR_BYTES
        );
        assert_eq!(
            operation["x-dependentQueryParameters"]["repository_id"],
            json!(["path"])
        );
        assert_eq!(
            operation["x-dependentQueryParameters"]["path"],
            json!(["repository_id"])
        );
        assert!(
            operation["responses"]["409"]["description"]
                .as_str()
                .unwrap()
                .contains("HISTORY_CHANGED")
        );
        assert!(
            array(&operation["parameters"])
                .unwrap()
                .iter()
                .all(|parameter| parameter["name"] != "all")
        );
    }
    assert_eq!(count, 9);
    let nodes = &api["paths"]["/api/file-manager/files/{file}/history"]["get"];
    assert_eq!(
        nodes["x-historyWindowBudget"]["max_preview_edges"],
        MAX_HISTORY_PREVIEW_EDGES
    );
    assert_eq!(
        nodes["x-mutuallyExclusiveQueryParameters"],
        json!([["cursor", "anchor_node_id"]])
    );
    assert_eq!(
        nodes["x-dependentQueryParameters"]["anchor_node_id"],
        json!(["history_root"])
    );
    assert_eq!(
        api["components"]["schemas"]["HistoryNodeFieldPage"]["properties"]["text"]["x-maxUtf8Bytes"],
        MAX_HISTORY_FIELD_BYTES
    );
}

#[test]
fn history_parent_zero_preview_uses_a_typed_forward_start_and_empty_pending_branch_is_valid() {
    let value = fixture("nodes-zero-preview");
    assert_eq!(value["edges"], json!([]));
    let cursor =
        HistoryCursor::parse(value["nodes"][0]["parents"]["cursor"].as_str().unwrap()).unwrap();
    assert_eq!(cursor.payload().position, HistoryPosition::Start);
    assert_eq!(cursor.payload().direction, HistoryDirection::Forward);
    assert!(matches!(
        &cursor.payload().collection,
        HistoryCollection::NodeParents { .. }
    ));
    let mut payload = cursor.payload().clone();
    payload.direction = HistoryDirection::Backward;
    assert!(HistoryCursor::from_payload(payload).is_err());
    let branch = fixture("branches-pending-empty");
    assert!(branch["branches"][0]["node_id"].is_null());
    assert!(branch["branches"][0]["content"]["version_id"].is_null());
    semantics("HistoryBranchesPage", &branch).unwrap();
}

#[test]
fn history_field_fragments_count_utf8_and_mark_only_the_current_right_boundary() {
    let first = fixture("node-field-first");
    let tail = fixture("node-field-tail");
    let empty = fixture("node-field-empty");
    assert_eq!(first["text"].as_str().unwrap().len(), 4);
    assert_eq!(first["end_of_field"], false);
    assert_eq!(tail["byte_offset"], "4");
    assert_eq!(tail["end_of_field"], true);
    assert!(tail["next_cursor"].is_null());
    assert!(tail.get("complete").is_none());
    assert_eq!(empty["total_bytes"], "0");
    assert_eq!(empty["byte_offset"], "0");
    assert_eq!(empty["text"], "");
    let joined = format!(
        "{}{}",
        first["text"].as_str().unwrap(),
        tail["text"].as_str().unwrap()
    );
    assert_eq!(joined, "文a字");
    assert_eq!(joined.len(), 7);
    for value in [&first, &tail, &empty] {
        semantics("HistoryNodeFieldPage", value).unwrap();
    }
}

#[test]
fn history_merge_inputs_keep_branch_node_pairs_and_graph_preview_endpoints_are_checked() {
    let input = fixture("merge-inputs");
    assert_eq!(input["inputs"][0]["ordinal"], "0");
    assert_eq!(input["inputs"][1]["ordinal"], "1");
    assert!(input["inputs"][0].get("branch_id").is_some());
    assert!(input["inputs"][0].get("node_id").is_some());
    semantics("HistoryMergeInputsPage", &input).unwrap();
    let mut nodes = fixture("nodes");
    nodes["edges"][0]["endpoint"] = "outside_window".into();
    assert!(semantics("HistoryNodesPage", &nodes).is_err());
    let mut old = fixture("nodes");
    old["versions"] = json!([]);
    assert!(
        storage_contract::decode_strict::<HistoryWindowPage>(&json_codec::to_vec(&old).unwrap())
            .is_err()
    );
}

#[test]
fn history_changed_uses_the_existing_error_envelope_and_exact_first_party_code() {
    assert_eq!(
        storage_contract::ErrorCode::HistoryChanged.as_str(),
        "HISTORY_CHANGED"
    );
    assert_eq!(
        json_codec::to_vec(&storage_contract::ErrorCode::HistoryChanged).unwrap(),
        b"\"HISTORY_CHANGED\""
    );
    let mut core = read(&root().join("docs/contracts/core.schema.json")).unwrap();
    core["$ref"] = "#/$defs/ErrorResponse".into();
    let error = json!({"error":{"code":"HISTORY_CHANGED","message":"Refresh current history","retryable":false,"request_id":"req_00000000000000000000000000000001"}});
    assert!(validator(&core).unwrap().is_valid(&error));
}

fn assert_shared_history_shape(name: &str, value: &Value) {
    for (file, prefix) in [
        ("core.schema.json", "#/$defs/"),
        ("openapi.json", "#/components/schemas/"),
    ] {
        let mut document = read(&root().join("docs/contracts").join(file)).unwrap();
        let shape = if file == "core.schema.json" {
            document["$defs"][name].clone()
        } else {
            document["components"]["schemas"][name].clone()
        };
        document["$ref"] = format!("{prefix}{name}").into();
        let check = validator(&document).unwrap();
        assert!(check.is_valid(value), "{file}: {name}");
        assert_eq!(
            array(&shape["required"])
                .unwrap()
                .iter()
                .map(|field| field.as_str().unwrap())
                .collect::<BTreeSet<_>>(),
            object(value)
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            "{file}: {name} must require every published field"
        );
        let mut extra = value.clone();
        extra["unknown_field"] = true.into();
        assert!(!check.is_valid(&extra), "{file}: {name} unknown field");
        for required in array(&shape["required"]).unwrap() {
            let field = required.as_str().unwrap();
            let mut missing = value.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(!check.is_valid(&missing), "{file}: {name} missing {field}");
        }
        if value.get("sequence").is_some() {
            let mut zero = value.clone();
            zero["sequence"] = "0".into();
            assert!(check.is_valid(&zero), "{file}: {name} zero sequence");
            for invalid in [
                json!("18446744073709551616"),
                json!("01"),
                json!("-1"),
                json!("18446744073709551615\n"),
                json!(1),
            ] {
                let mut malformed = value.clone();
                malformed["sequence"] = invalid;
                assert!(!check.is_valid(&malformed), "{file}: {name} sequence");
            }
        }
    }
}

#[test]
fn historical_merge_shared_schemas_match_actual_wire_types() {
    use storage_contract::file_history_merge::{FileHistorySource, FileHistorySourceRequest};

    let HistoryWindowPage::Branches {
        identity, branches, ..
    } = json_codec::from_value(fixture("branches")).unwrap()
    else {
        panic!("fixture must contain file branches");
    };
    let source = FileHistorySource {
        file_id: identity.file_id,
        source_node_id: branches[0].node_id.clone().unwrap(),
        history_root: identity.history_root,
        branch: branches[0].clone(),
    };
    source
        .validate(&FileHistorySourceRequest {
            file_id: source.file_id.clone(),
            node_id: source.source_node_id.clone(),
            history_root: source.history_root,
        })
        .unwrap();
    let input = storage_contract::FileHistoryMergeInput {
        branch_id: source.branch.branch_id.as_str().into(),
        expected_node_id: source.branch.node_id.as_ref().unwrap().as_str().into(),
        source_node_id: source.source_node_id.as_str().into(),
    };
    assert_shared_history_shape(
        "FileHistoryMergeInput",
        &json_codec::to_value(input).unwrap(),
    );
    assert_shared_history_shape("FileHistorySource", &json_codec::to_value(source).unwrap());
}

#[test]
fn directory_history_shared_schemas_match_actual_wire_types_and_u64_limits() {
    use storage_contract::directory_history::*;
    use storage_contract::history_window::{HistoryEndpoint, HistoryRoot};

    let repository = storage_contract::new_id("repo").unwrap();
    let branch_id = storage_contract::new_id("dbr").unwrap();
    let tree_id = storage_contract::new_id("dtree").unwrap();
    let identity = DirectoryHistoryIdentity {
        repository_id: repository.clone(),
        directory_id: repository.clone(),
        history_tree_id: storage_contract::new_id("history").unwrap(),
        history_root: HistoryRoot::parse(&"a".repeat(64)).unwrap(),
        collection: DirectoryHistoryCollection::Nodes,
        limit: 128,
    };
    let current = DirectoryHistoryCurrent {
        committed_tree_id: Some(tree_id.clone()),
        staged: false,
        directory_branch_id: Some(branch_id.clone()),
        tree_id: Some(tree_id.clone()),
    };
    let node = DirectoryHistoryNode {
        message: "Commit directory".into(),
        tree_id: tree_id.clone(),
        directory_branch_id: branch_id.clone(),
        parent_tree_id: Some(storage_contract::new_id("dtree").unwrap()),
        parent_endpoint: Some(HistoryEndpoint::OutsideWindow),
        commit_id: storage_contract::new_id("commit").unwrap(),
        path: String::new(),
        sequence: u64::MAX,
    };
    let branch = DirectoryHistoryBranch {
        committed_tree_id: None,
        staged: true,
        directory_branch_id: branch_id,
        name: "main".into(),
        parent_branch_id: None,
        head_tree_id: tree_id,
        materialized: true,
        current: true,
        sequence: u64::MAX,
    };
    for (name, value) in [
        (
            "DirectoryHistoryIdentity",
            json_codec::to_value(&identity).unwrap(),
        ),
        (
            "DirectoryHistoryCurrent",
            json_codec::to_value(&current).unwrap(),
        ),
        ("DirectoryHistoryNode", json_codec::to_value(&node).unwrap()),
        (
            "DirectoryHistoryBranch",
            json_codec::to_value(&branch).unwrap(),
        ),
    ] {
        assert_shared_history_shape(name, &value);
    }
    let mut page = DirectoryHistoryPage {
        identity,
        current,
        nodes: vec![node],
        branches: Vec::new(),
        page: DirectoryHistoryPagination {
            previous_cursor: None,
            next_cursor: None,
        },
    };
    for collection in [
        DirectoryHistoryCollection::Nodes,
        DirectoryHistoryCollection::Branches,
    ] {
        if collection == DirectoryHistoryCollection::Branches {
            page.identity.collection = collection;
            page.nodes.clear();
            page.branches.push(branch.clone());
        }
        let request = DirectoryHistoryRequest::parse(
            &repository,
            &repository,
            collection,
            BTreeMap::from([("limit".into(), "128".into())]),
        )
        .unwrap();
        page.validate(&request).unwrap();
        assert_shared_history_shape(
            "DirectoryHistoryPage",
            &json_codec::to_value(&page).unwrap(),
        );
    }
}

#[test]
fn directory_staging_schemas_match_public_wire_types() {
    use storage_contract::directory_staging::{DirectoryCommitBody, DirectoryStagingStatus};
    let tree = storage_contract::new_id("dtree").unwrap();
    let status = DirectoryStagingStatus {
        repository_id: storage_contract::new_id("repo").unwrap(),
        directory_branch_id: storage_contract::new_id("dbr").unwrap(),
        head_tree_id: tree.clone(),
        committed_tree_id: None,
        staged: true,
        added: u64::MAX,
        modified: 0,
        deleted: 1,
    };
    let body = DirectoryCommitBody {
        expected_tree_id: tree,
        expected_index_generation: 1,
        message: "Commit directory".into(),
        operation_id: storage_contract::OperationId::generate().unwrap(),
    };
    assert_shared_history_shape(
        "DirectoryStagingStatus",
        &json_codec::to_value(status).unwrap(),
    );
    assert_shared_history_shape("DirectoryCommitBody", &json_codec::to_value(body).unwrap());
}
