//! The logical view preserves the old request bytes, but grants no live rights.
use crate::business::namespace;
use crate::business::workspace::input::WorkspaceInput;
use crate::business::workspace::{native_commit_digest, workspace_commit_input};
use crate::{
    CommitRequest, Config, Engine, ErrorCode, NamespaceOperation, NamespaceOperationKind,
    OperationId, Repository, Workspace, canonical_json, object_hash,
};
use json_codec::json;
use storage_contract::RepositoryName;

fn request(workspace: &Workspace) -> CommitRequest {
    CommitRequest {
        workspace_id: workspace.workspace_id.clone(),
        operation_id: OperationId::generate().unwrap(),
        principal: workspace.principal.clone(),
        expected_head: workspace.base_head.clone(),
        fencing_token: workspace.fencing_token,
        message: "logical input fixture".into(),
        request_digest: None,
    }
}

fn fixture() -> (tempfile::TempDir, Engine, Repository) {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temp.path().join("system"),
        Config::development(crate::test_disks([temp.path().join("primary")])).unwrap(),
    ).unwrap();
    let repository = engine.create_repository(
        &RepositoryName::parse("logical workspace").unwrap(),
        "P01",
        1,
        "admin",
        OperationId::generate().unwrap(),
    ).unwrap();
    let workspace = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
    engine.stage_bytes(&workspace.workspace_id, "Folder/old.txt", b"old bytes").unwrap();
    engine.commit_workspace(request(&workspace)).unwrap();
    (temp, engine, repository)
}

#[test]
fn logical_workspace_input_preserves_native_request_bytes_and_ordered_edits() {
    let (_temp, mut engine, repository) = fixture();
    let old_entry = engine.namespace_tree(&repository.main_branch_id).unwrap()
        .get("Folder/old.txt").unwrap().clone();
    let workspace = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
    for (path, bytes) in [
        ("Zulu.txt", b"last path".as_slice()),
        ("Alpha.txt", b"first path".as_slice()),
        ("Folder/old.txt", b"old bytes".as_slice()),
    ] {
        engine.stage_bytes(&workspace.workspace_id, path, bytes).unwrap();
    }
    for operation in [
        NamespaceOperation::parse(NamespaceOperationKind::Mkdir, "Empty", None).unwrap(),
        NamespaceOperation::parse(
            NamespaceOperationKind::Rename, "Folder", Some("Moved"),
        ).unwrap(),
    ] {
        engine.namespace_operation(&workspace.workspace_id, operation).unwrap();
    }
    let workspace = engine.workspace(&workspace.workspace_id).unwrap();
    let source = canonical_json(&workspace).unwrap();
    let state = canonical_json(engine.state()).unwrap();
    let mut command = request(&workspace);
    for claim in [None, Some("not part of the native digest".to_owned())] {
        command.request_digest = claim;
        // The pre-refactor definition is a byte oracle, independent of getters.
        let sealed = object_hash("workspace", &json!({
            "workspace_id": workspace.workspace_id,
            "base_head": workspace.base_head,
            "files": workspace.files.values().map(|file| json!({
                "item_id": file.item_id,
                "path": file.path,
                "hash": file.content_hash,
                "size": file.size.to_string(),
                "generation": file.dirty_generation.to_string(),
            })).collect::<Vec<_>>(),
            "namespace_operations": workspace.namespace_operations,
        })).unwrap();
        let mut original_command = command.clone();
        original_command.request_digest = None;
        let original_input = json!({
            "request": original_command,
            "sealed_manifest_hash": sealed,
        });
        assert_eq!(
            canonical_json(&workspace_commit_input(&workspace, &command).unwrap()).unwrap(),
            canonical_json(&original_input).unwrap(),
        );
        let digest = object_hash("request", &json!({
            "kind": "workspace.commit",
            "principal": command.principal,
            "body": original_input,
        })).unwrap();
        assert_eq!(native_commit_digest(&workspace, &command).unwrap(), digest);
    }
    let logical = workspace.files().map(|file| {
        (
            file.item(), file.path(), file.size(), file.hash(), file.generation(),
        )
    }).collect::<Vec<_>>();
    let original_fields = workspace.files.values().map(|file| {
        (
            file.item_id.as_str(), &file.path, file.size, &file.content_hash,
            file.dirty_generation,
        )
    }).collect::<Vec<_>>();
    assert_eq!(logical, original_fields);
    for (key, context) in &workspace.selected_file_contexts {
        assert!(std::ptr::eq(
            workspace.selected_file_context(key).unwrap(), context,
        ));
    }
    for (file, branch) in &workspace.selected_file_branches {
        assert_eq!(workspace.selected_file_branch(file), Some(branch.as_str()));
    }
    assert_eq!(workspace.directory_selection_generation("not-present"), 0);
    let rows = engine.list_tree(&repository.main_branch_id).unwrap();
    let locations = namespace::workspace_upload_locations(&rows, &workspace).unwrap();
    let old_item = workspace.files.values()
        .find(|file| file.path.as_str() == "Folder/old.txt").unwrap();
    assert_eq!(locations[&old_item.item_id].as_str(), "Moved/old.txt");
    assert_eq!(canonical_json(&workspace).unwrap(), source);
    assert_eq!(canonical_json(engine.state()).unwrap(), state);

    command.request_digest = None;
    engine.commit_workspace(command).unwrap();
    let moved = engine.namespace_tree(&repository.main_branch_id).unwrap()
        .get("Moved/old.txt").unwrap().clone();
    // The unchanged upload did not become a new content node during the rename.
    assert_eq!(moved.version_id, old_entry.version_id);
    assert!(engine.namespace_tree(&repository.main_branch_id).unwrap().get("Empty").is_some());
}

#[test]
fn logical_workspace_input_never_substitutes_live_owner_fence_or_source() {
    let (temp, mut engine, repository) = fixture();
    let workspace = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
    let staged = engine.stage_bytes(
        &workspace.workspace_id, "Folder/old.txt", b"changed bytes",
    ).unwrap();
    let workspace = engine.workspace(&workspace.workspace_id).unwrap();
    let command = request(&workspace);
    let digest = native_commit_digest(&workspace, &command).unwrap();
    let before = canonical_json(engine.state()).unwrap();
    let parked = temp.path().join("held-upload");
    std::fs::rename(&staged.source_path, &parked).unwrap();

    let mut foreign = command.clone();
    foreign.principal = "another-principal".into();
    assert_eq!(
        engine.commit_workspace(foreign).unwrap_err().code(),
        ErrorCode::PermissionDenied,
    );
    let mut stale = command.clone();
    stale.fencing_token += 1;
    assert_eq!(
        engine.commit_workspace(stale).unwrap_err().code(),
        ErrorCode::FencingTokenInvalid,
    );
    // Computing the logical digest never opens or replaces the missing upload.
    assert_eq!(native_commit_digest(&workspace, &command).unwrap(), digest);
    assert!(engine.commit_workspace(command.clone()).is_err());
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    assert!(!engine.state().operations.contains_key(&command.operation_id));
    assert!(!staged.source_path.exists());
    assert_eq!(std::fs::read(&parked).unwrap(), b"changed bytes");
    std::fs::rename(&parked, &staged.source_path).unwrap();
    let result = engine.commit_workspace(command.clone()).unwrap();
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(engine.commit_workspace(command).unwrap(), result);

    // A genuine same-content save still returns before BODY/placement I/O.
    let workspace = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
    let same = engine.stage_bytes(
        &workspace.workspace_id, "Folder/old.txt", b"changed bytes",
    ).unwrap();
    let before_versions = canonical_json(&engine.state().versions).unwrap();
    let before_placements = canonical_json(&engine.state().placements).unwrap();
    let parked = temp.path().join("same-upload");
    std::fs::rename(&same.source_path, &parked).unwrap();
    let result = engine.commit_workspace(request(&workspace)).unwrap();
    assert_eq!(result.result["no_change"], true);
    assert_eq!(canonical_json(&engine.state().versions).unwrap(), before_versions);
    assert_eq!(canonical_json(&engine.state().placements).unwrap(), before_placements);
    assert!(!same.source_path.exists());
    assert_eq!(std::fs::read(parked).unwrap(), b"changed bytes");
}
