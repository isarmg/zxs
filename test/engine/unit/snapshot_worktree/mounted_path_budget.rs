//! Fresh mount admission is checked on the held original, not a replay-only model.
use super::{Fixture, principal, request};
use crate::Engine;
use name_policy::{LookupPath, Name};
use std::{io::Read, sync::Arc};
use storage_contract::{ErrorCode, OperationId};
use storage_contract::snapshot_worktree::{CreateWorktree, WorktreeHandle};
use storage_contract::snapshot_worktree::creation::AllocateWorktree;

fn named(automatic: &AllocateWorktree, name: &str) -> CreateWorktree {
    CreateWorktree {
        source: automatic.source,
        parent_directory_id: automatic.parent_directory_id,
        expected_root_generation: automatic.expected_root_generation,
        operation_id: OperationId::generate().unwrap(),
        name: Name::parse(name).unwrap(),
    }
}

fn reject_both(
    engine: &mut Engine, fixture: &Fixture, automatic: &AllocateWorktree, name: &str,
    code: ErrorCode,
) {
    let manual = named(automatic, name);
    let before = fixture.media();
    let state = Arc::clone(&engine.state);
    let failure = engine.create_directory_worktree(&manual, &principal()).unwrap_err();
    assert_eq!(failure.code(), code);
    assert!(failure.published_operation().is_none());
    let failure = engine.allocate_directory_worktree(automatic, &principal()).unwrap_err();
    assert_eq!(failure.code(), code);
    assert!(failure.published_operation().is_none());
    assert!(Arc::ptr_eq(&state, &engine.state));
    assert!(!engine.state().operations.contains_key(&manual.operation_id));
    assert!(!engine.state().operations.contains_key(&automatic.operation_id));
    assert_eq!(fixture.media(), before);
}

fn read_original(engine: &Engine, target: WorktreeHandle, path: &str) {
    let mut original = engine.open_directory_worktree_content(
        target, &LookupPath::parse(path).unwrap(),
    ).unwrap();
    let mut bytes = Vec::new();
    original.file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"unchanged nested original");
}

#[test]
fn depth_32_is_servable_but_a_33rd_component_never_reserves_a_mount() {
    for count in [30, 31] {
        let parent = vec!["d"; count].join("/");
        let relative = format!("{parent}/existing.txt");
        let (fixture, mut engine) = Fixture::with_entry_parent(Some(&parent));
        let automatic = request(&fixture, &engine);
        if count == 31 {
            reject_both(&mut engine, &fixture, &automatic, "w", ErrorCode::PathTooDeep);
        } else {
            let target = engine.allocate_directory_worktree(&automatic, &principal()).unwrap();
            read_original(&engine, target, &relative);
        }
    }
}

#[test]
fn fresh_mount_uses_its_real_prefix_and_accepts_the_exact_utf16_boundary() {
    let parent = format!("{}/{}", "é".repeat(92), "b".repeat(92));
    let relative = format!("{parent}/existing.txt");
    assert_eq!(format!("w/{relative}").encode_utf16().count(), 200);
    let (fixture, mut engine) = Fixture::with_entry_parent(Some(&parent));
    let automatic = request(&fixture, &engine);
    reject_both(&mut engine, &fixture, &automatic, "ww", ErrorCode::PathInvalid);
    let shorter = named(&automatic, "w");
    let target = engine.create_directory_worktree(&shorter, &principal()).unwrap();
    read_original(&engine, target, &relative);
    drop(engine);
    let mut engine = Engine::open(&fixture.system).unwrap();
    let before = fixture.media();
    assert_eq!(engine.create_directory_worktree(&shorter, &principal()).unwrap(), target);
    read_original(&engine, target, &relative);
    assert_eq!(fixture.media(), before);
}
