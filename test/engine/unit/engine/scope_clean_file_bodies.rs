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
}
