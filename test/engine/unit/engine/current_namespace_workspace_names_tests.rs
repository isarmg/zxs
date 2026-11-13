//! Synthetic admitted Current scopes isolate receipt naming and fencing. These
//! tests do not claim to execute durable D3 activation or a physical rename.
use super::*;

#[test]
fn workspace_receipt_keeps_admission_names_but_fences_after_current_name_change() {
    let mut fixture = Fixture::new(false);
    fixture.project(false);
    let workspace = fixture.engine.create_workspace(&fixture.repo.main_branch_id, "admin")
        .unwrap();
    let baseline = fixture.engine.workspace_namespace_baselines[&workspace.workspace_id].clone();
    let view = fixture.engine.current_namespaces.as_ref().unwrap().clone();
    let source = &view.branches[&workspace.branch_id].entries;
    assert_eq!(super::super::workspace::named_entries_hash(source, &baseline.names).unwrap(),
        baseline.entries_hash);
    let mut missing = baseline.names.clone();
    missing.remove(&fixture.retained.file_id.parse().unwrap());
    assert!(super::super::workspace::named_entries_hash(source, &missing).is_err());
    let mut next = fixture.engine.state.as_ref().clone();
    next.file_locations.insert(fixture.retained.file_id.clone(), "dir/latest".into());
    next.file_names.insert(fixture.retained.file_id.clone(), "latest".into());
    assert_eq!(view.validate_workspace(&next, &workspace, Some(&baseline)).unwrap_err().code(),
        ErrorCode::WorkspaceChanged);
    // Cold receipt reconstruction must use its saved names, not today's name.
    fixture.engine.state = std::sync::Arc::new(next);
    fixture.engine.restore_namespace_workspace_baselines().unwrap();
    let restored = &fixture.engine.workspace_namespace_baselines[&workspace.workspace_id];
    assert_eq!(restored.names, baseline.names);
    assert_eq!(restored.entries_hash, baseline.entries_hash);
    assert_eq!(view.validate_workspace(&fixture.engine.state, &workspace, Some(restored))
        .unwrap_err().code(), ErrorCode::WorkspaceChanged);
}

#[test]
fn new_workspace_receipt_cannot_self_authorize_a_rehashed_name_snapshot() {
    let mut fixture = Fixture::new(false);
    fixture.project(false);
    let previous = fixture.engine.state.as_ref().clone();
    let view = fixture.engine.current_namespaces.as_ref().unwrap().clone();
    let workspace = fixture.engine.create_workspace(&fixture.repo.main_branch_id, "admin")
        .unwrap();
    let mut next = fixture.engine.state.as_ref().clone();
    let id = next.operations.iter().find(|(id, result)| {
        !previous.operations.contains_key(*id) && result.kind == "workspace.namespace.admit"
    }).unwrap().0.clone();
    let result = next.operations.get_mut(&id).unwrap();
    let record = result.result.get_mut("namespace_admission").unwrap();
    let names = record.get_mut("names").unwrap().as_object_mut().unwrap();
    names.insert(fixture.retained.file_id.clone(), json_codec::json!("forged"));
    // Matching the self-authored digest cannot replace the previous Current proof.
    result.request_digest = Engine::native_request_digest(
        "workspace.namespace.admit", &workspace.principal, record,
    ).unwrap();
    assert!(super::super::workspace::verify_admissions(&view, &previous, &next).is_err());
}
