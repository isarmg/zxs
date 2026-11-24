use super::*;
use crate::{native_test_support as support, r3::DisplayName, session::Principal};
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

fn fixture() -> (Workspace, PathBuf, PathBuf) {
    let temporary = support::fixture("ordinary-reopen");
    let state = temporary.join("private");
    let file = temporary.join("edited.txt");
    fs::create_dir(&state).unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(&file, b"version to freeze").unwrap();
    let principal: Principal = serde_json::from_str(
        r#"{"origin":"https://no-contact.invalid:443","account":"fixture","trust":"synthetic"}"#,
    )
    .unwrap();
    let workspace = Workspace::initialize(&state, &principal).unwrap();
    (workspace, state, file)
}

#[test]
#[ignore = "requires newly allocated unprivileged ext4 fixture scratch"]
fn reopening_preserves_the_exact_freeze_and_the_under_lease_attempt_cut() {
    let (mut creator, state, file) = fixture();
    let review = creator
        .review(&file, Some(DisplayName::parse("original").unwrap()))
        .unwrap();
    let intent = creator.prepare(&review, true).unwrap().unwrap();
    drop(creator);
    fs::write(&file, b"later work must survive").unwrap();

    let reopened = Workspace::open_local(&state).unwrap();
    let op = &intent.operation_id;
    let selected = reopened.review_prepared_submit(op).unwrap();
    assert_eq!(selected.intent(), &intent);
    assert_eq!(selected.working_path(), file.to_str().unwrap());
    let fence = FullSend::Prepared(&selected);
    let (exact, phase, prior, lease) = fence.hold(&reopened).unwrap();
    assert_eq!(exact.intent, intent);
    assert_eq!((phase.as_str(), prior), ("prepared", None));
    for old_phase in ["unknown", "absent", "conflict", "confirmed"] {
        assert!(fence.require(&exact, old_phase).is_err());
    }
    drop(lease);

    // Another cooperative shell arms the same Original after the GUI preview.
    // Neither the Intent nor the already-selected frozen pathname changes.
    let other = Workspace::open_local(&state).unwrap();
    assert!(fence.hold(&other).is_err());
    let lease = other.hold_container(&intent.container_id).unwrap();
    let source = other.root.source(Path::new(&payload(op))).unwrap();
    other.arm_attempt(&exact, &source, &lease).unwrap();
    drop(lease);
    assert!(fence.hold(&reopened).is_err());
    assert!(reopened.review_prepared_submit(op).is_err());
    assert!(reopened.review_prepared_cancellation(op).is_err());
    assert_eq!(reopened.intent(op).unwrap(), intent);
    source.check().unwrap();
    assert_eq!(fs::read(file).unwrap(), b"later work must survive");
}

#[test]
#[ignore = "requires newly allocated unprivileged ext4 fixture scratch"]
fn an_unstarted_resumable_original_cannot_enter_the_ordinary_lane() {
    let (mut creator, state, file) = fixture();
    let review = creator
        .review_resumable(&file, Some(DisplayName::parse("segmented").unwrap()))
        .unwrap();
    let intent = creator.prepare_resumable(&review, true).unwrap().unwrap();
    drop(creator);
    let reopened = Workspace::open_local(&state).unwrap();
    let op = &intent.operation_id;
    assert!(reopened.review_prepared_submit(op).is_err());
    assert!(
        !reopened
            .review_prepared_cancellation(op)
            .unwrap()
            .is_ordinary_full_file()
    );
    assert_eq!(
        reopened.choose_upload_for_send(op).unwrap().intent(),
        &intent
    );
}
