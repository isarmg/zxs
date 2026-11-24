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
        ws.retain_prepared(&review_b, second, &anonymous, &old_b)
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
