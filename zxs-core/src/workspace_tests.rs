use super::{
    APP, DB, DB_SIDECARS, DisplayName, Id, Intent, Lookup, PayloadWorkspace, Permissions, Target,
    VERSION, Workspace, can_arm_original, payload, preserve_original_conflict,
    validate_workspace_header,
};
use crate::{native_test_support as support, session::Principal};
use std::{
    fs,
    os::unix::fs::{FileExt, PermissionsExt},
    path::{Path, PathBuf},
};

fn replace_lease(state: &Path, name: &Id) {
    let selected = state.join(format!("lease-{name}"));
    let detached = state.join(format!("detached-{name}"));
    assert!(!detached.exists());
    fs::rename(selected, detached).unwrap();
}

fn scratch_workspace(fixture: &str) -> (Workspace, PathBuf, PathBuf, PathBuf) {
    let dir = support::fixture(fixture);
    let state = dir.join("private");
    fs::create_dir(&state).unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    let principal: Principal = serde_json::from_str(
        r#"{"origin":"https://no-contact.invalid:443","account":"fixture","trust":"synthetic"}"#,
    )
    .unwrap();
    let a = dir.join("a.txt");
    let b = dir.join("b.txt");
    fs::write(&a, b"synthetic a").unwrap();
    fs::write(&b, b"synthetic b").unwrap();
    (
        Workspace::initialize(&state, &principal).unwrap(),
        state,
        a,
        b,
    )
}

#[test]
#[ignore = "requires newly allocated unprivileged ext4 fixture scratch"]
fn detached_lease_refuses_local_and_wire_custody() {
    let (mut ws, state, a, b) = scratch_workspace("workspace-lease");
    let name = DisplayName::parse("first").unwrap();
    let review_a = ws.review(&a, Some(name.clone())).unwrap();
    let intent = ws.prepare(&review_a, true).unwrap().unwrap();
    let op = &intent.operation_id;
    let (original, phase, receipt) = ws.original(op).unwrap();
    assert_eq!((phase.as_str(), receipt), ("prepared", None));
    let staged = ws.root.source(Path::new(&payload(op))).unwrap();
    let old = ws.hold_container(&intent.container_id).unwrap();
    old.check().unwrap();
    replace_lease(&state, &intent.container_id);
    let next = ws.hold_container(&intent.container_id).unwrap();
    assert!(old.check().is_err());
    next.check().unwrap();

    assert!(ws.arm_attempt(&original, &staged, &old).is_err());
    assert_eq!(ws.original(op).unwrap().1, "prepared");

    // A valid newly acquired lease can arm the exact same operation when it is
    // independently approved. No session or server is created in this fixture.
    ws.arm_attempt(&original, &staged, &next).unwrap();
    ws.check_attempt(&original, &staged, &next).unwrap();
    assert!(ws.check_attempt(&original, &staged, &old).is_err());
    for status in [Lookup::Unknown, Lookup::Confirmed { sequence: 1 }] {
        assert!(ws.settle(&original, status, &old).is_err());
        assert_eq!(ws.original(op).unwrap().1, "attempted");
        assert!(ws.basis(&a).unwrap().is_none());
        staged.check().unwrap();
    }

    let review_b = ws.review(&b, Some(name.clone())).unwrap();
    let second = Intent {
        container_id: Id::fresh().unwrap(),
        operation_id: Id::fresh().unwrap(),
        node_id: Id::fresh().unwrap(),
        seen_sequence: 0,
        target: Target::FirstMain { name },
        body: review_b.body.clone(),
    };
    second.validate().unwrap();
    let old_b = ws.hold_container(&second.container_id).unwrap();
    let mut anonymous = ws.root.unlinked().unwrap();
    assert_eq!(review_b.source.scan(&mut anonymous).unwrap(), second.body);
    anonymous
        .set_permissions(Permissions::from_mode(0o400))
        .unwrap();
    replace_lease(&state, &second.container_id);
    let next_b = ws.hold_container(&second.container_id).unwrap();
    next_b.check().unwrap();
    let second_leaf = payload(&second.operation_id);
    assert!(
        ws.retain_prepared(&review_b, second, &anonymous, &old_b, false)
            .is_err()
    );
    ws.root.absent(second_leaf.as_ref()).unwrap();
    assert_eq!(ws.pending().unwrap(), vec![op.clone()]);

    let result = ws
        .settle(&original, Lookup::Confirmed { sequence: 1 }, &next)
        .unwrap();
    assert!(!result.local_cleanup_pending);
    assert_eq!(ws.basis(&a).unwrap().unwrap().node, intent.node_id);
    ws.root.absent(payload(op).as_ref()).unwrap();
    assert_eq!(fs::read(a).unwrap(), b"synthetic a");
}

#[test]
fn payload_requires_both_raw_header_modes_to_be_rollback() {
    let mut header = [0_u8; 100];
    header[..16].copy_from_slice(b"SQLite format 3\x00");
    header[60..64].copy_from_slice(&(VERSION as u32).to_be_bytes());
    header[68..72].copy_from_slice(&(APP as u32).to_be_bytes());
    for mode in [[1, 1], [2, 2], [1, 2], [2, 1], [0, 1], [1, 0]] {
        header[18..20].copy_from_slice(&mode);
        assert_eq!(
            validate_workspace_header(&header, true).is_ok(),
            mode == [1, 1]
        );
        // The new marker condition does not change ordinary/receipt Workspace open.
        assert!(validate_workspace_header(&header, false).is_ok());
    }
}

#[test]
#[ignore = "requires newly allocated unprivileged ext4 fixture scratch"]
fn payload_reader_leaves_wal_marker_and_sidecars_untouched() {
    let (writer, state, _, _) = scratch_workspace("payload-rollback");
    drop(writer);
    let raw = state.join(DB);
    let names = || {
        let mut names: Vec<_> = fs::read_dir(&state)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort_unstable();
        names
    };
    let bare = names();
    assert_eq!(bare, [DB]);
    let reader = PayloadWorkspace::open_local(&state).unwrap();
    let rollback = fs::read(&raw).unwrap();
    assert_eq!(rollback[18..20], [1, 1]);

    for name in DB_SIDECARS {
        // Neither a recovery journal nor another auxiliary object is ours to open.
        let sidecar = state.join(name);
        let sidecar_bytes = b"original sidecar must not be repaired or removed";
        fs::write(&sidecar, sidecar_bytes).unwrap();
        let original_names = names();
        let Err(error) = PayloadWorkspace::open_local(&state) else {
            panic!("payload reader admitted sidecar {name}");
        };
        assert!(error.to_string().contains("without journal, WAL or SHM"));
        let checked = reader.checked_read::<()>(|| panic!("must refuse before querying SQLite"));
        assert!(
            checked
                .unwrap_err()
                .to_string()
                .contains("without journal, WAL or SHM")
        );
        assert_eq!(names(), original_names);
        assert_eq!(fs::read(&raw).unwrap(), rollback);
        assert_eq!(fs::read(&sidecar).unwrap(), sidecar_bytes);
        fs::remove_file(sidecar).unwrap();
    }
    assert_eq!(names(), bare);
    drop(reader);

    let original = fs::OpenOptions::new().write(true).open(&raw).unwrap();
    original.write_all_at(&[2, 2], 18).unwrap();
    original.sync_all().unwrap();
    drop(original);
    let wal_marked = fs::read(&raw).unwrap();
    assert_eq!(wal_marked[18..20], [2, 2]);
    let Err(error) = PayloadWorkspace::open_local(&state) else {
        panic!("payload reader admitted a WAL-marked DB with no sidecars");
    };
    assert!(error.to_string().contains("rollback-mode"));
    assert_eq!(names(), bare);
    assert_eq!(fs::read(raw).unwrap(), wal_marked);
}

#[test]
fn conflict_cannot_be_cleared_by_any_nonreceipt_observation() {
    for seen in [Lookup::Absent, Lookup::Unknown, Lookup::Conflict] {
        assert_eq!(
            preserve_original_conflict("conflict", seen),
            Lookup::Conflict
        );
    }
    assert!(!can_arm_original("conflict"));
    assert!(!can_arm_original("confirmed"));
    assert!(!can_arm_original("unrecognized"));
    let exact = Lookup::Confirmed { sequence: 2 };
    assert_eq!(preserve_original_conflict("conflict", exact), exact);
    for phase in ["prepared", "attempted", "unknown", "absent"] {
        assert!(can_arm_original(phase));
        for seen in [Lookup::Absent, Lookup::Unknown, Lookup::Conflict] {
            assert_eq!(preserve_original_conflict(phase, seen), seen);
        }
    }
}

#[test]
#[ignore = "requires newly allocated unprivileged ext4 fixture scratch"]
fn sticky_conflict_preserves_the_original_until_its_exact_receipt() {
    let (mut ws, _, file, _) = scratch_workspace("sticky-conflict");
    let review = ws
        .review(&file, Some(DisplayName::parse("first").unwrap()))
        .unwrap();
    let intent = ws.prepare(&review, true).unwrap().unwrap();
    let op = &intent.operation_id;
    let (original, _, _) = ws.original(op).unwrap();
    let lease = ws.hold_container(&intent.container_id).unwrap();
    let staged = ws.root.source(Path::new(&payload(op))).unwrap();

    for observation in [Lookup::Unknown, Lookup::Absent] {
        assert_eq!(
            ws.settle(&original, observation, &lease).unwrap().status,
            observation
        );
        assert_eq!(ws.original(op).unwrap().1, "prepared");
    }
    ws.arm_attempt(&original, &staged, &lease).unwrap();
    for observation in [Lookup::Unknown, Lookup::Absent] {
        assert_eq!(
            ws.settle(&original, observation, &lease).unwrap().status,
            observation
        );
        assert_eq!(ws.original(op).unwrap().1, "attempted");
    }

    assert_eq!(
        ws.settle(&original, Lookup::Conflict, &lease)
            .unwrap()
            .status,
        Lookup::Conflict
    );
    for observation in [Lookup::Unknown, Lookup::Absent, Lookup::Conflict] {
        assert_eq!(
            ws.settle(&original, observation, &lease).unwrap().status,
            Lookup::Conflict
        );
        assert_eq!(ws.original(op).unwrap().1, "conflict");
        assert!(ws.arm_attempt(&original, &staged, &lease).is_err());
        assert_eq!(ws.pending().unwrap(), vec![op.clone()]);
        assert!(ws.basis(&file).unwrap().is_none());
        staged.check().unwrap();
    }
    fs::write(&file, b"later unsent local edit").unwrap();
    let receipt = Lookup::Confirmed { sequence: 1 };
    assert_eq!(
        ws.settle(&original, receipt, &lease).unwrap().status,
        receipt
    );
    assert_eq!(ws.basis(&file).unwrap().unwrap().body, intent.body);
    assert_eq!(fs::read(file).unwrap(), b"later unsent local edit");
    ws.root.absent(payload(op).as_ref()).unwrap();
}

#[test]
#[ignore = "requires newly allocated unprivileged ext4 fixture scratch"]
fn cancel_only_never_armed_keeps_work_and_confirmed_basis() {
    let (mut ws, _, file, _) = scratch_workspace("cancel-prepared");
    let review = ws
        .review(&file, Some(DisplayName::parse("first").unwrap()))
        .unwrap();
    let first = ws.prepare(&review, true).unwrap().unwrap();
    let (original, _, _) = ws.original(&first.operation_id).unwrap();
    let lease = ws.hold_container(&first.container_id).unwrap();
    ws.settle(&original, Lookup::Confirmed { sequence: 1 }, &lease)
        .unwrap();
    drop(lease);
    let basis = ws.basis(&file).unwrap().unwrap();

    fs::write(&file, b"older local version A").unwrap();
    let review = ws.review(&file, None).unwrap();
    let candidate = ws.prepare(&review, true).unwrap().unwrap();
    let op = &candidate.operation_id;
    let leaf = payload(op);
    let original_stage = ws.root.source(Path::new(&leaf)).unwrap();
    let later = file.with_file_name("later-work");
    fs::write(&later, b"newer edited bytes B").unwrap();
    fs::rename(&later, &file).unwrap();

    let choice = ws.review_prepared_cancellation(op).unwrap();
    assert_eq!(&candidate, choice.intent());
    assert_eq!(choice.working_path(), file.to_str().unwrap());
    assert!(ws.cancel_prepared(choice, false).is_err());
    original_stage.check().unwrap();

    let stale = ws.review_prepared_cancellation(op).unwrap();
    fs::rename(&file, &later).unwrap();
    assert!(ws.review_prepared_cancellation(op).is_err());
    assert!(ws.cancel_prepared(stale, true).is_err());
    original_stage.check().unwrap();
    fs::rename(&later, &file).unwrap();

    let choice = ws.review_prepared_cancellation(op).unwrap();
    ws.cancel_prepared(choice, true).unwrap();
    assert!(ws.pending().unwrap().is_empty());
    assert!(ws.intent(op).is_err());
    ws.root.absent(leaf.as_ref()).unwrap();
    assert_eq!(ws.basis(&file).unwrap(), Some(basis.clone()));
    assert_eq!(fs::read(&file).unwrap(), b"newer edited bytes B");

    // A subsequent plan is possible only after a new explicit review. Once
    // armed its proof, body and work survive even apparent absence/ambiguity.
    let next_review = ws.review(&file, None).unwrap();
    let next = ws.prepare(&next_review, true).unwrap().unwrap();
    assert_ne!(&next.operation_id, op);
    let (armed, _, _) = ws.original(&next.operation_id).unwrap();
    let next_lease = ws.hold_container(&next.container_id).unwrap();
    let retained = ws
        .root
        .source(Path::new(&payload(&next.operation_id)))
        .unwrap();
    ws.arm_attempt(&armed, &retained, &next_lease).unwrap();
    for status in [Lookup::Absent, Lookup::Unknown, Lookup::Conflict] {
        ws.settle(&armed, status, &next_lease).unwrap();
        assert!(ws.review_prepared_cancellation(&next.operation_id).is_err());
        retained.check().unwrap();
    }
    assert_eq!(ws.basis(&file).unwrap(), Some(basis));

    cancel_reopens_after_verified_unlink_and_refuses_substitution();
}

fn cancel_reopens_after_verified_unlink_and_refuses_substitution() {
    let (mut ws, state, file, _) = scratch_workspace("cancel-cold-stage");
    let name = DisplayName::parse("first").unwrap();
    let review = ws.review(&file, Some(name.clone())).unwrap();
    let pending = ws.prepare(&review, true).unwrap().unwrap();
    let op = &pending.operation_id;
    let leaf = payload(op);
    let (original, _, _) = ws.original(op).unwrap();
    let lease = ws.hold_container(&pending.container_id).unwrap();
    // Simulate interruption after guarded unlink/fsync but before DELETE.
    ws.root
        .remove_verified(&leaf, &pending.body, original.sealed, || lease.check())
        .unwrap();
    drop(lease);
    drop(ws);

    let mut ws = Workspace::open_local(&state).unwrap();
    assert_eq!(ws.pending().unwrap(), vec![op.clone()]);
    ws.root.absent(leaf.as_ref()).unwrap();
    let selected = ws.review_prepared_cancellation(op).unwrap();
    ws.cancel_prepared(selected, true).unwrap();
    assert!(ws.pending().unwrap().is_empty());
    assert_eq!(fs::read(&file).unwrap(), b"synthetic a");
    assert!(ws.basis(&file).unwrap().is_none());

    let review = ws.review(&file, Some(name)).unwrap();
    let pending = ws.prepare(&review, true).unwrap().unwrap();
    let op = &pending.operation_id;
    let frozen = state.join(payload(op));
    let held_copy = state.join("preserved-synthetic-original");
    let selected = ws.review_prepared_cancellation(op).unwrap();
    fs::rename(&frozen, &held_copy).unwrap();
    fs::write(&frozen, b"another ordinary inode, not the frozen original").unwrap();
    assert!(ws.cancel_prepared(selected, true).is_err());
    assert_eq!(ws.pending().unwrap(), vec![op.clone()]);
    assert_eq!(fs::read(&held_copy).unwrap(), b"synthetic a");
    assert_eq!(
        fs::read(&frozen).unwrap(),
        b"another ordinary inode, not the frozen original"
    );
    assert_eq!(fs::read(&file).unwrap(), b"synthetic a");
}

#[test]
#[ignore = "requires newly allocated unprivileged ext4 fixture scratch"]
fn resumable_limit_and_early_custody_never_strand_an_unreadable_pending_record() {
    let (mut ws, state, oversized, normal) = scratch_workspace("resumable-custody");
    let name = DisplayName::parse("first.bin").unwrap();
    // Sparse size only: the candidate must refuse before hashing or copying gigabytes.
    fs::OpenOptions::new()
        .write(true)
        .open(&oversized)
        .unwrap()
        .set_len((1u64 << 40) + 1)
        .unwrap();
    assert!(ws.review_resumable(&oversized, Some(name.clone())).is_err());
    assert_eq!(ws.pending().unwrap(), [] as [Id; 0]);
    let mut review = ws.review_resumable(&normal, Some(name)).unwrap();
    let real = review.body.length;
    review.body.length = (1u64 << 40) + 1;
    assert!(ws.prepare_resumable(&review, true).is_err());
    assert_eq!(ws.pending().unwrap(), [] as [Id; 0]);
    review.body.length = real;
    let intent = ws.prepare_resumable(&review, true).unwrap().unwrap();
    drop(review);
    drop(ws);
    let ws = Workspace::open_local(&state).unwrap();
    let op = &intent.operation_id;
    assert_eq!(ws.pending().unwrap().as_slice(), std::slice::from_ref(op));
    assert_eq!(ws.choose_upload_for_send(op).unwrap().intent(), &intent);
    let (original, phase, _) = ws.original(op).unwrap();
    assert_eq!(phase, "prepared");
    let source = ws.root.source(Path::new(&payload(op))).unwrap();
    let lease = ws.hold_container(&intent.container_id).unwrap();
    assert!(ws.arm_attempt(&original, &source, &lease).is_err()); // no legacy full-file substitute
    drop(lease);
    let canceled = ws.review_prepared_cancellation(op).unwrap();
    ws.cancel_prepared(canceled, true).unwrap();
    assert_eq!(ws.pending().unwrap(), [] as [Id; 0]);
    assert_eq!(fs::read(normal).unwrap(), b"synthetic b");
}

#[test]
#[ignore = "requires newly allocated unprivileged ext4 fixture scratch"]
fn archived_late_receipt_is_history_only_even_after_another_original_owns_the_container() {
    use super::conflicts::{ARCHIVED, latest_generation, stamp_checkout};

    let (mut ws, state, old, spare) = scratch_workspace("archive-fresh-checkout");
    let reviewed = ws
        .review(&old, Some(DisplayName::parse("file").unwrap()))
        .unwrap();
    let first = ws.prepare(&reviewed, true).unwrap().unwrap();
    let lease = ws.hold_container(&first.container_id).unwrap();
    let base_original = ws.original(&first.operation_id).unwrap().0;
    ws.settle(&base_original, Lookup::Confirmed { sequence: 1 }, &lease)
        .unwrap();
    drop(lease);
    let old_basis = ws.basis(&old).unwrap().unwrap();
    let raw = serde_json::to_string(&old_basis).unwrap();
    ws.db
        .execute(
            "INSERT INTO basis VALUES(?1,?2)",
            (spare.to_str().unwrap(), &raw),
        )
        .unwrap();
    let previous_review = ws.review(&spare, None).unwrap();
    fs::write(&old, b"frozen old conflict").unwrap();
    let edited = ws.review(&old, None).unwrap();
    let intent = ws.prepare(&edited, true).unwrap().unwrap();
    let op = &intent.operation_id;
    let seal = state.join(payload(op));
    assert!(ws.review_conflict_archive(op).is_err());
    let lease = ws.hold_container(&intent.container_id).unwrap();
    let original = ws.original(op).unwrap().0;
    ws.settle(&original, Lookup::Conflict, &lease).unwrap();
    assert_eq!(
        ws.settle(&original, Lookup::Absent, &lease).unwrap().status,
        Lookup::Conflict
    );
    assert_eq!(ws.pending().unwrap().as_slice(), std::slice::from_ref(op));
    drop(lease);
    fs::write(&old, b"ordinary later edits").unwrap();
    let draft = ws.review_conflict_archive(op).unwrap();
    assert_eq!(draft.intent(), &intent);
    assert!(ws.archive_conflict(draft, false).is_err());
    assert_eq!(ws.pending().unwrap().as_slice(), std::slice::from_ref(op));
    let selected = ws.review_conflict_archive(op).unwrap();
    ws.archive_conflict(selected, true).unwrap();
    drop(ws);

    let mut ws = Workspace::open_local(&state).unwrap();
    assert_eq!(ws.pending().unwrap(), [] as [Id; 0]);
    assert_eq!(
        ws.archived_conflicts().unwrap().as_slice(),
        std::slice::from_ref(op)
    );
    assert_eq!(ws.intent(op).unwrap(), intent);
    assert_eq!(ws.basis(&old).unwrap().as_ref(), Some(&old_basis));
    assert_eq!(fs::read(&old).unwrap(), b"ordinary later edits");
    assert_eq!(fs::read(&seal).unwrap(), b"frozen old conflict");
    // Offline inspection and the older files still work; they cannot stage bytes for a new write.
    let read_old = ws.review(&old, None).unwrap();
    assert!(read_old.changed());
    assert!(ws.prepare(&read_old, true).is_err());
    assert!(ws.prepare(&previous_review, true).is_err());
    assert!(ws.db.execute(
        "INSERT INTO original(op,container,path,record,phase) VALUES(?1,?2,?3,?4,'prepared')",
        (Id::fresh().unwrap().as_str(), intent.container_id.as_str(), spare.to_str().unwrap(),
            serde_json::to_string(&original).unwrap())
    ).is_err()); // Even a pre-upgrade writer cannot occupy the released slot.

    // Simulate the exact new checkout transaction, not a rebind of either old path.
    let other = spare.with_file_name("new");
    fs::write(&other, b"new user merge").unwrap();
    let mut basis = old_basis.clone();
    basis.node = Id::fresh().unwrap();
    basis.seen_sequence = 3;
    let at = other.to_str().unwrap();
    let generation = latest_generation(&ws.db, &intent.container_id).unwrap();
    assert!(stamp_checkout(&ws.db, at, &intent.container_id, 0).is_err());
    ws.db
        .execute(
            "INSERT INTO basis VALUES(?1,?2)",
            (at, serde_json::to_string(&basis).unwrap()),
        )
        .unwrap();
    stamp_checkout(&ws.db, at, &intent.container_id, generation).unwrap();
    let new = ws
        .prepare(&ws.review(&other, None).unwrap(), true)
        .unwrap()
        .unwrap();
    assert_ne!(new.operation_id, *op);
    assert_eq!(new.target.expected_base(), Some(&basis.node));
    let lease = ws.hold_container(&intent.container_id).unwrap();
    assert_eq!(
        ws.settle(&original, Lookup::Confirmed { sequence: 2 }, &lease)
            .unwrap()
            .status,
        Lookup::Confirmed { sequence: 2 }
    );
    assert_eq!(
        ws.original(op).unwrap(),
        (original, ARCHIVED.into(), Some(2))
    );
    assert_eq!(ws.basis(&old).unwrap(), Some(old_basis));
    assert_eq!(ws.basis(&other).unwrap(), Some(basis));
    assert_eq!(ws.pending().unwrap(), [new.operation_id]);
    assert_eq!(fs::read(&seal).unwrap(), b"frozen old conflict");
}

#[test]
#[ignore = "requires newly allocated unprivileged ext4 fixture scratch"]
fn exact_receipt_before_archive_wins_over_the_stale_conflict_preview() {
    let (mut ws, _, file, _) = scratch_workspace("archive-preview-revoked");
    let reviewed = ws
        .review(&file, Some(DisplayName::parse("first").unwrap()))
        .unwrap();
    let intent = ws.prepare(&reviewed, true).unwrap().unwrap();
    let op = &intent.operation_id;
    let lease = ws.hold_container(&intent.container_id).unwrap();
    let original = ws.original(op).unwrap().0;
    ws.settle(&original, Lookup::Unknown, &lease).unwrap();
    assert!(ws.review_conflict_archive(op).is_err());
    ws.settle(&original, Lookup::Conflict, &lease).unwrap();
    let stale = ws.review_conflict_archive(op).unwrap();
    ws.settle(&original, Lookup::Confirmed { sequence: 1 }, &lease)
        .unwrap();
    drop(lease);
    assert!(ws.archive_conflict(stale, true).is_err());
    assert_eq!(ws.archived_conflicts().unwrap(), [] as [Id; 0]);
    assert_eq!(ws.basis(&file).unwrap().unwrap().node, intent.node_id);
    assert_eq!(ws.original(op).unwrap().2, Some(1));
}
