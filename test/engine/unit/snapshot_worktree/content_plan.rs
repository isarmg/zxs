//! Planning an entire set is side-effect free; none of these facts alone
//! authorizes a Candidate, a worktree move or an independently visible FILE.
use super::*;
use crate::persist::snapshot_worktree::content_batch::Reservation;
use crate::snapshot_worktree::commit::content::Originals;
use crate::working_tree::snapshot::{SnapshotEntryContent, SnapshotFileSource};
use storage_contract::{FileNodeId, snapshot_worktree::CommitWorktree};

#[test]
fn exact_files_share_the_business_operation_on_one_state_but_cannot_install_it() {
    let (fixture, mut engine) = Fixture::with_nested_directory(true);
    let scratch = tempfile::tempdir().unwrap();
    let principal: Principal = "admin".parse().unwrap();
    let root = engine.snapshot_root_context(fixture.targets[0].repository).unwrap();
    let instance = engine.allocate_directory_worktree(&AllocateWorktree {
        source: fixture.source, parent_directory_id: root.host.directory_id,
        expected_root_generation: root.host.expected_root_generation,
        operation_id: OperationId::generate().unwrap(), naming: WorktreeNaming::Automatic,
    }, &principal).unwrap();
    save(&mut engine, instance, &principal, scratch.path(), "first.txt", b"superseded");
    let equal = save(&mut engine, instance, &principal, scratch.path(), "Folder/Child/existing.txt",
        b"unchanged nested original");
    let current = save(&mut engine, instance, &principal, scratch.path(), "first.txt", b"final bytes");
    let request = CommitWorktree { target: instance,
        expected: engine.directory_worktree_status(instance).unwrap().revision,
        message: "publish my whole existing content set".into(), operation_id: OperationId::generate().unwrap() };
    drop(engine);
    let engine = Engine::open(&fixture.system).unwrap();
    let media = fixture.media();
    let index = engine.snapshot_receipts().unwrap();
    let capture = Originals::capture(&engine, &index, &request).unwrap();
    let ctx = engine.planning_context();
    let plan = capture.plan_reserve(&ctx, &principal).unwrap();
    let encoded = storage_contract::canonical_json(&plan).unwrap();
    assert_eq!(storage_contract::decode_strict::<Reservation>(&encoded).unwrap(), plan);
    assert_eq!(plan.rows.len(), 2);
    let projection = capture.project_reserved(&ctx, &principal, &plan).unwrap();
    assert_eq!(projection.versions.len(), plan.rows.len());
    assert_eq!(projection.manifest.entries().len(), capture.source_node.manifest().entries().len());
    assert_eq!(projection.manifest.node_id(), plan.destination_node);
    assert_eq!(projection.manifest.operation_id(), plan.publish_operation);
    for ((input, output), save) in plan.rows.iter().zip(&projection.rows).zip([current, equal]) {
        assert_eq!(input.edit_operation, save.operation_id);
        assert_eq!(input.edit.path, save.path);
        assert_eq!(input.edit.material.content_hash, save.content_hash);
        assert_eq!(output.file.path, save.path);
        assert_eq!(output.file.material, input.edit.material);
        assert_eq!(output.file.source, SnapshotFileSource::Container {
            branch: input.base.branch_id(), node: FileNodeId::Version(input.edit.material.version_id),
        });
        assert_eq!(output.content.allocations, input.allocations);
        assert_eq!(output.content.version.source_version_ids, [input.original.version_id.to_string()]);
        assert_eq!(projection.state.file_containers[&input.original.file_id].working, input.clean);
        let commit = &projection.state.commits[&input.commit.to_string()];
        assert_eq!(commit.commit_id, output.commit.to_string());
        assert_eq!(commit.tree_id, input.tree.to_string());
        assert_eq!(commit.parent_commit_ids, [input.branch_base.to_string()]);
        assert_eq!(commit.operation_id, plan.publish_operation);
        assert!(!engine.state().versions.contains_key(&input.edit.material.version_id.to_string()));
    }
    for entry in capture.source_node.manifest().entries() {
        let projected = projection.manifest.entries().iter().find(|next| next.path == entry.path).unwrap();
        match projection.rows.iter().find(|output| output.path == entry.path) {
            Some(row) => assert_eq!(projected.content, SnapshotEntryContent::File {
                material: row.file.material, source: row.file.source,
            }),
            None => assert_eq!(projected, entry),
        }
    }
    assert_ne!(plan.rows[0].commit, plan.rows[1].commit);
    let mut forged = plan.clone();
    forged.rows.swap(0, 1);
    assert!(capture.project_reserved(&ctx, &principal, &forged).is_err());
    let mut forged = plan.clone();
    forged.rows[1].allocations.pop().unwrap();
    assert!(capture.project_reserved(&ctx, &principal, &forged).is_err());
    let mut forged = plan.clone();
    forged.rows[1].commit = plan.rows[0].commit;
    assert!(capture.project_reserved(&ctx, &principal, &forged).is_err());
    assert_eq!(fixture.media(), media);
}
