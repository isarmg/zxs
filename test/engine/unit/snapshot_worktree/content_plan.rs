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
    // Enrich a manifest made from the actual admitted file/version selectors
    // with another spelling of one occurrence. This is the manifest-level
    // alias case, not a claim that ordinary copy creates FILE aliases.
    let original = capture.source_node.manifest();
    let selected = &plan.rows[0];
    let source_type = SnapshotFileSource::Container {
        branch: selected.base.branch_id(), node: selected.base.node_id() };
    let mut original_entries = original.entries().to_vec();
    let mut alias = original_entries.iter().find(|e| e.path == selected.edit.path).unwrap().clone();
    alias.path = PortablePath::parse("zz-untouched-alias.txt").unwrap();
    original_entries.push(alias.clone());
    let alias_source = crate::working_tree::snapshot::AdmittedSnapshotManifest::prepare(original.repository_id(),
        storage_contract::working_tree::SnapshotNodeId::generate().unwrap(), OperationId::generate().unwrap(), original_entries).unwrap();
    let rebuilt = crate::snapshot_worktree::commit::content::plan::replace_occurrences(&alias_source, plan.destination_node, plan.publish_operation,
        plan.rows.iter().zip(&projection.rows).map(|(input, output)| (&input.edit.path, input.original,
            SnapshotFileSource::Container { branch: input.base.branch_id(), node: input.base.node_id() }, &output.file))).unwrap();
    assert_eq!(rebuilt.entries().iter().find(|entry| entry.path == alias.path), Some(&alias));
    assert_eq!(rebuilt.payloads()[&selected.original.version_id].source_path, alias.path);
    assert_eq!(rebuilt.payloads()[&selected.edit.material.version_id].source_path, selected.edit.path);
    let mut duplicate = projection.rows[0].file.clone();
    duplicate.path = alias.path.clone();
    assert!(crate::snapshot_worktree::commit::content::plan::replace_occurrences(&alias_source, plan.destination_node,
        plan.publish_operation, [(&selected.edit.path, selected.original, source_type, &projection.rows[0].file),
        (&alias.path, selected.original, source_type, &duplicate)]).is_err(),
        "two selected paths referring to one FILE are not two independent versions");
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

#[test]
fn durable_reservation_freezes_both_file_branches_without_publishing_and_reopens_only_original() {
    use crate::snapshot_worktree::registry::ContentRetry;
    use storage_contract::file_draft::{CommitMode, DraftCommand};
    use storage_contract::snapshot_worktree::outcome::WorktreeCommitOutcome;
    let (fixture, mut engine) = Fixture::new();
    let scratch = tempfile::tempdir().unwrap();
    let principal: Principal = "admin".parse().unwrap();
    let other_principal: Principal = "different".parse().unwrap();
    let repository = fixture.targets[0].repository;
    let root = engine.snapshot_root_context(repository).unwrap();
    let instance = engine.allocate_directory_worktree(&AllocateWorktree {
        source: fixture.source, parent_directory_id: root.host.directory_id,
        expected_root_generation: root.host.expected_root_generation,
        operation_id: OperationId::generate().unwrap(), naming: WorktreeNaming::Automatic,
    }, &principal).unwrap();
    for target in &fixture.targets {
        save(&mut engine, instance, &principal, scratch.path(), target.path.as_str(), b"both saved");
    }
    let request = CommitWorktree { target: instance,
        expected: engine.directory_worktree_status(instance).unwrap().revision,
        message: "one batch".into(), operation_id: OperationId::generate().unwrap() };
    let branches = engine.state().branches.clone();
    let versions = engine.state().versions.clone();
    let index = engine.snapshot_receipts().unwrap();
    let captured = Originals::capture(&engine, &index, &request).unwrap();
    drop(index);
    crate::snapshot_worktree::commit::content::reserve(&mut engine, captured, &principal).unwrap();
    drop(engine);
    let mut engine = Engine::open(&fixture.system).unwrap();
    assert_eq!(engine.state().versions, versions);
    assert_eq!(engine.state().branches, branches);
    assert!(engine.directory_worktree_status(instance).is_err());
    assert!(engine.directory_worktree_commit_receipt(repository, request.operation_id, &other_principal).is_err());
    let public = engine.directory_worktree_commit_receipt(repository, request.operation_id, &principal).unwrap();
    assert_eq!(public.request, request);
    assert_eq!(public.result, WorktreeCommitOutcome::Pending {});
    for target in &fixture.targets {
        let mode = DraftCommand::SetMode { target: target.clone(),
            expected: engine.file_working_state(target).unwrap().revision(), mode: CommitMode::Manual };
        let operation = OperationId::generate().unwrap();
        assert!(engine.change_file_working(&mode, &principal, operation).is_err());
        assert!(!engine.state().operations.contains_key(&operation));
    }
    let index = engine.snapshot_receipts().unwrap();
    let mut other = request.clone();
    other.message.push_str(" differing intent");
    assert!(index.retry_content(&other, &principal).is_err());
    assert!(index.retry_content(&request, &other_principal).is_err());
    let ContentRetry::Reserved(frozen) = index.retry_content(&request, &principal).unwrap().unwrap()
        else { panic!("a reservation must never answer Complete"); };
    // Actual durable reservations seed the per-cold-pass set. An intervening
    // ordinary receipt may not temporarily change a frozen branch, even if
    // the later business publication would have restored the same outer view.
    let fences = index.content_replay_fences();
    let occupied = engine.state().operations.get(&request.operation_id).unwrap();
    assert!(fences.selects(frozen.plan().publish_operation));
    fences.require_next(engine.state(), occupied, engine.config().max_entries).unwrap();
    let mut displaced = engine.state().clone();
    let branch = displaced.branches.get_mut(&frozen.plan().rows[0].base.branch_id().to_string()).unwrap();
    branch.generation += 1;
    // Generation of the ordinary projected root is frozen independently too.
    displaced.branches.get_mut(&frozen.plan().host.namespace.branch_id.to_string()).unwrap().generation += 1;
    assert!(fences.require_next(&displaced, occupied, engine.config().max_entries).is_err());
    let original = Originals::retained(&engine, &index, &frozen).unwrap();
    let projection = original.project_reserved(&engine.planning_context(), &principal, frozen.plan()).unwrap();
    assert_eq!(projection.rows.len(), 2);
    assert_eq!(engine.state().versions, versions);
    assert_eq!(engine.state().branches, branches);
    let input = crate::snapshot_worktree::OriginalContentInput::capture(&engine, &index, &frozen, &principal).unwrap();
    let dirty: Vec<_> = input.rows().iter().map(|r| (
        r.edit.leaf.entry().clone(),
        crate::persist::layout::snapshot::file(&input.manifest().payloads()[&r.edit.material.version_id]),
        r.edit.material,
    )).collect();
    let original_node = fixture.snapshot_path().join(crate::persist::layout::snapshot::node(fixture.source.node_id).as_path());
    let sparse = original_node.join(crate::persist::layout::snapshot::WORKTREES.as_path())
        .join(crate::persist::layout::worktree::instance(instance.worktree_id).as_path());
    let (held, _, _) = engine.planning_context().prepare_sparse_content_node(input, crate::disk::MAX_METADATA).unwrap();
    held.require_before(&engine.planning_context()).unwrap();
    let stage = fixture.snapshot_path().join(crate::persist::layout::snapshot::batch(
        held.result().prepared_node.preparation).as_path());
    let final_name = fixture.snapshot_path().join(crate::persist::layout::snapshot::node(frozen.plan().destination_node).as_path());
    assert!(stage.is_dir() && !final_name.exists());
    // The scoped DISK consumer sees two precisely different frame futures,
    // every original/after occurrence and held sparse inodes, not a directory
    // root reconstructed from a global State or a matching file hash.
    use crate::snapshot_worktree::{BodyLocation, ContentMaterial, LedgerStage, NodeImage};
    let scope = held.material_scope(&engine.planning_context()).unwrap();
    assert_eq!(scope.worktree().0, request.target);
    assert_eq!(scope.operations(), (request.operation_id, frozen.plan().publish_operation));
    assert_eq!(scope.nodes().0, frozen.plan().source_binding);
    assert_eq!(*scope.revision(), request.expected);
    assert_eq!(scope.host().0.registration, frozen.plan().host.registration);
    let mut before = 0;
    let mut future = Vec::new(); let mut moves = Vec::new(); let mut roots = Vec::new();
    scope.visit(|row| {
        match row {
            ContentMaterial::Manifest { image, file, canonical, .. } => {
                assert_eq!(file.hash, ContentHash::from_digest(blake3::hash(canonical)));
                roots.push(image);
            }
            ContentMaterial::Body { image: NodeImage::Destination, location: BodyLocation::ReservedMove {
                held, destination_parent, .. }, material, .. } => {
                assert_eq!(material.content_hash, held.hash);
                assert_ne!(held.parent, destination_parent);
                moves.push(held.entity);
            }
            ContentMaterial::Frame { stage, checkpoint, bytes, hash, observed_entity, .. } => {
                assert_eq!(hash, ContentHash::from_digest(blake3::hash(bytes)));
                match stage {
                    LedgerStage::Original { entity } => { assert_eq!(observed_entity, Some(entity)); assert_ne!(entity.inode(), 0); before += 1; }
                    LedgerStage::ProspectiveCommit | LedgerStage::ProspectiveCompleted => { assert_eq!(observed_entity, None); future.push(checkpoint); }
                }
            }
            _ => {}
        }
        Ok(())
    }).unwrap();
    assert_eq!(roots, [NodeImage::Source, NodeImage::Destination]);
    assert_eq!(before, frozen.plan().before.sequence.get() as usize + 1);
    assert_eq!(future, [held.result().committing, held.result().completed]);
    assert_eq!(moves.len(), 2);
    assert_ne!(moves[0], moves[1]);
    drop(scope);
    let manifest = stage.join(crate::persist::layout::snapshot::MANIFEST.as_path());
    let kept = stage.parent().unwrap().join("preserved-tested-manifest");
    fs::rename(&manifest, &kept).unwrap(); fs::copy(&kept, &manifest).unwrap();
    assert!(held.require_before(&engine.planning_context()).is_err(), "a scope-consuming decision cannot adopt equal manifest bytes at a different inode");
    fs::remove_file(&manifest).unwrap(); fs::rename(&kept, &manifest).unwrap();
    held.require_before(&engine.planning_context()).unwrap();
    assert_eq!(fs::read_dir(stage.join(crate::persist::layout::snapshot::FILES.as_path())).unwrap().count(), 0);
    for (source, destination, material) in &dirty {
        let bytes = fs::read(sparse.join(source.as_path())).unwrap();
        assert_eq!(ContentHash::from_digest(blake3::hash(&bytes)), material.content_hash);
        assert!(!stage.join(crate::persist::layout::snapshot::FILES.as_path()).join(destination.as_path()).exists());
    }
    let impostor = stage.join(crate::persist::layout::snapshot::FILES.as_path()).join(dirty[0].1.as_path());
    fs::copy(sparse.join(dirty[0].0.as_path()), impostor).unwrap();
    assert!(held.require_before(&engine.planning_context()).is_err(), "same bytes at both names are not the decided inode");
    assert!(!final_name.exists());

}
