//! A real published Unborn file has a registered physical container without
//! any historical Version. Scope selectors must not turn that into an empty
//! Backup. Modified routing snapshots below exercise preflight, not authority.
use super::*;
use crate::{CommitRequest, Config, Engine, ErrorCode, history_package::AdmittedState};
use name_policy::PortablePath;
use storage_contract::{OperationId, file_draft::BirthRequest};

#[test]
fn registered_unborn_file_is_not_an_empty_original_or_complete_object_backup() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temp.path().join("system"),
        Config::development(crate::test_disks([temp.path().join("original")])).unwrap(),
    ).unwrap();
    let op = || OperationId::generate().unwrap();
    let repository = engine.create_disk_root("P01", "admin", op()).unwrap();
    let request = BirthRequest {
        repository: repository.repository_id.parse().unwrap(),
        path: PortablePath::parse("no-first-node.txt").unwrap(),
        expected_namespace_head: engine.state().branches[&repository.main_branch_id]
            .head_commit_id.parse().unwrap(),
    };
    let result = engine.create_unborn_file(&request, &"admin".parse().unwrap(), op()).unwrap();
    let birth = crate::persist::file_draft::birth::decode(&result).unwrap();
    let file = birth.target().file;
    let disk = &engine.config().disks[0];
    let record = &engine.state().file_containers[&file];
    assert!(record.replicas[&disk.disk_id.parse::<DiskId>().unwrap()].entity.inode() != 0);
    assert!(engine.state().versions.is_empty());

    let state = AdmittedState::admit_replayed(engine.state().clone()).unwrap();
    let stop = AtomicBool::new(false);
    assert_eq!(source_files(state.state(), disk, &stop).unwrap_err().code(),
        ErrorCode::UnsupportedOperation);
    let Err(error) = backup_databases::PreparedDatabases::capture(&state, &stop) else {
        panic!("an Unborn working file has no committed backup BODY");
    };
    assert_eq!(error.code(), ErrorCode::UnsupportedOperation);
    assert_eq!(engine.open_unborn_working(birth.target()).unwrap().logical_size(), 0);
    assert_eq!(engine.state().file_containers[&file], *record);
}

#[test]
fn registered_file_is_independent_of_body_routes_and_each_route_needs_its_original_binding() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temp.path().join("system"),
        Config::development(crate::test_disks([temp.path().join("original")])).unwrap(),
    ).unwrap();
    let op = || OperationId::generate().unwrap();
    let repository = engine.create_disk_root("P01", "admin", op()).unwrap();
    let workspace = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
    engine.stage_bytes(&workspace.workspace_id, "file", b"complete original").unwrap();
    engine.commit_workspace(CommitRequest {
        workspace_id: workspace.workspace_id, operation_id: op(), principal: "admin".into(),
        expected_head: workspace.base_head, fencing_token: workspace.fencing_token,
        message: "registered occurrence".into(), request_digest: None,
    }).unwrap();
    let disk = &engine.config().disks[0];
    let id = disk.disk_id.parse::<DiskId>().unwrap();
    let version = engine.state().versions.values().next().unwrap();
    let file = version.file_id.parse::<FileId>().unwrap();
    let expected = BTreeSet::from([file]);
    let stop = AtomicBool::new(false);
    assert_eq!(source_files(engine.state(), disk, &stop).unwrap(), expected);
    let admitted = AdmittedState::admit_replayed(engine.state().clone()).unwrap();
    let prepared = backup_databases::PreparedDatabases::capture(&admitted, &stop).unwrap();
    assert_eq!(prepared.file_count(), 1);
    for database in prepared.databases() {
        database.verify(database.bytes()).unwrap();
    }

    // The original FILE registration remains an obligation without a local BODY.
    let mut metadata_only = engine.state().clone();
    let placement = metadata_only.placements.get_mut(&version.version_id).unwrap();
    placement.route_slot_ids.retain(|slot| slot != &disk.slot_id);
    placement.binding_generations.remove(&disk.slot_id);
    assert_eq!(source_files(&metadata_only, disk, &stop).unwrap(), expected);

    let mut missing_replica = engine.state().clone();
    missing_replica.file_containers.get_mut(&file).unwrap().replicas.remove(&id);
    assert_eq!(source_files(&missing_replica, disk, &stop).unwrap_err().code(),
        ErrorCode::ContainerLayoutInvalid);
    let mut other_binding = disk.clone();
    other_binding.binding_generation += 1;
    assert_eq!(source_files(engine.state(), &other_binding, &stop).unwrap_err().code(),
        ErrorCode::ContainerLayoutInvalid);
}
