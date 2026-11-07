use super::*;
use platform_io::fs_safety::{
    PrivateDirectory,
    linux::{MountPolicy, OpenAt2Root},
};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
};
use storage_contract::{
    OperationId,
    storage_independent_source::{BatchStatus, CandidateStatus, decode_observation},
};

fn token(text: &str) -> Token {
    Token::new(text).unwrap()
}
fn identity(text: &str, role: Role) -> Identity {
    Identity {
        origin: Origin {
            pool_id: token("foreign_pool"),
            disk_id: token(text),
            slot_id: token(text),
            binding_generation: u64::MAX,
        },
        incarnation: Digest::body(text.as_bytes()),
        role,
    }
}
fn write(disk: &IndependentDisk, bytes: &[u8]) -> Digest {
    let review = disk
        .review_file(FileWrite {
            operation: OperationId::generate().unwrap(),
            expected_base: disk.report().unwrap().checkpoint,
            history: &token("history"),
            file: &token("file"),
            bytes,
        })
        .unwrap();
    disk.execute(&review).unwrap().publication()
}
fn backup(target: &IndependentDisk, path: &Path, source: &IndependentDisk) {
    let review = target
        .review_backup(
            OperationId::generate().unwrap(),
            target.report().unwrap().checkpoint,
            &[DiskSelection {
                path: path.to_owned(),
                identity: source.identity().clone(),
                known_checkpoint: source.report().unwrap().checkpoint,
            }],
        )
        .unwrap();
    target.execute(&review).unwrap();
}
fn held(path: &Path) -> PrivateDirectory {
    PrivateDirectory::open_existing(path).unwrap()
}
fn copy_directory(source: &Path, target: &Path) {
    fs::create_dir(target).unwrap();
    fs::set_permissions(target, fs::Permissions::from_mode(0o700)).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let path = target.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_directory(&entry.path(), &path);
        } else {
            fs::copy(entry.path(), path).unwrap();
        }
    }
}
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn walk(root: &Path, path: &Path, output: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let key = entry.path().strip_prefix(root).unwrap().to_owned();
            if entry.file_type().unwrap().is_dir() {
                output.insert(key, None);
                walk(root, &entry.path(), output);
            } else {
                output.insert(key, Some(fs::read(entry.path()).unwrap()));
            }
        }
    }
    let mut output = BTreeMap::new();
    walk(root, root, &mut output);
    output
}
struct Pair {
    root: tempfile::TempDir,
    primary: IndependentDisk,
    secondary: IndependentDisk,
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
        write(&primary, b"original");
        backup(&secondary, &root.path().join("primary"), &primary);
        Self {
            root,
            primary,
            secondary,
        }
    }
    fn path(&self, child: &str) -> PathBuf {
        self.root.path().join(child)
    }
    fn graph(&self) -> SourceTopology {
        IndependentSourceReader::from_directory(held(&self.path("secondary")))
            .unwrap()
            .source_topology()
            .unwrap()
    }
    fn source<'a>(&self, graph: &'a SourceTopology) -> &'a SourceNode {
        graph
            .nodes()
            .find(|node| node.identity() == self.primary.identity())
            .unwrap()
    }
    fn observe(
        &self,
        names: &[&str],
    ) -> storage_contract::storage_independent_source::IndependentSourceObservation {
        let graph = self.graph();
        let mut session = SourceCandidateSession::new();
        let request = session.begin(self.source(&graph));
        let response =
            request.observe_held(names.iter().map(|name| held(&self.path(name))).collect());
        assert_eq!(session.receive(response), SourceCandidateDelivery::Updated);
        let SourceCandidateView::Observed(batch) = session.view() else {
            panic!("expected verified observation: {:?}", session.view())
        };
        decode_observation(&encode_source_observation(batch).unwrap()).unwrap()
    }
}

#[test]
fn held_reader_to_real_verifier_to_wire_is_read_only_and_partial() {
    let pair = Pair::new();
    let before = snapshot(pair.root.path());
    let result = pair.observe(&["primary"]);
    assert_eq!(
        result.status(),
        BatchStatus::IncompleteObservation {
            matching_candidates: 1
        }
    );
    assert_eq!(
        result.observations()[0].status,
        CandidateStatus::SatisfiesPinnedCheckpoints
    );
    assert_eq!(
        result.node().identity.origin.binding_generation.get(),
        u64::MAX
    );
    assert_eq!(
        result.node().root.identity.incarnation.as_str(),
        pair.secondary.identity().incarnation.to_string()
    );
    assert_eq!(snapshot(pair.root.path()), before);
}
#[test]
fn same_directory_aliases_remain_rows_without_false_ambiguity() {
    let pair = Pair::new();
    let result = pair.observe(&["primary", "primary"]);
    assert_eq!(result.observations().len(), 2);
    assert_eq!(result.physical_candidates(), 1);
    assert_eq!(
        result.status(),
        BatchStatus::IncompleteObservation {
            matching_candidates: 1
        }
    );
}
#[test]
fn independent_same_identity_clones_remain_ambiguous_in_both_orders() {
    let pair = Pair::new();
    copy_directory(&pair.path("primary"), &pair.path("clone"));
    for names in [["primary", "clone"], ["clone", "primary"]] {
        let result = pair.observe(&names);
        assert_eq!(result.physical_candidates(), 2);
        assert_eq!(
            result.status(),
            BatchStatus::Ambiguous {
                matching_candidates: 2
            }
        );
    }
}
#[test]
fn zero_candidates_remains_not_observed() {
    let pair = Pair::new();
    let result = pair.observe(&[]);
    assert_eq!(result.status(), BatchStatus::NotObserved);
    assert!(result.observations().is_empty());
}
#[test]
fn foreign_identity_is_preserved_as_a_rejected_row() {
    let pair = Pair::new();
    IndependentDisk::initialize(&pair.path("other"), identity("other", Role::Primary)).unwrap();
    let result = pair.observe(&["other", "primary"]);
    assert_eq!(
        result.observations()[0].status,
        CandidateStatus::IdentityMismatch
    );
    assert_eq!(
        result.observations()[0]
            .checkpoint
            .identity
            .origin
            .disk_id
            .as_str(),
        "other"
    );
    assert_eq!(
        result.status(),
        BatchStatus::IncompleteObservation {
            matching_candidates: 1
        }
    );
}
#[test]
fn missing_pin_is_real_own_chain_evidence_not_latest_sequence() {
    let pair = Pair::new();
    copy_directory(&pair.path("primary"), &pair.path("older"));
    let current = write(&pair.primary, b"newer");
    backup(&pair.secondary, &pair.path("primary"), &pair.primary);
    let result = pair.observe(&["older"]);
    let CandidateStatus::MissingPinnedCheckpoints { missing } = &result.observations()[0].status
    else {
        panic!("expected missing pin")
    };
    assert_eq!(missing[0].as_str(), current.to_string());
    assert_eq!(
        result.status(),
        BatchStatus::IncompleteObservation {
            matching_candidates: 0
        }
    );
}
#[test]
fn empty_unpinned_root_does_not_gain_a_matching_candidate() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("empty");
    IndependentDisk::initialize(&path, identity("empty", Role::Primary)).unwrap();
    let reader = IndependentSourceReader::from_directory(held(&path)).unwrap();
    let graph = reader.source_topology().unwrap();
    let mut session = SourceCandidateSession::new();
    let response = session
        .begin(graph.nodes().next().unwrap())
        .observe_held(vec![held(&path)]);
    session.receive(response);
    let SourceCandidateView::Observed(batch) = session.view() else {
        panic!("expected batch")
    };
    let output = decode_observation(&encode_source_observation(batch).unwrap()).unwrap();
    assert_eq!(
        output.observations()[0].status,
        CandidateStatus::UnpinnedNode
    );
    assert_eq!(
        output.status(),
        BatchStatus::IncompleteObservation {
            matching_candidates: 0
        }
    );
}
#[test]
fn held_root_replacement_is_rejected_without_adopting_the_clone() {
    let pair = Pair::new();
    let root = held(&pair.path("primary"));
    fs::rename(pair.path("primary"), pair.path("original")).unwrap();
    copy_directory(&pair.path("original"), &pair.path("primary"));
    let before = snapshot(pair.root.path());
    assert!(IndependentSourceReader::from_directory(root).is_err());
    assert_eq!(snapshot(pair.root.path()), before);
}
#[test]
fn mismatched_diagnostic_path_cannot_substitute_another_held_directory() {
    let pair = Pair::new();
    let root = OpenAt2Root::open(&pair.path("primary"), MountPolicy::SameMount).unwrap();
    let wrong_name =
        PrivateDirectory::from_directory(root.into_directory(), pair.path("secondary")).unwrap();
    assert!(IndependentSourceReader::from_directory(wrong_name).is_err());
}
#[test]
fn linked_child_is_rejected_without_repair_or_path_fallback() {
    let pair = Pair::new();
    let source = pair.path("primary").join("objects");
    fs::rename(&source, pair.path("original_objects")).unwrap();
    symlink(pair.path("original_objects"), &source).unwrap();
    assert!(IndependentSourceReader::from_directory(held(&pair.path("primary"))).is_err());
    assert!(
        fs::symlink_metadata(&source)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}
#[test]
fn incomplete_initialization_fails_at_the_original_input_position() {
    let pair = Pair::new();
    PrivateDirectory::create(pair.path("incomplete")).unwrap();
    let before = snapshot(pair.root.path());
    let graph = pair.graph();
    let mut session = SourceCandidateSession::new();
    let response = session.begin(pair.source(&graph)).observe_held(vec![
        held(&pair.path("primary")),
        held(&pair.path("incomplete")),
    ]);
    session.receive(response);
    let SourceCandidateView::Failed { error, .. } = session.view() else {
        panic!("expected failure")
    };
    assert!(matches!(
        error,
        SourceCandidateBatchError::Candidate {
            index: 1,
            stage: SourceCandidateBatchStage::OpenDirectory,
            ..
        }
    ));
    assert!(std::error::Error::source(error).is_some());
    assert_eq!(snapshot(pair.root.path()), before);
}
#[test]
fn corrupted_metadata_fails_in_capture_and_never_becomes_not_observed() {
    let pair = Pair::new();
    let graph = pair.graph();
    fs::write(
        pair.path("primary")
            .join("publications/00000000000000000001.json"),
        b"corrupt",
    )
    .unwrap();
    let before = snapshot(pair.root.path());
    let mut session = SourceCandidateSession::new();
    let response = session
        .begin(pair.source(&graph))
        .observe_held(vec![held(&pair.path("primary"))]);
    session.receive(response);
    assert!(matches!(
        session.view(),
        SourceCandidateView::Failed {
            error: SourceCandidateBatchError::Candidate {
                index: 0,
                stage: SourceCandidateBatchStage::Capture,
                ..
            },
            ..
        }
    ));
    assert_eq!(snapshot(pair.root.path()), before);
}
#[test]
fn over_budget_rejects_before_reading_a_malformed_candidate_header() {
    let pair = Pair::new();
    let graph = pair.graph();
    let bad = pair.path("bad");
    PrivateDirectory::create(&bad).unwrap();
    let directories = (0..129).map(|_| held(&bad)).collect();
    let mut session = SourceCandidateSession::new();
    let response = session.begin(pair.source(&graph)).observe_held(directories);
    session.receive(response);
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
}
#[test]
fn old_held_response_cannot_replace_a_new_request_or_owner() {
    let pair = Pair::new();
    let graph = pair.graph();
    let node = pair.source(&graph);
    let mut session = SourceCandidateSession::new();
    let stale = session
        .begin(node)
        .observe_held(vec![held(&pair.path("primary"))]);
    let current = session.begin(node);
    assert_eq!(
        session.receive(stale),
        SourceCandidateDelivery::DiscardedStale
    );
    assert!(matches!(session.view(), SourceCandidateView::Pending(_)));
    let response = current.observe_held(vec![]);
    let mut other = SourceCandidateSession::new();
    let _other_request = other.begin(node);
    assert_eq!(
        other.receive(response),
        SourceCandidateDelivery::DiscardedStale
    );
    assert!(matches!(other.view(), SourceCandidateView::Pending(_)));
}
#[test]
fn cancellation_and_reselection_reject_held_reader_completion() {
    let pair = Pair::new();
    let graph = pair.graph();
    let mut session = SourceCandidateSession::new();
    let response = session
        .begin(pair.source(&graph))
        .observe_held(vec![held(&pair.path("primary"))]);
    session.invalidate();
    assert_eq!(
        session.receive(response),
        SourceCandidateDelivery::DiscardedStale
    );
    assert!(matches!(session.view(), SourceCandidateView::NotRequested));
}
#[test]
fn missing_bodies_do_not_turn_metadata_observation_into_content_availability() {
    let pair = Pair::new();
    for entry in fs::read_dir(pair.path("primary").join("bodies")).unwrap() {
        fs::remove_file(entry.unwrap().path()).unwrap();
    }
    let before = snapshot(pair.root.path());
    let result = pair.observe(&["primary"]);
    assert_eq!(
        result.observations()[0].status,
        CandidateStatus::SatisfiesPinnedCheckpoints
    );
    let text = json_codec::to_string(&result).unwrap();
    for forbidden in [
        "writable",
        "online",
        "capacity",
        "selected",
        "complete_discovery",
    ] {
        assert!(!text.contains(forbidden));
    }
    assert_eq!(snapshot(pair.root.path()), before);
}
#[test]
fn tertiary_multiple_source_publications_preserve_every_pin_in_wire() {
    let pair = Pair::new();
    let first = pair.primary.report().unwrap().checkpoint.unwrap();
    let s2 = IndependentDisk::initialize(
        &pair.path("secondary2"),
        identity("secondary2", Role::Secondary),
    )
    .unwrap();
    let second = write(&pair.primary, b"new revision");
    backup(&s2, &pair.path("primary"), &pair.primary);
    let tertiary =
        IndependentDisk::initialize(&pair.path("tertiary"), identity("tertiary", Role::Tertiary))
            .unwrap();
    let sources =
        [(&pair.secondary, "secondary"), (&s2, "secondary2")].map(|(disk, name)| DiskSelection {
            path: pair.path(name),
            identity: disk.identity().clone(),
            known_checkpoint: disk.report().unwrap().checkpoint,
        });
    let review = tertiary
        .review_backup(OperationId::generate().unwrap(), None, &sources)
        .unwrap();
    tertiary.execute(&review).unwrap();
    let graph = IndependentSourceReader::from_directory(held(&pair.path("tertiary")))
        .unwrap()
        .source_topology()
        .unwrap();
    let node = graph
        .nodes()
        .find(|node| node.identity() == pair.primary.identity())
        .unwrap();
    assert_eq!(node.checkpoints().len(), 2);
    assert_eq!(graph.relations().len(), 4);
    let display = source_topology_display(&graph).unwrap();
    assert_eq!(display.nodes().len(), graph.nodes().len());
    assert_eq!(display.relations().len(), 4);
    for (actual, expected) in display.relations().iter().zip(graph.relations()) {
        assert_eq!(
            actual.source.publication.as_str(),
            expected.source().publication().to_string()
        );
        assert_eq!(
            actual.consumer.publication.as_str(),
            expected.consumer().publication().to_string()
        );
        assert_eq!(
            actual.source.identity.origin.disk_id.as_str(),
            expected.source().identity().origin.disk_id.as_str()
        );
        assert_eq!(
            actual.consumer.identity.origin.disk_id.as_str(),
            expected.consumer().identity().origin.disk_id.as_str()
        );
    }
    let mut session = SourceCandidateSession::new();
    let response = session
        .begin(node)
        .observe_held(vec![held(&pair.path("primary"))]);
    session.receive(response);
    let SourceCandidateView::Observed(batch) = session.view() else {
        panic!("expected batch")
    };
    let output = decode_observation(&encode_source_observation(batch).unwrap()).unwrap();
    let mut expected = [first.to_string(), second.to_string()];
    expected.sort();
    assert_eq!(
        output
            .node()
            .pins
            .iter()
            .map(|pin| pin.as_str())
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(
        output.observations()[0].status,
        CandidateStatus::SatisfiesPinnedCheckpoints
    );
}

#[test]
fn empty_held_disk_projects_its_identity_without_claiming_any_source_publication() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("empty");
    let disk = IndependentDisk::initialize(&path, identity("empty", Role::Tertiary)).unwrap();
    let before = snapshot(root.path());
    let graph = IndependentSourceReader::from_directory(held(&path))
        .unwrap()
        .source_topology()
        .unwrap();
    let output = source_topology_display(&graph).unwrap();
    assert_eq!(
        output.root().identity.origin.disk_id.as_str(),
        disk.identity().origin.disk_id.as_str()
    );
    assert_eq!(output.root().sequence, 0);
    assert!(output.root().publication.is_none());
    assert_eq!(output.nodes().len(), 1);
    assert!(output.nodes()[0].pins.is_empty());
    assert!(output.relations().is_empty());
    assert_eq!(snapshot(root.path()), before);
}

#[test]
fn graph_display_does_not_read_missing_contents_or_claim_presence() {
    let pair = Pair::new();
    for entry in fs::read_dir(pair.path("secondary").join("bodies")).unwrap() {
        fs::remove_file(entry.unwrap().path()).unwrap();
    }
    let before = snapshot(pair.root.path());
    let output = source_topology_display(&pair.graph()).unwrap();
    assert_eq!(output.nodes().len(), 2);
    let text = json_codec::to_string(&output).unwrap();
    for forbidden in [
        "online",
        "writable",
        "availability",
        "capacity",
        "device",
        "inode",
        "path",
    ] {
        assert!(!text.contains(forbidden));
    }
    assert_eq!(snapshot(pair.root.path()), before);
}
