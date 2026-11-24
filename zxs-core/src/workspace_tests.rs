use super::{DisplayName, Id, Intent, Lookup, Permissions, Target, Workspace, payload};
use crate::{native_test_support as support, session::Principal};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

fn replace_lease(state: &Path, name: &Id) {
    let selected = state.join(format!("lease-{name}"));
    let detached = state.join(format!("detached-{name}"));
    assert!(!detached.exists());
    fs::rename(selected, detached).unwrap();
}

fn scratch_workspace() -> (Workspace, PathBuf, PathBuf, PathBuf) {
    let dir = support::fixture("workspace-lease");
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
    let (mut ws, state, a, b) = scratch_workspace();
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
