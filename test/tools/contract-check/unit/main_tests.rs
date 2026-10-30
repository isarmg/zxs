use super::*;
#[test]
fn operation_outcome_and_observation_vectors_use_distinct_strict_schemas() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/contracts");
    let mut core = read(&root.join("core.schema.json")).unwrap();
    let mut api = read(&root.join("openapi.json")).unwrap();
    let vectors = read(&root.join("vectors.json")).unwrap();
    for name in [
        "SaveOperation",
        "SaveOperationObservation",
        "GenericOperation",
        "GenericOperationObservation",
    ] {
        core["$ref"] = format!("#/$defs/{name}").into();
        api["$ref"] = format!("#/components/schemas/{name}").into();
        let core_schema = validator(&core).unwrap();
        let api_schema = validator(&api).unwrap();
        let mut cases = 0;
        for group in ["positive", "negative"] {
            for case in array(&vectors[group])
                .unwrap()
                .iter()
                .filter(|case| case["schema"] == name)
            {
                cases += 1;
                let shape_valid = group == "positive" || case["layer"] == "semantic";
                let value = &case["value"];
                assert_eq!(core_schema.is_valid(value), shape_valid, "{}", case["name"]);
                assert_eq!(api_schema.is_valid(value), shape_valid, "{}", case["name"]);
                if shape_valid {
                    assert_eq!(
                        semantics(name, value).is_ok(),
                        group == "positive",
                        "{}",
                        case["name"]
                    );
                }
            }
        }
        assert!(cases > 0, "missing fixed cases for {name}");
    }
}

#[test]
fn operation_get_routes_separate_outcome_polling_from_explicit_observation() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/contracts");
    let api = read(&root.join("openapi.json")).unwrap();
    for (route, name) in [
        ("/api/operations/{operation}/outcome", "OperationOutcome"),
        ("/api/operations/{operation}", "OperationObservation"),
    ] {
        assert_eq!(
            api["paths"][route]["get"]["responses"]["200"]["content"]["application/json"]["schema"]
                ["$ref"],
            format!("#/components/schemas/{name}")
        );
    }
    let mut immediate = 0;
    for path in object(&api["paths"]).unwrap().values() {
        for method in ["post", "put", "patch", "delete"] {
            let Some(responses) = path.get(method).and_then(|item| item.get("responses")) else {
                continue;
            };
            for response in object(responses).unwrap().values() {
                let shape = &response["content"]["application/json"]["schema"];
                if shape["properties"].get("operation_id").is_some() {
                    immediate += 1;
                    assert!(shape["properties"].get("current_health").is_none());
                    assert!(shape["properties"].get("checked_at").is_none());
                }
            }
        }
    }
    assert!(immediate >= 16, "missing direct mutation response shapes");
}

#[test]
fn checked_health_does_not_rewrite_the_outcome_or_publication_proof() {
    use storage_contract::client_protocol::{
        OperationStatus, operation_observation_status, operation_status,
    };
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/contracts");
    let mut core = read(&root.join("core.schema.json")).unwrap();
    let vectors = read(&root.join("vectors.json")).unwrap();
    let original = array(&vectors["positive"])
        .unwrap()
        .iter()
        .find(|case| case["name"] == "completed")
        .unwrap()["value"]
        .clone();
    assert!(original.get("current_health").is_none());
    assert_eq!(
        operation_status(&original),
        Some(OperationStatus::Published)
    );
    core["$ref"] = "#/$defs/SaveOperationObservation".into();
    let schema = validator(&core).unwrap();
    for state in ["HEALTHY", "DEGRADED", "UNAVAILABLE", "UNKNOWN"] {
        let mut observed = original.clone();
        observed["current_health"] = json!({"state":state,"checked_at":"2026-10-18T00:00:00Z"});
        assert!(schema.is_valid(&observed));
        semantics("SaveOperationObservation", &observed).unwrap();
        assert_eq!(observed["result"], original["result"]);
        assert_eq!(operation_status(&observed), None);
        assert_eq!(
            operation_observation_status(&observed),
            Some(OperationStatus::Published)
        );
    }
}

#[test]
fn generic_diagnostic_health_requires_a_published_operation() {
    use storage_contract::client_protocol::{OperationStatus, operation_observation_status};
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/contracts");
    let mut core = read(&root.join("core.schema.json")).unwrap();
    let vectors = read(&root.join("vectors.json")).unwrap();
    let original = array(&vectors["positive"])
        .unwrap()
        .iter()
        .find(|case| case["name"] == "generic-not-observed")
        .unwrap()["value"]
        .clone();
    core["$ref"] = "#/$defs/GenericOperationObservation".into();
    let schema = validator(&core).unwrap();
    for commit in [
        "STAGING",
        "SEALED",
        "PREPARED",
        "DECIDING",
        "DECIDED",
        "PROTECTED",
        "RECOVERING",
        "RECOVERY_REQUIRED",
        "IN_DOUBT",
        "ABORTED",
    ] {
        let mut observed = original.clone();
        observed["state"] = "in_doubt".into();
        observed["commit_state"] = commit.into();
        observed["protection_at_publish"] = "UNCONFIRMED".into();
        let expected = if commit == "ABORTED" {
            OperationStatus::Rejected
        } else {
            OperationStatus::Unknown
        };
        assert!(schema.is_valid(&observed), "{commit}");
        assert_eq!(operation_observation_status(&observed), Some(expected));
        observed["current_health"] = json!({"state":"UNKNOWN","checked_at":"2026-10-18T00:00:00Z"});
        assert!(
            !schema.is_valid(&observed),
            "fabricated observation for {commit}"
        );
        assert_eq!(operation_observation_status(&observed), None);
    }
    for (commit, state, protection, status) in [
        (
            "PUBLISHED",
            "completed",
            "NOT_APPLICABLE",
            OperationStatus::Published,
        ),
        (
            "ACKED",
            "completed",
            "NOT_APPLICABLE",
            OperationStatus::Published,
        ),
        (
            "PUBLISHED_DEGRADED",
            "in_doubt",
            "BELOW_TARGET",
            OperationStatus::Degraded,
        ),
    ] {
        let mut observed = original.clone();
        observed["state"] = state.into();
        observed["commit_state"] = commit.into();
        observed["protection_at_publish"] = protection.into();
        observed["current_health"] =
            json!({"state":"UNAVAILABLE","checked_at":"2026-10-18T00:00:00Z"});
        assert!(schema.is_valid(&observed), "{commit}");
        assert_eq!(operation_observation_status(&observed), Some(status));
    }
}

#[test]
fn checked_in_contracts_and_document_examples_are_consistent() {
    run(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")).unwrap();
}

#[test]
fn persisted_repository_schema_requires_an_exact_smb_binding_revision() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/contracts");
    let mut core = read(&root.join("core.schema.json")).unwrap();
    core["$ref"] = "#/$defs/HistoryRepository".into();
    let schema = validator(&core).unwrap();
    let vectors = read(&root.join("vectors.json")).unwrap();
    let repository = array(&vectors["positive"])
        .unwrap()
        .iter()
        .find_map(|case| case["value"].get("repository"))
        .unwrap()
        .clone();
    assert!(schema.is_valid(&repository));
    for invalid in [
        json!(null),
        json!(1),
        json!("01"),
        json!("18446744073709551616"),
    ] {
        let mut changed = repository.clone();
        changed["smb_binding_generation"] = invalid;
        assert!(!schema.is_valid(&changed));
    }
    for required in ["smb_binding_generation", "smb_enabled"] {
        let mut missing = repository.clone();
        missing.as_object_mut().unwrap().remove(required);
        assert!(!schema.is_valid(&missing), "missing {required}");
    }
}

#[test]
fn protection_policy_schema_requires_current_or_retained_role_without_a_default() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/contracts");
    let mut core = read(&root.join("core.schema.json")).unwrap();
    core["$ref"] = "#/$defs/ProtectionPolicy".into();
    let core_schema = validator(&core).unwrap();
    let mut api = read(&root.join("openapi.json")).unwrap();
    api["$ref"] = "#/components/schemas/ProtectionPolicy".into();
    let api_schema = validator(&api).unwrap();
    let vectors = read(&root.join("vectors.json")).unwrap();
    let mut roles = BTreeSet::new();
    for case in array(&vectors["positive"])
        .unwrap()
        .iter()
        .filter(|case| case["schema"] == "ProtectionPolicy")
    {
        let value = &case["value"];
        assert!(core_schema.is_valid(value), "{}", case["name"]);
        assert!(api_schema.is_valid(value), "{}", case["name"]);
        let encoded = storage_contract::canonical_json(value).unwrap();
        let policy: storage_contract::ProtectionPolicy =
            storage_contract::decode_strict(&encoded).unwrap();
        roles.insert(policy.inheritable);
        assert_eq!(policy.inheritable, value["inheritable"].as_bool().unwrap());
        assert_eq!(storage_contract::canonical_json(&policy).unwrap(), encoded);

        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove("inheritable");
        assert!(!core_schema.is_valid(&missing));
        assert!(!api_schema.is_valid(&missing));
        assert!(
            storage_contract::decode_strict::<storage_contract::ProtectionPolicy>(
                &storage_contract::canonical_json(&missing).unwrap()
            )
            .is_err()
        );
        for invalid in [json!(null), json!(0), json!("false")] {
            let mut changed = value.clone();
            changed["inheritable"] = invalid;
            assert!(!core_schema.is_valid(&changed));
            assert!(!api_schema.is_valid(&changed));
            assert!(
                storage_contract::decode_strict::<storage_contract::ProtectionPolicy>(
                    &storage_contract::canonical_json(&changed).unwrap()
                )
                .is_err()
            );
        }
    }
    assert_eq!(roles, BTreeSet::from([false, true]));
}

#[test]
fn sparse_branch_and_commit_canonical_bytes_omit_empty_pending_maps() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/contracts");
    let vectors = read(&root.join("vectors.json")).unwrap();
    let value = array(&vectors["positive"])
        .unwrap()
        .iter()
        .find(|case| case["name"] == "sparse-branch-without-pending-map")
        .unwrap()["value"]
        .clone();
    let before = storage_contract::canonical_json(&value).unwrap();
    let branch: storage_contract::Branch = storage_contract::decode_strict(&before).unwrap();
    assert!(branch.pending_file_versions.is_empty());
    assert_eq!(storage_contract::canonical_json(&branch).unwrap(), before);
    assert_eq!(
        storage_contract::object_hash("branch", &branch).unwrap(),
        storage_contract::object_hash("branch", &value).unwrap()
    );
    let mut created = branch;
    created.pending_file_versions.insert(
        storage_contract::new_id("file"),
        storage_contract::new_id("ver"),
    );
    let encoded = storage_contract::canonical_json(&created).unwrap();
    let decoded: storage_contract::Branch = storage_contract::decode_strict(&encoded).unwrap();
    assert_eq!(decoded.pending_file_versions, created.pending_file_versions);
    assert_ne!(encoded, before);
    created.pending_file_versions.clear();
    assert_eq!(storage_contract::canonical_json(&created).unwrap(), before);
    let value = array(&vectors["positive"])
        .unwrap()
        .iter()
        .find(|case| case["name"] == "sparse-commit-without-pending-map")
        .unwrap()["value"]
        .clone();
    let before = storage_contract::canonical_json(&value).unwrap();
    let mut commit: storage_contract::Commit = storage_contract::decode_strict(&before).unwrap();
    assert!(commit.pending_file_versions.is_empty());
    assert_eq!(storage_contract::canonical_json(&commit).unwrap(), before);
    assert_eq!(
        storage_contract::object_hash("commit", &commit).unwrap(),
        storage_contract::object_hash("commit", &value).unwrap()
    );
    commit.pending_file_versions.insert(
        commit.entries[0].file_id.clone(),
        commit.entries[0].version_id.clone(),
    );
    let encoded = storage_contract::canonical_json(&commit).unwrap();
    let decoded: storage_contract::Commit = storage_contract::decode_strict(&encoded).unwrap();
    assert_eq!(decoded.pending_file_versions, commit.pending_file_versions);
    commit.pending_file_versions.clear();
    assert_eq!(storage_contract::canonical_json(&commit).unwrap(), before);
}

#[test]
fn namespace_deletion_facts_preserve_order_time_and_sparse_commit_encoding() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/contracts");
    let vectors = read(&root.join("vectors.json")).unwrap();
    let value = array(&vectors["positive"])
        .unwrap()
        .iter()
        .find(|case| case["name"] == "commit-with-ordered-content-independent-deletions")
        .unwrap()["value"]
        .clone();
    let mut core = read(&root.join("core.schema.json")).unwrap();
    core["$ref"] = "#/$defs/Commit".into();
    let core_schema = validator(&core).unwrap();
    let mut api = read(&root.join("openapi.json")).unwrap();
    api["$ref"] = "#/components/schemas/Commit".into();
    let api_schema = validator(&api).unwrap();
    assert!(core_schema.is_valid(&value));
    assert!(api_schema.is_valid(&value));

    let encoded = storage_contract::canonical_json(&value).unwrap();
    let mut commit: storage_contract::Commit = storage_contract::decode_strict(&encoded).unwrap();
    let facts = commit.namespace_deletions.as_ref().unwrap();
    assert_eq!(facts.timestamp, u64::MAX);
    assert_eq!(facts.paths.len(), 3);
    assert_eq!(
        facts.paths[0].kind,
        storage_contract::NamespaceEntryKind::Directory
    );
    assert_eq!(
        facts.paths[1].kind,
        storage_contract::NamespaceEntryKind::File
    );
    assert_eq!(facts.paths[1], facts.paths[2]);
    assert_eq!(storage_contract::canonical_json(&commit).unwrap(), encoded);
    assert_eq!(
        storage_contract::object_hash("commit", &commit).unwrap(),
        storage_contract::object_hash("commit", &value).unwrap()
    );
    commit
        .namespace_deletions
        .as_mut()
        .unwrap()
        .paths
        .swap(0, 1);
    assert_ne!(storage_contract::canonical_json(&commit).unwrap(), encoded);
    commit.namespace_deletions = None;
    let sparse = storage_contract::canonical_json(&commit).unwrap();
    let mut without_facts = value.clone();
    without_facts
        .as_object_mut()
        .unwrap()
        .remove("namespace_deletions");
    assert_eq!(
        sparse,
        storage_contract::canonical_json(&without_facts).unwrap()
    );
    assert!(core_schema.is_valid(&without_facts));
    assert!(api_schema.is_valid(&without_facts));

    for invalid in [json!(null), json!({"timestamp":"1","paths":[]})] {
        let mut changed = value.clone();
        changed["namespace_deletions"] = invalid;
        assert!(!core_schema.is_valid(&changed));
        assert!(!api_schema.is_valid(&changed));
    }
}

#[test]
fn namespace_deletion_fact_shapes_reject_content_references_and_timestamp_aliases() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/contracts");
    let vectors = read(&root.join("vectors.json")).unwrap();
    let core = read(&root.join("core.schema.json")).unwrap();
    let api = read(&root.join("openapi.json")).unwrap();
    for name in ["NamespaceDeletions", "DeletedPath"] {
        let mut schema = core.clone();
        schema["$ref"] = format!("#/$defs/{name}").into();
        let core_schema = validator(&schema).unwrap();
        let mut schema = api.clone();
        schema["$ref"] = format!("#/components/schemas/{name}").into();
        let api_schema = validator(&schema).unwrap();
        for group in ["positive", "negative"] {
            for case in array(&vectors[group])
                .unwrap()
                .iter()
                .filter(|case| case["schema"] == name)
            {
                let value = &case["value"];
                assert_eq!(
                    core_schema.is_valid(value),
                    group == "positive",
                    "{}",
                    case["name"]
                );
                assert_eq!(
                    api_schema.is_valid(value),
                    group == "positive",
                    "{}",
                    case["name"]
                );
                if group == "positive" {
                    let encoded = storage_contract::canonical_json(value).unwrap();
                    let roundtrip = match name {
                        "NamespaceDeletions" => {
                            let decoded: storage_contract::NamespaceDeletions =
                                storage_contract::decode_strict(&encoded).unwrap();
                            storage_contract::canonical_json(&decoded).unwrap()
                        }
                        "DeletedPath" => {
                            let decoded: storage_contract::DeletedPath =
                                storage_contract::decode_strict(&encoded).unwrap();
                            storage_contract::canonical_json(&decoded).unwrap()
                        }
                        _ => unreachable!(),
                    };
                    assert_eq!(roundtrip, encoded);
                }
            }
        }
    }
}

#[test]
fn inherited_file_manager_projection_distinguishes_visible_parent_from_own_content() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/contracts/openapi.json");
    let mut api = read(&path).unwrap();
    let item = api["paths"]["/api/file-manager/disks/{slot}/entries"]["get"]["responses"]["200"]
        ["content"]["application/json"]["schema"]["properties"]["entries"]["items"]
        .clone();
    api["$defs"]["entry_projection"] = item;
    api["$ref"] = "#/$defs/entry_projection".into();
    let schema = validator(&api).unwrap();
    let source = storage_contract::new_id("ver");
    let inherited = json!({
        "entry_id":"entry-example","name":"a.txt","path":"a.txt","kind":"file",
        "container":true,"size":"12","file_id":storage_contract::new_id("file"),
        "version_id":source,"own_version_id":null,"inherited":true,"materialized":false,
        "content_state":"PENDING","visible_content_state":"AVAILABLE",
        "source_version_id":source,"selected_branch_id":storage_contract::new_id("br")
    });
    let accepts = |value: &Value| schema.is_valid(value) && validate_content_view(value).is_ok();
    assert!(accepts(&inherited));
    for (field, value) in [
        ("version_id", Value::Null),
        ("version_id", json!("")),
        ("version_id", json!(storage_contract::new_id("ver"))),
        ("own_version_id", json!(source)),
        ("inherited", json!(false)),
        ("source_version_id", Value::Null),
        ("content_state", json!("AVAILABLE")),
        ("visible_content_state", json!("PENDING")),
    ] {
        let mut invalid = inherited.clone();
        invalid[field] = value;
        assert!(!accepts(&invalid), "invalid inherited {field}");
    }
    let mut written = inherited.clone();
    let own = storage_contract::new_id("ver");
    written["materialized"] = true.into();
    written["inherited"] = false.into();
    written["version_id"] = own.clone().into();
    written["own_version_id"] = own.into();
    written["source_version_id"] = Value::Null;
    written["content_state"] = "AVAILABLE".into();
    written["size"] = "0".into();
    // A genuinely written zero-byte FULL is valid and distinguishable.
    assert!(accepts(&written));
    let mut wrong_own = written.clone();
    wrong_own["own_version_id"] = storage_contract::new_id("ver").into();
    assert!(!accepts(&wrong_own));
    let mut wrong_state = written.clone();
    wrong_state["visible_content_state"] = "PURGED".into();
    assert!(!accepts(&wrong_state));
    let mut purged_parent = inherited.clone();
    purged_parent["visible_content_state"] = "PURGED".into();
    assert!(accepts(&purged_parent));
    written["version_id"] = Value::Null;
    assert!(!accepts(&written));
    assert!(schema.is_valid(&json!({
        "entry_id":"virtual-directory","name":"P01","path":"P01","kind":"directory",
        "size":"0","file_id":"","version_id":""
    })));
    let creation_shape = api["paths"]["/api/file-manager/files/{file}/branches"]["post"]
        ["responses"]["200"]["content"]["application/json"]["schema"]["properties"]
        ["result"]
        .clone();
    let directory_branch_shape = api["paths"]
        ["/api/file-manager/disks/{slot}/directory-branches"]["post"]["responses"]["200"]
        ["content"]["application/json"]["schema"]["properties"]["result"]["properties"]
        ["branches"]["items"]
        .clone();
    api["$defs"]["creation"] = creation_shape;
    api["$defs"]["directory_branch_creation"] = directory_branch_shape;
    api["$ref"] = "#/$defs/creation".into();
    let creation_schema = validator(&api).unwrap();
    let creation = json!({
        "file_id":inherited["file_id"],"branch_id":inherited["selected_branch_id"],
        "commit_id":storage_contract::new_id("commit"),"history_tree_id":storage_contract::new_id("history"),
        "copied_files":"0","placeholder_files":"1","materialized":false,"selected":false,
        "inherited":true,"own_version_id":null,"version_id":source,
        "source_version_id":source,"content_state":"PENDING"
    });
    assert!(creation_schema.is_valid(&creation));
    let mut rewritten_receipt = creation.clone();
    rewritten_receipt["materialized"] = true.into();
    rewritten_receipt["inherited"] = false.into();
    rewritten_receipt["own_version_id"] = storage_contract::new_id("ver").into();
    assert!(!creation_schema.is_valid(&rewritten_receipt));
    api["$ref"] = "#/$defs/directory_branch_creation".into();
    let directory_branch_schema = validator(&api).unwrap();
    assert!(directory_branch_schema.is_valid(&json!({
        "file_id":inherited["file_id"],"branch_id":inherited["selected_branch_id"],"path":"a.txt",
        "materialized":false,"inherited":true,"own_version_id":null,
        "version_id":source,"source_version_id":source
    })));
}
#[test]
fn semantic_upload_overflow_is_rejected() {
    assert!(semantics("UploadStatus",&json!({"logical_size":u64::MAX.to_string(),"state":"COMPLETE","durable_ranges":[{"offset":u64::MAX.to_string(),"length":"1"}]})).is_err());
}
#[test]
fn openapi_duplicate_operation_ids_and_unbound_paths_are_rejected() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/contracts/openapi.json");
    let mut value = read(&root).unwrap();
    value["paths"]["/api/workspaces/{workspace}/commit"]["post"]["operationId"] =
        "getContentSaveOperation".into();
    assert!(openapi(&value).is_err());
    let mut value = read(&root).unwrap();
    value["paths"]["/api/workspaces/{workspace}/commit"]["post"]["parameters"] = json!([]);
    assert!(openapi(&value).is_err());
}

#[test]
fn history_version_contract_uses_source_counts_and_independent_bounded_members() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let api = read(&root.join("docs/contracts/openapi.json")).unwrap();
    let version = &api["components"]["schemas"]["HistoryVersionSummary"];
    assert_eq!(
        version["properties"]["source_count"]["$ref"],
        "#/components/schemas/UInt64"
    );
    assert!(version["properties"].get("source_version_ids").is_none());
    let members = &api["components"]["schemas"]["HistoryVersionSourcesPage"];
    assert_eq!(members["properties"]["sources"]["maxItems"], 512);
    assert_eq!(members["additionalProperties"], false);
    let fixture =
        read(&root.join("test/tools/contract-check/fixtures/history-window/versions.json"))
            .unwrap();
    let mut document = api.clone();
    document["$ref"] = "#/components/schemas/HistoryVersionsPage".into();
    let schema = validator(&document).unwrap();
    assert!(schema.is_valid(&fixture));
    let mut invalid = fixture.clone();
    invalid["versions"][0]["source_version_ids"] = json!([]);
    assert!(!schema.is_valid(&invalid));
    let mut invalid = fixture;
    invalid["versions"][0]["created_at"] = "2026-10-30T00:00:00Z".into();
    assert!(!schema.is_valid(&invalid));
}

#[test]
fn note_write_contracts_enforce_utf8_bytes_and_control_characters() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/contracts/openapi.json");
    let api = read(&path).unwrap();
    for (route, method) in [
        ("/api/file-manager/files/{file}/metadata", "post"),
        ("/api/file-manager/versions/{version}/metadata", "post"),
        ("/api/file-manager/files/{file}/nodes/{node}", "patch"),
    ] {
        let note = &api["paths"][route][method]["requestBody"]["content"]["application/json"]["schema"]
            ["properties"]["note"];
        let schema = validator(note).unwrap();
        // JSON Schema maxLength counts Unicode characters, not UTF-8 bytes.
        let byte_limit = note["x-maxUtf8Bytes"].as_u64().unwrap() as usize;
        assert_eq!(byte_limit, 128);
        let accepts = |value: &str| schema.is_valid(&json!(value)) && value.len() <= byte_limit;
        for valid in [
            String::new(),
            "a".repeat(128),
            format!("{}ab", "文".repeat(42)),
        ] {
            assert!(accepts(&valid), "valid note rejected: {route}");
        }
        for invalid in [
            "a".repeat(129),
            "文".repeat(43),
            "line\nnext".into(),
            "line\rnext".into(),
            "a\tb".into(),
            "a\0b".into(),
            "a\u{7f}b".into(),
            "a\u{85}b".into(),
        ] {
            assert!(!accepts(&invalid), "invalid note accepted: {route}");
        }
    }
}

#[test]
fn note_read_shapes_accept_current_valid_notes() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/contracts/openapi.json");
    let api = read(&path).unwrap();
    let entries = &api["paths"]["/api/file-manager/disks/{slot}/entries"]["get"]["responses"]["200"]
        ["content"]["application/json"]["schema"]["properties"]["entries"]["items"]["properties"]["note"];
    let history = &api["components"]["schemas"];
    // Response shapes are deliberately broader than the current note semantics.
    // Passing a shape check does not authorize an invalid persisted note or an
    // older format; the engine validates notes at write and history-read boundaries.
    for (scope, note) in [
        ("directory entry", entries),
        (
            "file history",
            &history["HistoryContextSummary"]["properties"]["note"],
        ),
        (
            "version history",
            &history["HistoryVersionSummary"]["properties"]["note"],
        ),
        (
            "history node",
            &history["HistoryNodeSummary"]["properties"]["note"],
        ),
    ] {
        let schema = validator(note).unwrap();
        for valid in [
            String::new(),
            "a".repeat(128),
            format!("{}ab", "文".repeat(42)),
        ] {
            storage_contract::validate_note(&valid).unwrap();
            assert!(
                schema.is_valid(&json!(valid)),
                "valid note rejected: {scope}"
            );
        }
        for invalid in [Value::Null, json!(false), json!(1), json!([]), json!({})] {
            assert!(
                !schema.is_valid(&invalid),
                "non-string note accepted: {scope}"
            );
        }
    }
}

#[test]
fn directory_batch_contract_requires_frozen_explicit_mappings() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/contracts/openapi.json");
    let mut api = read(&path).unwrap();
    api["$ref"]="#/paths/~1api~1file-manager~1disks~1%7Bslot%7D~1namespace-batch/post/requestBody/content/application~1json/schema".into();
    let schema = validator(&api).unwrap();
    let source = json!({"directory_branch_id":storage_contract::new_id("dbr"),"expected_tree_id":storage_contract::new_id("dtree"),"path":"a/report.txt","entry_id":storage_contract::new_id("entry")});
    let target = json!({"directory_branch_id":storage_contract::new_id("dbr"),"expected_tree_id":storage_contract::new_id("dtree"),"path":"b/report.txt"});
    let valid = json!({"kind":"move","operation_id":storage_contract::new_id("op"),"items":[{"source":source,"target":target}]});
    assert!(schema.is_valid(&valid));
    for kind in ["copy", "rename"] {
        let mut body = valid.clone();
        body["kind"] = kind.into();
        assert!(schema.is_valid(&body));
    }
    let mut delete = valid.clone();
    delete["kind"] = "delete".into();
    delete["items"][0].as_object_mut().unwrap().remove("target");
    assert!(schema.is_valid(&delete));
    for pointer in ["expected_tree_id", "entry_id"] {
        let mut invalid = valid.clone();
        invalid["items"][0]["source"]
            .as_object_mut()
            .unwrap()
            .remove(pointer);
        assert!(!schema.is_valid(&invalid));
    }
    let mut invalid = valid.clone();
    invalid["items"][0]["source"]["all_branches"] = true.into();
    assert!(!schema.is_valid(&invalid));
    invalid = valid.clone();
    invalid["items"] = json!([]);
    assert!(!schema.is_valid(&invalid));
    invalid = valid.clone();
    invalid["items"][0]
        .as_object_mut()
        .unwrap()
        .remove("target");
    assert!(!schema.is_valid(&invalid));
    invalid = valid.clone();
    invalid["kind"] = "delete".into();
    assert!(!schema.is_valid(&invalid));
    invalid = valid;
    invalid["kind"] = "overwrite".into();
    assert!(!schema.is_valid(&invalid));
}

#[test]
fn history_node_summary_preserves_real_content_ids_without_inline_relations_or_fields() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let api = read(&root.join("docs/contracts/openapi.json")).unwrap();
    let fixture =
        read(&root.join("test/tools/contract-check/fixtures/history-window/nodes.json")).unwrap();
    let mut document = api.clone();
    document["$ref"] = "#/components/schemas/HistoryNodeSummary".into();
    let schema = validator(&document).unwrap();
    let nodes = fixture["nodes"].as_array().unwrap();
    for node in nodes {
        assert!(schema.is_valid(node));
        for forbidden in [
            "parent_node_ids",
            "source_version_ids",
            "message",
            "author",
            "created_at",
        ] {
            let mut invalid = node.clone();
            invalid[forbidden] = json!([]);
            assert!(!schema.is_valid(&invalid), "{forbidden}");
        }
    }
    let rename = nodes.iter().find(|node| node["kind"] == "rename").unwrap();
    let mut invalid = rename.clone();
    invalid["version_id"] = rename["node_id"].clone();
    assert!(!schema.is_valid(&invalid));
    let mut invalid = rename.clone();
    invalid["node_id"] = rename["version_id"].clone();
    assert!(!schema.is_valid(&invalid));
    let note = &api["paths"]["/api/file-manager/files/{file}/nodes/{node}"]["patch"]["requestBody"]
        ["content"]["application/json"]["schema"]["properties"]["note"];
    assert_eq!(note["x-maxUtf8Bytes"], 128);
    let note_schema = validator(note).unwrap();
    assert!(note_schema.is_valid(&json!("")));
    assert!(!note_schema.is_valid(&json!("bad\nline")));
    let source = &api["paths"]["/api/file-manager/files/{file}/branches"]["post"]["requestBody"]["content"]
        ["application/json"]["schema"]["properties"]["source_node_id"];
    let mut source_document = api.clone();
    source_document["$ref"] = source["$ref"].clone();
    let source_schema = validator(&source_document).unwrap();
    assert!(source_schema.is_valid(&rename["version_id"]));
    assert!(source_schema.is_valid(&rename["node_id"]));
    assert!(!source_schema.is_valid(&fixture["identity"]["file_id"]));
}

#[test]
fn file_merge_tags_and_scope_backups_require_explicit_reviewed_baselines() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/contracts/openapi.json");
    let api = read(&path).unwrap();
    let request_schema = |route: &str, method: &str| {
        let mut document = api.clone();
        document["allOf"] = json!([
            api["paths"][route][method]["requestBody"]["content"]["application/json"]["schema"]
        ]);
        validator(&document).unwrap()
    };
    let file = storage_contract::new_id("file");
    let branch = storage_contract::new_id("br");
    let other = storage_contract::new_id("br");
    let version = storage_contract::new_id("ver");
    let meta = storage_contract::new_id("fmeta");
    let operation = storage_contract::new_id("op");
    let merge = request_schema("/api/file-manager/files/{file}/merge", "post");
    let body = json!({"inputs":[{"branch_id":branch,"expected_node_id":version},{"branch_id":other,"expected_node_id":meta}],"chosen_node_id":meta,"surviving_branch_id":branch,"confirmed":true,"operation_id":operation});
    assert!(merge.is_valid(&body));
    for key in ["confirmed", "surviving_branch_id", "chosen_node_id"] {
        let mut invalid = body.clone();
        invalid.as_object_mut().unwrap().remove(key);
        assert!(!merge.is_valid(&invalid));
    }
    let mut invalid = body.clone();
    invalid["confirmed"] = false.into();
    assert!(!merge.is_valid(&invalid));
    invalid = body.clone();
    invalid["inputs"][0]
        .as_object_mut()
        .unwrap()
        .remove("expected_node_id");
    assert!(!merge.is_valid(&invalid));
    invalid = body.clone();
    invalid["inputs"] = json!([body["inputs"][0]]);
    assert!(!merge.is_valid(&invalid));
    invalid = body;
    invalid["repository_id"] = storage_contract::new_id("repo").into();
    assert!(!merge.is_valid(&invalid));
    let tags = request_schema("/api/file-manager/files/{file}/tags", "post");
    let body = json!({"targets":[{"branch_id":branch,"source_node_id":version,"expected_node_id":meta}],"add":["审核"],"remove":[],"operation_id":operation});
    assert!(tags.is_valid(&body));
    let mut invalid = body.clone();
    invalid["targets"][0]
        .as_object_mut()
        .unwrap()
        .remove("expected_node_id");
    assert!(!tags.is_valid(&invalid));
    invalid = body.clone();
    invalid["add"] = json!(["bad\nlabel"]);
    assert!(!tags.is_valid(&invalid));
    invalid = body;
    invalid["add"] = json!((0..17).map(|n| format!("tag{n}")).collect::<Vec<_>>());
    assert!(!tags.is_valid(&invalid));
    let backups = request_schema("/api/repositories/{repo}/backup-configuration", "put");
    let body = json!({"path":"docs/report.txt","target_copies":3,"backup_prefixes":{"B01":"工作备份","C01":"archive"},"expected_scope_id":file,"expected_generation":"0","operation_id":operation});
    assert!(backups.is_valid(&body));
    for key in ["expected_scope_id", "expected_generation"] {
        let mut invalid = body.clone();
        invalid.as_object_mut().unwrap().remove(key);
        assert!(!backups.is_valid(&invalid));
    }
    let mut document = api.clone();
    document["$ref"] = "#/components/schemas/HistoryNodeSummary".into();
    let node = validator(&document).unwrap();
    let fixture = read(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../test/tools/contract-check/fixtures/history-window/nodes.json"),
    )
    .unwrap();
    let mut body = fixture["nodes"][0].clone();
    body["kind"] = "tags".into();
    body["node_id"] = meta.clone().into();
    body["version_id"] = version.clone().into();
    body["tags"] = json!(["审核"]);
    assert!(node.is_valid(&body));
    let mut invalid = body;
    invalid["version_id"] = meta.into();
    assert!(!node.is_valid(&invalid));
    // Reviewed response fixtures are source inputs, not optional local evidence.
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../test/tools/contract-check/fixtures/merge-tags-backup");
    for (route, name) in [
        (
            "/api/file-manager/files/{file}/history",
            "file-merge-tag-history.json",
        ),
        (
            "/api/repositories/{repo}/backup-configuration",
            "scoped-backup-configuration.json",
        ),
        (
            "/api/repositories/{repo}/backup-configuration",
            "backup-prefix-configuration.json",
        ),
        (
            "/api/repositories/{repo}/backup-configuration",
            "backup-mixed-routes.json",
        ),
        (
            "/api/file-manager/disks/{slot}/entries",
            "backup-prefix-files.json",
        ),
        (
            "/api/file-manager/disks/{slot}/entries",
            "backup-prefix-retained.json",
        ),
        (
            "/api/file-manager/disks/{slot}/entries",
            "backup-prefix-administrator.json",
        ),
        ("/api/file-manager/disks", "backup-route-disks.json"),
    ] {
        let wire = read(&fixtures.join(name))
            .unwrap_or_else(|error| panic!("required response fixture {name}: {error}"));
        let mut schema = api.clone();
        schema["allOf"] = json!([
            api["paths"][route]["get"]["responses"]["200"]["content"]["application/json"]["schema"]
        ]);
        validator(&schema)
            .unwrap()
            .validate(&wire)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
    }
}

#[test]
fn monitoring_contracts_share_snapshots_and_preserve_unavailable_values_and_auth() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/contracts/openapi.json");
    let api = read(&path).unwrap();
    let response_schema = |route: &str| {
        let mut schema = api.clone();
        let route = route.replace('~', "~0").replace('/', "~1");
        schema["$ref"] =
            format!("#/paths/{route}/get/responses/200/content/application~1json/schema").into();
        validator(&schema).unwrap()
    };
    let snapshot_schema = response_schema("/api/system/status");
    let snapshot = json!({
        "collected_at":"2026-10-19T00:00:00Z",
        "cpu":{"usage_percent":12.5,"logical_count":4,"power_watts":15.5,"power_unavailable_reason":null},
        "memory":{"total_bytes":"18446744073709551615","used_bytes":"10","available_bytes":"18446744073709551605"},
        "disks":[{"id":"P01","name":"P01","online":true,"read_only":false,"simulation":true,"total_bytes":"100","available_bytes":"50","used_bytes":"50","usage_percent":50,"power_watts":null,"power_unavailable_reason":"simulated_disk"}]
    });
    assert!(snapshot_schema.is_valid(&snapshot));
    let mut unavailable = snapshot.clone();
    unavailable["cpu"]["usage_percent"] = Value::Null;
    unavailable["cpu"]["power_watts"] = Value::Null;
    unavailable["cpu"]["power_unavailable_reason"] = "sampling".into();
    unavailable["disks"][0]["usage_percent"] = Value::Null;
    assert!(snapshot_schema.is_valid(&unavailable));
    let mut zero = snapshot.clone();
    zero["cpu"]["usage_percent"] = 0.into();
    zero["cpu"]["power_watts"] = 0.into();
    assert!(snapshot_schema.is_valid(&zero));
    for (pointer, value) in [
        ("/cpu/power_watts", json!(-1)),
        ("/cpu/usage_percent", json!(101)),
        (
            "/cpu/power_unavailable_reason",
            json!("/sys/private/sensor"),
        ),
        ("/disks/0/usage_percent", json!(-1)),
        ("/disks/0/power_watts", json!(-1)),
    ] {
        let mut invalid = snapshot.clone();
        *invalid.pointer_mut(pointer).unwrap() = value;
        assert!(!snapshot_schema.is_valid(&invalid));
    }
    let mut leaked_path = snapshot.clone();
    leaked_path["disks"][0]["physical_device"] = "/sys/private/device".into();
    assert!(!snapshot_schema.is_valid(&leaked_path));

    let history = &api["paths"]["/api/system/history"]["get"];
    let status = &api["paths"]["/api/system/status"]["get"];
    assert_eq!(history["operationId"], "getSystemHistory");
    for code in ["401", "403"] {
        assert_eq!(history["responses"][code], status["responses"][code]);
    }
    assert_eq!(
        history.get("security").unwrap_or(&api["security"]),
        status.get("security").unwrap_or(&api["security"])
    );
    assert_eq!(
        history["parameters"][0]["schema"]["enum"],
        json!(["1min", "30min", "1h", "24h", "1month"])
    );
    assert_eq!(history["parameters"][0]["schema"]["default"], "1min");
    let range_schema = validator(&history["parameters"][0]["schema"]).unwrap();
    assert!(!range_schema.is_valid(&json!("1year")));
    let history_schema = response_schema("/api/system/history");
    let response = json!({"range":"1h","interval_seconds":10,"from":"2026-10-18T23:00:00Z","to":"2026-10-19T00:00:00Z","samples":[snapshot,unavailable]});
    assert!(history_schema.is_valid(&response));
    let mut empty = response.clone();
    empty["samples"] = json!([]);
    assert!(history_schema.is_valid(&empty));
    let mut too_many = response.clone();
    too_many["samples"] = json!(vec![response["samples"][0].clone(); 362]);
    assert!(!history_schema.is_valid(&too_many));
}

#[test]
fn variable_membership_contract_keeps_fixed_quorum_and_publication_barriers() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/contracts");
    let config = read(&root.join("config-constraints.schema.json")).unwrap();
    let vectors = read(&root.join("vectors.json")).unwrap();
    let base = vectors["positive"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["schema"] == "config")
        .unwrap()["value"]
        .clone();
    for members in [1u64, 2, 3, 4, 9] {
        let mut value = base.clone();
        value["metadata"]["replica_members"] = members.into();
        for key in ["decision_members", "recovery_evidence_members"] {
            value["metadata"][key] = (members / 2 + 1).into();
        }
        schema_valid(&config, &value).unwrap();
        semantics("config", &value).unwrap();
        for key in ["decision_members", "recovery_evidence_members"] {
            let mut wrong = value.clone();
            wrong["metadata"][key] = (members / 2 + 2).into();
            assert!(semantics("config", &wrong).is_err());
        }
        for key in [
            "publish_after_all_members",
            "acknowledge_after_all_members",
            "require_fenced_membership_change",
        ] {
            let mut wrong = value.clone();
            wrong["metadata"][key] = false.into();
            assert!(schema_valid(&config, &wrong).is_err());
        }
        value["metadata"]["allow_automatic_degraded_writes"] = true.into();
        assert!(schema_valid(&config, &value).is_err());
    }
}

#[test]
fn publication_receipts_reject_duplicate_identity_even_with_different_contents() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/contracts");
    let mut core = read(&root.join("core.schema.json")).unwrap();
    core["$ref"] = "#/$defs/Publication".into();
    let vectors = read(&root.join("vectors.json")).unwrap();
    let value = &vectors["negative"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == "same-physical-member")
        .unwrap()["value"];
    let receipts = value["finalize_receipts"].as_array().unwrap();
    assert_ne!(receipts[0], receipts[1]);
    assert_eq!(receipts[0]["disk_id"], receipts[1]["disk_id"]);
    schema_valid(&core, value).unwrap();
    assert!(semantics("Publication", value).is_err());
}

#[test]
fn configuration_projection_failure_names_document_and_schema() {
    let schema = json!({"type":"object","properties":{"members":{"const":1}}});
    let error = validate_configuration_projection(&schema, &json!({"members":3}))
        .unwrap_err()
        .to_string();
    assert!(error.contains("docs/21-configuration.md"));
    assert!(error.contains("docs/contracts/config-constraints.schema.json"));
    assert!(error.contains("1 was expected"));
}

fn configuration_fixture() -> (Value, Value) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/contracts");
    let schema = read(&root.join("config-constraints.schema.json")).unwrap();
    let vectors = read(&root.join("vectors.json")).unwrap();
    let value = array(&vectors["positive"])
        .unwrap()
        .iter()
        .find(|case| case["name"] == "config")
        .unwrap()["value"]
        .clone();
    (schema, value)
}

#[test]
fn resource_projection_keeps_current_fields_and_rejects_obsolete_limits() {
    let (schema, base) = configuration_fixture();
    let draft = storage_contract::Config::development(std::iter::empty::<PathBuf>());
    assert_eq!(
        base["resource_limits"]["max_workspaces"],
        json!(draft.max_workspaces)
    );
    assert_eq!(configuration_projection(&schema, &base).unwrap(), base);
    for key in [
        "workspaces_per_account",
        "uploads_per_account",
        "uploads_per_pool",
        "staging_bytes_per_account",
        "staging_bytes_per_pool",
        "handles_per_session",
        "handles_per_pool",
        "queued_tasks_per_pool",
    ] {
        let mut document = base.clone();
        document["resource_limits"][key] = json!(128);
        let projection = configuration_projection(&schema, &document).unwrap();
        assert_eq!(projection["resource_limits"][key], json!(128));
        assert!(validate_configuration_projection(&schema, &projection).is_err());
    }
    for key in object(&schema["properties"]["resource_limits"]["properties"])
        .unwrap()
        .keys()
    {
        let mut document = base.clone();
        document["resource_limits"]
            .as_object_mut()
            .unwrap()
            .remove(key);
        let error = configuration_projection(&schema, &document)
            .unwrap_err()
            .to_string();
        assert!(error.contains(&format!("resource_limits.{key}")));
    }
    let mut document = base.clone();
    document["server"] = json!({"mode":"single-writer"});
    document["metadata"]["member_slot_ids"] = json!(["P01", "B01", "C01"]);
    assert_eq!(configuration_projection(&schema, &document).unwrap(), base);
}

#[test]
fn runtime_resource_projection_accepts_nondefault_and_independent_protocol_budgets() {
    let (schema, base) = configuration_fixture();
    for smaller in ["http", "smb"] {
        let mut value = base.clone();
        let r = &mut value["resource_limits"];
        r["max_workspaces"] = json!(256);
        r["http_transfer_slots"] = json!(5);
        r[format!("{smaller}_spool_bytes_per_account")] = json!("65536");
        r[format!("{smaller}_spool_bytes_total")] = json!("65536");
        r[format!("{smaller}_spool_recovery_reserve_bytes")] = json!("1");
        validate_configuration_projection(&schema, &value).unwrap();
    }
    for key in ["max_workspaces", "http_transfer_slots"] {
        for invalid in [json!(0), json!(-1), json!("2"), json!(null)] {
            let mut value = base.clone();
            value["resource_limits"][key] = invalid;
            assert!(schema_valid(&schema, &value).is_err(), "{key}");
        }
    }
}

#[test]
fn spool_projection_preserves_each_protocols_byte_and_cross_limit_boundaries() {
    let (schema, base) = configuration_fixture();
    for protocol in ["http", "smb"] {
        for (suffix, invalid, message) in [
            ("bytes_per_account", "65535", "owner/total limit"),
            ("bytes_per_account", "536870912001", "owner/total limit"),
            ("bytes_total", "107374182399", "owner/total limit"),
            ("recovery_reserve_bytes", "0", "recovery reserve"),
        ] {
            let mut value = base.clone();
            value["resource_limits"][format!("{protocol}_spool_{suffix}")] = json!(invalid);
            schema_valid(&schema, &value).unwrap();
            let error = semantics("config", &value).unwrap_err().to_string();
            assert!(error.contains(&format!("{protocol} spool {message}")));
        }
        let mut maximum = base.clone();
        for suffix in ["bytes_per_account", "bytes_total", "recovery_reserve_bytes"] {
            let key = format!("{protocol}_spool_{suffix}");
            maximum["resource_limits"][&key] = json!(u64::MAX.to_string());
            for invalid in [
                json!("18446744073709551616"),
                json!("065536"),
                json!(65536),
                json!(null),
            ] {
                let mut value = base.clone();
                value["resource_limits"][&key] = invalid;
                assert!(schema_valid(&schema, &value).is_err(), "{key}");
            }
        }
        validate_configuration_projection(&schema, &maximum).unwrap();
    }
}

#[test]
fn target_resource_and_alert_constraints_remain_checked_without_runtime_claims() {
    let (schema, base) = configuration_fixture();
    for (section, key, invalid, message) in [
        (
            "resource_limits",
            "default_page_items",
            json!(1001),
            "page limit",
        ),
        (
            "resource_limits",
            "event_retention_bytes",
            json!("0"),
            "event budget",
        ),
        (
            "observability",
            "decision_warning_seconds",
            json!(120),
            "alert order",
        ),
    ] {
        let mut value = base.clone();
        value[section][key] = invalid;
        schema_valid(&schema, &value).unwrap();
        assert!(
            semantics("config", &value)
                .unwrap_err()
                .to_string()
                .contains(message)
        );
    }
}

#[test]
fn single_administrator_authentication_contract_matches_existing_http_endpoints() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let api = read(&root.join("docs/contracts/openapi.json")).unwrap();
    let fixtures = root.join("test/tools/contract-check/fixtures/authentication");
    for (route, method, request, response) in [
        (
            "/api/auth/login",
            "post",
            Some("LoginRequest"),
            "LoginResponse",
        ),
        ("/api/auth/me", "get", None, "CurrentSessionResponse"),
        ("/api/auth/logout", "post", None, "LogoutResponse"),
        (
            "/api/auth/password",
            "post",
            Some("PasswordChangeRequest"),
            "PasswordChangeResponse",
        ),
    ] {
        let operation = &api["paths"][route][method];
        for (name, location) in request
            .map(|name| {
                (
                    name,
                    &operation["requestBody"]["content"]["application/json"]["schema"],
                )
            })
            .into_iter()
            .chain(std::iter::once((
                response,
                &operation["responses"]["200"]["content"]["application/json"]["schema"],
            )))
        {
            assert_eq!(location["$ref"], format!("#/components/schemas/{name}"));
            let mut schema = api.clone();
            schema["allOf"] = json!([location]);
            let validator = validator(&schema).unwrap();
            let body = read(&fixtures.join(format!("{name}.json"))).unwrap();
            assert!(validator.is_valid(&body), "{name}");
            for field in ["admin", "role", "disabled", "repositories", "accounts"] {
                let mut obsolete = body.clone();
                obsolete[field] = json!(null);
                assert!(!validator.is_valid(&obsolete), "{name} accepts {field}");
            }
            for required in body.as_object().unwrap().keys() {
                let mut missing = body.clone();
                missing.as_object_mut().unwrap().remove(required);
                assert!(!validator.is_valid(&missing), "{name} omits {required}");
            }
        }
        if route == "/api/auth/login" {
            assert_eq!(operation["security"], json!([]));
        } else {
            assert_eq!(
                operation.get("security").unwrap_or(&api["security"]),
                &json!([{"bearerAuth": []}, {"sessionCookie": []}])
            );
        }
    }
    let mut login_schema = api.clone();
    login_schema["$ref"] = "#/components/schemas/LoginResponse".into();
    let login_schema = validator(&login_schema).unwrap();
    let login = read(&fixtures.join("LoginResponse.json")).unwrap();
    for invalid_expiry in [
        json!(1792861200),
        json!("01"),
        json!("18446744073709551616"),
    ] {
        let mut invalid = login.clone();
        invalid["expires_at"] = invalid_expiry;
        assert!(!login_schema.is_valid(&invalid));
    }
    for route in api["paths"].as_object().unwrap().keys() {
        assert!(!route.starts_with("/api/admin/accounts"));
        assert!(!route.ends_with("/grants"));
    }
    let backup = &api["paths"]["/api/repositories/{repo}/backup-configuration"]["get"]["responses"]
        ["200"]["content"]["application/json"]["schema"];
    assert!(backup["properties"].get("can_manage").is_none());
}

#[test]
fn single_administrator_repository_schema_rejects_removed_access_fields() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/contracts");
    let mut core = read(&root.join("core.schema.json")).unwrap();
    core["$ref"] = "#/$defs/HistoryRepository".into();
    let schema = validator(&core).unwrap();
    let vectors = read(&root.join("vectors.json")).unwrap();
    let repository = array(&vectors["positive"])
        .unwrap()
        .iter()
        .find_map(|case| case["value"].get("repository"))
        .unwrap();
    assert!(schema.is_valid(repository));
    for (field, value) in [
        ("grants", json!({"read": [], "write": [], "manage": []})),
        ("account_scope_repository_id", json!(null)),
    ] {
        let mut obsolete = repository.clone();
        obsolete[field] = value;
        assert!(!schema.is_valid(&obsolete));
    }
}

#[test]
fn single_administrator_capacity_always_includes_global_and_principal_budgets() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/contracts");
    let mut core = read(&root.join("core.schema.json")).unwrap();
    core["$ref"] = "#/$defs/WorkspaceCapacity".into();
    let schema = validator(&core).unwrap();
    let vectors = read(&root.join("vectors.json")).unwrap();
    let value = array(&vectors["positive"])
        .unwrap()
        .iter()
        .find(|case| case["schema"] == "WorkspaceCapacity")
        .unwrap()["value"]
        .clone();
    assert!(schema.is_valid(&value));
    for field in ["global", "principal"] {
        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(!schema.is_valid(&missing), "missing {field}");
        let mut null = value.clone();
        null[field] = Value::Null;
        assert!(!schema.is_valid(&null), "null {field}");
    }
}

#[test]
fn schema_errors_remain_owned_typed_errors() {
    let error = validator(&json!({"type": 7})).err().unwrap();
    assert!(
        error
            .downcast_ref::<schema_validation::ValidationError<'static>>()
            .is_some()
    );
    let error = schema_valid(&json!({"type": "integer"}), &json!("text")).unwrap_err();
    assert!(
        error
            .downcast_ref::<schema_validation::ValidationError<'static>>()
            .is_some()
    );
}

#[test]
fn malformed_ref_is_rejected_before_bundling() {
    assert!(schema_validation::meta::validate(&json!({"$ref": 7})).is_err());
    assert_eq!(
        bundle(&json!({"$ref": "#/$defs/Item"})),
        json!({"$ref": "#/components/schemas/Item"})
    );
}

#[test]
fn configuration_context_preserves_schema_error_source() {
    let error =
        validate_configuration_projection(&json!({"type": "integer"}), &json!("text")).unwrap_err();
    assert!(
        error
            .source()
            .unwrap()
            .downcast_ref::<schema_validation::ValidationError<'static>>()
            .is_some()
    );
}
