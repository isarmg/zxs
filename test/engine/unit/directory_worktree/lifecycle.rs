use super::*;

fn commitment(bytes: &[u8]) -> MaterialCommitment {
    MaterialCommitment::from_digest(blake3::hash(bytes))
}
fn operation(byte: u8) -> OperationId {
    OperationId::from_random([byte; 16])
}
fn mount(name: &str) -> Mount {
    Mount {
        parent: DirectoryId::from_random([4; 16]),
        name: LookupName::parse(name).unwrap(),
    }
}
fn fresh(byte: u8) -> Worktree {
    Worktree::fresh(
        WorktreeId::from_random([byte; 16]),
        SnapshotSource {
            repository: SnapshotRepositoryId::from_random([2; 16]),
            node: SnapshotNodeId::from_random([3; 16]),
            branch: SnapshotBranchId::from_random([5; 16]),
            manifest: commitment(b"original complete snapshot"),
        },
        mount("History editing"),
    )
}
fn deletion(worktree: &mut Worktree, operation: OperationId) -> Publication {
    let DeletePlan::Publish(intent) = worktree.prepare_delete(operation).unwrap() else {
        panic!("unsaved worktree requires file publication");
    };
    let publication = Publication { intent, scope: PublicationScope::FilesOnly };
    worktree.complete_publication(publication).unwrap();
    publication
}
fn prepare(worktree: &mut Worktree, operation: OperationId)
    -> Result<PublicationIntent, LifecycleError>
{
    worktree.prepare_commit(
        operation,
        worktree.source().node,
        SnapshotBranchId::from_random([9; 16]),
    )
}
fn directory(intent: PublicationIntent) -> Publication {
    Publication {
        intent,
        scope: PublicationScope::Directory {
            branch: SnapshotBranchId::from_random([5; 16]),
            node: SnapshotNodeId::from_random([6; 16]),
        },
    }
}

#[test]
fn fresh_same_node_never_contains_edits_from_retained_instance() {
    let mut old = fresh(1);
    old.record_edit(commitment(b"edited plus added files")).unwrap();
    let saved = deletion(&mut old, operation(7));
    let new = fresh(8);
    assert_ne!(old.id(), new.id());
    assert_eq!(old.source(), new.source());
    assert_eq!(new.revision().manifest, new.source().manifest);
    assert_ne!(old.revision().manifest, new.revision().manifest);
    assert_eq!(new.phase(), Phase::Active { saved: None });
    assert_eq!(old.phase(), Phase::Deleted { publication: saved, deletion: operation(7) });
}

#[test]
fn unchanged_delete_keeps_original_mapping_and_forbids_directory_history() {
    let mut worktree = fresh(1);
    let publication = deletion(&mut worktree, operation(7));
    assert_eq!(publication.intent.mode, PublicationMode::FilesBeforeDelete);
    assert_eq!(publication.intent.revision.manifest, worktree.source().manifest);
    assert_eq!(publication.scope, PublicationScope::FilesOnly);
}

#[test]
fn deletion_cannot_publish_directory_history_or_discard_a_pending_snapshot() {
    let mut worktree = fresh(1);
    worktree.record_edit(commitment(b"changed")).unwrap();
    let DeletePlan::Publish(intent) = worktree.prepare_delete(operation(7)).unwrap() else {
        panic!("expected file publication");
    };
    let frozen = worktree.clone();
    assert!(matches!(
        worktree.complete_publication(directory(intent)),
        Err(LifecycleError::PublicationScope),
    ));
    assert!(matches!(worktree.record_edit(commitment(b"later")), Err(LifecycleError::Publishing)));
    assert!(matches!(prepare(&mut worktree, operation(9)), Err(LifecycleError::Publishing)));
    assert_eq!(worktree, frozen);
    assert_eq!(worktree.prepare_delete(operation(7)).unwrap(), DeletePlan::Publish(intent));
    assert!(matches!(worktree.restore(mount("other"), false), Err(LifecycleError::NotDeleted)));
    assert_eq!(worktree, frozen);
}

#[test]
fn explicit_commit_requires_directory_scope_and_exact_original_intent() {
    let mut worktree = fresh(1);
    let intent = prepare(&mut worktree, operation(7)).unwrap();
    let frozen = worktree.clone();
    assert!(matches!(
        worktree.complete_publication(Publication { intent, scope: PublicationScope::FilesOnly }),
        Err(LifecycleError::PublicationScope),
    ));
    let mut wrong = directory(intent);
    wrong.intent.revision.manifest = commitment(b"different complete manifest");
    assert!(matches!(
        worktree.complete_publication(wrong),
        Err(LifecycleError::PublicationMismatch),
    ));
    assert_eq!(worktree, frozen);
    let publication = directory(intent);
    worktree.complete_publication(publication).unwrap();
    let completed = worktree.clone();
    assert_eq!(prepare(&mut worktree, operation(7)).unwrap(), intent);
    worktree.complete_publication(publication).unwrap();
    assert_eq!(worktree, completed);
}

#[test]
fn restore_preserves_edits_and_file_receipt_without_republishing() {
    let mut worktree = fresh(1);
    worktree.record_edit(commitment(b"edited before deletion")).unwrap();
    let publication = deletion(&mut worktree, operation(7));
    let saved_revision = worktree.revision();
    worktree.restore(mount("Recovered editing"), false).unwrap();
    assert_eq!(worktree.revision(), saved_revision);
    assert_eq!(worktree.mount().name.as_str(), "Recovered editing");
    assert_eq!(worktree.phase(), Phase::Active { saved: Some(publication) });
    assert_eq!(worktree.prepare_delete(operation(8)).unwrap(), DeletePlan::Retain);
    assert_eq!(worktree.prepare_delete(operation(8)).unwrap(), DeletePlan::Retain);
    assert_eq!(worktree.phase(), Phase::Deleted { publication, deletion: operation(8) });
}

#[test]
fn conflicting_restore_does_not_overwrite_new_worktree_or_retained_contents() {
    let mut retained = fresh(1);
    retained.record_edit(commitment(b"retained edits")).unwrap();
    deletion(&mut retained, operation(7));
    let new = fresh(8);
    let before = retained.clone();
    assert!(matches!(
        retained.restore(new.mount().clone(), true),
        Err(LifecycleError::NameCollision),
    ));
    assert_eq!(retained, before);
    assert_eq!(new.revision().manifest, new.source().manifest);
    retained.restore(mount("Distinct restored name"), false).unwrap();
    assert_ne!(retained.mount(), new.mount());
    assert_ne!(retained.revision().manifest, new.revision().manifest);
}

#[test]
fn editing_after_restore_requires_new_file_publication_and_preserves_original_source() {
    let mut worktree = fresh(1);
    let previous = deletion(&mut worktree, operation(7));
    worktree.restore(mount("Restored"), false).unwrap();
    worktree.record_edit(commitment(b"second editing session")).unwrap();
    let DeletePlan::Publish(intent) = worktree.prepare_delete(operation(8)).unwrap() else {
        panic!("new edits require a new file publication");
    };
    assert_eq!(intent.source, previous.intent.source);
    assert!(intent.revision.generation > previous.intent.revision.generation);
    assert_ne!(intent.revision.manifest, previous.intent.revision.manifest);
    let frozen = worktree.clone();
    assert!(matches!(
        worktree.complete_publication(previous),
        Err(LifecycleError::PublicationMismatch),
    ));
    assert_eq!(worktree, frozen);
}

#[test]
fn delete_after_normal_commit_retains_existing_nodes_without_another_publication() {
    let mut worktree = fresh(1);
    let intent = prepare(&mut worktree, operation(7)).unwrap();
    let published = directory(intent);
    worktree.complete_publication(published).unwrap();
    assert_eq!(worktree.prepare_delete(operation(8)).unwrap(), DeletePlan::Retain);
    worktree.restore(mount("Restored saved directory"), false).unwrap();
    assert_eq!(worktree.phase(), Phase::Active { saved: Some(published) });
    assert_eq!(worktree.revision(), intent.revision);
}

#[test]
fn same_operation_after_later_edit_does_not_silently_publish_new_bytes() {
    let mut worktree = fresh(1);
    let publication = directory(prepare(&mut worktree, operation(7)).unwrap());
    worktree.complete_publication(publication).unwrap();
    worktree.record_edit(commitment(b"later edit")).unwrap();
    let before = worktree.clone();
    assert!(matches!(
        prepare(&mut worktree, operation(7)),
        Err(LifecycleError::PublicationMismatch),
    ));
    worktree.complete_publication(publication).unwrap();
    assert_eq!(worktree, before);
}

#[test]
fn latest_source_extends_its_branch_but_another_head_requires_an_independent_fork() {
    let mut first = fresh(1);
    let mut second = fresh(2);
    let original = first.source();
    let intent = prepare(&mut first, operation(7)).unwrap();
    assert_eq!(intent.mode, PublicationMode::DirectoryCommit {
        destination: DirectoryDestination::Continue {
            branch: original.branch,
            expected_head: original.node,
        },
    });
    let publication = directory(intent);
    first.complete_publication(publication).unwrap();
    let PublicationScope::Directory { node: advanced, .. } = publication.scope else {
        panic!("directory publication expected");
    };
    let fork = SnapshotBranchId::from_random([10; 16]);
    let after_concurrent_commit = second.prepare_commit(operation(8), advanced, fork).unwrap();
    assert_eq!(after_concurrent_commit.source.node, original.node);
    assert_eq!(after_concurrent_commit.mode, PublicationMode::DirectoryCommit {
        destination: DirectoryDestination::Fork {
            branch: fork,
            expected_source_head: advanced,
        },
    });
    assert!(matches!(
        second.complete_publication(directory(after_concurrent_commit)),
        Err(LifecycleError::PublicationScope),
    ));
    second.complete_publication(Publication {
        intent: after_concurrent_commit,
        scope: PublicationScope::Directory {
            branch: fork,
            node: SnapshotNodeId::from_random([11; 16]),
        },
    }).unwrap();
}

#[test]
fn non_head_source_does_not_reuse_the_source_branch_identity() {
    let mut worktree = fresh(1);
    let before = worktree.clone();
    assert!(matches!(
        worktree.prepare_commit(
            operation(7), SnapshotNodeId::from_random([12; 16]), worktree.source().branch,
        ),
        Err(LifecycleError::ForkIdentity),
    ));
    assert_eq!(worktree, before);
}
