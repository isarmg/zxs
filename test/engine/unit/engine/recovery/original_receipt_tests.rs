//! Real publications supply both cases. Negative State mutations are never
//! installed, serialized as receipts, or admitted as replacement originals.
use super::*;
use crate::{CommitRequest, Config, OriginalReceiptError};
use crate::publication_evidence::SharedAdmittedSnapshot;
use crate::snapshot_worktree::{ReceiptIndex, RegistrationError};

fn observed(
    engine: &mut Engine,
    operation: OperationId,
) -> (Candidate, Publication, PublicationSeal, SharedAdmittedSnapshot) {
    let prepared = engine.prepare_recovery().unwrap();
    let (_, candidates) = prepared.records.last_key_value().unwrap();
    assert_eq!(candidates.len(), 1);
    let (hash, receipts) = candidates.first_key_value().unwrap();
    let candidate: Candidate = verified_object_at(
        &engine.disk_resources, &prepared.online, "candidate", hash,
    ).unwrap();
    assert_eq!(candidate.operation_id, operation);
    let publication_record = receipts.iter().find(|receipt| {
        receipt.record.record_type == RecordType::Publication
    }).unwrap();
    let publication: Publication = verified_object_at(
        &engine.disk_resources, &prepared.online, "publication",
        &publication_record.record.object_hash,
    ).unwrap();
    // The exact same helper used by preflight examines these real held-log
    // receipts, not a fabricated publication/seal or an OperationResult label.
    let seal = published_barrier(
        engine, &prepared.online, receipts, &candidate, &publication,
    ).unwrap();
    let original = verified_recovery_snapshot_at(
        &engine.disk_resources, &prepared.online, &candidate.snapshot_hash,
    ).unwrap().into_shared();
    (candidate, publication, seal, original)
}

fn assert_visible_rejected(
    original: &SharedAdmittedSnapshot,
    candidate: &Candidate,
    publication: &Publication,
    seal: &PublicationSeal,
    visible: &State,
) {
    let error = original.project_original_receipt(candidate, publication, seal, visible)
        .err().expect("different visible State must not project an original");
    assert!(matches!(error, Error::OriginalReceipt(OriginalReceiptError::Visible)));
}

#[test]
#[cfg(feature = "fault-injection")]
fn actual_published_and_degraded_receipts_keep_originals_and_reject_visible_substitution() {
    use std::os::unix::fs::MetadataExt;

    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("system");
    let mut engine = Engine::initialize(
        &root,
        Config::development(crate::test_disks([temporary.path().join("primary")])).unwrap(),
    ).unwrap();
    let creation = OperationId::generate().unwrap();
    let repository = engine.create_disk_root("P01", "admin", creation).unwrap();
    let (candidate, publication, seal, original) = observed(&mut engine, creation);
    let permanent = original.share_state();
    let original_bytes = canonical_json(permanent.as_ref()).unwrap();
    let visible = engine.state().clone();
    let mut cursor = OriginalReceiptCursor::default();
    assert!(matches!(cursor.receipt(&visible, creation),
        Err(Error::OriginalReceipt(OriginalReceiptError::Prefix))));
    let projection = original.project_original_receipt(
        &candidate, &publication, &seal, &visible,
    ).unwrap();
    cursor.retain(projection, &engine.config, &engine.disk_resources).unwrap();
    let (receipt, actor) = cursor.receipt(&visible, creation).unwrap();
    assert_eq!(receipt, &permanent.operations[&creation]);
    assert_eq!(actor.as_str(), "admin");
    let repeated = original.project_original_receipt(
        &candidate, &publication, &seal, &visible,
    ).unwrap();
    assert!(matches!(cursor.retain(repeated, &engine.config, &engine.disk_resources),
        Err(Error::OriginalReceipt(OriginalReceiptError::Prefix))));
    let reobserved = original.project_original_receipt(
        &candidate, &publication, &seal, &visible,
    ).unwrap();
    cursor.retain_completed(reobserved, &engine.config, &engine.disk_resources).unwrap();

    let mut wrong_actor = visible.clone();
    wrong_actor.operation_principals.insert(creation, "different-actor".into());
    assert_visible_rejected(&original, &candidate, &publication, &seal, &wrong_actor);
    assert!(matches!(cursor.receipt(&wrong_actor, creation),
        Err(Error::OriginalReceipt(OriginalReceiptError::Prefix))));
    let mut wrong_namespace = visible.clone();
    wrong_namespace.repositories.remove(&repository.repository_id);
    assert_visible_rejected(&original, &candidate, &publication, &seal, &wrong_namespace);
    let mut forged_result = visible.clone();
    forged_result.operations.get_mut(&creation).unwrap().kind = "not-the-original-kind".into();
    assert_visible_rejected(&original, &candidate, &publication, &seal, &forged_result);
    assert!(matches!(cursor.receipt(&forged_result, creation),
        Err(Error::OriginalReceipt(OriginalReceiptError::Prefix))));
    // Global validation sees the retained pair even if the visible kind was
    // changed to one the legacy decoder would otherwise ignore.
    assert!(matches!(ReceiptIndex::capture_with_originals(&forged_result, &cursor),
        Err(RegistrationError::OriginalReceipt(
            Error::OriginalReceipt(OriginalReceiptError::Prefix),
        ))));
    assert_eq!(canonical_json(original.share_state().as_ref()).unwrap(), original_bytes);
    assert_eq!(engine.state(), &visible);
    drop(original);
    drop(permanent);

    // Keep a genuine indexed predecessor, so loss of its installed source
    // cannot be mistaken for the absence of directory duties during recovery.
    let context = engine.snapshot_root_context(repository.repository_id.parse().unwrap()).unwrap();
    let initialize = storage_contract::snapshot_worktree::InitializeSnapshot {
        host: context.host, source: context.source, operation_id: OperationId::generate().unwrap(),
    };
    let node = engine.initialize_directory_snapshot(
        &initialize, &"admin".parse().unwrap(),
    ).unwrap();
    cursor = engine.original_receipts.as_ref().unwrap()
        .replay_cursor(&engine.planning_context()).unwrap();

    // One real original operation reaches ACCEPT. Lose its only BODY and its
    // actual staged source, then use the real explicit loss decision, not a
    // hand-built recovery outcome or a synthetic original snapshot.
    let workspace = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
    engine.stage_bytes(&workspace.workspace_id, "lost.txt", b"actual original receipt body")
        .unwrap();
    let operation = OperationId::generate().unwrap();
    engine.inject_fault_once("after_accept_1");
    let error = engine.commit_workspace(CommitRequest {
        workspace_id: workspace.workspace_id.clone(), operation_id: operation,
        principal: "admin".into(), expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token, message: "receipt projection".into(),
        request_digest: None,
    }).unwrap_err();
    assert!(matches!(error, Error::Context {
        code: ErrorCode::InDoubt, source, ..
    } if source.code() == ErrorCode::InjectedFailure));
    let pending = engine.prepare_recovery().unwrap();
    let (_, candidates) = pending.records.last_key_value().unwrap();
    assert_eq!(candidates.len(), 1);
    let (hash, _) = candidates.first_key_value().unwrap();
    let accepted: Candidate = verified_object_at(
        &engine.disk_resources, &pending.online, "candidate", hash,
    ).unwrap();
    assert_eq!(accepted.operation_id, operation);
    assert_eq!(accepted.versions.len(), 1);
    let admitted = verified_recovery_snapshot_at(
        &engine.disk_resources, &pending.online, &accepted.snapshot_hash,
    ).unwrap();
    let original_result = admitted.for_candidate(&accepted).unwrap().state()
        .operations[&operation].clone();
    let (preparation, container_identity) = {
        let sources = crate::container_storage::PreparedContainerEntities::observe_candidate(
            &engine.planning_context(), &accepted, &admitted,
        ).unwrap();
        let version = &accepted.versions[0];
        assert_eq!(version.placement.route_slot_ids.len(), 1);
        let disk = engine.disk(&version.placement.route_slot_ids[0]).unwrap();
        let scope = sources.scope(
            version.version.file_id.parse().unwrap(), disk.disk_id.parse().unwrap(),
        ).unwrap();
        std::fs::remove_file(scope.body_path(&version.version).unwrap()).unwrap();
        let staged = &engine.workspaces[&workspace.workspace_id].files;
        assert_eq!(staged.len(), 1);
        std::fs::remove_file(&staged.values().next().unwrap().source_path).unwrap();
        let preparation = scope.container().unwrap().path().to_owned();
        let metadata = std::fs::metadata(&preparation).unwrap();
        (preparation, (metadata.dev(), metadata.ino()))
    };
    drop(admitted);
    drop(pending);
    let plan = engine.prepare_degraded_recovery(
        operation, "recovery-admin", "permanent loss of the only BODY and original stage",
    ).unwrap();
    let before = Arc::clone(&engine.state);
    let logs = engine.members().into_iter().map(|disk| {
        let path = log_path(&disk);
        let bytes = std::fs::read(&path).unwrap();
        (path, bytes)
    }).collect::<Vec<_>>();
    assert_eq!(engine.commit_degraded_recovery(
        &plan.recovery_id, &plan.digest, "recovery-admin", false,
    ).unwrap_err().code(), ErrorCode::PermanentLossNotAcknowledged);
    let installed = engine.original_receipts.take().unwrap();
    let missing = engine.commit_degraded_recovery(
        &plan.recovery_id, &plan.digest, "recovery-admin", true,
    ).unwrap_err();
    assert_eq!(missing.code(), ErrorCode::RecoveryRequired);
    let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(&missing);
    let mut original_missing = false;
    while let Some(source) = cause {
        if let Some(RegistrationError::OriginalReceiptUnavailable(operation)) =
            source.downcast_ref::<RegistrationError>()
        {
            assert!(*operation == initialize.operation_id || *operation == node.publication);
            original_missing = true;
        }
        cause = source.source();
    }
    assert!(original_missing);
    assert!(Arc::ptr_eq(&before, &engine.state));
    for (path, bytes) in logs { assert_eq!(std::fs::read(path).unwrap(), bytes); }
    engine.original_receipts = Some(installed);
    engine.inject_fault_once("after_recovery_finalize");
    let interrupted = engine.commit_degraded_recovery(
        &plan.recovery_id, &plan.digest, "recovery-admin", true,
    ).unwrap_err();
    assert_eq!(interrupted.code(), ErrorCode::InjectedFailure);
    let metadata = std::fs::metadata(&preparation).unwrap();
    assert_eq!((metadata.dev(), metadata.ino()), container_identity);
    assert!(!engine.state().operations.contains_key(&operation));
    drop(engine);

    // The original loss decision survives a cold pending open. It does not
    // turn the missing BODY into a successful payload proof on resumption.
    let mut engine = Engine::open_for_recovery(&root).unwrap();
    engine.inject_fault_once("after_recovery_publication_1");
    let interrupted = engine.commit_degraded_recovery(
        &plan.recovery_id, &plan.digest, "recovery-admin", true,
    ).unwrap_err();
    assert_eq!(interrupted.code(), ErrorCode::InjectedFailure);
    assert!(!preparation.exists());
    drop(engine);

    // A real Publication prefix must already have installed the same inode;
    // cold replay finishes its seals rather than adopting a same-name entry.
    let mut engine = Engine::open(&root).unwrap();
    let recovered = engine.commit_degraded_recovery(
        &plan.recovery_id, &plan.digest, "recovery-admin", true,
    ).unwrap();
    let version = &accepted.versions[0].version;
    let disk = engine.disk(&accepted.versions[0].placement.route_slot_ids[0]).unwrap();
    let scope = crate::engine::fresh_layout::containers::observe_replica(
        &engine.config, &engine.disk_resources, engine.state(),
        version.file_id.parse().unwrap(), &disk,
    ).unwrap();
    let metadata = std::fs::metadata(scope.container().unwrap().path()).unwrap();
    assert_eq!((metadata.dev(), metadata.ino()), container_identity);
    assert!(!scope.body_path(version).unwrap().exists());
    assert_eq!(recovered.current_health.as_str(), "UNAVAILABLE");
    drop(scope);
    let installed = engine.original_receipts.as_ref().unwrap();
    installed.require_context(&engine.planning_context()).unwrap();
    let actual = installed.replay_cursor(&engine.planning_context()).unwrap();
    assert_eq!(actual.receipt(engine.state(), operation).unwrap().0, &original_result);
    engine.snapshot_receipts().unwrap();
    let published_state = Arc::clone(&engine.state);
    assert_eq!(engine.commit_degraded_recovery(
        &plan.recovery_id, &plan.digest, "recovery-admin", true,
    ).unwrap(), recovered);
    assert!(Arc::ptr_eq(&published_state, &engine.state));
    assert_eq!(recovered.commit_state, storage_contract::CommitState::PublishedDegraded);
    assert_ne!(recovered, original_result);
    let (candidate, publication, seal, original) = observed(&mut engine, operation);
    assert!(publication.payload_receipts.is_empty());
    let projection = original.project_original_receipt(
        &candidate, &publication, &seal, engine.state(),
    ).unwrap();
    // The pre-crash cursor held a different open resource occurrence. Read
    // the original proof through this actual cold installation, then verify
    // exact reobservation does not replace or duplicate its retained entry.
    assert!(matches!(
        cursor.retain(projection, &engine.config, &engine.disk_resources),
        Err(Error::OriginalReceipt(OriginalReceiptError::Prefix)),
    ));
    cursor = actual;
    let reobserved = original.project_original_receipt(
        &candidate, &publication, &seal, engine.state(),
    ).unwrap();
    cursor.retain_completed(reobserved, &engine.config, &engine.disk_resources).unwrap();
    cursor.require_prefix(&visible).unwrap();
    ReceiptIndex::capture_with_originals(&visible, &cursor).unwrap();
    assert_eq!(cursor.receipt(engine.state(), operation).unwrap().0, &original_result);
    ReceiptIndex::capture_with_originals(engine.state(), &cursor).unwrap();
    // This assertion compares the actual writer's bytes with the read-side
    // shared overlay, including all legacy outcome/protection keys.
    assert_eq!(canonical_json(&publication.result).unwrap(), canonical_json(&recovered).unwrap());
    assert_eq!(cursor.receipt(engine.state(), operation).unwrap().1.as_str(), "admin");
    let original_bytes = canonical_json(original.share_state().as_ref()).unwrap();
    let journals = engine.members().into_iter().map(|disk| {
        let path = log_path(&disk);
        let bytes = std::fs::read(&path).unwrap();
        (path, bytes)
    }).collect::<Vec<_>>();
    let mut lost_overlay = engine.state().clone();
    lost_overlay.operations.insert(operation, original_result.clone());
    assert_visible_rejected(&original, &candidate, &publication, &seal, &lost_overlay);
    assert!(matches!(cursor.receipt(&lost_overlay, operation),
        Err(Error::OriginalReceipt(OriginalReceiptError::Prefix))));
    assert!(matches!(ReceiptIndex::capture_with_originals(&lost_overlay, &cursor),
        Err(RegistrationError::OriginalReceipt(
            Error::OriginalReceipt(OriginalReceiptError::Prefix),
        ))));
    assert_eq!(canonical_json(original.share_state().as_ref()).unwrap(), original_bytes);
    for (path, bytes) in journals {
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
    drop(original);
    drop(engine);

    let mut engine = Engine::open(&root).unwrap();
    let actual = engine.original_receipts.as_ref().unwrap()
        .replay_cursor(&engine.planning_context()).unwrap();
    assert_eq!(actual.receipt(engine.state(), operation).unwrap().0, &original_result);
    engine.snapshot_receipts().unwrap();
    assert_eq!(engine.operation(operation).unwrap(), recovered);
    assert_eq!(cursor.receipt(engine.state(), operation).unwrap().0, &original_result);
    assert_eq!(cursor.receipt(engine.state(), creation).unwrap().0.operation_id, creation);
    ReceiptIndex::capture_with_originals(engine.state(), &cursor).unwrap();
    assert_eq!(engine.commit_degraded_recovery(
        &plan.recovery_id, &plan.digest, "recovery-admin", true,
    ).unwrap(), recovered);
    let (candidate, publication, seal, original) = observed(&mut engine, operation);
    assert_eq!(original.project_original_receipt(
        &candidate, &publication, &seal, engine.state(),
    ).unwrap().in_prefix(engine.state()).unwrap().0, &original_result);
}

#[test]
fn actual_cold_directory_originals_pin_live_retries_and_refuse_missing_source() {
    use name_policy::Name;
    use std::path::{Path, PathBuf};
    use storage_contract::snapshot_worktree::{CreateWorktree, InitializeSnapshot};

    fn inventory(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
        fn visit(base: &Path, path: &Path, out: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
            for entry in std::fs::read_dir(path).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                let relative = path.strip_prefix(base).unwrap().to_path_buf();
                if entry.file_type().unwrap().is_dir() {
                    out.insert(relative, None);
                    visit(base, &path, out);
                } else {
                    out.insert(relative, Some(std::fs::read(path).unwrap()));
                }
            }
        }
        let mut result = BTreeMap::new();
        visit(root, root, &mut result);
        result
    }

    let temporary = tempfile::tempdir().unwrap();
    let system = temporary.path().join("system");
    let primary = temporary.path().join("primary");
    let mut engine = Engine::initialize(
        &system, Config::development(crate::test_disks([primary.clone()])).unwrap(),
    ).unwrap();
    let repository = engine.create_disk_root(
        "P01", "admin", OperationId::generate().unwrap(),
    ).unwrap();
    // The ordinary registered-root Complete already installed the same
    // authenticated source. No cold-only or test-supplied cursor bootstraps it.
    engine.snapshot_receipts().unwrap();
    let root = engine.snapshot_root_context(repository.repository_id.parse().unwrap()).unwrap();
    let principal = "admin".parse().unwrap();
    let initialize = InitializeSnapshot {
        host: root.host, source: root.source, operation_id: OperationId::generate().unwrap(),
    };
    let node = engine.initialize_directory_snapshot(&initialize, &principal).unwrap();
    let create = CreateWorktree {
        source: node, parent_directory_id: initialize.host.directory_id,
        name: Name::parse("actual-receipt-control").unwrap(),
        expected_root_generation: initialize.host.expected_root_generation,
        operation_id: OperationId::generate().unwrap(),
    };
    let worktree = engine.create_directory_worktree(&create, &principal).unwrap();
    let saved = engine.state().operations.clone();
    let status = engine.directory_worktree_status(worktree).unwrap();
    let hot_source = Arc::clone(engine.original_receipts.as_ref().unwrap());
    hot_source.require_context(&engine.planning_context()).unwrap();
    engine.snapshot_receipts().unwrap();
    drop(engine);

    // Startup itself traverses the actual journal/barrier/admitted snapshots
    // and installs the original prefix. The test does not supply a projection
    // or install a cursor to make this positive reach the new consumer.
    let mut engine = Engine::open(&system).unwrap();
    let installed = Arc::clone(engine.original_receipts.as_ref().unwrap());
    assert!(!Arc::ptr_eq(&hot_source, &installed));
    assert!(hot_source.require_context(&engine.planning_context()).is_err());
    engine.snapshot_receipts().unwrap();
    assert_eq!(engine.state().operations, saved);
    assert_eq!(engine.directory_worktree_status(worktree).unwrap(), status);
    let source = engine.state().clone();
    let cursor = installed.replay_cursor(&engine.planning_context()).unwrap();
    ReceiptIndex::capture_with_originals(&source, &cursor).unwrap();
    cursor.require_directory_index(&source).unwrap();
    let media = (inventory(&system), inventory(&primary));

    // Loss of one real original is a reduction of evidence, never a fabricated
    // successful receipt. Global indexing refuses it before a scoped lookup.
    let mut missing = installed.replay_cursor(&engine.planning_context()).unwrap();
    assert!(missing.records.remove(&node.publication).is_some());
    assert!(matches!(ReceiptIndex::capture_with_originals(&source, &missing),
        Err(RegistrationError::OriginalReceiptUnavailable(operation))
            if operation == node.publication));
    assert_eq!(missing.require_directory_index(&source).unwrap_err().code(),
        ErrorCode::RecoveryRequired);
    let retained = engine.original_receipts.take().unwrap();
    assert!(matches!(engine.snapshot_receipts(), Err(RegistrationError::Source)));
    engine.original_receipts = Some(retained);

    // No State-only path may turn a changed visible actor, digest, or kind
    // into another original. Full event validation still precedes lookup.
    let mut wrong_actor = source.clone();
    wrong_actor.operation_principals.insert(initialize.operation_id, "other-actor".into());
    assert!(matches!(ReceiptIndex::capture_with_originals(&wrong_actor, &cursor),
        Err(RegistrationError::OriginalReceipt(
            Error::OriginalReceipt(OriginalReceiptError::Prefix),
        ))));
    let mut wrong_digest = source.clone();
    wrong_digest.operations.get_mut(&initialize.operation_id).unwrap().request_digest =
        source.operations[&create.operation_id].request_digest.clone();
    assert!(matches!(ReceiptIndex::capture_with_originals(&wrong_digest, &cursor),
        Err(RegistrationError::OriginalReceipt(
            Error::OriginalReceipt(OriginalReceiptError::Prefix),
        ))));
    let mut wrong_kind = source.clone();
    wrong_kind.operations.get_mut(&node.publication).unwrap().kind = "not-indexed".into();
    assert!(matches!(ReceiptIndex::capture_with_originals(&wrong_kind, &cursor),
        Err(RegistrationError::OriginalReceipt(
            Error::OriginalReceipt(OriginalReceiptError::Prefix),
        ))));
    let mut repeated = source.clone();
    let event = source.events.iter().find(|event| {
        event.operation_id == initialize.operation_id
    }).unwrap().clone();
    repeated.events.push(event);
    assert!(matches!(ReceiptIndex::capture_with_originals(&repeated, &cursor),
        Err(RegistrationError::Event(operation)) if operation == initialize.operation_id));
    assert_eq!(engine.state(), &source);
    assert_eq!((inventory(&system), inventory(&primary)), media);

    let published = Arc::clone(&engine.state);
    let slot = engine.next_slot;
    let snapshot = engine.snapshot.clone();
    let media = (inventory(&system), inventory(&primary));
    for _ in 0..2 {
        assert_eq!(engine.initialize_directory_snapshot(&initialize, &principal).unwrap(), node);
        assert_eq!(engine.create_directory_worktree(&create, &principal).unwrap(), worktree);
        assert_eq!(engine.directory_worktree_status(worktree).unwrap(), status);
    }
    assert!(Arc::ptr_eq(&published, &engine.state));
    assert_eq!(engine.next_slot, slot);
    assert_eq!(engine.snapshot, snapshot);
    assert_eq!(engine.state().operations, saved);
    assert_eq!((inventory(&system), inventory(&primary)), media);

    let actual = Arc::clone(&engine.state);
    engine.state = Arc::new(engine.state().clone());
    assert!(matches!(engine.snapshot_receipts(), Err(RegistrationError::OriginalReceipt(
        Error::OriginalReceipt(OriginalReceiptError::Prefix),
    ))));
    engine.state = actual;
    let actual_snapshot = engine.snapshot.take();
    assert!(matches!(engine.snapshot_receipts(), Err(RegistrationError::OriginalReceipt(
        Error::OriginalReceipt(OriginalReceiptError::Prefix),
    ))));
    engine.snapshot = actual_snapshot;
    engine.snapshot_receipts().unwrap();
    assert_eq!((inventory(&system), inventory(&primary)), media);

    // An unrelated real publication carries, but does not manufacture, the
    // witnessed directory originals without rereading unrelated snapshots.
    // Later indexed completions add only their own real published projection.
    let second = OperationId::generate().unwrap();
    let name = storage_contract::RepositoryName::parse("receipt-unrelated-repository").unwrap();
    #[cfg(feature = "fault-injection")]
    {
        let before = Arc::clone(&engine.state);
        let slot = engine.next_slot;
        let snapshot = engine.snapshot.clone();
        engine.inject_fault_once("after_payloads");
        assert_eq!(engine.create_repository(&name, "P01", 1, "admin", second)
            .unwrap_err().code(), ErrorCode::InjectedFailure);
        assert!(Arc::ptr_eq(&before, &engine.state));
        assert_eq!(engine.next_slot, slot);
        assert_eq!(engine.snapshot, snapshot);
        assert!(Arc::ptr_eq(engine.original_receipts.as_ref().unwrap(), &installed));
        engine.snapshot_receipts().unwrap();
    }
    let repository = engine.create_repository(&name, "P01", 1, "admin", second).unwrap();
    let successor = Arc::clone(engine.original_receipts.as_ref().unwrap());
    assert!(!Arc::ptr_eq(&successor, &installed));
    assert!(matches!(installed.require_context(&engine.planning_context()),
        Err(Error::OriginalReceipt(OriginalReceiptError::Prefix))));
    successor.require_context(&engine.planning_context()).unwrap();
    engine.snapshot_receipts().unwrap();
    assert_eq!(engine.operation(worktree.publication).unwrap(), saved[&worktree.publication]);
    let mut second_create = create.clone();
    second_create.operation_id = OperationId::generate().unwrap();
    second_create.name = Name::parse("post-cold-live-completion").unwrap();
    #[cfg(feature = "fault-injection")]
    {
        let before = Arc::clone(&engine.state);
        let source = Arc::clone(engine.original_receipts.as_ref().unwrap());
        let slot = engine.next_slot;
        engine.inject_fault_once("after_payloads");
        assert_eq!(engine.create_directory_worktree(&second_create, &principal)
            .unwrap_err().code(), ErrorCode::InjectedFailure);
        assert!(Arc::ptr_eq(&before, &engine.state));
        assert!(Arc::ptr_eq(&source, engine.original_receipts.as_ref().unwrap()));
        assert_eq!(engine.next_slot, slot);
        assert!(!engine.state().operations.contains_key(&second_create.operation_id));
    }
    let second_worktree = engine.create_directory_worktree(&second_create, &principal).unwrap();
    engine.snapshot_receipts().unwrap();
    assert_eq!(engine.operation(worktree.publication).unwrap(), saved[&worktree.publication]);
    let final_operations = engine.state().operations.clone();
    drop(engine);

    // Reopening reconstructs a new actual source on newly admitted handles.
    // It never reuses the old process's installed proof or visible decoder.
    let mut engine = Engine::open(&system).unwrap();
    assert!(engine.original_receipts.is_some());
    assert!(matches!(successor.require_context(&engine.planning_context()),
        Err(Error::OriginalReceipt(OriginalReceiptError::Prefix))));
    engine.snapshot_receipts().unwrap();
    assert_eq!(engine.state().operations, final_operations);
    let media = (inventory(&system), inventory(&primary));
    let final_state = Arc::clone(&engine.state);
    for _ in 0..2 {
        assert_eq!(engine.create_repository(&name, "P01", 1, "admin", second).unwrap(), repository);
        assert_eq!(engine.create_directory_worktree(&create, &principal).unwrap(), worktree);
        assert_eq!(engine.create_directory_worktree(&second_create, &principal).unwrap(),
            second_worktree);
    }
    assert!(Arc::ptr_eq(&final_state, &engine.state));
    assert_eq!((inventory(&system), inventory(&primary)), media);
}
