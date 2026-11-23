use super::*;
use crate::test_support::PrivateTemp;

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
fn main(head: Option<NodeId>) -> Target {
    Target::Branch {
        name: MAIN.into(),
        expected_head: head,
    }
}
fn full(target: Target, node: NodeId, body: Body, initial_name: Option<String>) -> Change {
    Change::Save {
        target,
        node,
        body,
        initial_name,
    }
}
fn fixture() -> (PrivateTemp, PathBuf, Store) {
    let temp = PrivateTemp::under(&std::env::temp_dir());
    let root = temp.0.join("store");
    ensure_dir(&root).unwrap();
    let store = Store::open(&root).unwrap();
    (temp, root, store)
}
fn read_node(store: &Store, node: u8) -> Vec<u8> {
    let mut selected = store.node(id(1), id(node)).unwrap().unwrap();
    let mut bytes = Vec::new();
    selected.file.read_to_end(&mut bytes).unwrap();
    bytes
}
#[test]
fn first_main_save_and_stale_head_and_historical_fork_are_exact() {
    let (_temp, dir, store) = fixture();
    assert!(matches!(
        store.save(
            mutation(2),
            full(main(None), id(10), body(b"new"), Some("x.bin".into())),
            &b"ne"[..],
            || Ok(())
        ),
        Err(Error::WrongLength)
    ));
    assert!(store.describe(id(1)).unwrap().is_none());
    let first = store
        .save(
            mutation(2),
            full(main(None), id(10), body(b"new"), Some("x.bin".into())),
            &b"new"[..],
            || Ok(()),
        )
        .unwrap();
    assert_eq!(
        store.describe(id(1)).unwrap().unwrap().heads[MAIN],
        Some(id(10))
    );
    store
        .save(
            mutation(3),
            full(main(Some(id(10))), id(11), body(b"second"), None),
            &b"second"[..],
            || Ok(()),
        )
        .unwrap();
    assert!(matches!(
        store.save(mutation(4), full(main(Some(id(10))), id(12), body(b"old"), None), &b"old"[..], || Ok(())),
        Err(Error::Conflict(Conflict::HeadChanged(Some(head)))) if head==id(11)));
    let fork = Target::HistoricalMain {
        new_branch: "alternate".into(),
        original: id(10),
    };
    store
        .save(
            mutation(4),
            full(fork, id(12), body(b"old"), None),
            &b"old"[..],
            || Ok(()),
        )
        .unwrap();
    let current = store.describe(id(1)).unwrap().unwrap();
    assert_eq!(current.heads[MAIN], Some(id(11)));
    assert_eq!(current.heads["alternate"], Some(id(12)));
    let history = |branch| {
        store
            .branch_nodes(id(1), branch)
            .unwrap()
            .unwrap()
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
    let store = Store::open(&dir).unwrap();
    assert_eq!(store.receipt(mutation(2)).unwrap(), Some(first.clone()));
    assert_eq!(
        store
            .save(
                mutation(2),
                full(main(None), id(10), body(b"new"), Some("x.bin".into())),
                &b"new"[..],
                || Ok(())
            )
            .unwrap(),
        first
    );
    assert_eq!(
        store.describe(id(1)).unwrap().unwrap().heads[MAIN],
        Some(id(11))
    );
}
#[test]
fn renames_tags_and_branch_references_never_write_another_payload() {
    let (_temp, _dir, store) = fixture();
    store
        .save(
            mutation(2),
            full(main(None), id(10), body(b"x"), Some("first".into())),
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
    let state = store.describe(id(1)).unwrap().unwrap();
    assert_eq!(state.name, "renamed");
    assert_eq!(state.tags, ["已选"]);
    assert_eq!(state.heads["branch"], Some(id(10)));
    for (sequence, request) in [(2, id(3)), (3, id(4)), (4, id(5))] {
        assert!(
            !store
                .volume
                .container(id(1))
                .join(entry_leaf(sequence, request))
                .join("payload")
                .exists()
        );
    }
    assert_eq!(read_node(&store, 10), b"x");
}

// Leave an R copy ready but revoke the final publication authority.
fn leave_prepared_recycle_unselected(store: &Store, original: &Path) {
    let object = store.object(id(1)).unwrap();
    let mut memory = object.lock().unwrap();
    let state = store.volume.load(id(1), &mut memory).unwrap();
    let archive = store.archive.as_ref().unwrap();
    let incoming = store.stage_at(archive.staging().unwrap()).unwrap();
    let tentative = archive
        .prepare(
            mutation(6),
            store.volume.root(),
            original,
            state,
            &incoming,
            || Ok(()),
        )
        .unwrap();
    let selector = store.stage_at(archive.moves().unwrap()).unwrap();
    assert!(
        archive
            .choose(tentative, &selector, || Err(
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
    let store = Store::open_with_recycle(held(&system), held(&ordinary), held(&recycle)).unwrap();
    let first = store
        .save(
            mutation(2),
            full(main(None), id(10), body(b"one"), Some("name".into())),
            &b"one"[..],
            || Ok(()),
        )
        .unwrap();
    store
        .save(
            mutation(3),
            full(main(Some(id(10))), id(11), body(b"two"), None),
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

    let original = store.volume.container(id(1));
    leave_prepared_recycle_unselected(&store, &original);
    assert_eq!(read_node(&store, 11), b"two");
    assert_eq!(store.describe(id(1)).unwrap().unwrap().name, "name");
    let recycled = store.move_to_recycle(mutation(6), 4, || Ok(())).unwrap();
    assert_eq!(recycled.original_sequence, 4);
    assert!(!original.exists());
    assert!(store.describe(id(1)).unwrap().is_none());
    assert!(store.node(id(1), id(10)).unwrap().is_none());
    assert!(store.list().unwrap().is_empty());

    let archive = store.archive.as_ref().unwrap();
    let (_receipt, moved) = archive.selected(id(1)).unwrap().unwrap();
    let state = read_state(&moved, id(1)).unwrap();
    assert_eq!(state.name.as_deref(), Some("name"));
    assert_eq!(state.tags, ["known"]);
    assert_eq!(state.branches[MAIN], Some(id(11)));
    assert_eq!(state.branches["older"], Some(id(10)));
    for (node, payload) in [(10, b"one"), (11, b"two")] {
        let exact = &state.nodes[&id(node)];
        let path = moved
            .join(entry_leaf(exact.sequence, exact.request))
            .join("payload");
        assert_eq!(fs::read(path).unwrap(), payload);
    }
    // Simulate F path/name being unavailable, with independent SYSTEM and R
    // roots still held. Exact receipts make no ordinary path lookup.
    fs::rename(&ordinary, local.0.join("ordinary-offline")).unwrap();
    assert_eq!(store.recycle_receipt(mutation(6)).unwrap(), Some(recycled));
    assert_eq!(store.receipt(mutation(2)).unwrap(), Some(first));
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
    let store = Store::open_with_recycle(held(&system), held(&ordinary), held(&recycle)).unwrap();
    store
        .save(
            mutation(2),
            full(main(None), id(10), body(b"one"), Some("name".into())),
            &b"one"[..],
            || Ok(()),
        )
        .unwrap();
    store
        .save(
            mutation(3),
            full(main(Some(id(10))), id(11), body(b"two"), None),
            &b"two"[..],
            || Ok(()),
        )
        .unwrap();
    let original = store.volume.container(id(1));
    let first = original.join(entry_leaf(1, id(2)));
    let second = original.join(entry_leaf(2, id(3)));
    {
        let source = store.object(id(1)).unwrap();
        let mut memory = source.lock().unwrap();
        let state = store.volume.load(id(1), &mut memory).unwrap();
        let archive = store.archive.as_ref().unwrap();
        let incoming = store.stage_at(archive.staging().unwrap()).unwrap();
        archive
            .prepare(
                mutation(6),
                store.volume.root(),
                &original,
                state,
                &incoming,
                || Ok(()),
            )
            .unwrap();
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
    assert_eq!(store.recycle_receipt(mutation(6)).unwrap(), Some(receipt));
    assert!(store.describe(id(1)).unwrap().is_none());
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
    let error = Store::open_with_recycle(held(&system), original, held(&recycle))
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
        full(main(None), id(10), body(b"one"), Some("first".into())),
        &b"one"[..],
        || Ok(()),
    );
    assert!(matches!(result,Err(Error::Io(ref e)) if e.to_string().contains("replaced")));
    for path in [staging.as_path(), displaced.as_path(), store.volume.data()] {
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
    let open = || Store::open_with_recycle(held(&system), held(&ordinary), held(&recycle)).unwrap();
    let store = open();
    store
        .save(
            mutation(2),
            full(main(None), id(10), body(b"one"), Some("name".into())),
            &b"one"[..],
            || Ok(()),
        )
        .unwrap();
    let receipt = store.move_to_recycle(mutation(6), 1, || Ok(())).unwrap();
    let moves = store.archive.as_ref().unwrap().moves().unwrap();
    let selected = moves.join(hex(id(1)));
    let staged = moves.join("selector-cut");
    // Recreate the externally visible namespace at the post-rename
    // boundary; this fixture models a service stop, not physical power loss.
    write_file(&staged, &fs::read(&selected).unwrap()).unwrap();
    fs::rename(&staged, &selected).unwrap();
    drop(store);
    let opened = open();
    assert_eq!(opened.recycle_receipt(mutation(6)).unwrap(), Some(receipt));
    assert!(opened.describe(id(1)).unwrap().is_none());
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
    let store = Store::open_with_recycle(held(&system), held(&ordinary), held(&recycle)).unwrap();
    store
        .save(
            mutation(2),
            full(main(None), id(10), body(b"one"), Some("name".into())),
            &b"one"[..],
            || Ok(()),
        )
        .unwrap();
    let archive = store.archive.as_ref().unwrap();
    let original = store.volume.container(id(1));
    {
        let object = store.object(id(1)).unwrap();
        let mut memory = object.lock().unwrap();
        let state = store.volume.load(id(1), &mut memory).unwrap();
        let incoming = store.stage_at(archive.staging().unwrap()).unwrap();
        archive
            .prepare(
                mutation(6),
                store.volume.root(),
                &original,
                state,
                &incoming,
                || Ok(()),
            )
            .unwrap();
    }
    assert!(store.recycle_receipt(mutation(6)).unwrap().is_none());
    store
        .save(
            mutation(3),
            full(main(Some(id(10))), id(11), body(b"two"), None),
            &b"two"[..],
            || Ok(()),
        )
        .unwrap();
    let done = store.move_to_recycle(mutation(7), 2, || Ok(())).unwrap();
    assert_eq!(store.recycle_receipt(mutation(7)).unwrap(), Some(done));
    assert!(store.recycle_receipt(mutation(6)).unwrap().is_none());
    let (_, chosen) = archive.selected(id(1)).unwrap().unwrap();
    assert_eq!(
        read_state(&chosen, id(1)).unwrap().branches[MAIN],
        Some(id(11))
    );
    assert!(!original.exists());
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
        format!("{:?}", Conflict::HeadChanged(Some(id(1)))),
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
            full(main(None), id(10), body(b"saved"), Some("name".into())),
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
    let reopened = Store::open(&root).unwrap();
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
    assert!(reopened.receipt(mutation(3)).unwrap().is_none());
}
