use super::introduced;
use crate::{CommitRequest, Config, Engine, ErrorCode, NamespaceOperation, OperationId};
use storage_contract::NamespaceEntryKind;

fn request(workspace: &crate::Workspace) -> CommitRequest {
    CommitRequest {
        workspace_id: workspace.workspace_id.clone(),
        operation_id: OperationId::generate().unwrap(),
        principal: workspace.principal.clone(), expected_head: workspace.base_head.clone(),
        fencing_token: workspace.fencing_token, message: "native directory source".into(),
        request_digest: None,
    }
}

#[test]
fn native_directory_source_replays_original_request_and_rejects_forged_rows() {
    let temp = tempfile::tempdir().unwrap();
    let system = temp.path().join("system");
    let mut engine = Engine::initialize(
        &system,
        Config::development(crate::test_disks([temp.path().join("primary")])).unwrap(),
    ).unwrap();
    let root = engine.create_disk_root("P01", "admin", OperationId::generate().unwrap()).unwrap();
    let first = engine.create_workspace(&root.main_branch_id, "admin").unwrap();
    engine.stage_bytes(&first.workspace_id, "stable/old.bin", b"old").unwrap();
    engine.commit_workspace(request(&first)).unwrap();
    let workspace = engine.create_workspace(&root.main_branch_id, "admin").unwrap();
    for (path, body) in [("旅行/notes.txt", b"new".as_slice()), ("zero", b"".as_slice())] {
        engine.stage_bytes(&workspace.workspace_id, path, body).unwrap();
    }
    engine.namespace_operation(&workspace.workspace_id, NamespaceOperation::Mkdir {
        path: name_policy::PortablePath::parse("旅行/empty").unwrap(),
    }).unwrap();
    let workspace = engine.workspace(&workspace.workspace_id).unwrap();
    let request = request(&workspace);
    let digest = crate::business::workspace::native_commit_digest(&workspace, &request).unwrap();
    let before = engine.state().commits[&workspace.base_head].clone();
    let result = engine.commit_workspace(request.clone()).unwrap();
    assert_eq!(result.request_digest, digest);
    let after = engine.state().commits[result.commit_id.as_ref().unwrap()].clone();
    let (_, entries) = introduced(engine.state(), &result, &before, &after, 100).unwrap();
    assert_eq!(
        entries.iter().map(|entry| entry.path()).collect::<Vec<_>>(), ["旅行", "旅行/empty"],
    );
    let source = result.result["workspace_directory_source"].clone();
    let original = crate::business::workspace::workspace_commit_input(
        &workspace, &request,
    ).unwrap();
    assert_eq!(source["request"], original["request"]);
    assert_eq!(
        storage_contract::object_hash("workspace", &source["manifest"]).unwrap(),
        original["sealed_manifest_hash"].as_str().unwrap(),
    );
    drop(engine);
    let mut engine = Engine::open(&system).unwrap();
    assert_eq!(engine.commit_workspace(request).unwrap(), result);
    introduced(engine.state(), &result, &before, &after, 100).unwrap();

    // The native digest fixes the exact original request and every source row;
    // a same-kind receipt is not enough to grant directory creation.
    let mut changed = result.clone();
    changed.result["workspace_directory_source"]["manifest"]["namespace_operations"] =
        json_codec::json!([]);
    assert_eq!(introduced(engine.state(), &changed, &before, &after, 100).unwrap_err().code(),
        ErrorCode::PublicationInvalid);
    let mut changed = result.clone();
    changed.result["workspace_directory_source"]["request"]["principal"] =
        json_codec::json!("another-actor");
    assert_eq!(introduced(engine.state(), &changed, &before, &after, 100).unwrap_err().code(),
        ErrorCode::PublicationInvalid);
    let mut changed = result.clone();
    changed.result.as_object_mut().unwrap().remove("workspace_directory_source");
    assert_eq!(introduced(engine.state(), &changed, &before, &after, 100).unwrap_err().code(),
        ErrorCode::PublicationInvalid);
    let mut changed = result.clone();
    let old_directory = before.entries.iter()
        .find(|entry| entry.kind == NamespaceEntryKind::Directory).unwrap();
    changed.result["workspace_directory_source"]["directories"][0] =
        json_codec::json!(old_directory.entry_id);
    assert_eq!(introduced(engine.state(), &changed, &before, &after, 100).unwrap_err().code(),
        ErrorCode::PublicationInvalid);

    // Even a fresh trace ID cannot append a directory absent from the original
    // request. The full namespace replay, not a supplied directory list, decides.
    let mut changed = result.clone();
    let mut forged = after.clone();
    let id = storage_contract::EntryId::generate().unwrap();
    changed.result["workspace_directory_source"]["directories"]
        .as_array_mut().unwrap().push(json_codec::json!(id));
    forged.entries.push(crate::Entry {
        entry_id: id.to_string(), path: name_policy::PortablePath::parse("unrequested").unwrap(),
        kind: NamespaceEntryKind::Directory, file_id: String::new(), version_id: String::new(),
        size: 0,
    });
    assert_eq!(introduced(engine.state(), &changed, &before, &forged, 100).unwrap_err().code(),
        ErrorCode::PublicationInvalid);
}

#[test]
fn sealed_wire_parent_source_replays_and_rejects_changed_original_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let system = temp.path().join("system");
    let mut engine = Engine::initialize(
        &system,
        Config::development(crate::test_disks([temp.path().join("primary")])).unwrap(),
    ).unwrap();
    let root = engine.create_disk_root("P01", "admin", OperationId::generate().unwrap()).unwrap();
    let workspace = engine.create_workspace(&root.main_branch_id, "admin").unwrap();
    engine.stage_bytes(&workspace.workspace_id, "parent/child/notes.txt", b"sealed body").unwrap();
    let sealed = engine.seal_workspace(&workspace.workspace_id, "admin").unwrap();
    let mut body = json_codec::to_value(&sealed).unwrap();
    body["request_digest"] = crate::wire::commit_request_digest(&workspace.workspace_id, &body)
        .unwrap().into();
    let make_envelope = || crate::PublicCommitEnvelope::parse(&workspace.workspace_id, body.clone()).unwrap();
    let request = CommitRequest {
        workspace_id: workspace.workspace_id.clone(),
        operation_id: sealed.operation_id,
        principal: "admin".into(),
        expected_head: sealed.expected_head.clone(),
        fencing_token: sealed.fencing_token,
        message: sealed.message.clone(),
        request_digest: None,
    };
    let before = engine.state().commits[&workspace.base_head].clone();
    assert!(!before.entries.iter().any(|entry| entry.path() == "parent"));
    let result = engine.commit_workspace_wire(request.clone(), make_envelope()).unwrap();
    let after = engine.state().commits[result.commit_id.as_ref().unwrap()].clone();
    let field = "workspace_directory_wire_source";
    assert!(result.result.get("workspace_directory_source").is_none());
    assert_eq!(result.result[field]["body"], body);
    let (_, entries) = introduced(engine.state(), &result, &before, &after, 100).unwrap();
    assert_eq!(entries.iter().map(|entry| entry.path()).collect::<Vec<_>>(), ["parent", "parent/child"]);
    drop(engine);

    let mut engine = Engine::open(&system).unwrap();
    assert_eq!(engine.commit_workspace_wire(request, make_envelope()).unwrap(), result);
    introduced(engine.state(), &result, &before, &after, 100).unwrap();
    let rejected = |changed| {
        assert_eq!(introduced(engine.state(), &changed, &before, &after, 100).unwrap_err().code(),
            ErrorCode::PublicationInvalid);
    };
    let mut changed = result.clone();
    changed.result[field]["body"]["message"] = "substituted".into();
    rejected(changed);
    let mut changed = result.clone();
    changed.result[field]["source"]["manifest"]["files"][0]["generation"] = "29".into();
    rejected(changed);
    let mut changed = result.clone();
    changed.result[field]["source"]["request"]["principal"] = "another-actor".into();
    rejected(changed);
    let mut changed = result.clone();
    changed.result[field]["source"]["directories"].as_array_mut().unwrap().swap(0, 1);
    rejected(changed);
    let mut changed = result.clone();
    changed.result.as_object_mut().unwrap().remove(field);
    rejected(changed);
}
