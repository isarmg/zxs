use super::*;

#[test]
fn metadata_request_bodies_bind_absent_and_supplied_location() {
    let location = FileLocation {
        repository_id: "repo".into(),
        path: "report.txt".into(),
    };
    let mut tags = FileNodeTagsUpdateRequest {
        file_id: "file",
        targets: &[],
        add: &[],
        remove: &[],
        location: None,
        principal: "owner",
        operation_id: "op_00000000000000000000000000000000".parse().unwrap(),
    };
    let mut merge = FileBranchMergeRequest {
        file_id: "file",
        inputs: &[],
        chosen_node_id: "node",
        surviving_branch_id: "branch",
        location: None,
        principal: "owner",
        operation_id: "op_00000000000000000000000000000000".parse().unwrap(),
    };
    for unscoped in [tags.body(), merge.body()] {
        assert_eq!(unscoped.get("location"), Some(&Value::Null));
        let mut incomplete = unscoped.clone();
        incomplete.as_object_mut().unwrap().remove("location");
        assert_ne!(
            crate::object_hash("request", &unscoped).unwrap(),
            crate::object_hash("request", &incomplete).unwrap()
        );
    }
    tags.location = Some(&location);
    merge.location = Some(&location);
    for scoped in [tags.body(), merge.body()] {
        assert_eq!(
            scoped["location"],
            json!({"repository_id":"repo","path":"report.txt"})
        );
    }
}

#[test]
fn location_lookup_preserves_case_identity_kind_and_repository_boundaries() {
    let temporary = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temporary.path().join("system"),
        crate::Config::development([temporary.path().join("primary")]),
    )
    .unwrap();
    let repo = engine
        .create_disk_root("P01", "admin", OperationId::generate())
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    for path in ["Straße/Café.txt", "Other.txt"] {
        engine
            .stage_bytes(&workspace.workspace_id, path, b"body")
            .unwrap();
    }
    engine
        .commit_workspace(crate::CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "location fixture".into(),
            request_digest: None,
        })
        .unwrap();
    let file = engine
        .namespace_tree(&repo.main_branch_id)
        .unwrap()
        .get("Straße/Café.txt")
        .unwrap()
        .file_id
        .clone();
    let mut location = FileLocation {
        repository_id: repo.repository_id.clone(),
        path: "STRASSE/CAFÉ.TXT".into(),
    };
    validate_location(&engine, &file, Some(&location)).unwrap();
    for path in [
        "Straße",
        "Other.txt",
        "Missing.txt",
        "Straße/Café.txt/child",
    ] {
        location.path = path.into();
        assert_eq!(
            validate_location(&engine, &file, Some(&location))
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::FileNotFound,
            "{path}"
        );
    }
    location.path = "Straße/Café.txt".into();
    location.repository_id = "other-repository".into();
    assert_eq!(
        validate_location(&engine, &file, Some(&location))
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::FileNotFound
    );
    location.repository_id = repo.repository_id;
    // This request boundary still requires a creation-valid NFC display path;
    // choosing NamespaceTree does not relax it to LookupPath admission.
    location.path = "Straße/Cafe\u{301}.txt".into();
    assert!(validate_location(&engine, &file, Some(&location)).is_err());
    validate_location(&engine, "missing-file", None).unwrap();
}
