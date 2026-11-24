//! Normal interrupted-body cuts in disposable native Move fixtures.
use super::super::tests::{C, Fixture, RAW};
use super::*;
use crate::container_store::{entry_leaf, hex};
use std::{fs, io::Write};

fn old(f: &Fixture) -> std::path::PathBuf {
    f.source
        .root
        .path
        .join("data")
        .join(hex(C))
        .join(entry_leaf(1, [0x77; 16]))
        .join("payload")
}

#[test]
fn same_op_restarts_a_durable_guard_after_empty_private_mkdir() {
    let f = Fixture::new();
    let live = || f.state.require_named();
    {
        let selected = f.selected(&live);
        let pending = Pending::prepare(&selected).unwrap();
        pending.arm().unwrap();
        selected
            .target_staging
            .new_child_directory(&selected.names.target_stage)
            .unwrap();
    }
    let selected = f.selected(&live);
    let ready = Pending::prepare(&selected)
        .unwrap()
        .copy_to_ready()
        .unwrap();
    assert_eq!(fs::read(old(&f)).unwrap(), RAW);
    assert!(!f.target.root.path.join("data").join(hex(C)).exists());
    let built = ready
        .draft
        .target
        .at
        .path
        .join(entry_leaf(1, [0x77; 16]))
        .join("payload");
    assert_eq!(fs::read(built).unwrap(), RAW);
    assert!(f.marker(true).exists());
}

#[test]
fn same_op_discards_only_the_typed_own_partial_body_after_proving_inventory() {
    let f = Fixture::new();
    let live = || f.state.require_named();
    let (output, surprise);
    {
        let selected = f.selected(&live);
        let plan = Pending::prepare(&selected).unwrap();
        let guard = plan.arm().unwrap();
        let armed = Armed { plan, guard };
        let stage = work::Work::open(&armed).unwrap().into_stage().unwrap();
        let name = crate::container_store::entry_leaf(1, [0x77; 16]);
        let leaf = super::super::super::native_name(&name).unwrap();
        let at = stage.new_child_directory(&leaf).unwrap();
        let mut raw = at.create_opaque_record(c"payload").unwrap();
        raw.write_all(&RAW[..4]).unwrap();
        raw.sync_all().unwrap();
        at.file.sync_all().unwrap();
        output = at.path.join("payload");
        surprise = stage.path.join("not-ours");
        fs::write(&surprise, b"hold this separately").unwrap();
    }
    {
        let scope = f.selected(&live);
        assert!(Pending::prepare(&scope).unwrap().copy_to_ready().is_err());
        assert_eq!(fs::read(&surprise).unwrap(), b"hold this separately");
        assert_eq!(fs::read(&output).unwrap(), &RAW[..4]);
        assert_eq!(fs::read(old(&f)).unwrap(), RAW);
    }
    fs::remove_file(surprise).unwrap(); // simulate removal of our foreign fixture, not product cleanup
    let scope = f.selected(&live);
    Pending::prepare(&scope).unwrap().copy_to_ready().unwrap();
    assert_eq!(fs::read(output).unwrap(), RAW);
    assert_eq!(fs::read(old(&f)).unwrap(), RAW);
    assert!(f.marker(true).exists());
}

#[test]
fn later_phase_drafts_cannot_choose_or_discard_original_data() {
    let f = Fixture::new();
    let live = || f.state.require_named();
    let scope = f.selected(&live);
    let staged = Pending::prepare(&scope)
        .unwrap()
        .copy_to_ready()
        .unwrap()
        .arrive()
        .unwrap();
    let journal = &staged.armed.plan.journal;
    let early = journal.scoped_leaf(c"SWITCHED", true).unwrap();
    journal.parent.create_opaque_record(&early).unwrap();
    let path = journal.parent.checked_child_path(&early).unwrap();
    let canonical = scope.routes.checked_child_path(&scope.names.cid).unwrap();
    let old_route = fs::read(&canonical).unwrap();
    assert!(staged.switch().is_err());
    assert_eq!(fs::read(canonical).unwrap(), old_route);
    assert_eq!(fs::read(old(&f)).unwrap(), RAW);
    fs::remove_file(path).unwrap();

    let ready = HeldReady::reopen(&scope).unwrap().arrive().unwrap();
    let journal = &ready.armed.plan.journal;
    let early = journal.scoped_leaf(c"DONE", true).unwrap();
    let path = journal.parent.checked_child_path(&early).unwrap();
    let switched = ready.switch().unwrap();
    fs::File::create_new(path).unwrap();
    assert!(switched.complete().is_err());
    assert_eq!(fs::read(old(&f)).unwrap(), RAW);
    assert!(f.marker(true).exists());
}

#[test]
fn complete_body_without_receipt_can_be_safely_redone_if_its_hash_matches() {
    let f = Fixture::new();
    let live = || f.state.require_named();
    let output = {
        let selected = f.selected(&live);
        let plan = Pending::prepare(&selected).unwrap();
        let guard = plan.arm().unwrap();
        let armed = Armed { plan, guard };
        let stage = work::Work::open(&armed).unwrap().into_stage().unwrap();
        let leaf = super::super::super::native_name(&entry_leaf(1, [0x77; 16])).unwrap();
        let at = stage.new_child_directory(&leaf).unwrap();
        let mut raw = at.create_opaque_record(c"payload").unwrap();
        raw.write_all(RAW).unwrap();
        raw.sync_all().unwrap();
        assert!(!at.path.join("receipt").exists());
        at.path.join("payload")
    };
    let selected = f.selected(&live);
    Pending::prepare(&selected)
        .unwrap()
        .copy_to_ready()
        .unwrap();
    assert!(output.with_file_name("receipt").exists());
    assert_eq!(fs::read(output).unwrap(), RAW);
    assert_eq!(fs::read(old(&f)).unwrap(), RAW);
}
