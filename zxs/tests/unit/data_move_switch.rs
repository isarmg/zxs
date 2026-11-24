//! Private CR1 cut and exact cold SWITCHED fixtures. No source retirement.
use super::super::ready::{HeldReady, switch::HeldSwitch};
use super::*;
use std::os::unix::fs::MetadataExt;

fn incoming(f: &Fixture) -> PathBuf {
    f.system
        .path
        .join("container-routes")
        .join(format!(".{}.incoming", hex(C)))
}

fn target(f: &Fixture) -> PathBuf {
    f.target.root.path.join("data").join(hex(C))
}

fn branched_route(token: Digest) -> [u8; 52] {
    crate::container_store::routes::encode(C, token).unwrap()
}

fn preserved_source(f: &Fixture) {
    assert_eq!(fs::read(f.entry().join("payload")).unwrap(), RAW);
    assert_eq!(
        fs::read(
            target(f)
                .join(f.entry().file_name().unwrap())
                .join("payload")
        )
        .unwrap(),
        RAW
    );
    assert!(f.marker(true).is_file());
    assert!(!f.journal().join("DONE").exists());
}

fn arrived<'held, 'scope>(selected: &'held Selected<'scope>) -> HeldReady<'held, 'scope> {
    Pending::prepare(selected)
        .unwrap()
        .copy_to_ready()
        .unwrap()
        .arrive()
        .unwrap()
}

fn visible_new(ready: HeldReady<'_, '_>) {
    let result = ready.switch_with(
        || Ok(()),
        |_| Err(io::Error::other("fixture routes flush interrupted")),
        |_| panic!("the SWITCHED parent is not touched before the routes flush"),
    );
    assert!(result.is_err());
}

#[test]
fn switch_needs_p2_and_reuses_exact_candidate_in_both_old_entry_paths() {
    for cold in [false, true] {
        let f = Fixture::new();
        let live = || f.state.require_named();
        let selected = f.selected(&live);
        let ready = Pending::prepare(&selected)
            .unwrap()
            .copy_to_ready()
            .unwrap();
        assert!(ready.switch().is_err());
        assert!(HeldSwitch::resume(&selected).is_err());
        assert_eq!(fs::read(f.route()).unwrap(), branched_route(FROM));

        let ready = HeldReady::reopen(&selected).unwrap().arrive().unwrap();
        let actual = fs::metadata(incoming(&f)).unwrap().ino();
        let was = fs::metadata(f.route()).unwrap().ino();
        assert_ne!(actual, was);
        let switched = if cold {
            drop(ready);
            HeldSwitch::resume(&selected).unwrap()
        } else {
            ready.switch().unwrap()
        };
        assert_eq!(fs::metadata(f.route()).unwrap().ino(), actual);
        assert_eq!(fs::read(f.route()).unwrap(), branched_route(TO));
        assert!(!incoming(&f).exists());
        assert!(!f.journal().join("new-route").exists());
        assert!(!f.stage_path().exists());
        let ready_raw = fs::read(f.journal().join("READY")).unwrap();
        let marker = fs::read(f.journal().join("SWITCHED")).unwrap();
        assert_eq!(&marker[..8], b"ZXDMS1\0\0");
        assert_eq!(&marker[8..40], &ready_raw[204..]);
        assert_eq!(&marker[40..], &digest(&marker[..40]).unwrap());
        assert_eq!(&marker[40..], &switched.id().unwrap());
        assert_eq!(
            HeldSwitch::resume(&selected).unwrap().id().unwrap(),
            switched.id().unwrap()
        );
        preserved_source(&f);
    }
}

#[test]
fn a_failed_scoped_routes_flush_is_not_switched_but_recovers_the_exact_new_name() {
    let f = Fixture::new();
    let live = || f.state.require_named();
    let selected = f.selected(&live);
    let ready = arrived(&selected);
    let inode = fs::metadata(incoming(&f)).unwrap().ino();
    let observed = Cell::new(false);
    let result = ready.switch_with(
        || Ok(()),
        |file| {
            same_file(&selected.routes, &selected.routes.file, file).unwrap();
            assert_eq!(fs::metadata(f.route()).unwrap().ino(), inode);
            assert_eq!(fs::read(f.route()).unwrap(), branched_route(TO));
            assert!(!incoming(&f).exists());
            assert!(!f.journal().join("SWITCHED").exists());
            observed.set(true);
            Err(io::Error::other("fixture selected routes flush cut"))
        },
        |_| panic!("cannot flush SWITCHED while routes fsync failed"),
    );
    assert!(result.is_err());
    assert!(observed.get());
    assert!(!f.journal().join("SWITCHED").exists());
    preserved_source(&f);
    HeldSwitch::resume(&selected).unwrap();
    assert_eq!(fs::metadata(f.route()).unwrap().ino(), inode);
    assert!(f.journal().join("SWITCHED").is_file());
    preserved_source(&f);
}

#[test]
fn old_candidate_or_target_substitution_at_late_cut_cannot_select_or_write_switched() {
    for which in 0..3 {
        let f = Fixture::new();
        let live = || f.state.require_named();
        let selected = f.selected(&live);
        let ready = arrived(&selected);
        let location = match which {
            0 => f.route(),
            1 => incoming(&f),
            _ => target(&f)
                .join(f.entry().file_name().unwrap())
                .join("payload"),
        };
        let backup = match which {
            2 => f.target.root.path.join("kept-exact-payload"),
            _ => f.system.path.join("kept-exact-route"),
        };
        let contents = fs::read(&location).unwrap();
        let result = ready.switch_with(
            || {
                fs::rename(&location, &backup)?;
                write_file(&location, &contents)
            },
            |_| panic!("cannot sync a selection that did not occur"),
            |_| panic!("cannot create SWITCHED after invalid preselection"),
        );
        assert!(result.is_err());
        assert_eq!(fs::read(&location).unwrap(), contents);
        assert_eq!(fs::read(&backup).unwrap(), contents);
        assert_eq!(fs::read(f.route()).unwrap(), branched_route(FROM));
        assert_eq!(fs::read(incoming(&f)).unwrap(), branched_route(TO));
        assert!(!f.journal().join("SWITCHED").exists());
        assert!(HeldSwitch::resume(&selected).is_err());
        preserved_source(&f);
    }
}

#[test]
fn new_route_twin_mixed_incoming_or_partial_marker_refuse_before_any_flush() {
    for which in 0..3 {
        let f = Fixture::new();
        let live = || f.state.require_named();
        let selected = f.selected(&live);
        visible_new(arrived(&selected));
        let alien = match which {
            0 => {
                let canonical = f.route();
                fs::rename(&canonical, f.system.path.join("kept-original-new-route")).unwrap();
                write_file(&canonical, &branched_route(TO)).unwrap();
                canonical
            }
            1 => {
                let candidate = incoming(&f);
                write_file(&candidate, &branched_route(TO)).unwrap();
                candidate
            }
            _ => {
                let marker = f.journal().join("SWITCHED");
                write_file(&marker, b"partial-switch").unwrap();
                marker
            }
        };
        let raw = fs::read(&alien).unwrap();
        assert!(
            HeldSwitch::resume_with(
                &selected,
                |_| panic!("must not flush unproved selected namespace"),
                |_| panic!("must not touch unproved SWITCHED"),
            )
            .is_err()
        );
        assert_eq!(fs::read(&alien).unwrap(), raw);
        assert_eq!(fs::read(f.route()).unwrap(), branched_route(TO));
        assert_eq!(f.journal().join("SWITCHED").exists(), which == 2);
        preserved_source(&f);
    }
}

#[test]
fn visible_complete_switched_requalifies_same_record_after_failed_journal_parent_sync() {
    let f = Fixture::new();
    let live = || f.state.require_named();
    let selected = f.selected(&live);
    let ready = arrived(&selected);
    let marker = f.journal().join("SWITCHED");
    let observed = Cell::new(false);
    let result = ready.switch_with(
        || Ok(()),
        File::sync_all,
        |file| {
            let journal = HeldDir::trusted_storage(&f.journal()).unwrap();
            same_file(&journal, &journal.file, file).unwrap();
            assert_eq!(fs::read(f.route()).unwrap(), branched_route(TO));
            assert_eq!(fs::read(&marker).unwrap().len(), 72);
            observed.set(true);
            Err(io::Error::other("fixture original journal fsync cut"))
        },
    );
    assert!(result.is_err());
    assert!(observed.get());
    let inode = fs::metadata(&marker).unwrap().ino();
    let bytes = fs::read(&marker).unwrap();
    let done = HeldSwitch::resume(&selected).unwrap();
    assert_eq!(fs::metadata(&marker).unwrap().ino(), inode);
    assert_eq!(fs::read(&marker).unwrap(), bytes);
    assert_eq!(&bytes[40..], &done.id().unwrap());
    preserved_source(&f);
}

#[test]
fn a_post_rename_same_byte_route_swap_does_not_acknowledge_switched() {
    let f = Fixture::new();
    let live = || f.state.require_named();
    let selected = f.selected(&live);
    let ready = arrived(&selected);
    let replaced = Cell::new(false);
    let backup = f.system.path.join("held-selected-cr1");
    let result = ready.switch_with(
        || Ok(()),
        |file| {
            file.sync_all()?;
            fs::rename(f.route(), &backup)?;
            write_file(&f.route(), &branched_route(TO))?;
            replaced.set(true);
            Ok(())
        },
        |_| panic!("the routes name was replaced after sync"),
    );
    assert!(result.is_err());
    assert!(replaced.get());
    assert_eq!(fs::read(&backup).unwrap(), branched_route(TO));
    assert_eq!(fs::read(f.route()).unwrap(), branched_route(TO));
    assert!(!f.journal().join("SWITCHED").exists());
    assert!(HeldSwitch::resume(&selected).is_err());
    preserved_source(&f);
}
