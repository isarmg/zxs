use super::*;
use std::{
    collections::{BTreeMap, BTreeSet},
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
fn create(root: &Path, name: &str, role: Role) -> IndependentDisk {
    IndependentDisk::initialize(&root.join(name), identity(name, role)).unwrap()
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
fn backup(root: &Path, disk: &IndependentDisk, sources: &[&IndependentDisk]) -> Digest {
    let selections: Vec<_> = sources
        .iter()
        .map(|source| DiskSelection {
            path: root.join(source.identity().origin.disk_id.as_str()),
            identity: source.identity().clone(),
            known_checkpoint: source.report().unwrap().checkpoint,
        })
        .collect();
    let review = disk
        .review_backup(
            OperationId::generate().unwrap(),
            disk.report().unwrap().checkpoint,
            &selections,
        )
        .unwrap();
    disk.execute(&review).unwrap().publication()
}
fn node<'a>(graph: &'a SourceTopology, identity: &Identity) -> &'a SourceNode {
    graph
        .nodes()
        .find(|node| node.identity() == identity)
        .unwrap()
}
fn open(path: &Path, identity: &Identity, pin: Option<Digest>) -> IndependentDisk {
    IndependentDisk::open(&DiskSelection {
        path: path.to_owned(),
        identity: identity.clone(),
        known_checkpoint: pin,
    })
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
    fn visit(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let relative = entry.path().strip_prefix(root).unwrap().to_owned();
            if entry.file_type().unwrap().is_dir() {
                out.insert(relative, None);
                visit(root, &entry.path(), out);
            } else {
                out.insert(relative, Some(fs::read(entry.path()).unwrap()));
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}
struct Pair {
    root: tempfile::TempDir,
    primary: IndependentDisk,
    secondary: IndependentDisk,
    primary_pin: Digest,
}
impl Pair {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let primary = create(root.path(), "primary", Role::Primary);
        let secondary = create(root.path(), "secondary", Role::Secondary);
        let primary_pin = write(&primary, b"published primary");
        backup(root.path(), &secondary, &[&primary]);
        Self {
            root,
            primary,
            secondary,
            primary_pin,
        }
    }
}

#[test]
fn exact_checkpoint_and_own_chain_successor_are_read_only_matches() {
    let pair = Pair::new();
    let graph = pair.secondary.source_topology().unwrap();
    let expected = node(&graph, pair.primary.identity());
    let before = snapshot(pair.root.path());
    let exact = pair.primary.observe_source_node(expected).unwrap();
    assert_eq!(
        exact.status(),
        &SourceCandidateStatus::SatisfiesPinnedCheckpoints
    );
    assert_eq!(
        exact.candidate_binding().checkpoint(),
        Some(pair.primary_pin)
    );
    assert!(exact.is_for(expected));
    assert_eq!(snapshot(pair.root.path()), before);
    let next = write(&pair.primary, b"verified successor");
    let before = snapshot(pair.root.path());
    let successor = pair.primary.observe_source_node(expected).unwrap();
    assert_eq!(
        successor.status(),
        &SourceCandidateStatus::SatisfiesPinnedCheckpoints
    );
    assert_eq!(successor.candidate_binding().checkpoint(), Some(next));
    assert_eq!(successor.candidate_binding().sequence(), 2);
    assert_eq!(
        successor.node().checkpoints(),
        &BTreeSet::from([pair.primary_pin])
    );
    assert_eq!(snapshot(pair.root.path()), before);
}

#[test]
fn every_required_pin_must_be_in_the_candidates_own_previous_chain() {
    let temp = tempfile::tempdir().unwrap();
    let primary = create(temp.path(), "primary", Role::Primary);
    let left = create(temp.path(), "left", Role::Secondary);
    let right = create(temp.path(), "right", Role::Secondary);
    let tertiary = create(temp.path(), "tertiary", Role::Tertiary);
    let first = write(&primary, b"first");
    copy_directory(&temp.path().join("primary"), &temp.path().join("older"));
    let older = open(&temp.path().join("older"), primary.identity(), Some(first));
    backup(temp.path(), &left, &[&primary]);
    let second = write(&primary, b"second");
    backup(temp.path(), &right, &[&primary]);
    backup(temp.path(), &tertiary, &[&left, &right]);
    let graph = tertiary.source_topology().unwrap();
    let expected = node(&graph, primary.identity());
    assert_eq!(expected.checkpoints(), &BTreeSet::from([first, second]));
    let before = snapshot(temp.path());
    let too_old = older.observe_source_node(expected).unwrap();
    assert_eq!(
        too_old.status(),
        &SourceCandidateStatus::MissingPinnedCheckpoints(BTreeSet::from([second]))
    );
    assert_eq!(snapshot(temp.path()), before);
    write(&primary, b"third");
    assert_eq!(
        primary.observe_source_node(expected).unwrap().status(),
        &SourceCandidateStatus::SatisfiesPinnedCheckpoints
    );
}

#[test]
fn same_identity_and_sequence_on_a_different_fork_does_not_match() {
    let pair = Pair::new();
    let fork = IndependentDisk::initialize(
        &pair.root.path().join("fork"),
        pair.primary.identity().clone(),
    )
    .unwrap();
    let other = write(&fork, b"other fork");
    assert_ne!(other, pair.primary_pin);
    let graph = pair.secondary.source_topology().unwrap();
    let expected = node(&graph, pair.primary.identity());
    let result = fork.observe_source_node(expected).unwrap();
    assert_eq!(result.candidate_binding().identity(), expected.identity());
    assert_eq!(result.candidate_binding().sequence(), 1);
    assert_eq!(
        result.status(),
        &SourceCandidateStatus::MissingPinnedCheckpoints(BTreeSet::from([pair.primary_pin]))
    );
}

#[test]
fn each_identity_field_is_required_without_role_or_generation_inference() {
    let pair = Pair::new();
    let graph = pair.secondary.source_topology().unwrap();
    let expected = node(&graph, pair.primary.identity());
    for index in 0..6 {
        let mut altered = pair.primary.identity().clone();
        match index {
            0 => altered.origin.pool_id = token("other_pool"),
            1 => altered.origin.disk_id = token("other_disk"),
            2 => altered.origin.slot_id = token("other_slot"),
            3 => altered.origin.binding_generation += 1,
            4 => altered.incarnation = Digest::body(b"other incarnation"),
            5 => altered.role = Role::Secondary,
            _ => unreachable!(),
        }
        let disk = IndependentDisk::initialize(
            &pair.root.path().join(format!("changed_{index}")),
            altered.clone(),
        )
        .unwrap();
        let before = snapshot(pair.root.path());
        let result = disk.observe_source_node(expected).unwrap();
        assert_eq!(
            result.status(),
            &SourceCandidateStatus::IdentityMismatch,
            "field {index}"
        );
        assert_eq!(result.candidate_binding().identity(), &altered);
        assert_eq!(snapshot(pair.root.path()), before);
    }
}

#[test]
fn a_reachable_source_pin_does_not_make_the_consumer_that_source() {
    let pair = Pair::new();
    let graph = pair.secondary.source_topology().unwrap();
    let expected = node(&graph, pair.primary.identity());
    let verified = Disk::open(
        &pair.root.path().join("secondary"),
        pair.secondary.identity(),
    )
    .unwrap()
    .verify(None)
    .unwrap();
    assert!(verified.publications.contains_key(&pair.primary_pin));
    let result = pair.secondary.observe_source_node(expected).unwrap();
    assert_eq!(result.status(), &SourceCandidateStatus::IdentityMismatch);
}

#[test]
fn synthetic_extra_workset_entry_cannot_replace_the_own_previous_chain() {
    // Deliberately mutate an internal test value, not disk metadata. This is a
    // helper regression, not evidence that the verifier accepts such a workset.
    let pair = Pair::new();
    let fork_path = pair.root.path().join("fork");
    let fork = IndependentDisk::initialize(&fork_path, pair.primary.identity().clone()).unwrap();
    write(&fork, b"unrelated fork");
    let graph = pair.secondary.source_topology().unwrap();
    let expected = node(&graph, pair.primary.identity());
    let mut candidate = Disk::open(&fork_path, fork.identity())
        .unwrap()
        .verify(None)
        .unwrap();
    let source = Disk::open(&pair.root.path().join("primary"), pair.primary.identity())
        .unwrap()
        .verify(None)
        .unwrap();
    candidate.publications.insert(
        pair.primary_pin,
        source.publications[&pair.primary_pin].clone(),
    );
    let local = candidate.local_binding.clone().unwrap();
    let result = topology::observe_candidate(expected, &candidate, local).unwrap();
    assert_eq!(
        result.status(),
        &SourceCandidateStatus::MissingPinnedCheckpoints(BTreeSet::from([pair.primary_pin]))
    );
}

#[test]
fn result_is_bound_to_the_exact_request_node_and_not_a_later_capture() {
    let pair = Pair::new();
    let graph = pair.secondary.source_topology().unwrap();
    let expected = node(&graph, pair.primary.identity());
    let result = pair.primary.observe_source_node(expected).unwrap();
    let repeated = pair.secondary.source_topology().unwrap();
    assert_eq!(graph.binding(), repeated.binding());
    assert!(result.is_for(expected));
    assert!(result.clone().is_for(&expected.clone()));
    assert!(!result.is_for(node(&repeated, pair.primary.identity())));
    assert!(!result.is_for(node(&graph, pair.secondary.identity())));
    assert!(graph.contains_node(result.node()));
    assert!(!repeated.contains_node(result.node()));
    backup(pair.root.path(), &pair.secondary, &[&pair.primary]);
    let newer = pair.secondary.source_topology().unwrap();
    assert_ne!(graph.binding(), newer.binding());
    assert!(!result.is_for(node(&newer, pair.primary.identity())));
}

#[test]
fn an_unpublished_node_never_matches_by_vacuous_empty_pin_comparison() {
    let temp = tempfile::tempdir().unwrap();
    let disk = create(temp.path(), "primary", Role::Primary);
    let graph = disk.source_topology().unwrap();
    let expected = node(&graph, disk.identity());
    assert!(expected.checkpoints().is_empty());
    assert_eq!(
        disk.observe_source_node(expected).unwrap().status(),
        &SourceCandidateStatus::UnpinnedNode
    );
    write(&disk, b"later publication");
    assert_eq!(
        disk.observe_source_node(expected).unwrap().status(),
        &SourceCandidateStatus::UnpinnedNode
    );
}

#[test]
fn bodies_are_not_observed_but_bad_metadata_fails_the_whole_observation() {
    let pair = Pair::new();
    let graph = pair.secondary.source_topology().unwrap();
    let expected = node(&graph, pair.primary.identity());
    let report = pair.primary.report().unwrap();
    let body = pair
        .root
        .path()
        .join("primary/bodies")
        .join(format!("{}.bin", report.copies[0].commit));
    fs::write(&body, b"corrupt body").unwrap();
    let before = snapshot(pair.root.path());
    assert_eq!(
        pair.primary.observe_source_node(expected).unwrap().status(),
        &SourceCandidateStatus::SatisfiesPinnedCheckpoints
    );
    assert!(matches!(pair.primary.report(), Err(Error::Corrupt(_))));
    assert_eq!(snapshot(pair.root.path()), before);
    fs::remove_file(body).unwrap();
    assert_eq!(
        pair.primary.observe_source_node(expected).unwrap().status(),
        &SourceCandidateStatus::SatisfiesPinnedCheckpoints
    );
    let verified = Disk::open(&pair.root.path().join("primary"), pair.primary.identity())
        .unwrap()
        .verify(None)
        .unwrap();
    let manifest = verified.publications[&pair.primary_pin].manifest;
    fs::remove_file(
        pair.root
            .path()
            .join("primary/objects")
            .join(format!("{manifest}.json")),
    )
    .unwrap();
    let before = snapshot(pair.root.path());
    assert!(
        matches!(pair.primary.observe_source_node(expected), Err(Error::Missing(hash)) if hash == manifest)
    );
    assert_eq!(snapshot(pair.root.path()), before);
}

#[test]
fn replacement_of_the_held_candidate_directory_rejects_the_observation() {
    let pair = Pair::new();
    let graph = pair.secondary.source_topology().unwrap();
    let expected = node(&graph, pair.primary.identity());
    fs::rename(
        pair.root.path().join("primary"),
        pair.root.path().join("original"),
    )
    .unwrap();
    let replacement = create(pair.root.path(), "primary", Role::Primary);
    write(&replacement, b"replacement");
    let before = snapshot(pair.root.path());
    assert!(matches!(
        pair.primary.observe_source_node(expected),
        Err(Error::Identity)
    ));
    assert_eq!(snapshot(pair.root.path()), before);
}

#[test]
fn the_candidates_original_selection_pin_remains_required() {
    let pair = Pair::new();
    let graph = pair.secondary.source_topology().unwrap();
    let expected = node(&graph, pair.primary.identity());
    let newer = write(&pair.primary, b"newer known publication");
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
    assert_eq!(
        pair.primary.observe_source_node(expected).unwrap().status(),
        &SourceCandidateStatus::SatisfiesPinnedCheckpoints
    );
    assert!(matches!(
        pinned.observe_source_node(expected),
        Err(Error::StaleBase)
    ));
}

#[test]
fn two_physical_copies_each_satisfy_pins_without_selecting_a_unique_disk() {
    let pair = Pair::new();
    copy_directory(
        &pair.root.path().join("primary"),
        &pair.root.path().join("copy"),
    );
    let copy = open(
        &pair.root.path().join("copy"),
        pair.primary.identity(),
        Some(pair.primary_pin),
    );
    let graph = pair.secondary.source_topology().unwrap();
    let expected = node(&graph, pair.primary.identity());
    let before = snapshot(pair.root.path());
    let first = pair.primary.observe_source_node(expected).unwrap();
    let second = copy.observe_source_node(expected).unwrap();
    assert_eq!(
        first.status(),
        &SourceCandidateStatus::SatisfiesPinnedCheckpoints
    );
    assert_eq!(second.status(), first.status());
    assert_ne!(first.candidate_binding(), second.candidate_binding());
    assert_eq!(
        first.candidate_binding().identity(),
        second.candidate_binding().identity()
    );
    assert_eq!(snapshot(pair.root.path()), before);
}
