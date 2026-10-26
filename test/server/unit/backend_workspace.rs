use super::*;

#[test]
fn workspace_wire_projection_preserves_current_fields_without_local_paths() {
    let workspace = storage_contract::Workspace {
        workspace_id: "workspace".into(),
        purpose: WorkspacePurpose::Normal,
        branch_id: "branch".into(),
        principal: "actor".into(),
        base_head: "head".into(),
        fencing_token: u64::MAX,
        files: BTreeMap::from([(
            "entry".into(),
            storage_contract::StagedFile {
                item_id: "entry".into(),
                path: name_policy::PortablePath::parse("file.txt").unwrap(),
                source_path: "/private/secret-staging-path".into(),
                size: u64::MAX,
                content_hash: storage_contract::ContentHash::from_digest(blake3::hash(b"abc")),
                dirty_generation: 42,
                base_version_id: None,
            },
        )]),
        selected_file_branches: BTreeMap::from([("file".into(), "selected".into())]),
        selected_file_contexts: BTreeMap::new(),
        selected_directory_branches: BTreeMap::new(),
        directory_selection_generations: BTreeMap::from([("directory".into(), 7)]),
        file_selection_generations: BTreeMap::from([("file".into(), 9)]),
        namespace_operations: Vec::new(),
        created_at: 123,
        state: WorkspaceState::Open,
    };
    let mut expected = json_codec::to_value(&workspace).unwrap();
    expected["files"]["entry"]
        .as_object_mut()
        .unwrap()
        .remove("source_path");
    let projection = json_codec::to_value(WorkspaceResponse::from(workspace)).unwrap();
    assert_eq!(projection, expected);
    assert!(
        !json_codec::to_string(&projection)
            .unwrap()
            .contains("secret-staging-path")
    );
    assert_eq!(projection["fencing_token"], u64::MAX.to_string());
    assert_eq!(projection["files"]["entry"]["size"], u64::MAX.to_string());
}

#[test]
fn standalone_staged_file_response_omits_local_path_and_keeps_current_wire_shape() {
    let file = storage_contract::StagedFile {
        item_id: "entry".into(),
        path: name_policy::PortablePath::parse("File.txt").unwrap(),
        source_path: "/private/never-on-wire".into(),
        size: u64::MAX,
        content_hash: storage_contract::ContentHash::from_digest(blake3::hash(b"")),
        dirty_generation: u64::MAX,
        base_version_id: Some("version".into()),
    };
    let mut expected = json_codec::to_value(&file).unwrap();
    expected.as_object_mut().unwrap().remove("source_path");
    let response = json_codec::to_value(StagedFileResponse::from(file)).unwrap();
    assert_eq!(response, expected);
    assert_eq!(response["dirty_generation"], u64::MAX.to_string());
    assert_eq!(response["size"], u64::MAX.to_string());
    assert!(
        !json_codec::to_string(&response)
            .unwrap()
            .contains("never-on-wire")
    );
}

#[test]
fn write_session_wire_preserves_full_width_lease_and_identity() {
    let response = WriteSessionResponse::from(WorkspaceWriteSession {
        workspace_id: "workspace".into(),
        branch_id: "branch".into(),
        principal: "actor".into(),
        fencing_token: u64::MAX,
        expires_at: u64::MAX,
        state: WorkspaceState::Open,
    });
    assert_eq!(
        json_codec::to_value(response).unwrap(),
        json_codec::json!({
            "write_session_id":"workspace","workspace_id":"workspace","branch_id":"branch",
            "principal":"actor","fencing_token":"18446744073709551615","expires_at":"18446744073709551615",
            "scope":"workspace","state":"open"
        })
    );
}
