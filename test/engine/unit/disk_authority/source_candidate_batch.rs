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
fn identity(name: &str) -> Identity {
    Identity {
        origin: Origin {
            pool_id: token("original_pool"),
            disk_id: token(name),
            slot_id: token(name),
            binding_generation: 11,
        },
        incarnation: Digest::body(name.as_bytes()),
        role: Role::Primary,
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
fn open(path: &Path, identity: &Identity, pin: Option<Digest>) -> IndependentDisk {
    IndependentDisk::open(&DiskSelection {
        path: path.to_owned(),
        identity: identity.clone(),
        known_checkpoint: pin,
    })
    .unwrap()
}
fn node<'a>(graph: &'a SourceTopology, identity: &Identity) -> &'a SourceNode {
    graph
        .nodes()
        .find(|node| node.identity() == identity)
        .unwrap()
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
    let mut files = BTreeMap::new();
    visit(root, root, &mut files);
    files
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
        let primary =
            IndependentDisk::initialize(&root.path().join("primary"), identity("primary")).unwrap();
        let mut backup_id = identity("secondary");
        backup_id.role = Role::Secondary;
        let secondary =
            IndependentDisk::initialize(&root.path().join("secondary"), backup_id).unwrap();
        let pin = write(&primary, b"published source");
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
    fn clone_primary(&self, name: &str) -> IndependentDisk {
        let destination = self.root.path().join(name);
        copy_directory(&self.root.path().join("primary"), &destination);
        open(&destination, self.primary.identity(), Some(self.pin))
    }
}

#[test]
fn zero_or_one_matching_selection_never_means_unique_or_globally_absent() {
    let pair = Pair::new();
    let graph = pair.secondary.source_topology().unwrap();
    let expected = node(&graph, pair.primary.identity());
    let other =
        IndependentDisk::initialize(&pair.root.path().join("other"), identity("other")).unwrap();
    let before = snapshot(pair.root.path());
    let none = observe_source_candidates(expected, &[]).unwrap();
    assert_eq!(none.status(), &SourceCandidateBatchStatus::NotObserved);
    assert!(none.observations().is_empty());
    assert_eq!(none.physical_candidates(), 0);
    let no_match = observe_source_candidates(expected, &[&other]).unwrap();
    assert_eq!(
        no_match.status(),
        &SourceCandidateBatchStatus::IncompleteObservation {
            matching_candidates: 0
        }
    );
    assert_eq!(
        no_match.observations()[0].status(),
        &SourceCandidateStatus::IdentityMismatch
    );
    let one = observe_source_candidates(expected, &[&pair.primary]).unwrap();
    assert_eq!(
        one.status(),
        &SourceCandidateBatchStatus::IncompleteObservation {
            matching_candidates: 1
        }
    );
    assert_eq!(one.physical_candidates(), 1);
    assert_eq!(snapshot(pair.root.path()), before);
}

#[test]
fn distinct_same_identity_copies_are_ambiguous_regardless_of_input_order() {
    let pair = Pair::new();
    let copy = pair.clone_primary("copy");
    let graph = pair.secondary.source_topology().unwrap();
    let expected = node(&graph, pair.primary.identity());
    let before = snapshot(pair.root.path());
    for candidates in [[&pair.primary, &copy], [&copy, &pair.primary]] {
        let result = observe_source_candidates(expected, &candidates).unwrap();
        assert_eq!(
            result.status(),
            &SourceCandidateBatchStatus::Ambiguous {
                matching_candidates: 2
            }
        );
        assert_eq!(result.physical_candidates(), 2);
        assert_eq!(result.observations().len(), 2);
        assert!(
            result
                .observations()
                .iter()
                .all(|row| row.status() == &SourceCandidateStatus::SatisfiesPinnedCheckpoints)
        );
    }
    assert_eq!(snapshot(pair.root.path()), before);
}

#[test]
fn repeated_handles_share_one_physical_candidate_and_keep_every_observation() {
    let pair = Pair::new();
    let again = open(
        &pair.root.path().join("primary"),
        pair.primary.identity(),
        Some(pair.pin),
    );
    let graph = pair.secondary.source_topology().unwrap();
    let expected = node(&graph, pair.primary.identity());
    let result =
        observe_source_candidates(expected, &[&pair.primary, &again, &pair.primary]).unwrap();
    assert_eq!(
        result.status(),
        &SourceCandidateBatchStatus::IncompleteObservation {
            matching_candidates: 1
        }
    );
    assert_eq!(result.physical_candidates(), 1);
    assert_eq!(result.observations().len(), 3);
    assert!(result.observations().iter().all(|row| row.is_for(expected)));
}

#[test]
fn alias_inputs_do_not_hide_a_distinct_matching_copy() {
    let pair = Pair::new();
    let copy = pair.clone_primary("copy");
    let graph = pair.secondary.source_topology().unwrap();
    let expected = node(&graph, pair.primary.identity());
    let result =
        observe_source_candidates(expected, &[&pair.primary, &pair.primary, &copy]).unwrap();
    assert_eq!(
        result.status(),
        &SourceCandidateBatchStatus::Ambiguous {
            matching_candidates: 2
        }
    );
    assert_eq!(result.physical_candidates(), 2);
    assert_eq!(result.observations().len(), 3);
}

#[test]
fn rejection_reasons_remain_visible_beside_an_exact_match() {
    let pair = Pair::new();
    let fork = IndependentDisk::initialize(
        &pair.root.path().join("fork"),
        pair.primary.identity().clone(),
    )
    .unwrap();
    write(&fork, b"different fork");
    let graph = pair.secondary.source_topology().unwrap();
    let expected = node(&graph, pair.primary.identity());
    let result =
        observe_source_candidates(expected, &[&fork, &pair.primary, &pair.secondary]).unwrap();
    assert_eq!(
        result.status(),
        &SourceCandidateBatchStatus::IncompleteObservation {
            matching_candidates: 1
        }
    );
    assert_eq!(result.physical_candidates(), 3);
    assert!(
        matches!(result.observations()[0].status(), SourceCandidateStatus::MissingPinnedCheckpoints(missing) if missing.contains(&pair.pin))
    );
    assert_eq!(
        result.observations()[1].status(),
        &SourceCandidateStatus::SatisfiesPinnedCheckpoints
    );
    assert_eq!(
        result.observations()[2].status(),
        &SourceCandidateStatus::IdentityMismatch
    );
}

#[test]
fn one_failed_selected_candidate_rejects_the_batch_even_after_a_match() {
    let pair = Pair::new();
    let other_path = pair.root.path().join("other");
    let other = IndependentDisk::initialize(&other_path, identity("other")).unwrap();
    let pin = write(&other, b"other data");
    let verified = Disk::open(&other_path, other.identity())
        .unwrap()
        .verify(None)
        .unwrap();
    let manifest = verified.publications[&pin].manifest;
    fs::remove_file(other_path.join("objects").join(format!("{manifest}.json"))).unwrap();
    let graph = pair.secondary.source_topology().unwrap();
    let expected = node(&graph, pair.primary.identity());
    let before = snapshot(pair.root.path());
    assert!(matches!(
        observe_source_candidates(expected, &[&pair.primary, &other]),
        Err(SourceCandidateBatchError::Candidate { index: 1, stage: SourceCandidateBatchStage::Capture, source: Error::Missing(hash) }) if hash == manifest
    ));
    assert_eq!(snapshot(pair.root.path()), before);
}

#[test]
fn every_alias_keeps_its_original_known_checkpoint_requirement() {
    let pair = Pair::new();
    let graph = pair.secondary.source_topology().unwrap();
    let expected = node(&graph, pair.primary.identity());
    let newer = write(&pair.primary, b"newer known head");
    let pinned = open(
        &pair.root.path().join("primary"),
        pair.primary.identity(),
        Some(newer),
    );
    fs::remove_file(
        pair.root
            .path()
            .join("primary/publications/00000000000000000002.json"),
    )
    .unwrap();
    assert!(matches!(
        observe_source_candidates(expected, &[&pair.primary, &pinned]),
        Err(SourceCandidateBatchError::Candidate {
            index: 1,
            stage: SourceCandidateBatchStage::Capture,
            source: Error::StaleBase
        })
    ));
}

#[test]
fn replaced_held_candidate_root_is_not_adopted_or_repaired() {
    let pair = Pair::new();
    let graph = pair.secondary.source_topology().unwrap();
    let expected = node(&graph, pair.primary.identity());
    fs::rename(
        pair.root.path().join("primary"),
        pair.root.path().join("moved"),
    )
    .unwrap();
    IndependentDisk::initialize(
        &pair.root.path().join("primary"),
        pair.primary.identity().clone(),
    )
    .unwrap();
    let before = snapshot(pair.root.path());
    assert!(matches!(
        observe_source_candidates(expected, &[&pair.primary]),
        Err(SourceCandidateBatchError::Candidate {
            index: 0,
            stage: SourceCandidateBatchStage::Capture,
            source: Error::Identity
        })
    ));
    assert_eq!(snapshot(pair.root.path()), before);
}

#[test]
fn batches_keep_only_the_exact_source_node_capture_binding() {
    let pair = Pair::new();
    let first = pair.secondary.source_topology().unwrap();
    let second = pair.secondary.source_topology().unwrap();
    let expected = node(&first, pair.primary.identity());
    let result = observe_source_candidates(expected, &[&pair.primary]).unwrap();
    assert_eq!(first.binding(), second.binding());
    assert!(result.is_for(expected));
    assert!(result.clone().is_for(&expected.clone()));
    assert!(!result.is_for(node(&second, pair.primary.identity())));
    assert!(!result.is_for(node(&first, pair.secondary.identity())));
    assert!(first.contains_node(result.node()));
}

#[test]
fn nested_candidate_directories_are_not_treated_as_disjoint_disks() {
    let pair = Pair::new();
    pair.clone_primary("peer");
    let nested_path = pair.root.path().join("primary/nested");
    copy_directory(&pair.root.path().join("peer"), &nested_path);
    let nested = open(&nested_path, pair.primary.identity(), Some(pair.pin));
    let graph = pair.secondary.source_topology().unwrap();
    let expected = node(&graph, pair.primary.identity());
    let before = snapshot(pair.root.path());
    assert!(matches!(
        observe_source_candidates(expected, &[&pair.primary, &nested]),
        Err(SourceCandidateBatchError::OverlappingCandidates {
            first: 0,
            second: 1
        })
    ));
    assert_eq!(snapshot(pair.root.path()), before);
}

#[test]
fn candidate_count_limit_is_checked_before_candidate_disk_io() {
    let pair = Pair::new();
    let graph = pair.secondary.source_topology().unwrap();
    let expected = node(&graph, pair.primary.identity());
    fs::remove_file(pair.root.path().join("primary/identity.json")).unwrap();
    let candidates = vec![&pair.primary; candidate_set::MAX_CANDIDATES + 1];
    let before = snapshot(pair.root.path());
    assert!(matches!(
        observe_source_candidates(expected, &candidates),
        Err(SourceCandidateBatchError::CandidateBudget {
            actual: 129,
            maximum: 128
        })
    ));
    assert_eq!(snapshot(pair.root.path()), before);
}

#[test]
fn exact_candidate_limit_stays_partial_and_unpinned_nodes_never_match() {
    let temp = tempfile::tempdir().unwrap();
    let disk = IndependentDisk::initialize(&temp.path().join("empty"), identity("empty")).unwrap();
    let graph = disk.source_topology().unwrap();
    let expected = node(&graph, disk.identity());
    let candidates = vec![&disk; candidate_set::MAX_CANDIDATES];
    let result = observe_source_candidates(expected, &candidates).unwrap();
    assert_eq!(result.observations().len(), 128);
    assert_eq!(result.physical_candidates(), 1);
    assert_eq!(
        result.status(),
        &SourceCandidateBatchStatus::IncompleteObservation {
            matching_candidates: 0
        }
    );
    assert!(
        result
            .observations()
            .iter()
            .all(|row| row.status() == &SourceCandidateStatus::UnpinnedNode)
    );
}

#[test]
fn two_real_captures_of_a_changed_alias_are_rejected_instead_of_choosing_newer() {
    // Exercise the production collector with two actual captures around a real
    // publication; no raw observation or physical-identity fields are forged.
    let pair = Pair::new();
    let graph = pair.secondary.source_topology().unwrap();
    let expected = node(&graph, pair.primary.identity());
    let disk = Disk::open(&pair.root.path().join("primary"), pair.primary.identity()).unwrap();
    let mut batch = candidate_set::CandidateBatchBuilder::new(expected, 2).unwrap();
    batch
        .push(0, disk.capture_source_candidate(expected).unwrap())
        .unwrap();
    write(&pair.primary, b"changed during capture");
    let before = snapshot(pair.root.path());
    assert!(matches!(
        batch.push(1, disk.capture_source_candidate(expected).unwrap()),
        Err(SourceCandidateBatchError::ConflictingPhysicalObservation {
            first: 0,
            second: 1
        })
    ));
    assert_eq!(snapshot(pair.root.path()), before);
}

#[test]
fn collector_cannot_mix_real_observations_from_different_source_captures() {
    let pair = Pair::new();
    let first = pair.secondary.source_topology().unwrap();
    let second = pair.secondary.source_topology().unwrap();
    let disk = Disk::open(&pair.root.path().join("primary"), pair.primary.identity()).unwrap();
    let capture = disk
        .capture_source_candidate(node(&first, pair.primary.identity()))
        .unwrap();
    let mut batch =
        candidate_set::CandidateBatchBuilder::new(node(&second, pair.primary.identity()), 1)
            .unwrap();
    assert!(matches!(
        batch.push(0, capture),
        Err(SourceCandidateBatchError::DifferentSourceCapture { index: 0 })
    ));
}

#[test]
fn chain_entry_arithmetic_enforces_the_total_budget_and_rejects_overflow() {
    // Scalar boundary test, not a claim that 10,000 real captures were executed.
    assert_eq!(candidate_set::charge_chain_entries(0, 0).unwrap(), 0);
    assert_eq!(
        candidate_set::charge_chain_entries(9_999, 1).unwrap(),
        10_000
    );
    assert!(matches!(
        candidate_set::charge_chain_entries(10_000, 1),
        Err(SourceCandidateBatchError::CheckpointBudget { maximum: 10_000 })
    ));
    assert!(matches!(
        candidate_set::charge_chain_entries(u64::MAX, 1),
        Err(SourceCandidateBatchError::CheckpointBudget { maximum: 10_000 })
    ));
}
