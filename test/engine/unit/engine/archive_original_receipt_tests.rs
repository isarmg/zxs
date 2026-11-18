//! Actual frozen publications, not constructed receipts, seed the post-D3
//! index. The archive-wide negative is distinct from a scoped live retry.
use super::*;
use crate::engine::{OriginalReceiptCursor, membership_transition::pending::{
    CONTROL_ARCHIVE_DIRECTORY, member_directory,
}};
use crate::publication_evidence::OriginalReceiptProjection;
use crate::{Config, Engine, MembershipExitAction, OperationId, OriginalReceiptError};
use name_policy::Name;
use std::{collections::BTreeMap, fs, path::{Path, PathBuf}};
use storage_contract::snapshot_worktree::{CreateWorktree, InitializeSnapshot};

fn operation() -> OperationId {
    OperationId::generate().unwrap()
}

fn inventory(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn visit(root: &Path, path: &Path, entries: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let key = path.strip_prefix(root).unwrap().to_path_buf();
            if entry.file_type().unwrap().is_dir() {
                entries.insert(key, None);
                visit(root, &path, entries);
            } else {
                entries.insert(key, Some(fs::read(path).unwrap()));
            }
        }
    }
    let mut entries = BTreeMap::new();
    visit(root, root, &mut entries);
    entries
}

#[test]
fn actual_exit_archive_restores_original_index_and_rejects_unrelated_missing_source() {
    let temporary = tempfile::tempdir().unwrap();
    let system = temporary.path().join("system");
    let mut config = Config::development(crate::test_disks(
        (0..4).map(|index| temporary.path().join(format!("disk-{index}"))),
    )).unwrap();
    config.disks[3].level = 1;
    config.disks[3].upstream_slot_id = None;
    config.disks[3].metadata_member = false;
    let mut engine = Engine::initialize(&system, config).unwrap();
    let repository = engine.create_disk_root("D04", "archive-owner", operation()).unwrap();
    let principal = "archive-owner".parse().unwrap();
    let context = engine.snapshot_root_context(repository.repository_id.parse().unwrap()).unwrap();
    let initialize = InitializeSnapshot {
        host: context.host,
        source: context.source,
        operation_id: operation(),
    };
    let node = engine.initialize_directory_snapshot(&initialize, &principal).unwrap();
    let clone = CreateWorktree {
        source: node,
        parent_directory_id: initialize.host.directory_id,
        name: Name::parse("archived-worktree").unwrap(),
        expected_root_generation: initialize.host.expected_root_generation,
        operation_id: operation(),
    };
    let worktree = engine.create_directory_worktree(&clone, &principal).unwrap();
    let status = engine.directory_worktree_status(worktree).unwrap();
    let indexed_operations = [
        initialize.operation_id, node.publication, clone.operation_id, worktree.publication,
    ];
    let original_results = indexed_operations.into_iter().map(|operation| {
        (operation, engine.operation(operation).unwrap())
    }).collect::<BTreeMap<_, _>>();
    let owner_common = engine.config.disks[3].root.join("common-data");
    let original_material = inventory(&owner_common);
    let target = engine.config.disks[0].clone();
    let exit_operation = operation();
    engine.begin_disk_exit(
        &target.disk_id, MembershipExitAction::Deregister, principal.as_str(), exit_operation,
    ).unwrap();
    let archive = engine.build_managed_membership_control_archive().unwrap();
    engine.enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new()).unwrap();
    let joint = engine.resume_membership_joint_decision().unwrap();
    assert_eq!((joint.original_members, joint.target_members), (3, 2));
    assert!(joint.joint_quorum_accepted());
    let exit = engine.activate_pending_disk_exit().unwrap();
    assert_eq!(exit.operation_id, exit_operation);
    assert_eq!(engine.initialize_directory_snapshot(&initialize, &principal).unwrap(), node);
    assert_eq!(engine.create_directory_worktree(&clone, &principal).unwrap(), worktree);
    assert_eq!(engine.directory_worktree_status(worktree).unwrap(), status);
    assert_eq!(inventory(&owner_common), original_material);

    // The actual Stop makes this root irrelevant. A path occupying its old
    // name must not be opened to recover the archived original receipts.
    fs::rename(&target.root, temporary.path().join("detached-original")).unwrap();
    fs::write(&target.root, b"excluded root must not be opened").unwrap();
    drop(engine);
    let mut engine = Engine::open(&system).unwrap();
    assert!(!engine.is_read_only());
    assert_eq!(engine.operation(exit_operation).unwrap(), exit);
    let stable = Arc::clone(&engine.state);
    let stable_snapshot = engine.snapshot.clone();
    let stable_slot = engine.next_slot;
    for (operation, expected) in &original_results {
        let cursor = OriginalReceiptCursor::for_context(&engine.planning_context()).unwrap();
        let (original, actor) = cursor.receipt(&engine.state, *operation).unwrap();
        assert_eq!(original, expected);
        assert_eq!(actor, &principal);
        assert_eq!(&engine.operation(*operation).unwrap(), expected);
    }
    assert_eq!(engine.initialize_directory_snapshot(&initialize, &principal).unwrap(), node);
    assert_eq!(engine.create_directory_worktree(&clone, &principal).unwrap(), worktree);
    assert_eq!(engine.directory_worktree_status(worktree).unwrap(), status);
    assert!(Arc::ptr_eq(&engine.state, &stable));
    assert_eq!(engine.snapshot, stable_snapshot);
    assert_eq!(engine.next_slot, stable_slot);
    assert_eq!(inventory(&owner_common), original_material);

    // Equal State values do not preserve an installed occurrence. This test
    // replacement is only a rejection probe; it is never serialized or used
    // as a source for a successful operation.
    engine.state = Arc::new(stable.as_ref().clone());
    assert!(matches!(
        OriginalReceiptCursor::for_context(&engine.planning_context()),
        Err(Error::OriginalReceipt(OriginalReceiptError::Prefix)),
    ));
    engine.state = Arc::clone(&stable);

    let generation = engine.config.generation().unwrap();
    assert!(matches!(
        engine.replay_generation_original_receipts(generation),
        Err(Error::OriginalReceipt(OriginalReceiptError::Prefix)),
    ));
    let mut initialization_snapshot = None;
    let mut projected = 0;
    let mut links=crate::engine::recovery::ArchiveWorkspacePrefix::default();
    engine.replay_generation_publications(generation, |publication| {
        // Use the genuine prior slot held by the restricted archive cursor.
        // A byte-equal clone isn't a loan, and an otherwise identical
        // Candidate must not spend the decision at another material observer.
        if let Some(prior)=links.predecessor(publication)? {
            links.index(prior)?;
            let copied=prior.clone();
            assert!(matches!(links.index(&copied),Err(Error::OriginalReceipt(OriginalReceiptError::Prefix))));
            let next=publication.original_state().as_ref();
            let candidate=publication.candidate();
            let decision=crate::engine::workspace_provenance::candidate(prior,next,candidate,||links.index(prior))?;
            decision.require_pair(prior,next,candidate)?;
            assert!(decision.require_pair(&copied,next,candidate).is_err());
            assert!(decision.require_pair(prior,&next.clone(),candidate).is_err());
            assert!(decision.require_pair(prior,next,&candidate.clone()).is_err());
        }
        links.advance(publication)?;
        assert!(links.advance(publication).is_err());
        let visible = publication.visible_state();
        let projection = OriginalReceiptProjection::from_archive(publication, &visible)?;
        let (original, actor) = projection.in_prefix(&visible)?;
        assert_eq!(actor.as_str(), "archive-owner");
        if publication.candidate().operation_id == initialize.operation_id {
            assert_eq!(original, &original_results[&initialize.operation_id]);
            initialization_snapshot = Some(publication.candidate().snapshot_hash.clone());
        }
        let mut substituted = visible.as_ref().clone();
        substituted.operation_principals.insert(original.operation_id, "different-owner".into());
        assert!(matches!(
            OriginalReceiptProjection::from_archive(publication, &substituted),
            Err(Error::OriginalReceipt(OriginalReceiptError::Visible)),
        ));
        projected += 1;
        Ok(())
    }).unwrap();
    assert!(projected >= indexed_operations.len());

    // Fault the exact object reached by the real archive callback. The shared
    // global reader must fail even though the requested worktree's own final
    // receipt remains present. A live scoped retry retains its already-read
    // original evidence; it does not pretend to freshly validate this archive.
    let disk = engine.config.metadata_members().find(|disk| {
        engine.disk_resources.available(disk).is_some()
    }).unwrap();
    let transition = object_hash(INTENT_KIND, &generation.portable.intent().unwrap()).unwrap();
    let directory = engine.disk_resources.get(disk).unwrap().root.private_child(
        &member_directory(disk, &transition).join(CONTROL_ARCHIVE_DIRECTORY),
    ).unwrap();
    let store = ArchiveStore { root: directory };
    let snapshot_path = store.object_path("snapshot", &initialization_snapshot.unwrap()).unwrap();
    let saved = temporary.path().join("held-archived-snapshot");
    fs::rename(&snapshot_path, &saved).unwrap();
    let after_fault = inventory(store.root.path());
    let mut visits = 0;
    let error = engine.replay_generation_publications(generation, |_| {
        visits += 1;
        Ok(())
    }).unwrap_err();
    assert_eq!(error.code(), ErrorCode::IoError);
    assert!(matches!(error, Error::Filesystem(platform_io::fs_safety::Error::Io(source))
        if source.kind() == std::io::ErrorKind::NotFound));
    assert_eq!(visits, 0);
    assert_eq!(inventory(store.root.path()), after_fault);
    assert!(Arc::ptr_eq(&engine.state, &stable));
    assert_eq!(engine.snapshot, stable_snapshot);
    assert_eq!(engine.next_slot, stable_slot);
    assert_eq!(engine.directory_worktree_status(worktree).unwrap(), status);
    fs::rename(&saved, &snapshot_path).unwrap();
    drop(store);

    // A real new Current namespace admission must extend the installed index
    // rather than dropping its archived prefix at the first live completion.
    let prior = engine.state.operations.len();
    let workspace = engine
        .create_workspace(&repository.main_branch_id, principal.as_str()).unwrap();
    assert!(engine.workspaces.contains_key(&workspace.workspace_id));
    assert_eq!(engine.state.operations.len(), prior + 1);
    assert!(engine.state.versions.is_empty());
    assert_eq!(engine.initialize_directory_snapshot(&initialize, &principal).unwrap(), node);
    assert_eq!(engine.directory_worktree_status(worktree).unwrap(), status);
    drop(engine);
    let mut engine = Engine::open(&system).unwrap();
    assert!(!engine.is_read_only());
    assert_eq!(engine.initialize_directory_snapshot(&initialize, &principal).unwrap(), node);
    assert_eq!(engine.create_directory_worktree(&clone, &principal).unwrap(), worktree);
    assert_eq!(engine.directory_worktree_status(worktree).unwrap(), status);
    assert_eq!(inventory(&owner_common), original_material);
    assert!(engine.state.versions.is_empty());
}
