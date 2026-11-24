//! Private held-namespace gate, not a real Helper/mount qualification.
use super::*;
use crate::test_support::PrivateTemp;
use std::{fs, os::unix::fs::symlink};

#[test]
fn move_guard_never_admits_another_operation_or_follows_an_unknown_stage() {
    let temp = PrivateTemp::under(&std::env::temp_dir());
    let state = HeldDir::trusted_storage(&temp.0).unwrap();
    let request = MoveRequest::checked([1; 16], [2; 16], ([3; 32], [4; 32])).unwrap();
    let active = request.active_name().unwrap();
    let active = temp.0.join(active.to_str().unwrap());
    fs::create_dir(temp.0.join(".move-guard-draft-inactive")).unwrap();
    require_move_absent(&state).unwrap();
    require_stage_absent(&state).unwrap();
    fs::write(&active, b"native content follows the gate").unwrap();
    assert!(require_move_absent(&state).is_err());
    assert!(require_stage_absent(&state).is_err());
    require_move_scope(&state, request).unwrap();
    let foreign = temp.0.join(".server-stage-move-unknown");
    symlink("nothing", &foreign).unwrap();
    assert!(require_move_scope(&state, request).is_err());
    fs::remove_file(&foreign).unwrap();
    let retiring = temp.0.join(".server-stage-retire-data");
    fs::create_dir(&retiring).unwrap();
    assert!(require_move_scope(&state, request).is_err());
    fs::remove_file(active).unwrap();
    require_move_absent(&state).unwrap(); // normal/old gates add only the new interlock
    assert!(require_stage_absent(&state).is_err());
}
