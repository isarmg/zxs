use super::*;
use crate::{
    container_store::{Body, DataRoot, ensure_dir, hex, write_file},
    test_support::PrivateTemp,
};
use std::{
    cell::Cell,
    fs::{self, OpenOptions},
    os::unix::fs::FileExt,
    path::PathBuf,
};

const SCOPE: Scope = Scope {
    pool: [9; 16],
    epoch: [10; 16],
};
const OP: Mutation = Mutation {
    container: [1; 16],
    request: [2; 16],
};
const RAW: &[u8] = b"original raw opaque bytes";
const TOKEN: [u8; 32] = [5; 32];

const LIVE: fn() -> io::Result<()> = || Ok(());

fn sha(data: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(data).unwrap();
    digest.finish()
}

fn intent() -> Intent {
    Intent {
        scope: SCOPE,
        mutation: OP,
        node: [3; 16],
        target: Target::FirstMain {
            name: "opaque.dat".into(),
        },
        body: Body {
            length: RAW.len() as u64,
            sha256: sha(RAW),
        },
    }
}

fn block(bytes: &[u8]) -> Chunk {
    Chunk {
        mutation: OP,
        upload: intent().upload_id().unwrap(),
        at: 0,
        length: bytes.len() as u64,
        sha256: sha(bytes),
    }
}

struct Fixture {
    _temporary: PrivateTemp,
    system: HeldDir,
    data: PathBuf,
    peer: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temporary = PrivateTemp::under(&std::env::temp_dir());
        let system = temporary.0.join("system");
        let data = temporary.0.join("data");
        ensure_dir(&system).unwrap();
        ensure_dir(&data).unwrap();
        let peer = temporary.0.join("peer");
        ensure_dir(&peer).unwrap();
        Self {
            system: HeldDir::trusted_storage(&system).unwrap(),
            data,
            peer,
            _temporary: temporary,
        }
    }

    // Same-filesystem temporary directories intentionally do not claim to
    // exercise the production role/mount separation at server startup.
    fn store(&self) -> Store {
        self.open_store(false)
    }

    fn open_store(&self, two: bool) -> Store {
        let owner = self.system.opaque_owner(c"container-owner").unwrap();
        let data = DataRoot {
            token: TOKEN,
            root: HeldDir::trusted_storage(&self.data).unwrap(),
        };
        let mut volumes = vec![data];
        if two {
            volumes.push(DataRoot {
                token: [6; 32],
                root: HeldDir::trusted_storage(&self.peer).unwrap(),
            });
        }
        Store::open_roster(&self.system, volumes, owner).unwrap()
    }

    fn lane<'a>(&'a self, store: &'a Store) -> Uploads<'a> {
        Uploads::bind(store, &self.system, SCOPE).unwrap()
    }

    fn staging(&self) -> PathBuf {
        let name = format!("u-{}", hex(intent().upload_id().unwrap()));
        self.data.join("staging").join(name)
    }

    fn control(&self) -> PathBuf {
        self.system
            .path
            .join("upload-intents")
            .join(hex(OP.container))
    }

    fn assert_not_published(store: &Store) {
        assert_eq!(store.routes.get(OP.container).unwrap(), None);
        assert!(store.operation_receipt(OP).unwrap().is_none());
    }
}

struct UnexpectedRead;
impl Read for UnexpectedRead {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        panic!("rejected upload scope consumed source bytes");
    }
}

#[test]
fn lost_begin_and_partial_raw_cut_keep_the_same_ready_account_and_no_progress() {
    let f = Fixture::new();
    {
        let store = f.store();
        let lane = f.lane(&store);
        lane.begin(&intent(), &LIVE).unwrap();
        let path = f.staging().join("payload");
        let file = OpenOptions::new().write(true).open(path).unwrap();
        // Simulate process loss during the raw write, before its flush or
        // any new checkpoint, not a filesystem power-loss guarantee.
        file.write_all_at(&RAW[..3], 0).unwrap();
    }
    let store = f.store();
    let lane = f.lane(&store);
    let observed = lane.status(OP, None, &LIVE).unwrap();
    assert_eq!(observed.offset, 0);
    assert_eq!(observed.upload, intent().upload_id().unwrap());
    assert_eq!(lane.begin(&intent(), &LIVE).unwrap(), observed);
    assert_eq!(fs::read_dir(f.data.join("staging")).unwrap().count(), 1);
    let journal = fs::read(f.control()).unwrap();
    let mut wrong = block(RAW);
    wrong.sha256[0] ^= 1;
    assert!(matches!(
        lane.put(wrong, &mut &RAW[..], &LIVE),
        Err(Failure::InvalidBody)
    ));
    assert_eq!(fs::read(f.staging().join("payload")).unwrap(), RAW[..3]);
    assert_eq!(fs::read(f.control()).unwrap(), journal);
    let accepted = lane.put(block(RAW), &mut &RAW[..], &LIVE).unwrap();
    assert_eq!(accepted.offset, RAW.len() as u64);
    assert_eq!(accepted.status, Status::Complete);
    assert_eq!(fs::read(f.staging().join("payload")).unwrap(), RAW);
    Fixture::assert_not_published(&store);
}

fn stopped_after_raw_sync(f: &Fixture, choose: bool) {
    let store = f.store();
    let lane = f.lane(&store);
    lane.begin(&intent(), &LIVE).unwrap();
    let directory = lane.directory(false, &LIVE).unwrap().unwrap();
    let old = journal::read(&directory, OP.container).unwrap().unwrap();
    let mut raw = material::Stage::open(&store.volumes[0], &old.record).unwrap();
    raw.write(0, RAW.len() as u64, RAW, LIVE, LIVE).unwrap();
    let mut next = old.record.clone();
    next.phase = raw.phase(RAW.len() as u64);
    let pending = journal::prepare(&directory, Some(&old), next, LIVE).unwrap();
    if choose {
        pending.choose(LIVE, LIVE).unwrap();
    }
    // Drop before the directory sync and acknowledgement, whether or not the
    // rename became visible. There is deliberately no rollback or unlink.
}

#[test]
fn synced_raw_and_a_synced_unselected_checkpoint_are_not_an_ack() {
    let f = Fixture::new();
    stopped_after_raw_sync(&f, false);
    let temporary = f
        .system
        .path
        .join("upload-intents")
        .join(format!(".{}-00.incoming", hex(OP.container)));
    let unknown = fs::read(&temporary).unwrap();
    let store = f.store();
    let lane = f.lane(&store);
    assert_eq!(lane.status(OP, None, &LIVE).unwrap().offset, 0);
    assert_eq!(fs::read(f.staging().join("payload")).unwrap(), RAW);
    let accepted = lane.put(block(RAW), &mut &RAW[..], &LIVE).unwrap();
    assert_eq!(accepted.offset, RAW.len() as u64);
    assert_eq!(fs::read(temporary).unwrap(), unknown);
    Fixture::assert_not_published(&store);
}

#[test]
fn visible_exact_checkpoint_after_rename_is_settled_before_answering() {
    let f = Fixture::new();
    stopped_after_raw_sync(&f, true);
    let old_control = fs::read(f.control()).unwrap();
    let store = f.store();
    let lane = f.lane(&store);
    let observed = lane.status(OP, None, &LIVE).unwrap();
    assert_eq!(observed.status, Status::Complete);
    assert_eq!(observed.offset, RAW.len() as u64);
    assert_eq!(
        lane.put(block(RAW), &mut &RAW[..], &LIVE).unwrap(),
        observed
    );
    let mut changed = RAW.to_vec();
    changed[0] ^= 1;
    assert!(matches!(
        lane.put(block(&changed), &mut changed.as_slice(), &LIVE),
        Err(Failure::Conflict)
    ));
    assert_eq!(fs::read(f.control()).unwrap(), old_control);
    assert_eq!(fs::read(f.staging().join("payload")).unwrap(), RAW);
    Fixture::assert_not_published(&store);
}

#[test]
fn another_epoch_or_replaced_ready_payload_cannot_read_or_overwrite() {
    let f = Fixture::new();
    {
        let store = f.store();
        f.lane(&store).begin(&intent(), &LIVE).unwrap();
    }
    let store = f.store();
    let lane = f.lane(&store);
    let old_control = fs::read(f.control()).unwrap();
    let mut newer = SCOPE;
    newer.epoch = [11; 16];
    let wrong = Uploads::bind(&store, &f.system, newer).unwrap();
    assert!(matches!(
        wrong.status(OP, None, &LIVE),
        Err(Failure::OldEpoch)
    ));
    assert!(matches!(
        wrong.put(block(RAW), &mut UnexpectedRead, &LIVE),
        Err(Failure::OldEpoch)
    ));

    let path = f.staging().join("payload");
    let original = f.data.join("protected-original");
    fs::rename(&path, &original).unwrap();
    write_file(&path, b"foreign").unwrap();
    assert!(matches!(
        lane.put(block(RAW), &mut UnexpectedRead, &LIVE),
        Err(Failure::Io(_))
    ));
    assert_eq!(fs::read(path).unwrap(), b"foreign");
    assert_eq!(fs::read(original).unwrap(), []);
    assert_eq!(fs::read(f.control()).unwrap(), old_control);
    Fixture::assert_not_published(&store);
}

#[test]
fn revoked_begin_creates_nothing_and_selected_r3_receipt_never_invents_upload_id() {
    let f = Fixture::new();
    let store = f.store();
    let lane = f.lane(&store);
    let denied = || Err(io::ErrorKind::PermissionDenied.into());
    assert!(matches!(
        lane.begin(&intent(), &denied),
        Err(Failure::Io(ref err)) if err.kind() == io::ErrorKind::PermissionDenied
    ));
    assert!(!f.control().parent().unwrap().exists());
    assert!(!f.staging().exists());
    store.save(OP, change(&intent()), RAW, LIVE).unwrap();
    assert!(matches!(
        lane.status(OP, None, &LIVE),
        Err(Failure::Conflict)
    ));
    assert!(matches!(
        lane.begin(&intent(), &LIVE),
        Err(Failure::Conflict)
    ));
    assert!(store.operation_receipt(OP).unwrap().is_some());
    assert!(!f.staging().exists());
}

#[test]
fn a_different_cr1_after_intake_never_relocates_the_ready_upload() {
    let f = Fixture::new();
    let store = f.open_store(true);
    let lane = f.lane(&store);
    lane.begin(&intent(), &LIVE).unwrap();
    let old = fs::read(f.control()).unwrap();
    let chosen = Record::decode(&old).unwrap().data_token;
    let (rest, other, new_token) = if chosen == TOKEN {
        (&f.data, &f.peer, [6; 32])
    } else {
        (&f.peer, &f.data, TOKEN)
    };
    let raw = rest
        .join("staging")
        .join(f.staging().file_name().unwrap())
        .join("payload");
    store.routes.reserve(OP.container, new_token).unwrap();
    assert!(matches!(
        lane.put(block(RAW), &mut UnexpectedRead, &LIVE),
        Err(Failure::Conflict)
    ));
    assert!(matches!(
        lane.status(OP, None, &LIVE),
        Err(Failure::Conflict)
    ));
    assert_eq!(fs::read(f.control()).unwrap(), old);
    assert_eq!(fs::read(raw).unwrap(), []);
    assert_eq!(fs::read_dir(other.join("staging")).unwrap().count(), 0);
    assert_eq!(store.routes.get(OP.container).unwrap(), Some(new_token));
    assert!(store.operation_receipt(OP).unwrap().is_none());
}

#[test]
fn old_epoch_controls_still_consume_the_fixed_active_limit() {
    let f = Fixture::new();
    let store = f.store();
    let lane = f.lane(&store);
    let directory = lane.directory(true, &LIVE).unwrap().unwrap();
    for id in 64..80_u8 {
        let mut plan = intent();
        plan.mutation.container = [id; 16];
        plan.scope.epoch = [11; 16];
        let record = Record {
            data_token: TOKEN,
            intent: plan,
            phase: Phase::Preparing,
        };
        let writer = journal::prepare(&directory, None, record, LIVE).unwrap();
        writer.choose(LIVE, LIVE).unwrap();
        writer.confirm().unwrap();
    }
    assert!(matches!(
        lane.begin(&intent(), &LIVE),
        Err(Failure::Io(ref error)) if error.kind() == io::ErrorKind::StorageFull
    ));
    assert_eq!(fs::read_dir(&directory.path).unwrap().count(), MAX_ACTIVE);
    assert!(!f.control().exists() && !f.staging().exists());
    Fixture::assert_not_published(&store);
}

struct BodyCut<F> {
    input: &'static [u8],
    once: Option<F>,
}
impl<F: FnOnce()> Read for BodyCut<F> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if let Some(cut) = self.once.take() {
            cut();
        }
        self.input.read(bytes)
    }
}

#[test]
fn validity_and_the_original_raw_file_are_rechecked_after_body_intake() {
    let f = Fixture::new();
    let store = f.store();
    let lane = f.lane(&store);
    lane.begin(&intent(), &LIVE).unwrap();
    let journal = fs::read(f.control()).unwrap();
    let payload = f.staging().join("payload");
    let active = Cell::new(true);
    let authorize = || {
        if active.get() {
            Ok(())
        } else {
            Err(io::ErrorKind::PermissionDenied.into())
        }
    };
    let mut revoked = BodyCut {
        input: RAW,
        once: Some(|| active.set(false)),
    };
    assert!(matches!(
        lane.put(block(RAW), &mut revoked, &authorize),
        Err(Failure::Io(ref error)) if error.kind() == io::ErrorKind::PermissionDenied
    ));
    assert_eq!(fs::read(&payload).unwrap(), []);
    assert_eq!(fs::read(f.control()).unwrap(), journal);

    let original = f.data.join("held-copy");
    let mut changed = BodyCut {
        input: RAW,
        once: Some(|| {
            fs::rename(&payload, &original).unwrap();
            write_file(&payload, b"replacement").unwrap();
        }),
    };
    assert!(matches!(
        lane.put(block(RAW), &mut changed, &LIVE),
        Err(Failure::Io(_))
    ));
    assert_eq!(fs::read(original).unwrap(), []);
    assert_eq!(fs::read(payload).unwrap(), b"replacement");
    assert_eq!(fs::read(f.control()).unwrap(), journal);
    Fixture::assert_not_published(&store);
}

#[test]
fn a_visible_first_namespace_does_not_bypass_its_held_parent_flush() {
    let f = Fixture::new();
    let store = f.store();
    let lane = f.lane(&store);
    let failed = |parent: &File| {
        native::require_same(&f.system, parent, &f.system.file)?;
        Err(io::Error::other("fixture: parent flush not confirmed"))
    };
    assert!(lane.directory_with(true, &LIVE, failed).is_err());
    let visible = f.system.child_directory(NAMESPACE).unwrap().unwrap();
    assert!(store.uploads.control.lock().unwrap().is_none());
    assert!(!f.control().exists() && !f.staging().exists());

    // A retry after mkdir has become visible must still perform the exact
    // same parent flush, even through the read-only lookup path.
    let tried = Cell::new(false);
    let failed_again = |parent: &File| {
        tried.set(true);
        failed(parent)
    };
    assert!(lane.directory_with(false, &LIVE, failed_again).is_err());
    assert!(tried.get());
    assert!(store.uploads.control.lock().unwrap().is_none());

    let confirmed = lane.directory(false, &LIVE).unwrap().unwrap();
    assert_eq!(confirmed.entity, visible.entity);
    assert!(confirmed.same_mount(&visible).unwrap());
    assert!(!f.control().exists() && !f.staging().exists());
}

#[test]
fn either_control_name_replaced_during_long_proof_refuses_native_selection() {
    for change_predecessor in [false, true] {
        let f = Fixture::new();
        let store = f.store();
        let lane = f.lane(&store);
        lane.begin(&intent(), &LIVE).unwrap();
        let directory = lane.directory(false, &LIVE).unwrap().unwrap();
        let old = journal::read(&directory, OP.container).unwrap().unwrap();
        let mut stage = material::Stage::open(&store.volumes[0], &old.record).unwrap();
        stage.write(0, RAW.len() as u64, RAW, LIVE, LIVE).unwrap();
        let mut next = old.record.clone();
        next.phase = stage.phase(RAW.len() as u64);
        let writer = journal::prepare(&directory, Some(&old), next, LIVE).unwrap();
        let pending = directory
            .path
            .join(format!(".{}-00.incoming", hex(OP.container)));
        let selected = f.control();
        let (old_bytes, pending_bytes) =
            (fs::read(&selected).unwrap(), fs::read(&pending).unwrap());
        let (watch, exact) = if change_predecessor {
            (&selected, &old_bytes)
        } else {
            (&pending, &pending_bytes)
        };
        let displaced = f.system.path.join("fixture-held-original");
        let proved = || {
            fs::rename(watch, &displaced)?;
            write_file(watch, exact)
        };
        let reached_final_auth = Cell::new(false);
        let authorize = || {
            reached_final_auth.set(true);
            Ok(())
        };
        assert!(writer.choose(proved, authorize).is_err());
        assert!(!reached_final_auth.get());
        assert_eq!(fs::read(selected).unwrap(), old_bytes);
        assert_eq!(fs::read(pending).unwrap(), pending_bytes);
        assert_eq!(&fs::read(displaced).unwrap(), exact);
        Fixture::assert_not_published(&store);
    }
}
