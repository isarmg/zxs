use super::*;
use crate::{CommitRequest, Config, Engine, Mode};
use name_policy::Name;
use storage_contract::{OperationId, new_id};

#[test]
fn registered_history_accepts_only_its_single_main_and_reachable_file_commits() {
    let temp = tempfile::tempdir().unwrap();
    let system = temp.path().join("system");
    let config = Config::independent_roots(Mode::Development, [
        (Name::parse("Primary").unwrap(), temp.path().join("p")),
    ]).unwrap();
    let mut engine = Engine::initialize(&system, config).unwrap();
    let owner = engine.create_disk_root("D01", "alice", OperationId::generate().unwrap()).unwrap();
    let workspace = engine.create_workspace(&owner.main_branch_id, "alice").unwrap();
    let current = json_codec::to_value(&workspace).unwrap();
    for field in ["selected_directory_branches", "directory_selection_generations"] {
        assert!(current.get(field).is_none());
        let mut invalid = current.clone();
        invalid[field] = json_codec::json!({});
        assert!(json_codec::from_value::<storage_contract::Workspace>(invalid).is_err());
    }
    engine.stage_bytes(&workspace.workspace_id, "photos/camera.zxs", b"ordinary file").unwrap();
    engine.commit_workspace(CommitRequest {
        workspace_id: workspace.workspace_id, operation_id: OperationId::generate().unwrap(),
        principal: "alice".into(), expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token, message: "ordinary file".into(), request_digest: None,
    }).unwrap();
    let valid = HistoryManifest::from_state(engine.state(), &owner.history_tree_id).unwrap();
    assert!(valid.branches.values().any(|branch| branch.branch_kind == BranchKind::File));
    let selection = valid.file_contexts.values().next().expect("published FILE has an exact selector");
    let mut wire = json_codec::to_value(selection).unwrap();
    assert!(wire.get("inherited_from").is_none());
    for former in [json_codec::Value::Null, json_codec::json!(&owner.main_branch_id)] {
        wire["inherited_from"] = former;
        assert!(json_codec::from_value::<FileContext>(wire.clone()).is_err());
    }
    let main = valid.branches[&owner.main_branch_id].clone();
    assert!(valid.commits[&main.head_commit_id].entries.iter().any(|entry|
        entry.kind == NamespaceEntryKind::Directory && entry.path() == "photos"));

    let mut generic = valid.clone();
    generic.repository.file_manager_root = false;
    assert!(generic.validate().is_err());

    let mut extra = valid.clone();
    let mut foreign_branch = main.clone();
    foreign_branch.branch_id = new_id("br").unwrap();
    extra.branches.insert(foreign_branch.branch_id.clone(), foreign_branch);
    assert!(extra.validate().is_err());

    let id = new_id("commit").unwrap();
    let mut detached = valid.clone();
    let mut commit = detached.commits[&main.head_commit_id].clone();
    commit.commit_id.clone_from(&id);
    commit.tree_id = new_id("tree").unwrap();
    commit.parent_commit_ids.clear();
    detached.commits.insert(id.clone(), commit.clone());
    assert!(detached.validate().is_err());

    commit.parent_commit_ids = vec![id.clone()];
    detached.commits.insert(id.clone(), commit.clone());
    assert!(detached.validate().is_err());

    commit.parent_commit_ids = vec![valid.history.root_commit_id.clone()];
    commit.kind = "directory_commit".into();
    detached.commits.insert(id, commit);
    assert!(detached.validate().is_err());

    drop(engine);
    let reopened = Engine::open(&system).unwrap();
    let after = HistoryManifest::from_state(reopened.state(), &owner.history_tree_id).unwrap();
    assert_eq!(valid, after);
}

#[test]
fn full_native_state_requires_original_root_registration_and_creation_event_in_both_directions() {
    use crate::engine::fresh_layout::namespace::require_root_origins;
    let temp = tempfile::tempdir().unwrap();
    let config = Config::independent_roots(Mode::Development, [
        (Name::parse("Primary").unwrap(), temp.path().join("p")),
    ]).unwrap();
    let mut engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
    require_root_origins(engine.state()).unwrap();
    let operation = OperationId::generate().unwrap();
    let owner = engine.create_disk_root("D01", "alice", operation).unwrap();
    let valid = engine.state();
    require_root_origins(valid).unwrap();

    let mut no_current = valid.clone();
    no_current.repositories.remove(&owner.repository_id).unwrap();
    assert!(require_root_origins(&no_current).is_err());

    let mut no_original = valid.clone();
    no_original.operations.remove(&operation).unwrap();
    assert!(require_root_origins(&no_original).is_err());

    let mut no_event = valid.clone();
    no_event.events = Default::default();
    assert!(require_root_origins(&no_event).is_err());

    let mut duplicate = valid.clone();
    let event = valid.events.iter().find(|event| event.operation_id == operation).unwrap().clone();
    duplicate.events.push(event);
    assert!(require_root_origins(&duplicate).is_err());

    let mut false_geometry = valid.clone();
    let mut receipt = false_geometry.operations[&operation].clone();
    let original = &mut receipt.result["file_manager_root_origin"]["genesis"];
    original["areas"]["common_data"]["entity"] = original["root_entity"].clone();
    false_geometry.operations.insert(operation, receipt);
    assert!(require_root_origins(&false_geometry).is_err());
}
