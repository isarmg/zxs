use super::*;
use crate::{
    container_store::{
        DataRoot, Mutation, OperationReceipt, Store, Target, ensure_dir, hex,
        upload::{Chunk, Intent, Scope, Status, Uploads, change, journal},
        write_file,
    },
    test_support::PrivateTemp,
};
use std::{
    cell::Cell,
    fs,
    path::PathBuf,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

const LIVE: fn() -> io::Result<()> = || Ok(());
const RAW: &[u8] = b"full publication digest proof";
const MUTATION: Mutation = Mutation {
    container: [1; 16],
    request: [2; 16],
};
const SCOPE: Scope = Scope {
    pool: [4; 16],
    epoch: [5; 16],
};
const TOKEN: [u8; 32] = [6; 32];

fn expected() -> Body {
    let mut hash = Sha256::new();
    hash.update(RAW).unwrap();
    Body {
        length: RAW.len() as u64,
        sha256: hash.finish(),
    }
}

fn offset(state: &HashState) -> Option<u64> {
    match state {
        HashState::Prefix { offset, .. } => Some(*offset),
        HashState::Exact => None,
    }
}

#[test]
fn a_read_that_returns_after_expiry_never_enters_the_saved_sha_state() {
    let allowed = Cell::new(true);
    let authorize = || {
        if allowed.get() {
            Ok(())
        } else {
            Err(expired())
        }
    };
    let mut state = HashState::new();
    let read = |out: &mut [u8], at| {
        assert_ne!(out, []);
        if at == 0 {
            out[..4].copy_from_slice(&RAW[..4]);
            return Ok(4);
        }
        assert_eq!(at, 4);
        out[0] = b'!';
        allowed.set(false); // the in-flight raw read returned after expiry
        Ok(1)
    };
    assert!(matches!(
        state.advance(&expected(), read, &authorize),
        Err(Failure::Expired)
    ));
    assert_eq!(offset(&state), Some(4));

    allowed.set(true);
    let restart = Cell::new(None);
    let actual = |out: &mut [u8], at| {
        if restart.get().is_none() {
            restart.set(Some(at));
        }
        let at = usize::try_from(at).unwrap();
        let size = out.len().min(RAW.len() - at);
        out[..size].copy_from_slice(&RAW[at..at + size]);
        Ok(size)
    };
    assert!(state.advance(&expected(), actual, &authorize).is_ok());
    assert_eq!(restart.get(), Some(4));
    assert_eq!(offset(&state), None);
}

#[test]
fn expired_eof_must_be_rechecked_even_after_reaching_the_length() {
    let allowed = Cell::new(true);
    let authorize = || {
        if allowed.get() {
            Ok(())
        } else {
            Err(expired())
        }
    };
    let mut state = HashState::new();
    let body = expected();
    let read = |out: &mut [u8], at| {
        if at == 0 {
            out.copy_from_slice(RAW);
            Ok(RAW.len())
        } else {
            assert_eq!(at, body.length);
            allowed.set(false);
            Ok(0)
        }
    };
    assert!(matches!(
        state.advance(&body, read, &authorize),
        Err(Failure::Expired)
    ));
    assert_eq!(offset(&state), Some(body.length));
    allowed.set(true);
    assert!(matches!(
        state.advance(
            &body,
            |_, at| {
                assert_eq!(at, body.length);
                Ok(1)
            },
            &authorize
        ),
        Err(Failure::InvalidBody)
    ));
}

struct Fixture {
    _temp: PrivateTemp,
    system: HeldDir,
    data: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = PrivateTemp::under(&std::env::temp_dir());
        let system = temp.0.join("system");
        let data = temp.0.join("data");
        ensure_dir(&system).unwrap();
        ensure_dir(&data).unwrap();
        Self {
            system: HeldDir::trusted_storage(&system).unwrap(),
            data,
            _temp: temp,
        }
    }

    fn store(&self) -> Store {
        let owner = self.system.opaque_owner(c"container-owner").unwrap();
        let volume = DataRoot {
            token: TOKEN,
            root: HeldDir::trusted_storage(&self.data).unwrap(),
        };
        Store::open_roster(&self.system, vec![volume], owner).unwrap()
    }

    fn control(&self) -> PathBuf {
        self.system
            .path
            .join("upload-intents")
            .join(hex(MUTATION.container))
    }

    fn payload(&self, upload: [u8; 32]) -> PathBuf {
        self.data
            .join("staging")
            .join(format!("u-{}", hex(upload)))
            .join("payload")
    }
}

fn plan() -> Intent {
    Intent {
        scope: SCOPE,
        mutation: MUTATION,
        node: [3; 16],
        body: expected(),
        target: Target::FirstMain {
            name: "large-opaque.bin".into(),
        },
    }
}

fn submitted(lane: &Uploads<'_>) -> [u8; 32] {
    let plan = plan();
    let upload = lane.begin(&plan, &LIVE).unwrap().upload;
    let chunk = Chunk {
        mutation: MUTATION,
        upload,
        at: 0,
        length: plan.body.length,
        sha256: plan.body.sha256,
    };
    assert_eq!(
        lane.put(chunk, &mut &RAW[..], &LIVE).unwrap().status,
        Status::Complete
    );
    upload
}

fn pause_after_prefix(lane: &Uploads<'_>, upload: [u8; 32], now: Instant) {
    let directory = lane.directory(false, &LIVE).unwrap().unwrap();
    let current = journal::read(&directory, MUTATION.container)
        .unwrap()
        .unwrap();
    let stage = Stage::open(&lane.store.volumes[0], &current.record).unwrap();
    let mut lease = lane
        .store
        .uploads
        .full_proofs
        .acquire_at(Key::exact(SCOPE, MUTATION, upload), now)
        .unwrap();
    let mut witness = lease
        .witness_or(|| Witness::capture(&directory, &current, &stage))
        .unwrap();
    let live = Cell::new(true);
    let authorize = || if live.get() { Ok(()) } else { Err(expired()) };
    let read = |out: &mut [u8], at| {
        if at == 0 {
            stage.read_held_at(&mut out[..3], 0)
        } else {
            let result = stage.read_held_at(out, at);
            live.set(false);
            result
        }
    };
    assert!(matches!(
        witness.state.advance(&expected(), read, &authorize),
        Err(Failure::Expired)
    ));
    assert_eq!(offset(&witness.state), Some(3));
    lease.pause_at(witness, now).unwrap();
}

fn assert_unselected(store: &Store) {
    assert_eq!(store.routes.get(MUTATION.container).unwrap(), None);
    assert!(store.operation_receipt(MUTATION).unwrap().is_none());
}

#[test]
fn only_fresh_explicit_same_epoch_commit_can_use_an_exact_dormant_proof() {
    let f = Fixture::new();
    let store = f.store();
    let lane = Uploads::bind(&store, &f.system, SCOPE).unwrap();
    let upload = submitted(&lane);
    let now = Instant::now();
    pause_after_prefix(&lane, upload, now);
    assert_eq!(
        lane.status(MUTATION, Some(upload), &LIVE).unwrap().status,
        Status::Complete
    );
    assert_unselected(&store);
    let bank = &store.uploads.full_proofs;
    assert!(matches!(
        bank.0.lock().unwrap().get(&Key::exact(SCOPE, MUTATION, upload)),
        Some(Entry::Parked { since, witness }) if *since == now && offset(&witness.state) == Some(3)
    ));

    let replaced_scope = Scope {
        epoch: [9; 16],
        ..SCOPE
    };
    let wrong = Uploads::bind(&store, &f.system, replaced_scope).unwrap();
    assert!(matches!(
        wrong.commit(MUTATION, upload, &LIVE),
        Err(Failure::OldEpoch)
    ));
    assert_unselected(&store);
    let wrong_upload = [99; 32];
    assert!(matches!(
        lane.commit(MUTATION, wrong_upload, &LIVE),
        Err(Failure::Conflict)
    ));
    let other_op = Mutation {
        request: [98; 16],
        ..MUTATION
    };
    assert!(matches!(
        lane.commit(other_op, upload, &LIVE),
        Err(Failure::Conflict)
    ));
    let blocked = || Err(io::ErrorKind::PermissionDenied.into());
    assert!(matches!(
        lane.commit(MUTATION, upload, &blocked),
        Err(Failure::Io(_))
    ));
    assert!(matches!(
        bank.0.lock().unwrap().get(&Key::exact(SCOPE, MUTATION, upload)),
        Some(Entry::Parked { since, witness }) if *since == now && offset(&witness.state) == Some(3)
    ));

    let authorized_again = Uploads::bind(&store, &f.system, SCOPE).unwrap();
    let committed = authorized_again.commit(MUTATION, upload, &LIVE).unwrap();
    assert_eq!(
        store.operation_receipt(MUTATION).unwrap(),
        Some(OperationReceipt::Mutation(committed.receipt))
    );
    assert!(committed.retired.unwrap());
    assert!(bank.0.lock().unwrap().is_empty());
}

#[test]
fn exact_same_bytes_at_a_replaced_control_name_do_not_authorize_cached_proof() {
    let f = Fixture::new();
    let store = f.store();
    let lane = Uploads::bind(&store, &f.system, SCOPE).unwrap();
    let upload = submitted(&lane);
    pause_after_prefix(&lane, upload, Instant::now());
    let control = f.control();
    let bytes = fs::read(&control).unwrap();
    let original = f.system.path.join("fixture-original-control");
    fs::rename(&control, &original).unwrap();
    write_file(&control, &bytes).unwrap();
    assert!(matches!(
        lane.commit(MUTATION, upload, &LIVE),
        Err(Failure::Io(_))
    ));
    assert!(store.uploads.full_proofs.0.lock().unwrap().is_empty());
    assert_eq!(fs::read(f.payload(upload)).unwrap(), RAW);
    assert_eq!(fs::read(control).unwrap(), bytes);
    assert_eq!(fs::read(original).unwrap(), bytes);
    assert_unselected(&store);
}

#[test]
fn idle_and_running_proofs_share_the_same_bound_and_idle_expiry_has_no_disk_effect() {
    let f = Fixture::new();
    let store = f.store();
    let lane = Uploads::bind(&store, &f.system, SCOPE).unwrap();
    let upload = submitted(&lane);
    let now = Instant::now();
    let bank = &store.uploads.full_proofs;
    pause_after_prefix(&lane, upload, now);
    let active = bank
        .acquire_at(Key::exact(SCOPE, MUTATION, [7; 32]), now)
        .unwrap();
    assert!(
        matches!(bank.acquire_at(Key::exact(SCOPE, MUTATION, [8; 32]), now), Err(error) if error.kind() == io::ErrorKind::WouldBlock)
    );
    let later = now.checked_add(IDLE).unwrap();
    let next = bank
        .acquire_at(Key::exact(SCOPE, MUTATION, [8; 32]), later)
        .unwrap();
    drop(next);
    drop(active);
    assert!(bank.0.lock().unwrap().is_empty());
    assert_eq!(fs::read(f.payload(upload)).unwrap(), RAW);
    assert!(f.control().exists());
    assert_unselected(&store);
}

#[test]
fn changing_the_original_raw_stamp_discards_only_the_ram_proof() {
    let f = Fixture::new();
    let store = f.store();
    let lane = Uploads::bind(&store, &f.system, SCOPE).unwrap();
    let upload = submitted(&lane);
    pause_after_prefix(&lane, upload, Instant::now());
    let path = f.payload(upload);
    let opened = fs::OpenOptions::new().write(true).open(&path).unwrap();
    opened
        .set_times(
            fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH + Duration::from_secs(101)),
        )
        .unwrap();
    assert!(matches!(
        lane.commit(MUTATION, upload, &LIVE),
        Err(Failure::Io(_))
    ));
    assert!(store.uploads.full_proofs.0.lock().unwrap().is_empty());
    assert_eq!(fs::read(path).unwrap(), RAW);
    assert!(f.control().exists());
    assert_unselected(&store);
}

#[test]
fn a_fresh_birth_mismatch_discards_the_dormant_proof_not_either_file() {
    let f = Fixture::new();
    let store = f.store();
    let lane = Uploads::bind(&store, &f.system, SCOPE).unwrap();
    let upload = submitted(&lane);
    pause_after_prefix(&lane, upload, Instant::now());
    let path = f.payload(upload);
    let original = f.data.join("fixture-held-raw");
    fs::rename(&path, &original).unwrap();
    write_file(&path, RAW).unwrap();
    assert!(matches!(
        lane.commit(MUTATION, upload, &LIVE),
        Err(Failure::Io(_))
    ));
    assert!(store.uploads.full_proofs.0.lock().unwrap().is_empty());
    assert_eq!(fs::read(path).unwrap(), RAW);
    assert_eq!(fs::read(original).unwrap(), RAW);
    assert!(f.control().exists());
    assert_unselected(&store);
}

#[test]
fn a_known_missing_stage_before_acquire_does_not_reuse_proof_after_return() {
    let f = Fixture::new();
    let store = f.store();
    let lane = Uploads::bind(&store, &f.system, SCOPE).unwrap();
    let upload = submitted(&lane);
    pause_after_prefix(&lane, upload, Instant::now());
    let staging = f.data.join("staging");
    let absent = f.data.join("fixture-staging-absent");
    fs::rename(&staging, &absent).unwrap();
    assert!(matches!(
        lane.commit(MUTATION, upload, &LIVE),
        Err(Failure::Io(_))
    ));
    assert!(store.uploads.full_proofs.0.lock().unwrap().is_empty());
    fs::rename(absent, staging).unwrap();
    assert_eq!(
        lane.status(MUTATION, Some(upload), &LIVE).unwrap().status,
        Status::Complete
    );
    assert_eq!(fs::read(f.payload(upload)).unwrap(), RAW);
    assert_unselected(&store);
    assert!(store.uploads.full_proofs.0.lock().unwrap().is_empty());
}

#[test]
fn old_receipt_selection_ends_the_exact_dormant_proof_without_touching_raw() {
    let f = Fixture::new();
    let store = f.store();
    let lane = Uploads::bind(&store, &f.system, SCOPE).unwrap();
    let upload = submitted(&lane);
    pause_after_prefix(&lane, upload, Instant::now());
    let chosen = store.save(MUTATION, change(&plan()), RAW, LIVE).unwrap();
    let replay = lane.commit(MUTATION, upload, &LIVE).unwrap();
    assert_eq!(replay.receipt, chosen);
    assert!(!replay.retired.unwrap());
    assert!(store.uploads.full_proofs.0.lock().unwrap().is_empty());
    assert_eq!(fs::read(f.payload(upload)).unwrap(), RAW);
    assert!(f.control().exists());
}

pub(super) struct ObserveDrop {
    bank: Weak<Bank>,
    observed_open: Arc<AtomicBool>,
}

impl Drop for ObserveDrop {
    fn drop(&mut self) {
        if let Some(bank) = self.bank.upgrade() {
            self.observed_open
                .store(bank.0.try_lock().is_ok(), Ordering::Relaxed);
        }
    }
}

#[test]
fn file_bearing_witnesses_drop_after_the_bank_lock_even_on_a_refusal() {
    let f = Fixture::new();
    let store = f.store();
    let lane = Uploads::bind(&store, &f.system, SCOPE).unwrap();
    let upload = submitted(&lane);
    let directory = lane.directory(false, &LIVE).unwrap().unwrap();
    let control = journal::read(&directory, MUTATION.container)
        .unwrap()
        .unwrap();
    let stage = Stage::open(&store.volumes[0], &control.record).unwrap();
    for expired in [false, true] {
        let bank = Arc::new(Bank::default());
        let original = Key::exact(SCOPE, MUTATION, upload);
        let other = Key::exact(SCOPE, MUTATION, [12; 32]);
        let observed_open = Arc::new(AtomicBool::new(false));
        let now = Instant::now();
        let mut proof = Witness::capture(&directory, &control, &stage).unwrap();
        assert!(proof.drop_probe.is_none());
        proof.drop_probe = Some(ObserveDrop {
            bank: Arc::downgrade(&bank),
            observed_open: Arc::clone(&observed_open),
        });
        let parked = bank.acquire_at(original, now).unwrap();
        parked.pause_at(Box::new(proof), now).unwrap();
        let active = bank.acquire_at(other, now).unwrap();
        if expired {
            let next = now.checked_add(IDLE).unwrap();
            assert!(
                matches!(bank.acquire_at(other, next), Err(error) if error.kind() == io::ErrorKind::WouldBlock)
            );
        } else {
            bank.discard_idle(original);
        }
        assert!(observed_open.load(Ordering::Relaxed));
        drop(active);
    }
    assert_unselected(&store);
    assert_eq!(fs::read(f.payload(upload)).unwrap(), RAW);
}
