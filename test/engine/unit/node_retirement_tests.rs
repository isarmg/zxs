//! Reverse closure rejects missing evidence and state resurrection.
use crate::*;

fn fixture() -> (tempfile::TempDir, Engine, State, OperationId, String) {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(&temp.path().join("system"), Config::development([temp.path().join("disk")]).unwrap()).unwrap();
    let repository = engine.create_disk_root("P01", "admin", OperationId::generate().unwrap()).unwrap();
    let save = |engine: &mut Engine, bytes: &[u8]| {
        let workspace = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
        engine.stage_bytes(&workspace.workspace_id, "a", bytes).unwrap();
        engine.commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id, operation_id: OperationId::generate().unwrap(), principal: "admin".into(),
            expected_head: workspace.base_head, fencing_token: workspace.fencing_token, message: "retirement closure".into(), request_digest: None,
        }).unwrap();
    };
    save(&mut engine, b"old");
    let entry = engine.namespace_tree(&repository.main_branch_id).unwrap().get("a").unwrap().clone();
    save(&mut engine, b"new");
    let before = engine.state().clone();
    let read = engine.capture_shared_history(storage_contract::shared_history::SharedHistoryRequest::parse(
        &entry.file_id, engine.metadata_view().history_root(&repository.history_tree_id).unwrap()).unwrap(),
        storage_contract::history_window::HistoryWindowCancellation::new()).unwrap();
    let selected = PermanentNodeRemovalSelection::new(vec![storage_contract::history_window::HistoryVersionId::parse(&entry.version_id).unwrap()]).unwrap();
    let operation = OperationId::generate().unwrap();
    engine.retire_file_nodes(&read, &selected, "admin", operation).unwrap();
    (temp, engine, before, operation, entry.version_id)
}

#[test]
fn retirement_snapshot_requires_receipt_actor_exact_event_and_current_retired_state() {
    let (_temp, engine, before, operation, version) = fixture();
    let state = engine.state();
    crate::node_retirement_engine::admit_current(state).unwrap();
    crate::node_retirement_engine::verify(&before, state).unwrap();
    let mut missing = state.clone();
    missing.operations.remove(&operation);
    assert!(crate::node_retirement_engine::admit_current(&missing).is_err());
    let mut missing = state.clone();
    missing.operation_principals.remove(&operation);
    assert!(crate::node_retirement_engine::admit_current(&missing).is_err());
    let mut revived = state.clone();
    revived.content_states.insert(version.clone(), ContentState::Available);
    assert!(crate::node_retirement_engine::admit_current(&revived).is_err());
    let mut tampered = state.clone();
    tampered.version_notes.insert(version, "changed archived evidence".into());
    assert!(crate::node_retirement_engine::admit_current(&tampered).is_err());
    assert!(crate::node_retirement_engine::verify(state, &before).is_err(), "an old full state cannot replace a retained retirement");
}

#[test]
fn retirement_never_accepts_bare_state_token_as_authority() {
    let (_temp, _engine, mut before, _operation, version) = fixture();
    before.content_states.insert(version, ContentState::Retired);
    assert!(crate::node_retirement_engine::admit_current(&before).is_err());
}

#[test]
fn cold_retirement_closure_rejects_new_inbound_sources_without_using_version_sequence() {
    let (_temp, mut engine, _before, _operation, old) = fixture();
    let repository = engine.state().repositories.values().next().unwrap().clone();
    let workspace = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
    engine.stage_bytes(&workspace.workspace_id, "a", b"later live content").unwrap();
    engine.commit_workspace(CommitRequest {
        workspace_id: workspace.workspace_id, operation_id: OperationId::generate().unwrap(),
        principal: "admin".into(), expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token, message: "after retirement".into(), request_digest: None,
    }).unwrap();
    let entry = engine.namespace_tree(&repository.main_branch_id).unwrap().get("a").unwrap().clone();
    let mut changed = engine.state().clone();
    let mut version = changed.versions[&entry.version_id].clone();
    assert!(!version.source_version_ids.contains(&old));
    version.source_version_ids.push(old);
    // Imported historical records may carry an old sequence. It is not a proof
    // that a newly attached inbound reference predates the retirement.
    version.version_sequence = 1;
    changed.versions.insert(entry.version_id, version);
    assert!(crate::node_retirement_engine::admit_current(&changed).is_err());
}

#[test]
fn cold_retirement_requires_the_frozen_partition_and_inbound_closure_fields() {
    let (_temp, engine, _before, operation, _version) = fixture();
    let state = engine.state();
    let mut changed = state.clone();
    let mut receipt = changed.operations[&operation].clone();
    receipt.result["retirement"]["separation"]["file_components"] = json_codec::json!([]);
    changed.operations.insert(operation, receipt);
    assert!(crate::node_retirement_engine::admit_current(&changed).is_err());
    let mut missing = state.clone();
    let mut receipt = missing.operations[&operation].clone();
    receipt.result["retirement"].as_object_mut().unwrap().remove("inbound_references");
    missing.operations.insert(operation, receipt);
    assert!(crate::node_retirement_engine::admit_current(&missing).is_err(), "missing current-format proof is not silently defaulted");
}

#[test]
fn cleanup_obligation_remains_the_original_binding_after_placement_change() {
    let (_temp, engine, _before, operation, version) = fixture();
    let before = crate::node_retirement_engine::original_cleanup_bindings(engine.state()).unwrap();
    let original = before[&version].clone();
    let mut changed = engine.state().clone();
    let mut placement = changed.placements[&version].clone();
    for generation in placement.binding_generations.values_mut() { *generation += 1; }
    changed.placements.insert(version.clone(), placement);
    crate::node_retirement_engine::admit_current(&changed).unwrap();
    assert_eq!(crate::node_retirement_engine::original_cleanup_bindings(&changed).unwrap()[&version], original);
    let mut missing = changed;
    let mut receipt = missing.operations[&operation].clone();
    receipt.result["retirement"].as_object_mut().unwrap().remove("cleanup_routes");
    missing.operations.insert(operation, receipt);
    assert!(crate::node_retirement_engine::admit_current(&missing).is_err());
}

fn terminal_fixture() -> (tempfile::TempDir, Engine, State, OperationId, String) {
    let (temp, mut engine, _before, _operation, old) = fixture();
    let repository = engine.state().repositories.values().next().unwrap().clone();
    let current = engine.namespace_tree(&repository.main_branch_id).unwrap().get("a").unwrap().clone();
    engine.create_file_branch(&current.file_id, &current.version_id, &storage_contract::BranchName::parse("retained").unwrap(),
        "admin", OperationId::generate().unwrap()).unwrap();
    engine.file_namespace(&repository.repository_id,
        NamespaceOperation::parse(storage_contract::NamespaceOperationKind::Delete, "a", None).unwrap(),
        "admin", OperationId::generate().unwrap()).unwrap();
    let before = engine.state().clone();
    let read = engine.capture_shared_history(storage_contract::shared_history::SharedHistoryRequest::parse(
        &current.file_id, engine.metadata_view().history_root(&repository.history_tree_id).unwrap()).unwrap(),
        storage_contract::history_window::HistoryWindowCancellation::new()).unwrap();
    let selected = PermanentNodeRemovalSelection::new(vec![storage_contract::history_window::HistoryVersionId::parse(&current.version_id).unwrap()]).unwrap();
    let operation = OperationId::generate().unwrap();
    let outcome = engine.retire_file_nodes(&read, &selected, "admin", operation).unwrap();
    assert_eq!(outcome.separation().unwrap().terminated_files().iter().map(|file| file.as_str()).collect::<Vec<_>>(), vec![current.file_id.as_str()]);
    assert_eq!(engine.state().content_states[&old], ContentState::Retired);
    (temp, engine, before, operation, current.version_id)
}

#[test]
fn terminal_identity_rejects_a_new_version_even_without_retired_source_edges() {
    let (_temp, engine, before, _operation, version) = terminal_fixture();
    crate::node_retirement_engine::verify(&before, engine.state()).unwrap();
    let mut revived = engine.state().clone();
    let mut fresh = revived.versions[&version].clone();
    let new_id = crate::new_id("ver").unwrap();
    fresh.version_id = new_id.clone();
    fresh.source_version_ids.clear();
    // Keep the original stable file_id but fabricate a fresh content identity.
    // The terminal cohort, not only the old node/source guard, must reject it.
    revived.versions.insert(new_id.clone(), fresh);
    revived.content_states.insert(new_id, ContentState::Available);
    assert!(crate::node_retirement_engine::admit_current(&revived).is_err());
}

#[test]
fn completely_retired_file_requires_its_exact_terminal_receipt() {
    let (_temp, engine, _before, operation, _version) = terminal_fixture();
    let mut missing = engine.state().clone();
    let mut receipt = missing.operations[&operation].clone();
    receipt.result["retirement"].as_object_mut().unwrap().remove("file_terminations");
    missing.operations.insert(operation, receipt);
    assert!(crate::node_retirement_engine::admit_current(&missing).is_err());
    let mut erased = engine.state().clone();
    let mut receipt = erased.operations[&operation].clone();
    receipt.result["retirement"]["file_terminations"]["files"] = json_codec::json!([]);
    erased.operations.insert(operation, receipt);
    assert!(crate::node_retirement_engine::admit_current(&erased).is_err());
}


#[test]
fn terminal_frontier_rejects_branch_changes_new_aliases_and_selection_changes_on_cold_admission() {
    let (_temp, engine, _before, operation, version) = terminal_fixture();
    let file = engine.state().versions[&version].file_id.clone();
    let branch = engine.state().branches.values().find(|branch| branch.file_id.as_deref() == Some(file.as_str())).unwrap().clone();
    let mut changed = engine.state().clone();
    let mut modified = branch.clone();
    modified.generation += 1;
    changed.branches.insert(branch.branch_id.clone(), modified);
    assert!(crate::node_retirement_engine::admit_current(&changed).is_err());
    let mut changed = engine.state().clone();
    changed.file_branch_aliases.insert(branch.branch_id.clone(), branch.branch_id.clone());
    assert!(crate::node_retirement_engine::admit_current(&changed).is_err());
    let mut changed = engine.state().clone();
    if changed.file_selections.contains_key(&file) { changed.file_selections.remove(&file); }
    else { changed.file_selections.insert(file.clone(), branch.branch_id.clone()); }
    assert!(crate::node_retirement_engine::admit_current(&changed).is_err());
    let mut changed = engine.state().clone();
    let mut receipt = changed.operations[&operation].clone();
    receipt.result["retirement"]["file_terminations"]["files"][0].as_object_mut().unwrap().remove("retained_frontier");
    changed.operations.insert(operation, receipt);
    assert!(crate::node_retirement_engine::admit_current(&changed).is_err());
}
