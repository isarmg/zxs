//! Disposable private DATA retirement and exact DONE fixtures.
use super::super::ready::{
    HeldReady,
    switch::{
        HeldSwitch,
        retire::{self, Completion, HistoryRequest},
    },
};
use super::*;

const FIRST: Id = [0x77; 16];
const SECOND: Id = [0x78; 16];
const THIRD: Id = [0x79; 16];

fn source(f: &Fixture) -> PathBuf {
    f.source.root.path.join("data").join(hex(C))
}

fn target(f: &Fixture) -> PathBuf {
    f.target.root.path.join("data").join(hex(C))
}

fn entry(at: &Path, sequence: u64, op: Id) -> PathBuf {
    at.join(crate::container_store::entry_leaf(sequence, op))
}

fn selected<'h, 'r>(scope: &'h Selected<'r>) -> HeldSwitch<'h, 'r> {
    arrived(scope).switch().unwrap()
}

fn arrived<'h, 'r>(scope: &'h Selected<'r>) -> HeldReady<'h, 'r> {
    Pending::prepare(scope)
        .unwrap()
        .copy_to_ready()
        .unwrap()
        .arrive()
        .unwrap()
}

fn append_history(f: &Fixture) {
    let owner = f.system.opaque_owner(c"container-owner").unwrap();
    let store = Store::open_roster(
        &f.system,
        vec![
            DataRoot {
                token: FROM,
                root: held(&f.source.root),
            },
            DataRoot {
                token: TO,
                root: held(&f.target.root),
            },
        ],
        owner,
    )
    .unwrap();
    store
        .save(
            Mutation {
                container: C,
                request: SECOND,
            },
            Change::Save {
                target: Target::Branch {
                    name: "main".into(),
                    expected_head: [0x53; 16],
                },
                node: [0x54; 16],
                body: Body {
                    length: 0,
                    sha256: digest(b"").unwrap(),
                },
            },
            &b""[..],
            || Ok(()),
        )
        .unwrap();
    store
        .change(
            Mutation {
                container: C,
                request: THIRD,
            },
            Change::Rename {
                expected_sequence: 2,
                name: "after.bin".into(),
            },
            || Ok(()),
        )
        .unwrap();
}

fn completed(f: &Fixture, result: Completion) -> Digest {
    let Completion::Completed(id) = result else {
        panic!("expected new completion")
    };
    assert!(!source(f).exists());
    assert!(!f.marker(true).exists());
    assert_eq!(
        fs::read(entry(&target(f), 1, FIRST).join("payload")).unwrap(),
        RAW
    );
    assert_eq!(
        fs::read(f.route()).unwrap(),
        crate::container_store::routes::encode(C, TO).unwrap()
    );
    let raw = fs::read(f.journal().join("DONE")).unwrap();
    let switched = fs::read(f.journal().join("SWITCHED")).unwrap();
    assert_eq!(&raw[..8], b"ZXDMD1\0\0");
    assert_eq!(&raw[8..40], &switched[40..]);
    assert_eq!(&raw[40..], &id);
    id
}

#[test]
fn switched_retires_only_originals_and_done_releases_only_its_guard() {
    let f = Fixture::new();
    append_history(&f);
    let live = || f.state.require_named();
    let scope = f.selected(&live);
    let done = selected(&scope).complete().unwrap();
    completed(&f, done);
    assert_eq!(
        fs::read(entry(&target(&f), 2, SECOND).join("payload")).unwrap(),
        b""
    );
    assert!(!entry(&target(&f), 3, THIRD).join("payload").exists());
    assert!(entry(&target(&f), 3, THIRD).join("receipt").is_file());
}

#[test]
fn a_legal_partially_retired_entry_can_finish_same_op_without_touching_target() {
    let f = Fixture::new();
    append_history(&f);
    let stopped = Cell::new(false);
    let live = || {
        f.state.require_named()?;
        if stopped.get()
            && !f.entry().join("payload").exists()
            && f.entry().join("receipt").exists()
        {
            return Err(io::ErrorKind::TimedOut.into());
        }
        Ok(())
    };
    let scope = f.selected(&live);
    let cut = selected(&scope);
    stopped.set(true);
    assert_eq!(
        cut.complete().err().unwrap().kind(),
        io::ErrorKind::TimedOut
    );
    assert!(!f.entry().join("payload").exists());
    assert!(f.entry().join("receipt").is_file());
    assert!(entry(&source(&f), 2, SECOND).join("payload").is_file());
    assert!(!f.journal().join("DONE").exists());
    assert!(f.marker(true).is_file());
    stopped.set(false);
    completed(&f, retire::resume(&scope).unwrap());
}

#[test]
fn an_unknown_later_entry_nonprefix_loss_or_payload_without_receipt_stops_before_unlinks() {
    for fault in 0..3 {
        let f = Fixture::new();
        append_history(&f);
        let live = || f.state.require_named();
        let scope = f.selected(&live);
        drop(selected(&scope));
        let later = entry(&source(&f), 2, SECOND);
        match fault {
            0 => write_file(&later.join("foreign"), b"never remove this").unwrap(),
            1 => fs::remove_file(later.join("payload")).unwrap(),
            _ => fs::remove_file(f.entry().join("receipt")).unwrap(),
        }
        assert!(retire::resume(&scope).is_err());
        assert_eq!(fs::read(f.entry().join("payload")).unwrap(), RAW);
        assert_eq!(f.entry().join("receipt").exists(), fault != 2);
        assert!(later.join("receipt").exists());
        assert_eq!(later.join("payload").exists(), fault != 1);
        assert_eq!(later.join("foreign").exists(), fault == 0);
        assert!(!f.journal().join("DONE").exists());
        assert!(f.marker(true).is_file());
    }
}

#[test]
fn scoped_target_child_lost_after_one_unlink_cannot_authorize_the_next_cut() {
    let f = Fixture::new();
    let target_payload = entry(&target(&f), 1, FIRST).join("payload");
    let backup = f.target.root.path.join("kept-proved-payload");
    let replace = Cell::new(false);
    let live = || {
        f.state.require_named()?;
        if replace.get() && !f.entry().join("payload").exists() {
            replace.set(false);
            fs::rename(&target_payload, &backup)?;
            write_file(&target_payload, RAW)?;
        }
        Ok(())
    };
    let scope = f.selected(&live);
    let cut = selected(&scope);
    replace.set(true);
    assert!(cut.complete().is_err());
    assert!(!replace.get());
    assert!(!f.entry().join("payload").exists());
    assert!(f.entry().join("receipt").is_file());
    assert_eq!(fs::read(&backup).unwrap(), RAW);
    assert_eq!(fs::read(&target_payload).unwrap(), RAW);
    assert!(retire::resume(&scope).is_err());
    assert!(!f.journal().join("DONE").exists());
    assert!(f.marker(true).is_file());
}

#[test]
fn visible_complete_but_unconfirmed_switched_must_settle_before_cold_retirement() {
    let f = Fixture::new();
    let live = || f.state.require_named();
    let scope = f.selected(&live);
    let observed = Cell::new(false);
    let result = arrived(&scope).switch_with(
        || Ok(()),
        File::sync_all,
        |file| {
            let journal = HeldDir::trusted_storage(&f.journal()).unwrap();
            same_file(&journal, &journal.file, file).unwrap();
            assert_eq!(fs::read(f.journal().join("SWITCHED")).unwrap().len(), 72);
            observed.set(true);
            Err(io::Error::other("fixture original SWITCHED parent cut"))
        },
    );
    assert!(result.is_err());
    assert!(observed.get());
    assert_eq!(fs::read(f.entry().join("payload")).unwrap(), RAW);
    completed(&f, retire::resume(&scope).unwrap());
}

#[test]
fn historical_done_can_settle_and_release_without_old_data_or_current_route() {
    let f = Fixture::new();
    let identity = f.scope();
    let hold = || {
        f.state.require_named()?;
        if f.journal().join("DONE").exists() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        Ok(())
    };
    {
        let scope = f.selected(&hold);
        assert_eq!(
            selected(&scope).complete().err().unwrap().kind(),
            io::ErrorKind::TimedOut
        );
    }
    assert!(!source(&f).exists());
    assert!(f.marker(true).exists());
    let expected = fs::read(f.journal().join("DONE")).unwrap();
    assert_eq!(expected.len(), 72);
    fs::rename(f.route(), f.system.path.join("kept-current-cr1")).unwrap();
    fs::rename(
        &f.source.root.path,
        f.source.root.path.with_file_name("former-source"),
    )
    .unwrap();
    let former_target = f.target.root.path.with_file_name("former-target");
    fs::rename(&f.target.root.path, &former_target).unwrap();

    let system_lease = f.system.existing_opaque_owner(c"container-owner").unwrap();
    let live = || f.state.require_named();
    let request = |identity| HistoryRequest {
        state: &f.state,
        system: &f.system,
        system_owner: &system_lease,
        identity,
        live: &live,
    };
    let foreign = f.state.path.join(".server-stage-move-another");
    write_file(&foreign, b"retain another active stage").unwrap();
    assert!(retire::history(request(identity)).is_err());
    assert!(f.marker(true).exists());
    assert_eq!(fs::read(&foreign).unwrap(), b"retain another active stage");
    fs::remove_file(&foreign).unwrap();
    assert!(retire::history(request((identity.0, SYSTEM, (identity.2.1, identity.2.0)))).is_err());
    assert!(f.marker(true).exists());

    let Completion::Historical(id) = retire::history(request(identity)).unwrap() else {
        panic!("history does not assert current disk placement")
    };
    assert_eq!(&expected[40..], &id);
    assert!(!f.marker(true).exists());
    assert_eq!(
        retire::history(request(identity)).unwrap(),
        Completion::Historical(id)
    );
    assert!(!f.route().exists());
    let body = entry(&former_target.join("data").join(hex(C)), 1, FIRST).join("payload");
    assert_eq!(fs::read(body).unwrap(), RAW);
}
