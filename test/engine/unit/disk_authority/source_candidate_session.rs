use super::*;
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};
use storage_contract::OperationId;

fn token(value: &str) -> Token {
    Token::new(value).unwrap()
}
fn identity(name: &str, role: Role) -> Identity {
    Identity {
        origin: Origin {
            pool_id: token("source_pool"),
            disk_id: token(name),
            slot_id: token(name),
            binding_generation: 7,
        },
        incarnation: Digest::body(name.as_bytes()),
        role,
    }
}
fn write(disk: &IndependentDisk, bytes: &[u8]) -> Digest {
    let request = disk
        .review_file(FileWrite {
            operation: OperationId::generate().unwrap(),
            expected_base: disk.report().unwrap().checkpoint,
            history: &token("history"),
            file: &token("file"),
            bytes,
        })
        .unwrap();
    disk.execute(&request).unwrap().publication()
}
fn copy_directory(source: &Path, destination: &Path) {
    fs::create_dir(destination).unwrap();
    fs::set_permissions(destination, fs::Permissions::from_mode(0o700)).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_directory(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn visit(root: &Path, path: &Path, files: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let relative = entry.path().strip_prefix(root).unwrap().to_owned();
            if entry.file_type().unwrap().is_dir() {
                files.insert(relative, None);
                visit(root, &entry.path(), files);
            } else {
                files.insert(relative, Some(fs::read(entry.path()).unwrap()));
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}
fn node<'a>(graph: &'a SourceTopology, identity: &Identity) -> &'a SourceNode {
    graph
        .nodes()
        .find(|node| node.identity() == identity)
        .unwrap()
}
fn observed(session: &SourceCandidateSession) -> &SourceCandidateBatch {
    match session.view() {
        SourceCandidateView::Observed(batch) => batch,
        other => panic!("expected an observed batch, got {other:?}"),
    }
}
fn assert_matches(session: &SourceCandidateSession, matching: usize) {
    assert_eq!(
        observed(session).status(),
        &SourceCandidateBatchStatus::IncompleteObservation {
            matching_candidates: matching
        }
    );
}
struct Pair {
    root: tempfile::TempDir,
    primary: IndependentDisk,
    secondary: IndependentDisk,
    pin: Digest,
}
impl Pair {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let primary = IndependentDisk::initialize(
            &root.path().join("primary"),
            identity("primary", Role::Primary),
        )
        .unwrap();
        let secondary = IndependentDisk::initialize(
            &root.path().join("secondary"),
            identity("secondary", Role::Secondary),
        )
        .unwrap();
        let pin = write(&primary, b"actual published source");
        let request = secondary
            .review_backup(
                OperationId::generate().unwrap(),
                None,
                &[DiskSelection {
                    path: root.path().join("primary"),
                    identity: primary.identity().clone(),
                    known_checkpoint: Some(pin),
                }],
            )
            .unwrap();
        secondary.execute(&request).unwrap();
        Self {
            root,
            primary,
            secondary,
            pin,
        }
    }
    fn copy(&self, name: &str) -> IndependentDisk {
        let path = self.root.path().join(name);
        copy_directory(&self.root.path().join("primary"), &path);
        IndependentDisk::open(&DiskSelection {
            path,
            identity: self.primary.identity().clone(),
            known_checkpoint: Some(self.pin),
        })
        .unwrap()
    }
    fn broken_copy(&self) -> (IndependentDisk, Digest) {
        let copy = self.copy("broken");
        let path = self.root.path().join("broken");
        let verified = Disk::open(&path, copy.identity())
            .unwrap()
            .verify(None)
            .unwrap();
        let manifest = verified.publications[&self.pin].manifest;
        fs::remove_file(path.join("objects").join(format!("{manifest}.json"))).unwrap();
        (copy, manifest)
    }
}

#[test]
fn pending_observed_and_invalidated_states_keep_real_read_only_facts() {
    let pair = Pair::new();
    let graph = pair.secondary.source_topology().unwrap();
    let expected = node(&graph, pair.primary.identity());
    let before = snapshot(pair.root.path());
    let mut session = SourceCandidateSession::new();
    assert!(matches!(session.view(), SourceCandidateView::NotRequested));
    let request = session.begin(expected);
    assert!(
        matches!(session.view(), SourceCandidateView::Pending(node) if graph.contains_node(node) && node.identity() == expected.identity())
    );
    assert_eq!(
        session.receive(request.observe(&[&pair.primary])),
        SourceCandidateDelivery::Updated
    );
    assert_matches(&session, 1);
    assert!(observed(&session).is_for(expected));
    session.invalidate();
    assert!(matches!(session.view(), SourceCandidateView::NotRequested));
    assert_eq!(snapshot(pair.root.path()), before);
}

#[test]
fn starting_again_clears_an_observation_before_new_verification() {
    let pair = Pair::new();
    let graph = pair.secondary.source_topology().unwrap();
    let expected = node(&graph, pair.primary.identity());
    let mut session = SourceCandidateSession::default();
    let first = session.begin(expected).observe(&[&pair.primary]);
    session.receive(first);
    assert_matches(&session, 1);
    let next = session.begin(expected);
    assert!(matches!(session.view(), SourceCandidateView::Pending(_)));
    session.receive(next.observe(&[]));
    assert_eq!(
        observed(&session).status(),
        &SourceCandidateBatchStatus::NotObserved
    );
}

#[test]
fn older_success_cannot_overwrite_newer_success_for_the_same_node() {
    let pair = Pair::new();
    let graph = pair.secondary.source_topology().unwrap();
    let expected = node(&graph, pair.primary.identity());
    let mut session = SourceCandidateSession::new();
    let older = session.begin(expected).observe(&[&pair.primary]);
    let newer = session.begin(expected).observe(&[&pair.secondary]);
    assert_eq!(session.receive(newer), SourceCandidateDelivery::Updated);
    assert_matches(&session, 0);
    assert_eq!(
        session.receive(older),
        SourceCandidateDelivery::DiscardedStale
    );
    assert_matches(&session, 0);
    assert_eq!(
        observed(&session).observations()[0].status(),
        &SourceCandidateStatus::IdentityMismatch
    );
}

#[test]
fn an_older_failure_cannot_clear_a_newer_success() {
    let pair = Pair::new();
    let (broken, missing) = pair.broken_copy();
    let graph = pair.secondary.source_topology().unwrap();
    let expected = node(&graph, pair.primary.identity());
    assert!(
        matches!(observe_source_candidates(expected, &[&broken]), Err(SourceCandidateBatchError::Candidate { source: Error::Missing(hash), .. }) if hash == missing)
    );
    let mut session = SourceCandidateSession::new();
    let older = session.begin(expected).observe(&[&broken]);
    let newer = session.begin(expected).observe(&[&pair.primary]);
    session.receive(newer);
    assert_eq!(
        session.receive(older),
        SourceCandidateDelivery::DiscardedStale
    );
    assert_matches(&session, 1);
}

#[test]
fn older_success_cannot_replace_the_current_failed_result() {
    let pair = Pair::new();
    let (broken, missing) = pair.broken_copy();
    let graph = pair.secondary.source_topology().unwrap();
    let expected = node(&graph, pair.primary.identity());
    let mut session = SourceCandidateSession::new();
    let older = session.begin(expected).observe(&[&pair.primary]);
    let current = session.begin(expected).observe(&[&broken]);
    assert_eq!(session.receive(current), SourceCandidateDelivery::Updated);
    assert_eq!(
        session.receive(older),
        SourceCandidateDelivery::DiscardedStale
    );
    assert!(
        matches!(session.view(), SourceCandidateView::Failed { error: SourceCandidateBatchError::Candidate { source: Error::Missing(hash), .. }, .. } if *hash == missing)
    );
}

#[test]
fn older_failure_does_not_end_a_newer_pending_request() {
    let pair = Pair::new();
    let (broken, missing) = pair.broken_copy();
    let graph = pair.secondary.source_topology().unwrap();
    let expected = node(&graph, pair.primary.identity());
    assert!(
        matches!(observe_source_candidates(expected, &[&broken]), Err(SourceCandidateBatchError::Candidate { source: Error::Missing(hash), .. }) if hash == missing)
    );
    let mut session = SourceCandidateSession::new();
    let older = session.begin(expected).observe(&[&broken]);
    let pending = session.begin(expected);
    assert_eq!(
        session.receive(older),
        SourceCandidateDelivery::DiscardedStale
    );
    assert!(
        matches!(session.view(), SourceCandidateView::Pending(node) if graph.contains_node(node) && node.identity() == expected.identity())
    );
    assert_eq!(
        session.receive(pending.observe(&[&pair.primary])),
        SourceCandidateDelivery::Updated
    );
    assert_matches(&session, 1);
}

#[test]
fn late_success_after_invalidation_cannot_resurrect_a_match() {
    let pair = Pair::new();
    let graph = pair.secondary.source_topology().unwrap();
    let expected = node(&graph, pair.primary.identity());
    let mut session = SourceCandidateSession::new();
    let response = session.begin(expected).observe(&[&pair.primary]);
    session.invalidate();
    assert_eq!(
        session.receive(response),
        SourceCandidateDelivery::DiscardedStale
    );
    assert!(matches!(session.view(), SourceCandidateView::NotRequested));
}

#[test]
fn work_after_invalidation_stays_read_only_and_cannot_reopen_the_view() {
    let pair = Pair::new();
    let graph = pair.secondary.source_topology().unwrap();
    let expected = node(&graph, pair.primary.identity());
    let before = snapshot(pair.root.path());
    let mut session = SourceCandidateSession::new();
    let request = session.begin(expected);
    session.invalidate();
    // Invalidation is delivery fencing, not a promise to cancel filesystem I/O.
    let response = request.observe(&[&pair.primary]);
    assert_eq!(
        session.receive(response),
        SourceCandidateDelivery::DiscardedStale
    );
    assert!(matches!(session.view(), SourceCandidateView::NotRequested));
    assert_eq!(snapshot(pair.root.path()), before);
}

#[test]
fn switching_source_nodes_rejects_old_success_without_clearing_pending_work() {
    let pair = Pair::new();
    let graph = pair.secondary.source_topology().unwrap();
    let primary = node(&graph, pair.primary.identity());
    let secondary = node(&graph, pair.secondary.identity());
    let mut session = SourceCandidateSession::new();
    let older = session.begin(primary).observe(&[&pair.primary]);
    let current = session.begin(secondary);
    assert_eq!(
        session.receive(older),
        SourceCandidateDelivery::DiscardedStale
    );
    assert!(
        matches!(session.view(), SourceCandidateView::Pending(node) if node.identity() == secondary.identity())
    );
    session.receive(current.observe(&[&pair.secondary]));
    assert!(observed(&session).is_for(secondary));
    assert!(!observed(&session).is_for(primary));
}

#[test]
fn an_equal_logical_recapture_is_not_the_same_pending_request() {
    let pair = Pair::new();
    let first = pair.secondary.source_topology().unwrap();
    let second = pair.secondary.source_topology().unwrap();
    assert_eq!(first.binding(), second.binding());
    let old_node = node(&first, pair.primary.identity());
    let new_node = node(&second, pair.primary.identity());
    let mut session = SourceCandidateSession::new();
    let older = session.begin(old_node).observe(&[&pair.primary]);
    let current = session.begin(new_node);
    assert_eq!(
        session.receive(older),
        SourceCandidateDelivery::DiscardedStale
    );
    assert!(
        matches!(session.view(), SourceCandidateView::Pending(node) if second.contains_node(node) && !first.contains_node(node))
    );
    session.receive(current.observe(&[&pair.primary]));
    assert!(observed(&session).is_for(new_node));
    assert!(!observed(&session).is_for(old_node));
}

#[test]
fn another_owner_with_the_same_source_cannot_receive_this_response() {
    let pair = Pair::new();
    let graph = pair.secondary.source_topology().unwrap();
    let expected = node(&graph, pair.primary.identity());
    let mut first = SourceCandidateSession::new();
    let mut second = SourceCandidateSession::new();
    let foreign = first.begin(expected).observe(&[&pair.primary]);
    let own = second.begin(expected);
    assert_eq!(
        second.receive(foreign),
        SourceCandidateDelivery::DiscardedStale
    );
    assert!(matches!(second.view(), SourceCandidateView::Pending(_)));
    assert_eq!(
        second.receive(own.observe(&[&pair.primary])),
        SourceCandidateDelivery::Updated
    );
    assert_matches(&second, 1);
}

#[test]
fn closing_and_reopening_an_owner_cannot_revive_an_old_request() {
    let pair = Pair::new();
    let graph = pair.secondary.source_topology().unwrap();
    let expected = node(&graph, pair.primary.identity());
    let older = {
        let mut closed = SourceCandidateSession::new();
        closed.begin(expected).observe(&[&pair.primary])
    };
    let mut reopened = SourceCandidateSession::new();
    let current = reopened.begin(expected);
    assert_eq!(
        reopened.receive(older),
        SourceCandidateDelivery::DiscardedStale
    );
    assert!(matches!(reopened.view(), SourceCandidateView::Pending(_)));
    reopened.receive(current.observe(&[&pair.primary]));
    assert_matches(&reopened, 1);
}

#[test]
fn a_current_failure_keeps_its_exact_source_error_and_supports_a_fresh_retry() {
    let pair = Pair::new();
    let (broken, missing) = pair.broken_copy();
    let graph = pair.secondary.source_topology().unwrap();
    let expected = node(&graph, pair.primary.identity());
    let before = snapshot(pair.root.path());
    let mut session = SourceCandidateSession::new();
    let failed = session.begin(expected).observe(&[&broken]);
    assert_eq!(session.receive(failed), SourceCandidateDelivery::Updated);
    assert!(
        matches!(session.view(), SourceCandidateView::Failed { node, error: SourceCandidateBatchError::Candidate { index: 0, stage: SourceCandidateBatchStage::Capture, source: Error::Missing(hash) } } if graph.contains_node(node) && *hash == missing)
    );
    let retry = session.begin(expected);
    assert!(matches!(session.view(), SourceCandidateView::Pending(_)));
    session.receive(retry.observe(&[&pair.primary]));
    assert_matches(&session, 1);
    assert_eq!(snapshot(pair.root.path()), before);
}

#[test]
fn two_real_matching_copies_remain_ambiguous_after_delivery() {
    let pair = Pair::new();
    let copy = pair.copy("copy");
    let graph = pair.secondary.source_topology().unwrap();
    let expected = node(&graph, pair.primary.identity());
    let before = snapshot(pair.root.path());
    let mut session = SourceCandidateSession::new();
    let response = session.begin(expected).observe(&[&pair.primary, &copy]);
    session.receive(response);
    assert_eq!(
        observed(&session).status(),
        &SourceCandidateBatchStatus::Ambiguous {
            matching_candidates: 2
        }
    );
    assert_eq!(observed(&session).physical_candidates(), 2);
    assert_eq!(snapshot(pair.root.path()), before);
}

#[test]
fn empty_input_and_unpinned_nodes_keep_their_original_non_matching_states() {
    let temp = tempfile::tempdir().unwrap();
    let disk = IndependentDisk::initialize(
        &temp.path().join("empty"),
        identity("empty", Role::Tertiary),
    )
    .unwrap();
    let graph = disk.source_topology().unwrap();
    let expected = node(&graph, disk.identity());
    let mut session = SourceCandidateSession::new();
    let empty = session.begin(expected).observe(&[]);
    session.receive(empty);
    assert_eq!(
        observed(&session).status(),
        &SourceCandidateBatchStatus::NotObserved
    );
    let unpinned = session.begin(expected).observe(&[&disk]);
    session.receive(unpinned);
    assert_matches(&session, 0);
    assert_eq!(
        observed(&session).observations()[0].status(),
        &SourceCandidateStatus::UnpinnedNode
    );
    assert_eq!(observed(&session).node().identity().role, Role::Tertiary);
}

#[test]
fn threaded_reverse_delivery_uses_real_results_without_sleep_or_fabricated_batches() {
    let pair = Pair::new();
    let graph = pair.secondary.source_topology().unwrap();
    let expected = node(&graph, pair.primary.identity());
    let before = snapshot(pair.root.path());
    let mut session = SourceCandidateSession::new();
    let older = session.begin(expected);
    std::thread::scope(|scope| {
        let (ready, observed) = std::sync::mpsc::sync_channel(0);
        let (release, wait) = std::sync::mpsc::sync_channel(0);
        let primary = &pair.primary;
        let worker = scope.spawn(move || {
            let response = older.observe(&[primary]);
            ready.send(()).unwrap();
            wait.recv().unwrap();
            response
        });
        observed.recv().unwrap();
        let current = session.begin(expected).observe(&[&pair.secondary]);
        session.receive(current);
        assert_matches(&session, 0);
        release.send(()).unwrap();
        assert_eq!(
            session.receive(worker.join().unwrap()),
            SourceCandidateDelivery::DiscardedStale
        );
        assert_matches(&session, 0);
    });
    assert_eq!(snapshot(pair.root.path()), before);
}

#[test]
fn candidate_budget_failure_is_an_error_not_a_partial_display_result() {
    let pair = Pair::new();
    let graph = pair.secondary.source_topology().unwrap();
    let expected = node(&graph, pair.primary.identity());
    let mut session = SourceCandidateSession::new();
    let valid = session.begin(expected).observe(&[&pair.primary]);
    session.receive(valid);
    assert_matches(&session, 1);
    let too_many = vec![&pair.primary; 129];
    let response = session.begin(expected).observe(&too_many);
    assert_eq!(session.receive(response), SourceCandidateDelivery::Updated);
    assert!(matches!(
        session.view(),
        SourceCandidateView::Failed {
            error: SourceCandidateBatchError::CandidateBudget {
                actual: 129,
                maximum: 128
            },
            ..
        }
    ));
    session.invalidate();
    assert!(matches!(session.view(), SourceCandidateView::NotRequested));
}

#[test]
fn delivery_never_reopens_or_repairs_a_directory_removed_after_capture() {
    let pair = Pair::new();
    let graph = pair.secondary.source_topology().unwrap();
    let expected = node(&graph, pair.primary.identity());
    let mut session = SourceCandidateSession::new();
    let historical = session.begin(expected).observe(&[&pair.primary]);
    fs::rename(
        pair.root.path().join("primary"),
        pair.root.path().join("removed_from_selection"),
    )
    .unwrap();
    let before = snapshot(pair.root.path());
    // Delivery owns no directory handle and makes no current-presence claim.
    // An actual mount/session event adapter must explicitly invalidate the owner.
    assert_eq!(
        session.receive(historical),
        SourceCandidateDelivery::Updated
    );
    assert_matches(&session, 1);
    assert_eq!(snapshot(pair.root.path()), before);
}
