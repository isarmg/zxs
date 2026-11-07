use super::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};
use storage_contract::OperationId;

fn token(text: &str) -> Token {
    Token::new(text).unwrap()
}
fn identity(name: &str, role: Role) -> Identity {
    Identity {
        origin: Origin {
            pool_id: token("original_pool"),
            disk_id: token(name),
            slot_id: token(name),
            binding_generation: 17,
        },
        incarnation: Digest::body(name.as_bytes()),
        role,
    }
}
fn create(root: &Path, name: &str, role: Role) -> IndependentDisk {
    IndependentDisk::initialize(&root.join(name), identity(name, role)).unwrap()
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
fn backup(root: &Path, disk: &IndependentDisk, sources: &[&IndependentDisk]) -> Digest {
    let selections: Vec<_> = sources
        .iter()
        .map(|source| DiskSelection {
            path: root.join(source.identity().origin.disk_id.as_str()),
            identity: source.identity().clone(),
            known_checkpoint: source.report().unwrap().checkpoint,
        })
        .collect();
    let request = disk
        .review_backup(
            OperationId::generate().unwrap(),
            disk.report().unwrap().checkpoint,
            &selections,
        )
        .unwrap();
    disk.execute(&request).unwrap().publication()
}
fn node<'a>(topology: &'a SourceTopology, identity: &Identity) -> &'a SourceNode {
    topology
        .nodes()
        .find(|node| node.identity() == identity)
        .unwrap()
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

#[test]
fn empty_disks_keep_their_exact_role_without_inventing_upstreams() {
    let temp = tempfile::tempdir().unwrap();
    for (name, role) in [
        ("primary", Role::Primary),
        ("secondary", Role::Secondary),
        ("tertiary", Role::Tertiary),
    ] {
        let disk = create(temp.path(), name, role);
        let before = snapshot(temp.path());
        let topology = disk.source_topology().unwrap();
        assert_eq!(topology.binding().identity(), disk.identity());
        assert_eq!(topology.binding().checkpoint(), None);
        assert_eq!(topology.binding().sequence(), 0);
        assert_eq!(topology.nodes().len(), 1);
        assert!(node(&topology, disk.identity()).checkpoints().is_empty());
        assert!(topology.relations().is_empty());
        assert_eq!(snapshot(temp.path()), before);
    }
}

#[test]
fn tertiary_preserves_two_source_pins_for_one_primary_and_exact_edge_endpoints() {
    let temp = tempfile::tempdir().unwrap();
    let primary = create(temp.path(), "primary", Role::Primary);
    let left = create(temp.path(), "left", Role::Secondary);
    let right = create(temp.path(), "right", Role::Secondary);
    let tertiary = create(temp.path(), "tertiary", Role::Tertiary);
    let first = write(&primary, b"first");
    let left_pin = backup(temp.path(), &left, &[&primary]);
    let second = write(&primary, b"second");
    let right_pin = backup(temp.path(), &right, &[&primary]);
    let root_pin = backup(temp.path(), &tertiary, &[&left, &right]);
    // A newer physically present source does not replace either recorded pin.
    let third = write(&primary, b"third");
    let before = snapshot(temp.path());
    let topology = tertiary.source_topology().unwrap();
    assert_eq!(topology.nodes().len(), 4);
    assert_eq!(topology.binding().identity().role, Role::Tertiary);
    assert_eq!(topology.binding().checkpoint(), Some(root_pin));
    assert_eq!(
        node(&topology, primary.identity()).checkpoints(),
        &BTreeSet::from([first, second])
    );
    assert!(
        !node(&topology, primary.identity())
            .checkpoints()
            .contains(&third)
    );
    let edges: BTreeMap<_, _> = topology
        .relations()
        .iter()
        .map(|edge| {
            (
                (edge.source().publication(), edge.consumer().publication()),
                (edge.source().identity(), edge.consumer().identity()),
            )
        })
        .collect();
    assert_eq!(
        edges,
        BTreeMap::from([
            ((first, left_pin), (primary.identity(), left.identity())),
            ((second, right_pin), (primary.identity(), right.identity())),
            ((left_pin, root_pin), (left.identity(), tertiary.identity())),
            (
                (right_pin, root_pin),
                (right.identity(), tertiary.identity())
            ),
        ])
    );
    assert_eq!(snapshot(temp.path()), before);
}

#[test]
fn previous_publications_are_verified_but_their_old_sources_are_not_projected() {
    let temp = tempfile::tempdir().unwrap();
    let old = create(temp.path(), "old", Role::Primary);
    let current = create(temp.path(), "current", Role::Primary);
    let secondary = create(temp.path(), "secondary", Role::Secondary);
    let tertiary = create(temp.path(), "tertiary", Role::Tertiary);
    let old_pin = write(&old, b"old source");
    write(&current, b"current source");
    let previous = backup(temp.path(), &secondary, &[&old]);
    let selected = backup(temp.path(), &secondary, &[&current]);
    backup(temp.path(), &tertiary, &[&secondary]);
    let verified = Disk::open(&temp.path().join("tertiary"), tertiary.identity())
        .unwrap()
        .verify(None)
        .unwrap();
    assert!(verified.publications.contains_key(&old_pin));
    assert!(verified.publications.contains_key(&previous));
    let topology = tertiary.source_topology().unwrap();
    assert_eq!(topology.nodes().len(), 3);
    assert_eq!(topology.relations().len(), 2);
    assert!(
        topology
            .nodes()
            .all(|node| node.identity() != old.identity())
    );
    assert_eq!(
        node(&topology, secondary.identity()).checkpoints(),
        &BTreeSet::from([selected])
    );
    // Even excluded historical metadata must remain intact for verification.
    fs::remove_file(
        temp.path()
            .join("tertiary/objects")
            .join(format!("{old_pin}.json")),
    )
    .unwrap();
    assert!(matches!(tertiary.source_topology(), Err(Error::Missing(pin)) if pin == old_pin));
}

#[test]
fn metadata_only_capture_neither_reads_bodies_nor_claims_body_availability() {
    let temp = tempfile::tempdir().unwrap();
    let primary = create(temp.path(), "primary", Role::Primary);
    let secondary = create(temp.path(), "secondary", Role::Secondary);
    write(&primary, b"valid body");
    backup(temp.path(), &secondary, &[&primary]);
    let before = secondary.source_topology().unwrap();
    let copy = secondary.report().unwrap().copies[0].commit;
    let body = temp
        .path()
        .join("secondary/bodies")
        .join(format!("{copy}.bin"));
    fs::write(&body, b"corrupt body").unwrap();
    let saved = snapshot(temp.path());
    let corrupt = secondary.source_topology().unwrap();
    assert_eq!(before.binding(), corrupt.binding());
    assert_eq!(before.relations(), corrupt.relations());
    assert!(matches!(secondary.report(), Err(Error::Corrupt(_))));
    assert_eq!(snapshot(temp.path()), saved);
    fs::remove_file(body).unwrap();
    let absent = secondary.source_topology().unwrap();
    assert_eq!(before.relations(), absent.relations());
    assert_eq!(
        secondary.report().unwrap().copies[0].availability,
        Availability::TemporarilyUnavailable
    );
    assert!(
        matches!(secondary.read_content(copy), Err(Error::ContentUnavailable(pin)) if pin == copy)
    );
}

#[test]
fn corrupt_or_missing_metadata_never_returns_a_partial_graph() {
    for missing in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let primary = create(temp.path(), "primary", Role::Primary);
        let pin = write(&primary, b"body");
        let verified = Disk::open(&temp.path().join("primary"), primary.identity())
            .unwrap()
            .verify(None)
            .unwrap();
        let manifest = verified.publications[&pin].manifest;
        let path = temp
            .path()
            .join("primary/objects")
            .join(format!("{manifest}.json"));
        if missing {
            fs::remove_file(&path).unwrap();
        } else {
            fs::write(&path, b"{}").unwrap();
        }
        let saved = snapshot(temp.path());
        let result = primary.source_topology();
        if missing {
            assert!(matches!(result, Err(Error::Missing(hash)) if hash == manifest));
        } else {
            assert!(matches!(result, Err(Error::Corrupt(_))));
        }
        assert_eq!(snapshot(temp.path()), saved);
    }
}

#[test]
fn nodes_and_physical_bindings_remain_specific_to_their_capture() {
    let temp = tempfile::tempdir().unwrap();
    let disk = create(temp.path(), "primary", Role::Primary);
    write(&disk, b"body");
    let first = disk.source_topology().unwrap();
    let retained = first.nodes().next().unwrap().clone();
    let second = disk.source_topology().unwrap();
    assert_eq!(first.binding(), second.binding());
    assert_eq!(retained.binding(), first.binding());
    assert!(first.contains_node(&retained));
    assert!(first.clone().contains_node(&retained));
    assert!(!second.contains_node(&retained));
    let other =
        IndependentDisk::initialize(&temp.path().join("other"), disk.identity().clone()).unwrap();
    assert_ne!(first.binding(), other.source_topology().unwrap().binding());
    write(&disk, b"next");
    assert_ne!(first.binding(), disk.source_topology().unwrap().binding());
    assert_eq!(
        retained.checkpoints(),
        first.nodes().next().unwrap().checkpoints()
    );
}

#[test]
fn replacing_a_held_disk_root_rejects_capture_without_modifying_either_directory() {
    let temp = tempfile::tempdir().unwrap();
    let disk = create(temp.path(), "primary", Role::Primary);
    write(&disk, b"original");
    fs::rename(temp.path().join("primary"), temp.path().join("moved")).unwrap();
    let replacement = create(temp.path(), "primary", Role::Primary);
    write(&replacement, b"replacement");
    let saved = snapshot(temp.path());
    assert!(matches!(disk.source_topology(), Err(Error::Identity)));
    assert_eq!(snapshot(temp.path()), saved);
}

#[test]
fn known_checkpoint_stays_a_required_ancestor_after_open() {
    let temp = tempfile::tempdir().unwrap();
    let disk = create(temp.path(), "primary", Role::Primary);
    let first = write(&disk, b"first");
    let second = write(&disk, b"second");
    let pinned = IndependentDisk::open(&DiskSelection {
        path: temp.path().join("primary"),
        identity: disk.identity().clone(),
        known_checkpoint: Some(second),
    })
    .unwrap();
    assert_eq!(
        pinned.source_topology().unwrap().binding().checkpoint(),
        Some(second)
    );
    fs::remove_file(
        temp.path()
            .join("primary/publications/00000000000000000002.json"),
    )
    .unwrap();
    assert_eq!(
        disk.source_topology().unwrap().binding().checkpoint(),
        Some(first)
    );
    assert!(matches!(pinned.source_topology(), Err(Error::StaleBase)));
}

#[test]
fn missing_local_decision_cannot_be_substituted_by_an_object_copy() {
    let temp = tempfile::tempdir().unwrap();
    let disk = create(temp.path(), "primary", Role::Primary);
    let first = write(&disk, b"first");
    write(&disk, b"second");
    let decision = temp
        .path()
        .join("primary/publications/00000000000000000001.json");
    fs::rename(
        decision,
        temp.path()
            .join("primary/objects")
            .join(format!("{first}.json")),
    )
    .unwrap();
    assert!(matches!(
        disk.source_topology(),
        Err(Error::Invalid(
            "durable publication files do not equal the local proof chain"
        ))
    ));
}

#[test]
fn same_disk_token_in_different_pools_preserves_complete_source_identities() {
    let temp = tempfile::tempdir().unwrap();
    let first_id = identity("same", Role::Primary);
    let mut second_id = first_id.clone();
    second_id.origin.pool_id = token("other_pool");
    second_id.origin.slot_id = token("other_slot");
    second_id.origin.binding_generation = 23;
    second_id.incarnation = Digest::body(b"other incarnation");
    let first = IndependentDisk::initialize(&temp.path().join("first"), first_id).unwrap();
    let second = IndependentDisk::initialize(&temp.path().join("second"), second_id).unwrap();
    write(&first, b"first");
    write(&second, b"second");
    let secondary = create(temp.path(), "secondary", Role::Secondary);
    let selections: Vec<_> = [("first", &first), ("second", &second)]
        .into_iter()
        .map(|(name, disk)| DiskSelection {
            path: temp.path().join(name),
            identity: disk.identity().clone(),
            known_checkpoint: disk.report().unwrap().checkpoint,
        })
        .collect();
    let review = secondary
        .review_backup(OperationId::generate().unwrap(), None, &selections)
        .unwrap();
    secondary.execute(&review).unwrap();
    let topology = secondary.source_topology().unwrap();
    assert_eq!(topology.nodes().len(), 3);
    assert_eq!(
        node(&topology, first.identity()).identity(),
        first.identity()
    );
    assert_eq!(
        node(&topology, second.identity()).identity(),
        second.identity()
    );
    assert_eq!(topology.relations().len(), 2);
}
