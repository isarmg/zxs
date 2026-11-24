use super::*;
use crate::test_support::PrivateTemp;

const POOL: Id = [7; 16];
const RECYCLE_TOKEN: [u8; 32] = [0xA5; 32];

fn id(n: u8) -> Id {
    [n; 16]
}
fn mutation(n: u8) -> Mutation {
    Mutation {
        container: id(1),
        request: id(n),
    }
}
fn body(bytes: &[u8]) -> Body {
    let mut sha = Sha256::new();
    sha.update(bytes).unwrap();
    Body {
        length: bytes.len() as u64,
        sha256: sha.finish(),
    }
}
fn main(head: NodeId) -> Target {
    Target::Branch {
        name: MAIN.into(),
        expected_head: head,
    }
}
fn full(target: Target, node: NodeId, body: Body) -> Change {
    Change::Save { target, node, body }
}
fn first_node(node: NodeId, body: Body, name: String) -> Change {
    full(Target::FirstMain { name }, node, body)
}
fn held_data(path: &Path, token: u8) -> DataRoot {
    DataRoot {
        root: HeldDir::trusted_storage(path).unwrap(),
        token: [token; 32],
    }
}

// Exercises the real SYSTEM routes and DATA layout under their owner leases,
// without claiming that sibling temporary roots pass production mount admission.
fn open_test_roster(system: &Path, data: Vec<DataRoot>) -> io::Result<Store> {
    let system = HeldDir::trusted_storage(system)?;
    let owner = system.opaque_owner(c"container-owner")?;
    Store::open_roster(&system, data, owner)
}

fn open_fixture(data: &Path) -> Store {
    let system = data.with_file_name("fixture-system");
    ensure_dir(&system).unwrap();
    open_test_roster(&system, vec![held_data(data, 1)]).unwrap()
}

fn fixture() -> (PrivateTemp, PathBuf, Store) {
    let temp = PrivateTemp::under(&std::env::temp_dir());
    let root = temp.0.join("store");
    ensure_dir(&root).unwrap();
    let store = open_fixture(&root);
    (temp, root, store)
}
fn head(page: &DetailPage, branch: &str) -> Option<NodeId> {
    page.heads
        .iter()
        .find_map(|(name, node)| (name == branch).then_some(*node))
}

fn read_node(store: &Store, node: u8) -> Vec<u8> {
    let mut selected = store.node(id(1), id(node)).unwrap().unwrap();
    let mut bytes = Vec::new();
    selected.file.read_to_end(&mut bytes).unwrap();
    bytes
}
#[test]
fn rejected_payload_leaves_no_staging_directory() {
    let (_temp, _dir, store) = fixture();
    assert!(matches!(
        store.save(
            mutation(2),
            first_node(id(10), body(b"right"), "x.bin".into()),
            &b"wrong"[..],
            || Ok(())
        ),
        Err(Error::WrongDigest)
    ));
    let staging = store.volumes[0].staging();
    assert!(fs::read_dir(&staging.path).unwrap().next().is_none());
    assert_eq!(store.routes.get(id(1)).unwrap(), None);
}

#[test]
fn stage_cleanup_leaves_unregistered_member_untouched() {
    let (_temp, _dir, store) = fixture();
    let mut stage = store.stage(&store.volumes[0]).unwrap();
    stage.write(c"payload", b"staged").unwrap();
    let known = stage.directory.path.join("payload");
    let unrelated = stage.directory.path.join("other");
    write_file(&unrelated, b"other").unwrap();
    drop(stage);
    assert!(!known.exists());
    assert_eq!(fs::read(unrelated).unwrap(), b"other");
}

#[test]
fn first_main_save_and_stale_head_and_historical_fork_are_exact() {
    let (_temp, dir, store) = fixture();
    assert!(matches!(
        store.save(
            mutation(2),
            first_node(id(10), body(b"new"), "x.bin".into()),
            &b"ne"[..],
            || Ok(())
        ),
        Err(Error::WrongLength)
    ));
    assert!(store.detail_page(id(1), None, None).unwrap().is_none());
    assert_eq!(store.routes.get(id(1)).unwrap(), None);
    let first = store
        .save(
            mutation(2),
            first_node(id(10), body(b"new"), "x.bin".into()),
            &b"new"[..],
            || Ok(()),
        )
        .unwrap();
    assert_eq!(
        head(
            &store.detail_page(id(1), None, None).unwrap().unwrap(),
            MAIN
        ),
        Some(id(10))
    );
    store
        .save(
            mutation(3),
            full(main(id(10)), id(11), body(b"second")),
            &b"second"[..],
            || Ok(()),
        )
        .unwrap();
    assert!(matches!(
        store.save(mutation(4), full(main(id(10)), id(12), body(b"old")), &b"old"[..], || Ok(())),
        Err(Error::Conflict(Conflict::HeadChanged(head))) if head==id(11)));
    let fork = Target::HistoricalMain {
        new_branch: "alternate".into(),
        original: id(10),
    };
    store
        .save(
            mutation(4),
            full(fork, id(12), body(b"old")),
            &b"old"[..],
            || Ok(()),
        )
        .unwrap();
    let current = store.detail_page(id(1), None, None).unwrap().unwrap();
    assert_eq!(head(&current, MAIN), Some(id(11)));
    assert_eq!(head(&current, "alternate"), Some(id(12)));
    let history = |branch| {
        store
            .branch_history(id(1), branch, head(&current, branch).unwrap(), None)
            .unwrap()
            .unwrap()
            .nodes
            .into_iter()
            .map(|(node, _)| node)
            .collect::<Vec<_>>()
    };
    assert_eq!(history(MAIN), [id(11), id(10)]);
    assert_eq!(history("alternate"), [id(12), id(10)]);
    assert_eq!(
        store.node(id(1), id(12)).unwrap().unwrap().node.prior,
        Some(id(10))
    );
    assert_eq!(read_node(&store, 10), b"new");
    assert_eq!(read_node(&store, 11), b"second");
    assert_eq!(read_node(&store, 12), b"old");
    drop(store);
    let store = open_fixture(&dir);
    assert_eq!(
        store.operation_receipt(mutation(2)).unwrap(),
        Some(OperationReceipt::Mutation(first.clone()))
    );
    assert_eq!(
        store
            .save(
                mutation(2),
                first_node(id(10), body(b"new"), "x.bin".into()),
                &b"new"[..],
                || Ok(())
            )
            .unwrap(),
        first
    );
    assert_eq!(
        head(
            &store.detail_page(id(1), None, None).unwrap().unwrap(),
            MAIN
        ),
        Some(id(11))
    );
}
#[test]
fn only_a_published_main_ancestor_can_start_a_historical_fork() {
    let (_temp, _, store) = fixture();
    let save = |request, target, node| {
        store.save(
            mutation(request),
            full(target, id(node), body(b"x")),
            &b"x"[..],
            || Ok(()),
        )
    };
    save(
        2,
        Target::FirstMain {
            name: "original".into(),
        },
        10,
    )
    .unwrap();
    save(3, main(id(10)), 11).unwrap();
    let fork = |name: &str, original| Target::HistoricalMain {
        new_branch: name.into(),
        original: id(original),
    };
    save(4, fork("first-fork", 10), 12).unwrap();
    for wrong_base in [11, 12, 99] {
        // current main, another branch, missing
        assert!(matches!(
            save(5, fork("second-fork", wrong_base), 13),
            Err(Error::Conflict(Conflict::NotHistoricalMain))
        ));
        assert!(store.operation_receipt(mutation(5)).unwrap().is_none());
    }
    save(5, fork("second-fork", 10), 13).unwrap();
    assert_eq!(
        store.node(id(1), id(13)).unwrap().unwrap().node.prior,
        Some(id(10))
    );
}

#[test]
fn a_read_revoked_during_lookup_does_not_expose_an_absent_receipt() {
    use crate::workspace_protocol::{
        self,
        dispatch::{self, Body as Incoming, Failure},
    };
    let (_temp, _, store) = fixture();
    let url = format!(
        "/zxs/v1/containers/{}/operations/{}",
        hex(id(1)),
        hex(id(2))
    );
    let request = workspace_protocol::request("GET", &url, &[]).unwrap();
    let permitted = std::cell::Cell::new(true);
    assert!(matches!(
        dispatch::dispatch(&store, request, Incoming::<io::Empty>::Empty, |_| permitted
            .replace(false)),
        Err(Failure::Denied)
    ));
}

#[test]
fn renames_tags_and_branch_references_never_write_another_payload() {
    let (_temp, _dir, store) = fixture();
    store
        .save(
            mutation(2),
            first_node(id(10), body(b"x"), "first".into()),
            &b"x"[..],
            || Ok(()),
        )
        .unwrap();
    store
        .change(
            mutation(3),
            Change::Rename {
                expected_sequence: 1,
                name: "renamed".into(),
            },
            || Ok(()),
        )
        .unwrap();
    assert!(matches!(
        store.change(
            mutation(4),
            Change::Tags {
                expected_sequence: 1,
                tags: vec![]
            },
            || Ok(())
        ),
        Err(Error::Conflict(Conflict::SequenceChanged(2)))
    ));
    store
        .change(
            mutation(4),
            Change::Tags {
                expected_sequence: 2,
                tags: vec!["已选".into()],
            },
            || Ok(()),
        )
        .unwrap();
    store
        .change(
            mutation(5),
            Change::Branch {
                name: "branch".into(),
                from: id(10),
            },
            || Ok(()),
        )
        .unwrap();
    let state = store.detail_page(id(1), None, None).unwrap().unwrap();
    assert_eq!(state.name, "renamed");
    assert_eq!(state.tags, ["已选"]);
    assert_eq!(head(&state, "branch"), Some(id(10)));
    for (sequence, request) in [(2, id(3)), (3, id(4)), (4, id(5))] {
        assert!(
            !store.volumes[0]
                .container(id(1))
                .join(entry_leaf(sequence, request))
                .join("payload")
                .exists()
        );
    }
    assert_eq!(read_node(&store, 10), b"x");
}

// Leave an R copy ready but revoke the final publication authority.
fn leave_prepared_recycle_unselected(store: &Store) {
    let object = store.object(id(1)).unwrap();
    let mut memory = object.lock().unwrap();
    let state = store.volumes[0].load(id(1), &mut memory).unwrap();
    let archive = store.archive.as_ref().unwrap();
    let mut incoming = store.flat_stage_at(archive.staging().unwrap()).unwrap();
    let tentative = archive
        .prepare(mutation(6), &store.volumes[0], state, &mut incoming, || {
            Ok(())
        })
        .unwrap();
    let mut selector = store.flat_stage_at(archive.moves().unwrap()).unwrap();
    assert!(
        archive
            .choose(&tentative, &mut selector, || Err(
                io::ErrorKind::PermissionDenied.into()
            ))
            .is_err()
    );
    assert!(archive.selected(id(1)).unwrap().is_none());
}

#[test]
#[ignore = "requires private writable /dev/shm on a filesystem distinct from the ordinary temporary storage"]
fn recycle_selects_whole_container_and_retains_the_exact_receipt_without_f() {
    let local = PrivateTemp::under(&std::env::temp_dir());
    let remote = PrivateTemp::under(Path::new("/dev/shm"));
    let system = local.0.join("system");
    let ordinary = local.0.join("ordinary");
    let recycle = remote.0.join("recycle");
    for path in [&system, &ordinary, &recycle] {
        ensure_dir(path).unwrap();
    }
    let held = |p: &Path| HeldDir::trusted_storage(p).unwrap();
    let store = Store::open_with_recycle(
        held(&system),
        vec![held_data(&ordinary, 1)],
        held(&recycle),
        POOL,
        RECYCLE_TOKEN,
    )
    .unwrap();
    let first = store
        .save(
            mutation(2),
            first_node(id(10), body(b"one"), "name".into()),
            &b"one"[..],
            || Ok(()),
        )
        .unwrap();
    store
        .save(
            mutation(3),
            full(main(id(10)), id(11), body(b"two")),
            &b"two"[..],
            || Ok(()),
        )
        .unwrap();
    store
        .change(
            mutation(4),
            Change::Branch {
                name: "older".into(),
                from: id(10),
            },
            || Ok(()),
        )
        .unwrap();
    store
        .change(
            mutation(5),
            Change::Tags {
                expected_sequence: 3,
                tags: vec!["known".into()],
            },
            || Ok(()),
        )
        .unwrap();

    let original = store.volumes[0].container(id(1));
    leave_prepared_recycle_unselected(&store);
    assert_eq!(read_node(&store, 11), b"two");
    assert_eq!(
        store.detail_page(id(1), None, None).unwrap().unwrap().name,
        "name"
    );
    let recycled = store.move_to_recycle(mutation(6), 4, || Ok(())).unwrap();
    assert_eq!(recycled.original_sequence, 4);
    assert!(!original.exists());
    assert!(store.detail_page(id(1), None, None).unwrap().is_none());
    assert!(store.node(id(1), id(10)).unwrap().is_none());
    assert!(store.catalog_page(None, |_| true).unwrap().items.is_empty());

    let archive = store.archive.as_ref().unwrap();
    let (_receipt, moved) = archive.selected(id(1)).unwrap().unwrap();
    let state = read_state(&moved, id(1)).unwrap();
    assert_eq!(state.name.as_deref(), Some("name"));
    assert_eq!(state.tags, ["known"]);
    assert_eq!(state.branches[MAIN], id(11));
    assert_eq!(state.branches["older"], id(10));
    for (node, payload) in [(10, b"one"), (11, b"two")] {
        let exact = &state.nodes[&id(node)];
        let path = moved
            .path
            .join(entry_leaf(exact.sequence, exact.request))
            .join("payload");
        assert_eq!(fs::read(path).unwrap(), payload);
    }
    // Simulate F path/name being unavailable, with independent SYSTEM and R
    // roots still held. Exact receipts make no ordinary path lookup.
    fs::rename(&ordinary, local.0.join("ordinary-offline")).unwrap();
    assert_eq!(
        store.operation_receipt(mutation(6)).unwrap(),
        Some(OperationReceipt::Recycle(recycled))
    );
    assert_eq!(
        store.operation_receipt(mutation(2)).unwrap(),
        Some(OperationReceipt::Mutation(first))
    );
    assert_eq!(store.routes.get(id(1)).unwrap(), Some([1; 32]));
    assert!(matches!(
        store.move_to_recycle(mutation(6), 4, || Ok(())),
        Err(Error::CleanupPending { receipt, .. }) if receipt == recycled
    ));
}

#[test]
#[ignore = "requires private writable /dev/shm on a different filesystem"]
fn an_archived_operation_lookup_never_turns_a_missing_receipt_into_absence() {
    let local = PrivateTemp::under(&std::env::temp_dir());
    let remote = PrivateTemp::under(Path::new("/dev/shm"));
    let system = local.0.join("system");
    let data = local.0.join("data");
    let recycle = remote.0.join("recycle");
    for path in [&system, &data, &recycle] {
        ensure_dir(path).unwrap();
    }
    let held = |p: &Path| HeldDir::trusted_storage(p).unwrap();
    let store = Store::open_with_recycle(
        held(&system),
        vec![held_data(&data, 1)],
        held(&recycle),
        POOL,
        RECYCLE_TOKEN,
    )
    .unwrap();
    store
        .save(
            mutation(2),
            first_node(id(10), body(b"x"), "saved".into()),
            &b"x"[..],
            || Ok(()),
        )
        .unwrap();
    let selected = store.move_to_recycle(mutation(3), 1, || Ok(())).unwrap();
    assert_eq!(
        store.operation_receipt(mutation(3)).unwrap(),
        Some(OperationReceipt::Recycle(selected))
    );
    assert!(store.operation_receipt(mutation(4)).unwrap().is_none());
    let (_, archived) = store
        .archive
        .as_ref()
        .unwrap()
        .selected(id(1))
        .unwrap()
        .unwrap();
    let record = archived.path.join(entry_leaf(1, id(2))).join("receipt");
    fs::rename(&record, record.with_file_name("offline-record")).unwrap();
    for request in [2, 3, 4] {
        assert!(store.operation_receipt(mutation(request)).is_err());
    }
}

#[test]
#[ignore = "requires private writable /dev/shm on a filesystem distinct from the ordinary temporary storage"]
fn retired_data_proof_settles_only_the_exact_recycled_retry() {
    let local = PrivateTemp::under(&std::env::temp_dir());
    let remote = PrivateTemp::under(Path::new("/dev/shm"));
    let system = local.0.join("system");
    let original = local.0.join("original");
    let remaining = local.0.join("remaining");
    let recycle = remote.0.join("recycle");
    for path in [&system, &original, &remaining, &recycle] {
        ensure_dir(path).unwrap();
    }
    let held = |p: &Path| HeldDir::trusted_storage(p).unwrap();
    let open = |data: &Path, token, pool| {
        Store::open_with_recycle(
            held(&system),
            vec![held_data(data, token)],
            held(&recycle),
            pool,
            RECYCLE_TOKEN,
        )
        .unwrap()
    };
    let store = open(&original, 1, POOL);
    store
        .save(
            mutation(2),
            first_node(id(10), body(b"one"), "name".into()),
            &b"one"[..],
            || Ok(()),
        )
        .unwrap();
    let done = store.move_to_recycle(mutation(6), 1, || Ok(())).unwrap();
    let not_recycled = Mutation {
        container: id(11),
        request: id(12),
    };
    store
        .routes
        .reserve(not_recycled.container, [1; 32])
        .unwrap();
    drop(store);

    let pending = |store: &Store| {
        assert!(matches!(
            store.move_to_recycle(mutation(6), 1, || Ok(())),
            Err(Error::CleanupPending { receipt, .. }) if receipt == done
        ));
    };
    let store = open(&remaining, 2, POOL);
    // A selector and matching ready are not authority for a source token no
    // longer enrolled and lacking its exact retirement record.
    assert!(matches!(store.move_to_recycle(mutation(6), 1, || Ok(())),
        Err(Error::Io(error)) if error.kind() == io::ErrorKind::NotConnected));
    assert!(matches!(store.operation_receipt(mutation(6)), Err(error)
        if error.kind() == io::ErrorKind::NotConnected));
    drop(store);

    let retired = system.join("retired-data");
    ensure_dir(&retired).unwrap();
    let token = [1; 32];
    let mut proof = [0; 52];
    proof[..4].copy_from_slice(b"DT1\0");
    proof[4..20].copy_from_slice(&POOL);
    proof[20..].copy_from_slice(&token);
    write_file(&retired.join(hex(token)), &proof).unwrap();

    let store = open(&remaining, 2, [8; 16]);
    assert!(store.operation_receipt(mutation(6)).is_err());
    assert!(matches!(store.move_to_recycle(mutation(6), 1, || Ok(())), Err(Error::Io(_))));
    drop(store);
    let store = open(&original, 1, POOL);
    fs::rename(&original, local.0.join("original-offline")).unwrap();
    // Known enrollment may report a selected R receipt while its DATA mount is absent.
    assert_eq!(store.operation_receipt(mutation(6)).unwrap(), Some(OperationReceipt::Recycle(done)));
    pending(&store);
    drop(store);

    let store = open(&remaining, 2, POOL);
    assert_eq!(
        store.move_to_recycle(mutation(6), 1, || Ok(())).unwrap(),
        done
    );
    assert!(matches!(
        store.move_to_recycle(mutation(7), 1, || Ok(())),
        Err(Error::Conflict(Conflict::RequestReused))
    ));
    assert_eq!(store.routes.get(id(1)).unwrap(), Some(token));
    assert!(matches!(
        store.detail_page(not_recycled.container, None, None),
        Err(Error::Io(e)) if e.kind() == io::ErrorKind::NotConnected
    ));
    assert!(matches!(
        store.move_to_recycle(not_recycled, 1, || Ok(())),
        Err(Error::Io(e)) if e.kind() == io::ErrorKind::NotConnected
    ));
}

#[test]
#[ignore = "requires private writable /dev/shm on a filesystem distinct from the ordinary temporary storage"]
fn a_replaced_descendant_leaves_the_entire_original_pending_without_any_unlink() {
    let local = PrivateTemp::under(&std::env::temp_dir());
    let remote = PrivateTemp::under(Path::new("/dev/shm"));
    let system = local.0.join("system");
    let ordinary = local.0.join("ordinary");
    let recycle = remote.0.join("recycle");
    for path in [&system, &ordinary, &recycle] {
        ensure_dir(path).unwrap();
    }
    let held = |p: &Path| HeldDir::trusted_storage(p).unwrap();
    let store = Store::open_with_recycle(
        held(&system),
        vec![held_data(&ordinary, 1)],
        held(&recycle),
        POOL,
        RECYCLE_TOKEN,
    )
    .unwrap();
    store
        .save(
            mutation(2),
            first_node(id(10), body(b"one"), "name".into()),
            &b"one"[..],
            || Ok(()),
        )
        .unwrap();
    store
        .save(
            mutation(3),
            full(main(id(10)), id(11), body(b"two")),
            &b"two"[..],
            || Ok(()),
        )
        .unwrap();
    let original = store.volumes[0].container(id(1));
    let first = original.join(entry_leaf(1, id(2)));
    let second = original.join(entry_leaf(2, id(3)));
    {
        let source = store.object(id(1)).unwrap();
        let mut memory = source.lock().unwrap();
        let state = store.volumes[0].load(id(1), &mut memory).unwrap();
        let archive = store.archive.as_ref().unwrap();
        let mut incoming = store.flat_stage_at(archive.staging().unwrap()).unwrap();
        let prepared = archive
            .prepare(mutation(6), &store.volumes[0], state, &mut incoming, || {
                Ok(())
            })
            .unwrap();
        let mut selector = store.flat_stage_at(archive.moves().unwrap()).unwrap();
        archive.choose(&prepared, &mut selector, || Ok(())).unwrap();
    }
    // Simulate an unexpected local replacement that preserves both the
    // exact path and bytes, but not the copied original native identity.
    let forged = local.0.join("other-owned");
    write_file(&forged, b"two").unwrap();
    fs::rename(&forged, second.join("payload")).unwrap();
    let result = store.move_to_recycle(mutation(6), 2, || Ok(()));
    let Err(Error::CleanupPending { receipt, .. }) = result else {
        panic!("retirement may not unlink changed members");
    };
    assert_eq!(
        store.operation_receipt(mutation(6)).unwrap(),
        Some(OperationReceipt::Recycle(receipt))
    );
    assert!(store.detail_page(id(1), None, None).unwrap().is_none());
    assert_eq!(fs::read(first.join("payload")).unwrap(), b"one");
    assert!(first.join("receipt").exists());
    assert_eq!(fs::read(second.join("payload")).unwrap(), b"two");
    assert!(second.join("receipt").exists());
    assert!(matches!(
        store.move_to_recycle(mutation(6), 2, || Ok(())),
        Err(Error::CleanupPending { .. })
    ));
}

#[test]
fn startup_refuses_a_replaced_qualified_ordinary_directory_before_writing_layout() {
    let local = PrivateTemp::under(&std::env::temp_dir());
    let system = local.0.join("system");
    let ordinary = local.0.join("ordinary");
    let recycle = local.0.join("recycle");
    let displaced = local.0.join("displaced");
    for path in [&system, &ordinary, &recycle] {
        ensure_dir(path).unwrap();
    }
    let held = |p: &Path| HeldDir::trusted_storage(p).unwrap();
    let original = held(&ordinary);
    fs::rename(&ordinary, &displaced).unwrap();
    ensure_dir(&ordinary).unwrap();
    // The held-role admission runs before checking the volume pair or
    // creating an owner/layout entry anywhere. This fixture needs no mount.
    let error = Store::open_with_recycle(
        held(&system),
        vec![DataRoot {
            token: [1; 32],
            root: original,
        }],
        held(&recycle),
        POOL,
        RECYCLE_TOKEN,
    )
    .err()
    .unwrap();
    assert!(error.to_string().contains("replaced"));
    for path in [&ordinary, &displaced, &system, &recycle] {
        assert!(fs::read_dir(path).unwrap().next().is_none());
    }
}

#[test]
fn a_later_staging_substitution_cannot_redirect_a_whole_node_upload() {
    let (_temp, root, store) = fixture();
    let staging = root.join("staging");
    let displaced = root.join("displaced");
    fs::rename(&staging, &displaced).unwrap();
    ensure_dir(&staging).unwrap();
    let result = store.save(
        mutation(2),
        first_node(id(10), body(b"one"), "first".into()),
        &b"one"[..],
        || Ok(()),
    );
    assert!(matches!(result,Err(Error::Io(ref e)) if e.to_string().contains("replaced")));
    for path in [
        staging.as_path(),
        displaced.as_path(),
        store.volumes[0].contents().path.as_path(),
    ] {
        assert!(fs::read_dir(path).unwrap().next().is_none());
    }
}

#[test]
#[ignore = "requires private writable /dev/shm on a filesystem distinct from the ordinary temporary storage"]
fn cold_open_qualifies_existing_recycle_selector_before_readback() {
    let local = PrivateTemp::under(&std::env::temp_dir());
    let remote = PrivateTemp::under(Path::new("/dev/shm"));
    let system = local.0.join("system");
    let ordinary = local.0.join("ordinary");
    let recycle = remote.0.join("recycle");
    for path in [&system, &ordinary, &recycle] {
        ensure_dir(path).unwrap();
    }
    let held = |p: &Path| HeldDir::trusted_storage(p).unwrap();
    let open = || {
        Store::open_with_recycle(
            held(&system),
            vec![held_data(&ordinary, 1)],
            held(&recycle),
            POOL,
            RECYCLE_TOKEN,
        )
        .unwrap()
    };
    let store = open();
    store
        .save(
            mutation(2),
            first_node(id(10), body(b"one"), "name".into()),
            &b"one"[..],
            || Ok(()),
        )
        .unwrap();
    let receipt = store.move_to_recycle(mutation(6), 1, || Ok(())).unwrap();
    let moves = store.archive.as_ref().unwrap().moves().unwrap();
    let selected = moves.path.join(hex(id(1)));
    let staged = moves.path.join("selector-cut");
    // Recreate the externally visible namespace at the post-rename
    // boundary; this fixture models a service stop, not physical power loss.
    write_file(&staged, &fs::read(&selected).unwrap()).unwrap();
    fs::rename(&staged, &selected).unwrap();
    drop(store);
    let opened = open();
    assert_eq!(
        opened.operation_receipt(mutation(6)).unwrap(),
        Some(OperationReceipt::Recycle(receipt))
    );
    assert!(opened.detail_page(id(1), None, None).unwrap().is_none());
}

#[test]
#[ignore = "requires private writable /dev/shm on a filesystem distinct from the ordinary temporary storage"]
fn an_unselected_copy_does_not_block_a_later_exact_recycle() {
    let local = PrivateTemp::under(&std::env::temp_dir());
    let remote = PrivateTemp::under(Path::new("/dev/shm"));
    let system = local.0.join("system");
    let ordinary = local.0.join("ordinary");
    let recycle = remote.0.join("recycle");
    for path in [&system, &ordinary, &recycle] {
        ensure_dir(path).unwrap();
    }
    let held = |p: &Path| HeldDir::trusted_storage(p).unwrap();
    let store = Store::open_with_recycle(
        held(&system),
        vec![held_data(&ordinary, 1)],
        held(&recycle),
        POOL,
        RECYCLE_TOKEN,
    )
    .unwrap();
    store
        .save(
            mutation(2),
            first_node(id(10), body(b"one"), "name".into()),
            &b"one"[..],
            || Ok(()),
        )
        .unwrap();
    let archive = store.archive.as_ref().unwrap();
    let original = store.volumes[0].container(id(1));
    {
        let object = store.object(id(1)).unwrap();
        let mut memory = object.lock().unwrap();
        let state = store.volumes[0].load(id(1), &mut memory).unwrap();
        let mut incoming = store.flat_stage_at(archive.staging().unwrap()).unwrap();
        archive
            .prepare(mutation(6), &store.volumes[0], state, &mut incoming, || {
                Ok(())
            })
            .unwrap();
    }
    assert!(store.operation_receipt(mutation(6)).unwrap().is_none());
    store
        .save(
            mutation(3),
            full(main(id(10)), id(11), body(b"two")),
            &b"two"[..],
            || Ok(()),
        )
        .unwrap();
    let done = store.move_to_recycle(mutation(7), 2, || Ok(())).unwrap();
    assert_eq!(
        store.operation_receipt(mutation(7)).unwrap(),
        Some(OperationReceipt::Recycle(done))
    );
    assert!(store.operation_receipt(mutation(6)).unwrap().is_none());
    let (_, chosen) = archive.selected(id(1)).unwrap().unwrap();
    assert_eq!(read_state(&chosen, id(1)).unwrap().branches[MAIN], id(11));
    assert!(!original.exists());
}

#[test]
#[ignore = "requires private writable /dev/shm on a different filesystem from ordinary temporary storage"]
fn an_exact_unselected_rc2_can_resume_but_a_changed_proof_cannot_switch_visibility() {
    let local = PrivateTemp::under(&std::env::temp_dir());
    let remote = PrivateTemp::under(Path::new("/dev/shm"));
    let system = local.0.join("system");
    let ordinary = local.0.join("ordinary");
    let recycle = remote.0.join("recycle");
    for path in [&system, &ordinary, &recycle] {
        ensure_dir(path).unwrap();
    }
    let held = |p: &Path| HeldDir::trusted_storage(p).unwrap();
    let open = || Store::open_with_recycle(
        held(&system), vec![held_data(&ordinary, 1)], held(&recycle), POOL, RECYCLE_TOKEN,
    ).unwrap();
    let store = open();
    store.save(
        mutation(2), first_node(id(10), body(b"one"), "name".into()), &b"one"[..], || Ok(()),
    ).unwrap();
    leave_prepared_recycle_unselected(&store);
    drop(store);
    let envelope = recycle.join("containers")
        .join(format!("{}-{}", hex(id(1)), hex(id(6))));
    let proof = envelope.join("source-proof");
    let original = fs::read(&proof).unwrap();
    assert_eq!(original.len(), 104);
    let stamp = fs::read(envelope.join("ready")).unwrap();
    assert_eq!(&stamp[..4], b"RC2\0");
    assert_eq!(stamp.len(), 172);
    let mut damaged = original.clone();
    damaged[103] ^= 1;
    fs::write(&proof, damaged).unwrap();
    let store = open();
    assert!(store.move_to_recycle(mutation(6), 1, || Ok(())).is_err());
    assert_eq!(read_node(&store, 10), b"one");
    assert!(store.operation_receipt(mutation(6)).unwrap().is_none());
    fs::write(&proof, original).unwrap();
    let r_body = envelope.join("container").join(entry_leaf(1, id(2))).join("payload");
    let hidden = envelope.join("fixture-held-body");
    fs::rename(&r_body, &hidden).unwrap();
    assert!(store.move_to_recycle(mutation(6), 1, || Ok(())).is_err());
    assert_eq!(read_node(&store, 10), b"one");
    assert!(store.operation_receipt(mutation(6)).unwrap().is_none());
    fs::rename(&hidden, &r_body).unwrap();
    for bad in [b"x".as_slice(), b"two"] {
        fs::write(&r_body, bad).unwrap();
        assert!(store.move_to_recycle(mutation(6), 1, || Ok(())).is_err());
        assert_eq!(read_node(&store, 10), b"one");
        assert!(store.operation_receipt(mutation(6)).unwrap().is_none());
    }
    fs::write(&r_body, b"one").unwrap();

    // Publish the exact prepared receipt, simulate a stop before original cleanup,
    // then lose R body integrity. The durable selector remains visible but no F entry is removed.
    let selected = RecycleReceipt::decode(&stamp).unwrap();
    {
        let archive = store.archive.as_ref().unwrap();
        let object = store.object(id(1)).unwrap();
        let mut memory = object.lock().unwrap();
        let volume = &store.volumes[0];
        let state = volume.load(id(1), &mut memory).unwrap();
        let mut incoming = store.flat_stage_at(archive.staging().unwrap()).unwrap();
        let prepared = archive
            .prepare(mutation(6), volume, state, &mut incoming, || Ok(()))
            .unwrap();
        assert_eq!(prepared.receipt, selected);

        // An unselected recovery must not publish if the exact hashed R child
        // is swapped under the same receipt/envelope before the selector cut.
        let archived = envelope.join("container");
        let outside_cut = envelope.join("fixture-held-container");
        fs::rename(&archived, &outside_cut).unwrap();
        ensure_dir(&archived).unwrap();
        {
            let mut cut = store.flat_stage_at(archive.moves().unwrap()).unwrap();
            assert!(archive.choose(&prepared, &mut cut, || Ok(())).is_err());
        }
        assert!(archive.selected(id(1)).unwrap().is_none());
        assert_eq!(
            fs::read(volume.container(id(1)).join(entry_leaf(1, id(2))).join("payload"))
                .unwrap(),
            b"one"
        );
        fs::remove_dir(&archived).unwrap();
        fs::rename(&outside_cut, &archived).unwrap();
        let mut cut = store.flat_stage_at(archive.moves().unwrap()).unwrap();
        archive.choose(&prepared, &mut cut, || Ok(())).unwrap();
    }
    drop(store);
    let store = open();
    fs::rename(&r_body, &hidden).unwrap();
    assert!(matches!(store.move_to_recycle(mutation(6), 1, || Ok(())), Err(Error::CleanupPending { .. })));
    assert_eq!(store.operation_receipt(mutation(6)).unwrap(), Some(OperationReceipt::Recycle(selected)));
    assert!(store.volumes[0].container(id(1)).join(entry_leaf(1, id(2))).join("payload").is_file());
    fs::rename(&hidden, &r_body).unwrap();
    for bad in [b"x".as_slice(), b"two"] {
        fs::write(&r_body, bad).unwrap();
        assert!(matches!(store.move_to_recycle(mutation(6), 1, || Ok(())), Err(Error::CleanupPending { .. })));
        assert!(store.volumes[0].container(id(1)).join(entry_leaf(1, id(2))).join("payload").is_file());
    }
    fs::write(&r_body, b"one").unwrap();

    // Recovery can lose authority after the archived copy is verified but
    // before original retirement. No original member may be unlinked.
    let archive = store.archive.as_ref().unwrap();
    let (_, at) = archive.selected(id(1)).unwrap().unwrap();
    let known = read_state(&at, id(1)).unwrap();
    let verified = std::cell::Cell::new(false);
    let revoked = archive
        .retire(
            selected,
            &store.volumes[0],
            &known,
            || {
                recycle_material::verify_recovered(&known.receipts, &at, || Ok(()))?;
                verified.set(true);
                Ok(())
            },
            || {
                if verified.get() {
                    Err(io::Error::from(io::ErrorKind::PermissionDenied))
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();
    assert_eq!(revoked.kind(), io::ErrorKind::PermissionDenied);
    assert!(verified.get());
    let source = store.volumes[0].container(id(1)).join(entry_leaf(1, id(2)));
    assert_eq!(fs::read(source.join("payload")).unwrap(), b"one");
    assert!(source.join("receipt").is_file());
    assert_eq!(
        store.operation_receipt(mutation(6)).unwrap(),
        Some(OperationReceipt::Recycle(selected))
    );

    let completed = store.move_to_recycle(mutation(6), 1, || Ok(())).unwrap();
    assert_eq!(completed.encode(), stamp);
    assert_eq!(store.operation_receipt(mutation(6)).unwrap(), Some(OperationReceipt::Recycle(completed)));
    assert!(!store.volumes[0].container(id(1)).exists());
}

#[test]
fn diagnostics_keep_the_outcome_but_not_private_details() {
    let private = || {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "/not-a-real-path/private; account=fixture-account",
        )
    };
    assert_eq!(
        format!("{:?}", Error::Io(private())),
        "Io(PermissionDenied)"
    );
    assert_eq!(
        format!("{:?}", Error::Uncertain(private())),
        "Uncertain(PermissionDenied)"
    );
    assert_eq!(
        format!("{:?}", Conflict::Invalid(private())),
        "Invalid(PermissionDenied)"
    );
    assert_eq!(
        format!("{:?}", Conflict::HeadChanged(id(1))),
        "HeadChanged { head_exists: true }"
    );
    assert_eq!(
        format!("{:?}", Conflict::SequenceChanged(3)),
        "SequenceChanged(3)"
    );
}

#[test]
fn reopen_discards_interrupted_intake_without_touching_selected_data() {
    let (_temp, root, store) = fixture();
    store
        .save(
            mutation(2),
            first_node(id(10), body(b"saved"), "name".into()),
            &b"saved"[..],
            || Ok(()),
        )
        .unwrap();
    let unfinished = root.join("staging/0000000000000001");
    let unknown = root.join("staging/0000000000000002");
    let foreign_link = root.join("staging/0000000000000003");
    let foreign_file = root.join("staging/0000000000000004");
    let wrong_member = root.join("staging/0000000000000005");
    ensure_dir(&unfinished).unwrap();
    ensure_dir(&unknown).unwrap();
    write_file(&unfinished.join("payload"), b"unfinished").unwrap();
    write_file(&unknown.join("not-an-intake-file"), b"leave alone").unwrap();
    std::os::unix::fs::symlink(&unknown, &foreign_link).unwrap();
    write_file(&foreign_file, b"not a directory").unwrap();
    ensure_dir(&wrong_member).unwrap();
    write_file(&wrong_member.join("payload"), b"keep both old names").unwrap();
    std::os::unix::fs::symlink(&foreign_file, wrong_member.join("receipt")).unwrap();
    // A killed service cannot run Stage::drop; these represent the cold
    // on-disk records without needing to start or stop a real service.
    drop(store);
    let reopened = open_fixture(&root);
    assert!(!unfinished.exists());
    assert!(unknown.join("not-an-intake-file").is_file());
    assert!(fs::symlink_metadata(foreign_link).unwrap().is_symlink());
    assert!(foreign_file.is_file());
    assert_eq!(
        fs::read(wrong_member.join("payload")).unwrap(),
        b"keep both old names"
    );
    assert!(
        fs::symlink_metadata(wrong_member.join("receipt"))
            .unwrap()
            .is_symlink()
    );
    assert_eq!(read_node(&reopened, 10), b"saved");
    assert!(reopened.operation_receipt(mutation(3)).unwrap().is_none());
}

#[test]
fn startup_refuses_aliased_data_roles_before_writing_layout() {
    let local = PrivateTemp::under(&std::env::temp_dir());
    let system = local.0.join("system");
    let data = local.0.join("data");
    let recycle = local.0.join("recycle");
    for path in [&system, &data, &recycle] {
        ensure_dir(path).unwrap();
    }
    let error = Store::open_with_recycle(
        HeldDir::trusted_storage(&system).unwrap(),
        vec![held_data(&data, 1), held_data(&data, 2)],
        HeldDir::trusted_storage(&recycle).unwrap(),
        POOL,
        RECYCLE_TOKEN,
    )
    .err()
    .unwrap();
    assert!(error.to_string().contains("overlap"));
    for path in [&system, &data, &recycle] {
        assert!(fs::read_dir(path).unwrap().next().is_none());
    }
}

#[test]
fn new_routes_do_not_adopt_legacy_data_or_an_unrouted_leaf() {
    let local = PrivateTemp::under(&std::env::temp_dir());
    let system = local.0.join("system");
    let data = local.0.join("data");
    let contents = data.join("data");
    let orphan = contents.join(hex(id(1)));
    for path in [&system, &data, &contents, &orphan] {
        ensure_dir(path).unwrap();
    }
    let original = orphan.join("original");
    write_file(&original, b"preserve").unwrap();
    let open = || open_test_roster(&system, vec![held_data(&data, 1)]);
    assert!(open().is_err());
    let registry = system.join("container-routes");
    assert!(!registry.exists());
    assert_eq!(fs::read(&original).unwrap(), b"preserve");

    // An empty existing directory or a staged-only route is not proof of enrollment.
    ensure_dir(&registry).unwrap();
    assert!(open().is_err());
    write_file(
        &registry.join(format!(".{}.incoming", hex(id(1)))),
        b"stage",
    )
    .unwrap();
    assert!(open().is_err());
    assert_eq!(fs::read(&original).unwrap(), b"preserve");

    fs::rename(&orphan, local.0.join("retained-legacy")).unwrap();
    let store = open().unwrap();
    fs::rename(local.0.join("retained-legacy"), &orphan).unwrap();
    assert!(store.detail_page(id(1), None, None).unwrap().is_none());
    assert!(matches!(
        store.save(
            mutation(2),
            first_node(id(10), body(b"new"), "name".into()),
            &b"new"[..],
            || Ok(()),
        ),
        Err(Error::Io(_))
    ));
    assert_eq!(store.routes.get(id(1)).unwrap(), None);
    assert_eq!(fs::read(&original).unwrap(), b"preserve");
}

#[test]
fn first_save_reserves_only_on_valid_intake_and_reuses_reserved_disk_after_reopen() {
    let (_root, directory, store) = fixture();
    let save = || first_node(id(10), body(b"new"), "name".into());
    assert!(matches!(
        store.save(mutation(2), save(), &b"bad"[..], || Ok(())),
        Err(Error::WrongDigest)
    ));
    assert!(matches!(
        store.save(mutation(2), save(), &b"new"[..], || {
            Err(io::ErrorKind::PermissionDenied.into())
        }),
        Err(Error::Io(_))
    ));
    assert_eq!(store.routes.get(id(1)).unwrap(), None);
    assert!(!store.volumes[0].container(id(1)).exists());

    let gates = std::cell::Cell::new(0);
    assert!(matches!(
        store.save(mutation(2), save(), &b"new"[..], || {
            let n = gates.get();
            gates.set(n + 1);
            if n == 0 {
                Ok(())
            } else {
                Err(io::ErrorKind::PermissionDenied.into())
            }
        }),
        Err(Error::Io(_))
    ));
    assert_eq!(gates.get(), 2);
    assert_eq!(store.routes.get(id(1)).unwrap(), Some([1; 32]));
    assert!(store.operation_receipt(mutation(2)).unwrap().is_none());
    drop(store);

    let store = open_fixture(&directory);
    assert!(store.detail_page(id(1), None, None).unwrap().is_none());
    let receipt = store
        .save(mutation(2), save(), &b"new"[..], || Ok(()))
        .unwrap();
    assert_eq!(
        store.operation_receipt(mutation(2)).unwrap(),
        Some(OperationReceipt::Mutation(receipt))
    );
    assert_eq!(read_node(&store, 10), b"new");
}

#[test]
fn exact_routes_keep_other_ids_usable_when_one_data_root_is_missing() {
    let local = PrivateTemp::under(&std::env::temp_dir());
    let system = local.0.join("system");
    let left = local.0.join("left");
    let right = local.0.join("right");
    for path in [&system, &left, &right] {
        ensure_dir(path).unwrap();
    }
    let open = |reverse: bool| {
        let mut data = vec![held_data(&left, 1), held_data(&right, 2)];
        if reverse {
            data.reverse();
        }
        open_test_roster(&system, data).unwrap()
    };
    let store = open(false);
    let op = |container, request| Mutation {
        container: id(container),
        request: id(request),
    };
    let first = |name: &str| first_node(id(10), body(b"one"), name.into());
    store
        .save(op(1, 5), first("first"), &b"one"[..], || Ok(()))
        .unwrap();
    assert!(matches!(
        store.save(op(2, 5), first("first"), &b"one"[..], || Ok(())),
        Err(Error::Conflict(Conflict::AlreadyExists))
    ));
    assert_eq!(store.routes.get(id(2)).unwrap(), None);
    store
        .save(op(2, 5), first("second"), &b"one"[..], || Ok(()))
        .unwrap();
    assert_eq!(store.routes.get(id(1)).unwrap(), Some([2; 32]));
    assert_eq!(store.routes.get(id(2)).unwrap(), Some([1; 32]));
    assert!(right.join("data").join(hex(id(1))).is_dir());
    assert!(left.join("data").join(hex(id(2))).is_dir());
    drop(store);

    let store = open(true);
    assert_eq!(
        store
            .catalog_page(None, |_| true)
            .unwrap()
            .items
            .iter()
            .map(|item| item.id)
            .collect::<Vec<_>>(),
        [id(1), id(2)]
    );
    store.routes.reserve(id(3), [9; 32]).unwrap();
    assert!(matches!(
        store.detail_page(id(3), None, None),
        Err(Error::Io(e)) if e.kind() == io::ErrorKind::NotConnected
    ));

    fs::rename(&right, local.0.join("right-offline")).unwrap();
    assert!(store.detail_page(id(1), None, None).is_err());
    assert!(store.operation_receipt(op(1, 5)).is_err());
    assert!(store.catalog_page(None, |_| true).is_err());
    assert!(matches!(
        store.change(
            op(2, 6),
            Change::Rename {
                expected_sequence: 1,
                name: "uncertain".into()
            },
            || Ok(())
        ),
        Err(Error::Io(_))
    ));
    assert!(matches!(
        store.save(op(4, 5), first("uncertain"), &b"one"[..], || Ok(())),
        Err(Error::Io(_))
    ));
    assert_eq!(store.routes.get(id(4)).unwrap(), None);
    assert!(store.operation_receipt(op(2, 5)).unwrap().is_some());
    assert!(
        store
            .save(
                op(2, 7),
                full(main(id(10)), id(11), body(b"two")),
                &b"two"[..],
                || Ok(())
            )
            .is_ok()
    );
    assert_eq!(
        head(
            &store.detail_page(id(2), None, None).unwrap().unwrap(),
            MAIN
        ),
        Some(id(11))
    );
    assert!(!left.join("data").join(hex(id(1))).exists());
}

#[test]
fn absent_reads_cannot_grow_idle_slots_or_split_an_active_container_lock() {
    let (_temp, _dir, store) = fixture();
    let active = store.object(id(1)).unwrap();
    for missing in 2..=u8::MAX {
        assert!(
            store
                .detail_page(id(missing), None, None)
                .unwrap()
                .is_none()
        );
    }
    assert!(Arc::ptr_eq(&active, &store.object(id(1)).unwrap()));
    assert!(store.objects.lock().unwrap().len() <= OBJECT_SLOT_TARGET);
}

#[test]
fn compact_catalog_uses_selected_store_names_in_raw_id_order() {
    let (_temp, _, store) = fixture();
    for n in [3, 1, 2] {
        store
            .save(
                Mutation {
                    container: id(n),
                    request: id(2),
                },
                first_node(id(10), body(b""), format!("file-{n}")),
                &b""[..],
                || Ok(()),
            )
            .unwrap();
    }
    let first = store.catalog_page(None, |key| key != id(2)).unwrap();
    let actual: Vec<_> = first
        .items
        .into_iter()
        .map(|row| (row.id, row.name, row.sequence))
        .collect();
    assert_eq!(
        actual,
        [
            (id(1), String::from("file-1"), 1),
            (id(3), String::from("file-3"), 1)
        ]
    );
    assert_eq!(first.next_after, None);
    let tail = store.catalog_page(Some(id(2)), |_| true).unwrap();
    assert_eq!(tail.items[0].id, id(3));
    assert_eq!(tail.next_after, None);
}

#[test]
fn compact_catalog_bounds_the_page_and_never_reads_denied_ids() {
    let key = |number: u16| {
        let mut result = [0; 16];
        result[14..].copy_from_slice(&number.to_be_bytes());
        result
    };
    let enumerated = || (1_u16..=257).map(key);
    let project = |id| {
        Ok(Some(CatalogRow {
            id,
            name: "file".into(),
            sequence: 1,
        }))
    };
    let visible = |id| id != key(2);
    let first = catalog_rows(enumerated(), None, visible, |id| {
        assert_ne!(id, key(2));
        project(id)
    })
    .unwrap();
    assert_eq!(first.items.len(), CATALOG_PAGE_ROWS);
    assert_eq!(first.next_after, Some(key(257)));
    let terminal = catalog_rows(enumerated(), first.next_after, visible, project).unwrap();
    assert!(terminal.items.is_empty());
    assert_eq!(terminal.next_after, None);
    let full = catalog_rows(
        enumerated(),
        None,
        |_| true,
        |id| {
            assert_ne!(id, key(257), "full page reads no lookahead");
            project(id)
        },
    )
    .unwrap();
    assert_eq!(full.next_after, Some(key(256)));
    let last = catalog_rows(enumerated(), full.next_after, |_| true, project).unwrap();
    assert_eq!(last.items[0].id, key(257));
    assert_eq!(last.next_after, None);
}

#[test]
fn branch_history_resume_is_an_exact_descending_parent_chain() {
    let key = |sequence: u16| {
        let mut bytes = [0; 16];
        bytes[14..].copy_from_slice(&sequence.to_be_bytes());
        bytes
    };
    let mut state = State {
        sequence: 258,
        ..State::default()
    };
    state.branches.insert(MAIN.into(), key(257));
    state.branches.insert(String::from("fork"), key(258));
    let body = body(b"");
    for sequence in 1_u16..=258 {
        let prior = match sequence {
            1 => None,
            258 => Some(key(2)),
            n => Some(key(n - 1)),
        };
        state.nodes.insert(
            key(sequence),
            Node {
                prior,
                original_branch: if sequence == 258 { "fork" } else { MAIN }.into(),
                body: body.clone(),
                sequence: u64::from(sequence),
                request: key(sequence),
            },
        );
    }
    let first = select_history_page(&state, MAIN, key(257), None).unwrap();
    assert_eq!(first.head, key(257));
    assert_eq!(first.nodes.len(), HISTORY_PAGE_ROWS);
    assert_eq!(first.nodes[0].0, key(257));
    assert_eq!(first.nodes.last().unwrap().0, key(2));
    assert_eq!(first.next, Some(key(1)));
    let last = select_history_page(&state, MAIN, first.head, first.next).unwrap();
    assert_eq!(last.nodes.len(), 1);
    assert_eq!(last.nodes[0].0, key(1));
    assert_eq!(last.next, None);
    assert!(matches!(
        select_history_page(&state, MAIN, first.head, Some(key(258))),
        Err(Error::Conflict(Conflict::MissingNode))
    ));
    let shared = select_history_page(&state, "fork", key(258), Some(key(2))).unwrap();
    assert_eq!(
        shared.nodes.iter().map(|item| item.0).collect::<Vec<_>>(),
        [key(2), key(1)]
    );
    state.branches.insert(MAIN.into(), key(256));
    assert!(matches!(
        select_history_page(&state, MAIN, first.head, first.next),
        Err(Error::Conflict(Conflict::HeadChanged(id))) if id == key(256)
    ));
    state.branches.remove(MAIN);
    assert!(matches!(
        select_history_page(&state, MAIN, first.head, first.next),
        Err(Error::Conflict(Conflict::MissingBranch))
    ));
}

#[test]
fn detail_heads_page_preserves_revision_and_exact_lexical_order() {
    let mut state = State {
        sequence: 700,
        tags: vec!["标签".into()],
        ..State::default()
    };
    state.branches.insert(String::from("main"), id(1));
    state
        .branches
        .extend((1..=DETAIL_PAGE_ROWS).map(|n| (format!("{n:032x}"), id(1))));
    let first = select_detail_page(id(2), "original", &state, None, None).unwrap();
    assert_eq!(first.id, id(2));
    assert_eq!(first.sequence, 700);
    assert_eq!(first.name, "original");
    assert_eq!(first.tags, ["标签"]);
    assert_eq!(first.heads.len(), DETAIL_PAGE_ROWS);
    assert_eq!(
        first.next_after.as_deref(),
        Some("00000000000000000000000000000100")
    );
    let second = select_detail_page(
        id(2),
        "original",
        &state,
        Some(first.sequence),
        first.next_after.as_deref(),
    )
    .unwrap();
    assert_eq!(second.heads, [(String::from("main"), id(1))]);
    assert!(second.next_after.is_none());
    assert!(matches!(
        select_detail_page(id(2), "original", &state, Some(699), None),
        Err(Error::Conflict(Conflict::SequenceChanged(700)))
    ));
}
