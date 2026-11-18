use super::*;
use crate::{CommitRequest, Config, Engine};
use storage_contract::OperationId;

#[test]
fn full_body_table_requires_complete_named_versions_and_original_inodes() {
    let temporary = tempfile::tempdir().unwrap();
    let config = Config::development([(
        name_policy::Name::parse("Primary").unwrap(), temporary.path().join("disk"),
    )]).unwrap();
    let slot = config.disks[0].slot_id.clone();
    let mut engine = Engine::initialize(&temporary.path().join("system"), config).unwrap();
    let repo = engine.create_disk_root(&slot, "alice", OperationId::generate().unwrap()).unwrap();
    for payload in [b"first".as_slice(), b"second".as_slice()] {
        let workspace = engine.create_workspace(&repo.main_branch_id, "alice").unwrap();
        engine.stage_bytes(&workspace.workspace_id, "visible.txt", payload).unwrap();
        engine.commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id, expected_head: workspace.base_head,
            operation_id: OperationId::generate().unwrap(), principal: "alice".into(),
            fencing_token: workspace.fencing_token, message: "ordinary save".into(), request_digest: None,
        }).unwrap();
    }
    let file: FileId = engine.list_tree(&repo.main_branch_id).unwrap()[0].file_id.parse().unwrap();
    let ctx = engine.planning_context();
    let projection = crate::state_projection::ProjectionTransition::new(ctx.state, ctx.state, None).unwrap();
    let rows = ResidentFileRows::current(&ctx, &ctx.config.disks[0], file).unwrap();
    let bodies = rows.clean_full_bodies(&ctx, projection.previous()).unwrap();
    assert_eq!(bodies.pages().entries(), 2);
    assert_eq!(bodies.names.len(), 2);
    let material = bodies.material(&rows).unwrap();
    assert_eq!(material.identity, file);
    assert_eq!(material.logical_root, rows.rows());
    assert_eq!(material.representation, rows.representation());
    assert_eq!(&material.body_inventory, bodies.pages().root());

    let parents = crate::engine::scope_closure::directory::OrdinaryDirectories::original(&ctx, &ctx.config.disks[0]).unwrap();
    let complete = super::super::census::ResidentFiles::original(&ctx, &ctx.config.disks[0], &parents).unwrap();
    let mut inventory = OrdinalBuilder::new(2);
    complete.append_to(&mut inventory).unwrap();
    let actual = inventory.into_occurrences().unwrap();
    assert_eq!(actual.len(), 1);
    let row = &actual[0];
    assert_eq!(row["object"]["kind"], "file_container");
    assert_eq!(row["object"]["file_id"], file.to_string());
    assert_eq!(row["object"]["material"], json_codec::to_value(&material).unwrap());
    assert_eq!(row["ownership"]["kind"], "original");
    assert_eq!(row["location"]["name"], "visible.txt");
    assert_eq!(row["lifecycle"]["kind"], "current");
    let physical = bodies.version_dir.path();
    let unknown = physical.join("unselected-extra");
    std::fs::write(&unknown, b"unselected").unwrap();
    assert!(bodies.recheck(&rows).is_err());
    assert!(rows.clean_full_bodies(&ctx, projection.previous()).is_err());
    assert_eq!(std::fs::read(&unknown).unwrap(), b"unselected");
    std::fs::remove_file(unknown).unwrap();

    let leaf = physical.join(bodies.names.values().next().unwrap().name.as_path());
    let replacement = physical.join("replacement");
    std::fs::write(&replacement, std::fs::read(&leaf).unwrap()).unwrap();
    std::fs::set_permissions(&replacement, std::fs::metadata(&leaf).unwrap().permissions()).unwrap();
    std::fs::rename(replacement, leaf).unwrap();
    assert!(bodies.recheck(&rows).is_err());
    assert!(complete.require(ctx.config, &parents).is_err());
}

#[test]
fn real_unborn_birth_requires_its_file_and_existing_empty_history_directory() {
    use storage_contract::file_draft::BirthRequest;
    let tmp = tempfile::tempdir().unwrap();
    let config = Config::development([(name_policy::Name::parse("Primary").unwrap(), tmp.path().join("disk"))]).unwrap();
    let slot = config.disks[0].slot_id.clone();
    let mut engine = Engine::initialize(&tmp.path().join("system"), config).unwrap();
    let actor = "alice".parse().unwrap();
    let repo = engine.create_disk_root(&slot, "alice", OperationId::generate().unwrap()).unwrap();
    let birth = OperationId::generate().unwrap();
    let request = BirthRequest {
        repository: repo.repository_id.parse().unwrap(),
        path: name_policy::PortablePath::parse("not-a-version").unwrap(),
        expected_namespace_head: engine.state().branches[&repo.main_branch_id].head_commit_id.parse().unwrap(),
    };
    engine.create_unborn_file(&request, &actor, birth).unwrap();
    let ctx = engine.planning_context();
    let (&file, native) = ctx.state.file_containers.iter().find(|(_, r)| r.created_by == birth).unwrap();
    assert!(native.working.unborn().is_some());
    let disk = &ctx.config.disks[0];
    let rows = ResidentFileRows::current(&ctx, disk, file).unwrap();
    let projection = crate::state_projection::ProjectionSnapshot::new(ctx.state);
    assert!(rows.clean_full_bodies(&ctx, projection.view()).is_err());
    let body = rows.resident_bodies(&ctx, projection.view()).unwrap();
    assert_eq!(body.pages().entries(), 0);
    let parents = crate::engine::scope_closure::directory::OrdinaryDirectories::original(&ctx, disk).unwrap();
    let all = super::super::census::ResidentFiles::original(&ctx, disk, &parents).unwrap();
    let mut inventory = OrdinalBuilder::new(1);
    all.append_to(&mut inventory).unwrap();
    let actual = inventory.into_occurrences().unwrap();
    assert_eq!(actual.len(), 1);
    assert_eq!(actual[0]["operation_id"], json_codec::to_value(birth).unwrap());
    assert_eq!(actual[0]["object"]["material"], json_codec::to_value(body.material(&rows).unwrap()).unwrap());
    // A real admitted empty history is not "any bytes whose name we don't understand".
    let unknown = body.version_dir.path().join("unowned");
    std::fs::write(&unknown, b"keep this").unwrap();
    assert!(all.require(ctx.config, &parents).is_err());
    assert!(rows.resident_bodies(&ctx, projection.view()).is_err());
    assert_eq!(std::fs::read(unknown).unwrap(), b"keep this");
}
