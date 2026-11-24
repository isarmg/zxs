use super::*;
use crate::{
    container_store::{
        Body, Change, DataRoot, Mutation, Receipt, Target, ensure_dir, entry_leaf, hex, write_file,
    },
    test_support::PrivateTemp,
};
use std::{
    cell::Cell,
    fs,
    path::{Path, PathBuf},
};

const TOKEN: [u8; 32] = [0x35; 32];
const POOL: Id = [0xab; 16];
const RECYCLE: [u8; 32] = [0xcd; 32];
const ONE: Id = [1; 16];
const TWO: Id = [2; 16];
const THREE: Id = [3; 16];
const FOUR: Id = [4; 16];
const RAW: &[u8] = b"distinct raw bytes";

fn op(id: Id, n: u8) -> Mutation {
    Mutation {
        container: id,
        request: [n; 16],
    }
}

fn first(name: &str) -> Change {
    let mut hash = Sha256::new();
    hash.update(RAW).unwrap();
    Change::Save {
        target: Target::FirstMain {
            name: name.to_owned(),
        },
        node: [0x19; 16],
        body: Body {
            length: RAW.len() as u64,
            sha256: hash.finish(),
        },
    }
}

struct Fixture {
    store: Store,
    system: HeldDir,
    temp: PrivateTemp,
    recycle_temp: Option<PrivateTemp>,
}

fn fixture_dir(root: &PrivateTemp, name: &str) -> HeldDir {
    let path = root.0.join(name);
    ensure_dir(&path).unwrap();
    HeldDir::trusted_storage(&path).unwrap()
}

impl Fixture {
    fn open(with_recycle: bool) -> Self {
        let temp = PrivateTemp::under(&std::env::temp_dir());
        let system = fixture_dir(&temp, "system");
        let data = DataRoot {
            token: TOKEN,
            root: fixture_dir(&temp, "data_disk"),
        };
        // The actual recycle path intentionally rejects a same-filesystem
        // copy. Require the same distinct, privately leased roots used by
        // the existing Linux recycle fixtures.
        let recycle_temp = with_recycle.then(|| PrivateTemp::under(Path::new("/dev/shm")));
        let store = if let Some(remote) = &recycle_temp {
            Store::open_with_recycle(
                HeldDir::trusted_storage(&system.path).unwrap(),
                vec![data],
                fixture_dir(remote, "recycle"),
                POOL,
                RECYCLE,
            )
            .unwrap()
        } else {
            let owner = system.opaque_owner(c"container-owner").unwrap();
            Store::open_roster(&system, vec![data], owner).unwrap()
        };
        Self {
            store,
            system,
            temp,
            recycle_temp,
        }
    }

    fn reopen(self) -> Self {
        let Self {
            store,
            system,
            temp,
            recycle_temp,
        } = self;
        drop(store);
        let data = DataRoot {
            root: fixture_dir(&temp, "data_disk"),
            token: TOKEN,
        };
        let store = Store::open_with_recycle(
            HeldDir::trusted_storage(&system.path).unwrap(),
            vec![data],
            fixture_dir(recycle_temp.as_ref().unwrap(), "recycle"),
            POOL,
            RECYCLE,
        )
        .unwrap();
        Self {
            store,
            system,
            temp,
            recycle_temp,
        }
    }

    fn recycle_scratch(&self, name: &str) -> PathBuf {
        self.recycle_temp.as_ref().unwrap().0.join(name)
    }

    fn view(&self, previous: Option<&View>) -> io::Result<View> {
        self.with_limits(previous, &Limits::default())
    }

    fn with_limits(&self, previous: Option<&View>, limits: &Limits) -> io::Result<View> {
        View::build(&self.store, &self.system, previous, limits, || Ok(()))
    }

    fn save(&self, id: Id, name: &str) {
        self.store
            .save(op(id, 11), first(name), RAW, || Ok(()))
            .unwrap();
    }

    fn data(&self, id: Id) -> PathBuf {
        self.store.volumes[0].container(id)
    }

    fn entry(&self, id: Id, sequence: u64, request: u8) -> PathBuf {
        self.data(id).join(entry_leaf(sequence, [request; 16]))
    }

    fn prepare_recycle(&self, mutation: Mutation) {
        let volume = &self.store.volumes[0];
        let state = volume.read(mutation.container).unwrap();
        let archive = self.store.archive.as_ref().unwrap();
        let mut intake = self
            .store
            .flat_stage_at(archive.staging().unwrap())
            .unwrap();
        archive
            .prepare(mutation, volume, &state, &mut intake, || Ok(()))
            .unwrap();
    }

    fn recycle(&self, mutation: Mutation) {
        self.store.move_to_recycle(mutation, 1, || Ok(())).unwrap();
    }
}

#[test]
fn cold_view_separates_active_reserved_prepared_and_selected_recycled() {
    let f = Fixture::open(true);
    f.save(ONE, "Case.bin");
    f.save(TWO, "Removed.bin");
    f.store.routes.reserve(THREE, TOKEN).unwrap();
    f.store.routes.reserve(FOUR, TOKEN).unwrap();
    let original_empty = f.store.volumes[0].create_container(FOUR).unwrap();
    f.store.volumes[0].sync_data().unwrap();
    let initial = f.view(None).unwrap();
    assert_eq!(initial.rows.len(), 4);
    assert!(matches!(initial.rows[&THREE].place, Place::Reserved(None)));
    assert!(matches!(
        initial.rows[&FOUR].place,
        Place::Reserved(Some(at)) if at == Live::capture(&original_empty).unwrap()
    ));
    assert!(!initial.vacant(TWO, "Case.bin"));
    assert!(initial.vacant(ONE, "Case.bin"));
    assert!(initial.vacant(TWO, "case.bin"));

    let remove = op(TWO, 12);
    f.prepare_recycle(remove);
    let arrived = f.view(Some(&initial)).unwrap();
    assert!(matches!(arrived.rows[&TWO].place, Place::Active { .. }));
    assert!(!arrived.vacant(ONE, "Removed.bin"));

    f.recycle(remove);
    let chosen = f.view(Some(&arrived)).unwrap();
    assert!(
        matches!(chosen.rows[&TWO].place, Place::Recycled { receipt, .. } if receipt.mutation == remove)
    );
    assert_eq!(chosen.rows[&TWO].token, TOKEN);
    assert!(chosen.vacant(ONE, "Removed.bin"));
    assert!(f.store.vacant(ONE, "Removed.bin").unwrap());
    assert_eq!(fs::read(f.entry(ONE, 1, 11).join("payload")).unwrap(), RAW);
}

#[test]
fn known_active_cannot_become_empty_displaced_or_unrouted() {
    let f = Fixture::open(false);
    f.save(ONE, "Known.bin");
    let active = f.view(None).unwrap();
    let source = f.data(ONE);
    let held = f.temp.0.join("held_original");
    fs::rename(&source, &held).unwrap();
    assert!(f.view(Some(&active)).is_err());

    ensure_dir(&source).unwrap();
    assert!(f.view(Some(&active)).is_err());
    let same = source.join(entry_leaf(1, [11; 16]));
    ensure_dir(&same).unwrap();
    fs::copy(
        held.join(entry_leaf(1, [11; 16])).join("receipt"),
        same.join("receipt"),
    )
    .unwrap();
    assert!(f.view(Some(&active)).is_err());
    let untrusted = f.temp.0.join("replacement");
    fs::rename(&source, &untrusted).unwrap();
    fs::rename(&held, &source).unwrap();
    assert!(!f.view(Some(&active)).unwrap().vacant(TWO, "Known.bin"));

    let route = f.system.path.join("container-routes").join(hex(ONE));
    fs::rename(route, f.temp.0.join("held_route")).unwrap();
    assert!(f.view(Some(&active)).is_err());
    assert!(f.view(None).is_err()); // The visible DATA orphan is not a negative proof.
}

#[test]
fn changed_earlier_bytes_and_a_valid_but_truncated_ledger_are_not_new_names() {
    let f = Fixture::open(false);
    f.save(ONE, "Before.bin");
    let one = f.view(None).unwrap();
    f.store
        .change(
            op(ONE, 12),
            Change::Rename {
                expected_sequence: 1,
                name: "After.bin".to_owned(),
            },
            || Ok(()),
        )
        .unwrap();
    let two = f.view(Some(&one)).unwrap();
    assert!(two.vacant(TWO, "Before.bin"));
    assert!(!two.vacant(TWO, "After.bin"));

    f.store
        .change(
            op(ONE, 13),
            Change::Branch {
                name: "feature".to_owned(),
                from: [0x19; 16],
            },
            || Ok(()),
        )
        .unwrap();
    let three = f.view(Some(&two)).unwrap();
    assert_eq!(three.rows[&ONE].place.ledger().unwrap().sequence, 3);

    let original = f.entry(ONE, 1, 11).join("receipt");
    let kept = f.temp.0.join("kept_receipt");
    fs::rename(&original, &kept).unwrap();
    let changed = Receipt {
        mutation: op(ONE, 11),
        sequence: 1,
        change: first("Hidden rewrite.bin"),
    };
    write_file(&original, &codec::encode(&changed).unwrap()).unwrap();
    // Without the observed prefix, the later canonical rename still wins.
    assert!(!f.view(None).unwrap().vacant(TWO, "After.bin"));
    assert!(f.view(Some(&two)).is_err());
    fs::rename(&kept, &original).unwrap();
    fs::rename(f.entry(ONE, 3, 13), f.temp.0.join("kept_entry")).unwrap();
    assert!(f.view(Some(&three)).is_err());
}

#[test]
fn prior_recycled_requires_original_selector_and_named_archived_child() {
    let f = Fixture::open(true);
    f.save(ONE, "Archive.bin");
    let remove = op(ONE, 12);
    f.recycle(remove);
    let recycled = f.view(None).unwrap();
    let archive = f.store.archive.as_ref().unwrap();
    let (_, at) = archive.selected(ONE).unwrap().unwrap();
    let keep = f.recycle_scratch("archive_child");
    fs::rename(&at.path, &keep).unwrap();
    ensure_dir(&at.path).unwrap();
    let entry = at.path.join(entry_leaf(1, [11; 16]));
    ensure_dir(&entry).unwrap();
    fs::copy(
        keep.join(entry_leaf(1, [11; 16])).join("receipt"),
        entry.join("receipt"),
    )
    .unwrap();
    assert!(f.view(Some(&recycled)).is_err());
    fs::rename(&at.path, f.recycle_scratch("archive_substitute")).unwrap();
    fs::rename(&keep, &at.path).unwrap();
    assert!(f.view(Some(&recycled)).unwrap().vacant(TWO, "Archive.bin"));

    let moves = archive.moves().unwrap();
    fs::rename(moves.path.join(hex(ONE)), f.temp.0.join("kept_selector")).unwrap();
    assert!(f.view(Some(&recycled)).is_err());
    assert_eq!(
        fs::read(at.path.join(entry_leaf(1, [11; 16])).join("payload")).unwrap(),
        RAW
    );
}

#[test]
fn invalid_scope_and_late_held_namespace_swap_cannot_certify() {
    let f = Fixture::open(false);
    let g = Fixture::open(false);
    f.save(ONE, "X");
    let exact = f.view(None).unwrap();
    assert!(g.view(Some(&exact)).is_err());
    assert!(View::build(&f.store, &g.system, None, &Limits::default(), || Ok(())).is_err());

    let staging = &f.store.volumes[0].staging().path;
    let keep = f.temp.0.join("original_staging");
    let result = View::build(
        &f.store,
        &f.system,
        Some(&exact),
        &Limits::default(),
        || {
            fs::rename(staging, &keep)?;
            ensure_dir(staging)?;
            Ok(())
        },
    );
    assert!(result.is_err());
    fs::remove_dir(staging).unwrap();
    fs::rename(&keep, staging).unwrap();
    assert!(!f.view(Some(&exact)).unwrap().vacant(TWO, "X"));
}

#[test]
fn bounds_cover_routes_distinct_labels_and_encoded_and_decoded_ledger() {
    let f = Fixture::open(false);
    f.save(ONE, "Key.bin");
    f.store.routes.reserve(TWO, TOKEN).unwrap();
    f.store
        .change(
            op(ONE, 12),
            Change::Tags {
                expected_sequence: 1,
                tags: vec!["a".into(), "b".into()],
            },
            || Ok(()),
        )
        .unwrap();
    let first = fs::read(f.entry(ONE, 1, 11).join("receipt")).unwrap().len();
    for limits in [
        Limits {
            rows: 1,
            ..Limits::default()
        },
        Limits {
            labels: 6,
            ..Limits::default()
        },
        Limits {
            ledger_rows: 1,
            ..Limits::default()
        },
        Limits {
            ledger_bytes: first - 1,
            ..Limits::default()
        },
        Limits {
            ledger_tags: 1,
            ..Limits::default()
        },
    ] {
        assert_eq!(
            f.with_limits(None, &limits).unwrap_err().kind(),
            io::ErrorKind::FileTooLarge
        );
    }
    let exact = f
        .with_limits(
            None,
            &Limits {
                rows: 2,
                labels: 7,
                ledger_rows: 2,
                ledger_tags: 2,
                ..Limits::default()
            },
        )
        .unwrap();
    assert!(!exact.vacant(TWO, "Key.bin"));
    assert_eq!(exact.rows.len(), 2);
}

#[test]
fn duplicate_live_label_and_a_selector_without_its_route_never_build() {
    let f = Fixture::open(true);
    f.save(ONE, "Shared");
    f.save(TWO, "Other");
    let two = f.entry(TWO, 1, 11).join("receipt");
    fs::rename(&two, f.temp.0.join("held_two")).unwrap();
    write_file(
        &two,
        &codec::encode(&Receipt {
            mutation: op(TWO, 11),
            sequence: 1,
            change: first("Shared"),
        })
        .unwrap(),
    )
    .unwrap();
    assert!(f.view(None).is_err());

    fs::rename(f.temp.0.join("held_two"), &two).unwrap();
    let archive = f.store.archive.as_ref().unwrap();
    let foreign = archive.moves().unwrap().path.join(hex(THREE));
    write_file(&foreign, b"not a selector").unwrap();
    assert!(f.view(None).is_err());
    assert_eq!(fs::read(foreign).unwrap(), b"not a selector");
}

#[test]
fn a_canonical_recycled_ledger_cannot_reuse_its_selected_delete_op() {
    let f = Fixture::open(true);
    f.save(ONE, "Archive.bin");
    let remove = op(ONE, 12);
    f.recycle(remove);
    let (_, at) = f
        .store
        .archive
        .as_ref()
        .unwrap()
        .selected(ONE)
        .unwrap()
        .unwrap();

    let entry = at.path.join(entry_leaf(1, remove.request));
    fs::rename(at.path.join(entry_leaf(1, [11; 16])), &entry).unwrap();
    let control = entry.join("receipt");
    fs::rename(&control, f.recycle_scratch("original_control")).unwrap();
    write_file(
        &control,
        &codec::encode(&Receipt {
            mutation: remove,
            sequence: 1,
            change: first("Archive.bin"),
        })
        .unwrap(),
    )
    .unwrap();

    assert!(f.store.recycled(ONE).unwrap().is_some());
    assert!(f.store.operation_receipt(remove).is_err());
    assert!(f.view(None).is_err());
    assert_eq!(fs::read(entry.join("payload")).unwrap(), RAW);
}

#[test]
fn warm_unique_creates_and_renames_do_not_enumerate_routes_or_replay_other_ledgers() {
    let f = Fixture::open(true);
    f.save(ONE, "Initial.bin");
    let warm = f.store.name_clock.counts();
    assert!(warm.0 > 0);
    f.save(TWO, "Second.bin");
    f.save(THREE, "Third.bin");
    f.store
        .change(
            op(ONE, 12),
            Change::Rename {
                expected_sequence: 1,
                name: "Renamed.bin".into(),
            },
            || Ok(()),
        )
        .unwrap();
    assert_eq!(f.store.name_clock.counts(), warm);

    assert!(matches!(
        f.store
            .save(op(FOUR, 11), first("Renamed.bin"), RAW, || Ok(())),
        Err(super::super::Error::Conflict(
            super::super::Conflict::AlreadyExists
        ))
    ));
    assert_eq!(f.store.routes.get(FOUR).unwrap(), None);
    assert_eq!(f.store.name_clock.counts(), warm);
    f.save(FOUR, "Initial.bin");
    assert_eq!(f.store.name_clock.counts(), warm);
    assert_eq!(fs::read(f.entry(ONE, 1, 11).join("payload")).unwrap(), RAW);
}

#[test]
fn a_failed_final_authorization_leaves_only_a_reservation_and_rebuilds_before_other_names() {
    let f = Fixture::open(true);
    f.save(ONE, "First");
    let warm = f.store.name_clock.counts();
    let first_gate = Cell::new(true);
    assert!(
        f.store
            .save(op(TWO, 11), first("Retry-name"), RAW, || {
                if first_gate.replace(false) {
                    Ok(())
                } else {
                    Err(io::ErrorKind::PermissionDenied.into())
                }
            })
            .is_err()
    );
    assert_eq!(f.store.name_clock.counts(), warm);
    assert_eq!(f.store.routes.get(TWO).unwrap(), Some(TOKEN));
    assert!(f.store.operation_receipt(op(TWO, 11)).unwrap().is_none());
    assert!(!f.entry(TWO, 1, 11).exists());

    f.save(THREE, "Retry-name");
    let rebuilt = f.store.name_clock.counts();
    assert!(rebuilt.0 > warm.0 && rebuilt.1 > warm.1);
    f.save(TWO, "Exact-second");
    assert_eq!(f.store.name_clock.counts(), rebuilt);
}

#[test]
fn observed_active_loss_cannot_become_a_warm_or_cold_negative() {
    let f = Fixture::open(true);
    f.save(ONE, "Known");
    let warm = f.store.name_clock.counts();
    let origin = f.data(ONE);
    let kept = f.temp.0.join("known_original");
    fs::rename(&origin, &kept).unwrap();
    assert!(
        f.store
            .save(op(TWO, 11), first("Known"), RAW, || Ok(()))
            .is_err()
    );
    assert!(f.store.name_clock.counts().0 > warm.0);
    assert_eq!(f.store.routes.get(TWO).unwrap(), None);
    assert!(!f.data(TWO).exists());

    fs::rename(&kept, &origin).unwrap();
    f.save(TWO, "Available");
    assert_eq!(fs::read(f.entry(ONE, 1, 11).join("payload")).unwrap(), RAW);
}

#[test]
fn a_warm_name_receipt_remains_exact_after_later_metadata_and_observed_fault() {
    let f = Fixture::open(true);
    f.save(ONE, "Before");
    let tags = |n| Change::Tags {
        expected_sequence: n,
        tags: vec!["tag".into()],
    };
    f.store.change(op(ONE, 12), tags(1), || Ok(())).unwrap();
    f.store
        .change(
            op(ONE, 13),
            Change::Rename {
                expected_sequence: 2,
                name: "After".into(),
            },
            || Ok(()),
        )
        .unwrap();
    f.store.change(op(ONE, 14), tags(3), || Ok(())).unwrap();
    let warm = f.store.name_clock.counts();

    let receipt = f.entry(ONE, 3, 13).join("receipt");
    let saved = f.temp.0.join("original_rename");
    fs::rename(&receipt, &saved).unwrap();
    write_file(
        &receipt,
        &codec::encode(&Receipt {
            mutation: op(ONE, 13),
            sequence: 3,
            change: Change::Rename {
                expected_sequence: 2,
                name: "Replaced".into(),
            },
        })
        .unwrap(),
    )
    .unwrap();

    // A separate actual read observes temporary native custody loss. Merely
    // returning the original name cannot make the previous cache fresh.
    let staging = &f.store.volumes[0].staging().path;
    let held = f.temp.0.join("held_staging");
    fs::rename(staging, &held).unwrap();
    assert!(f.store.node(ONE, [0x19; 16]).is_err());
    fs::rename(&held, staging).unwrap();

    assert!(
        f.store
            .save(op(TWO, 11), first("After"), RAW, || Ok(()))
            .is_err()
    );
    assert!(f.store.name_clock.counts().0 > warm.0);
    assert_eq!(f.store.routes.get(TWO).unwrap(), None);
    assert!(!f.data(TWO).exists());
    fs::rename(&saved, &receipt).unwrap();
    f.save(TWO, "Available");
}

#[test]
fn recycle_selection_and_outside_cleanup_invalidate_the_warm_projection() {
    let f = Fixture::open(true);
    f.save(ONE, "Released");
    let warm = f.store.name_clock.counts();
    f.recycle(op(ONE, 12));
    assert_eq!(f.store.name_clock.counts(), warm);
    f.save(TWO, "Released");
    let rebuilt = f.store.name_clock.counts();
    assert!(rebuilt.0 > warm.0);
    f.save(THREE, "Another");
    assert_eq!(f.store.name_clock.counts(), rebuilt);
    assert!(matches!(
        f.store.operation_receipt(op(ONE, 12)).unwrap(),
        Some(super::super::OperationReceipt::Recycle(_))
    ));
}

const PEER: [u8; 32] = [0x72; 32];
const UNUSED: [u8; 32] = [0x93; 32];

struct Degraded {
    store: Store,
    local: PrivateTemp,
    _archive: PrivateTemp,
}

impl Degraded {
    fn open() -> Self {
        let local = PrivateTemp::under(&std::env::temp_dir());
        let archived = PrivateTemp::under(Path::new("/dev/shm"));
        let system = fixture_dir(&local, "system");
        let owner = system.opaque_owner(c"container-owner").unwrap();
        // Only this fixture's data peers share a device: exercise native
        // required-role evidence without claiming physical pool enrollment.
        let data = [
            (TOKEN, "data_disk"),
            (PEER, "peer_disk"),
            (UNUSED, "unused_disk"),
        ]
        .into_iter()
        .map(|(token, name)| DataRoot {
            root: fixture_dir(&local, name),
            token,
        })
        .collect();
        let mut store = Store::open_roster(&system, data, owner).unwrap();
        store.archive = Some(
            super::super::recycle::Recycle::open(
                system,
                fixture_dir(&archived, "recycle"),
                super::super::recycle_receipt::RecycleRole {
                    pool: POOL,
                    token: RECYCLE,
                },
            )
            .unwrap(),
        );
        Self {
            store,
            local,
            _archive: archived,
        }
    }

    fn first(&self, id: Id, token: [u8; 32], name: &str) {
        self.store.routes.reserve(id, token).unwrap();
        self.store
            .save(op(id, 11), first(name), RAW, || Ok(()))
            .unwrap();
    }

    fn displacement(&self, token: [u8; 32]) -> (PathBuf, PathBuf) {
        let original = self.store.enrolled(token).unwrap().root().path.clone();
        let moved = self.local.0.join(format!("held_{}", token[0]));
        (original, moved)
    }

    fn disconnect(&self, token: [u8; 32]) {
        let (original, moved) = self.displacement(token);
        fs::rename(original, moved).unwrap();
    }
}

#[test]
fn exact_fallback_retains_the_name_it_confirms_before_a_later_lost_child() {
    for seeded in [false, true] {
        let f = Degraded::open();
        if seeded {
            f.first(ONE, TOKEN, "Known");
        }
        f.disconnect(UNUSED);
        f.first(TWO, TOKEN, "Exact fallback");
        let (from, keep) = (
            f.store.enrolled(TOKEN).unwrap().container(TWO),
            f.local.0.join("original_second_container"),
        );
        fs::rename(&from, &keep).unwrap();
        assert_eq!(f.store.routes.get(TWO).unwrap(), Some(TOKEN));
        assert!(
            f.store
                .save(op(THREE, 11), first("Exact fallback"), RAW, || Ok(()))
                .is_err()
        );
        assert_eq!(f.store.routes.get(THREE).unwrap(), None);
        assert!(!f.store.enrolled(TOKEN).unwrap().container(THREE).exists());
        fs::rename(&keep, &from).unwrap();
        assert_eq!(
            fs::read(from.join(entry_leaf(1, [11; 16])).join("payload")).unwrap(),
            RAW
        );
    }
}

#[test]
fn exact_fallback_still_refuses_a_required_peer_lost_at_the_native_cut() {
    let f = Degraded::open();
    f.first(ONE, TOKEN, "Known");
    f.first(TWO, PEER, "Still required");
    f.disconnect(UNUSED);
    let (peer, kept) = f.displacement(PEER);
    let mut adapter = NativeCut {
        stage: f.store.stage(&f.store.volumes[0]).unwrap(),
        before_install: &|| fs::rename(&peer, &kept),
        after_sync: &|| panic!("a missing required role must not select"),
    };
    assert!(rename_at_cut(&f.store, &mut adapter).is_err());
    assert!(
        !f.store.volumes[0]
            .container(ONE)
            .join(entry_leaf(2, [12; 16]))
            .exists()
    );
    fs::rename(&kept, &peer).unwrap();
    assert!(f.store.operation_receipt(op(ONE, 12)).unwrap().is_none());
}

struct NativeCut<'a> {
    stage: super::super::FlatStage<'a>,
    before_install: &'a dyn Fn() -> io::Result<()>,
    after_sync: &'a dyn Fn(),
}

impl super::super::Publication for NativeCut<'_> {
    fn stage_receipt(
        &mut self,
        receipt: &Receipt,
        authorize: &impl Fn() -> io::Result<()>,
    ) -> Result<(), super::super::Error> {
        self.stage.stage_receipt(receipt, authorize)
    }

    fn install(
        &self,
        parent: &HeldDir,
        name: &CStr,
        authorize: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        (self.before_install)()?;
        self.stage.install(parent, name, authorize)
    }

    fn confirm_moved(&self, _parent: &HeldDir, _name: &CStr) -> io::Result<()> {
        (self.after_sync)();
        Ok(())
    }
}

fn rename_at_cut(store: &Store, cut: &mut NativeCut<'_>) -> Result<Receipt, super::super::Error> {
    let route = &store.volumes[0];
    let object = store.object(ONE).unwrap();
    let mut held = object.lock().unwrap();
    store.publish_under_gate(
        op(ONE, 12),
        Change::Rename {
            expected_sequence: 1,
            name: "Intended".into(),
        },
        (route, &mut held),
        cut,
        || Ok(()),
    )
}

#[test]
fn late_named_replacement_is_rejected_at_the_actual_receipt_cut() {
    let f = Fixture::open(true);
    f.save(ONE, "Before");
    let root = f.recycle_scratch("recycle").join("containers");
    let original = f.recycle_scratch("original_arrived");
    let mut adapter = NativeCut {
        stage: f.store.stage(&f.store.volumes[0]).unwrap(),
        before_install: &|| {
            fs::rename(&root, &original)?;
            ensure_dir(&root)?;
            Ok(())
        },
        after_sync: &|| panic!("a displaced common authority must not select"),
    };
    assert!(rename_at_cut(&f.store, &mut adapter).is_err());
    assert!(!f.entry(ONE, 2, 12).exists());
    fs::remove_dir(&root).unwrap();
    fs::rename(&original, &root).unwrap();
    assert!(f.store.operation_receipt(op(ONE, 12)).unwrap().is_none());
    assert_eq!(
        f.store.detail_page(ONE, None, None).unwrap().unwrap().name,
        "Before"
    );
}

#[test]
fn a_post_flush_index_fault_does_not_change_a_confirmed_receipt() {
    let f = Fixture::open(true);
    f.save(ONE, "Old");
    let warm = f.store.name_clock.counts();
    let mut adapter = NativeCut {
        stage: f.store.stage(&f.store.volumes[0]).unwrap(),
        before_install: &|| Ok(()),
        after_sync: &|| f.store.name_clock.fault(),
    };
    let receipt = rename_at_cut(&f.store, &mut adapter).unwrap();
    assert_eq!(f.store.name_clock.counts(), warm);
    assert_eq!(
        f.store.operation_receipt(op(ONE, 12)).unwrap(),
        Some(super::super::OperationReceipt::Mutation(receipt))
    );
    f.save(TWO, "Old");
    assert!(f.store.name_clock.counts().0 > warm.0);
}

#[test]
fn last_route_generation_denial_leaves_only_the_private_candidate() {
    let f = Fixture::open(true);
    f.save(ONE, "Baseline");
    let ticket = f
        .store
        .names
        .lock()
        .unwrap()
        .admit(&f.store, TWO, "New", TOKEN)
        .unwrap()
        .unwrap();
    let looked = Cell::new(0);
    let check = || {
        looked.set(looked.get() + 1);
        ticket.require(&f.store, ChangeCut::default().route())
    };
    let last = || {
        if looked.get() == 2 {
            f.store.name_clock.fault();
        }
        ticket.require_generation(&f.store)
    };
    let _guard = f.store.names.lock().unwrap();
    assert!(
        f.store
            .routes
            .reserve_name_checked(TWO, TOKEN, &check, &last)
            .is_err()
    );
    assert_eq!(looked.get(), 2);
    assert_eq!(f.store.routes.get(TWO).unwrap(), None);
    assert!(!f.data(TWO).exists());
    assert!(
        f.store
            .routes
            .name_namespace()
            .path
            .join(format!(".{}.incoming", hex(TWO)))
            .is_file()
    );
}

#[test]
fn durably_recycled_but_not_retired_cannot_revive_after_selector_loss() {
    let f = Fixture::open(true);
    f.save(ONE, "Known");
    let selector = f.system.path.join("container-moves").join(hex(ONE));
    let deleted = op(ONE, 12);
    assert!(matches!(
        f.store.move_to_recycle(deleted, 1, || {
            if selector.exists() { Err(io::ErrorKind::PermissionDenied.into()) } else { Ok(()) }
        }),
        Err(super::super::Error::CleanupPending { receipt, .. }) if receipt.mutation == deleted
    ));
    assert_eq!(fs::read(f.entry(ONE, 1, 11).join("payload")).unwrap(), RAW);
    fs::rename(&selector, f.temp.0.join("old_selector")).unwrap();
    let arrivals = f.recycle_scratch("recycle").join("containers");
    assert_eq!(fs::read_dir(&arrivals).unwrap().count(), 1);
    let another = op(ONE, 19);
    let Err(super::super::Error::Io(error)) = f.store.move_to_recycle(another, 1, || Ok(())) else {
        panic!("an observed selected recycle cannot be replaced with a new delete");
    };
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("cannot be selected again"));
    assert_eq!(fs::read_dir(&arrivals).unwrap().count(), 1);
    assert!(!selector.exists());
    assert!(f.store.operation_receipt(another).unwrap().is_none());
    assert_eq!(fs::read(f.entry(ONE, 1, 11).join("payload")).unwrap(), RAW);
    assert!(
        f.store
            .change(
                op(ONE, 13),
                Change::Rename {
                    expected_sequence: 1,
                    name: "Must not revive".into(),
                },
                || Ok(())
            )
            .is_err()
    );
    assert!(!f.entry(ONE, 2, 13).exists());
    assert_eq!(fs::read(f.entry(ONE, 1, 11).join("payload")).unwrap(), RAW);
}

#[test]
fn failed_witness_retention_returns_the_real_commit_and_never_grants_from_the_partial_map() {
    let f = Fixture::open(true);
    f.save(ONE, "Old");
    let target = f.data(ONE);
    let preserved = f.temp.0.join("confirmed_but_not_named");
    let mut adapter = NativeCut {
        stage: f.store.stage(&f.store.volumes[0]).unwrap(),
        before_install: &|| Ok(()),
        after_sync: &|| fs::rename(&target, &preserved).unwrap(),
    };
    let confirmed = rename_at_cut(&f.store, &mut adapter).unwrap();
    assert_eq!(confirmed.mutation, op(ONE, 12));
    fs::rename(&preserved, &target).unwrap();
    assert_eq!(
        f.store.operation_receipt(op(ONE, 12)).unwrap(),
        Some(super::super::OperationReceipt::Mutation(confirmed)),
    );
    assert!(
        f.store
            .save(op(TWO, 11), first("Old"), RAW, || Ok(()))
            .is_err()
    );
    assert_eq!(f.store.routes.get(TWO).unwrap(), None);
}

#[test]
fn reopened_explicit_recycle_replay_retains_the_already_selected_cut_before_cleanup() {
    let f = Fixture::open(true);
    f.save(ONE, "Before recycle");
    let selector = f.system.path.join("container-moves").join(hex(ONE));
    let remove = op(ONE, 12);
    let Err(super::super::Error::CleanupPending {
        receipt: before, ..
    }) = f.store.move_to_recycle(remove, 1, || {
        if selector.exists() {
            Err(io::ErrorKind::PermissionDenied.into())
        } else {
            Ok(())
        }
    })
    else {
        panic!("first selection did not leave the original for cleanup")
    };
    let f = f.reopen();
    assert_eq!(f.store.name_clock.counts(), (0, 0));
    let first_check = Cell::new(true);
    let Err(super::super::Error::CleanupPending {
        receipt: replayed, ..
    }) = f.store.move_to_recycle(remove, 1, || {
        if first_check.replace(false) {
            Ok(())
        } else {
            Err(io::ErrorKind::PermissionDenied.into())
        }
    })
    else {
        panic!("exact reopen did not return the known selected result")
    };
    assert_eq!(replayed, before);
    assert_eq!(f.store.name_clock.counts(), (0, 0));
    assert_eq!(fs::read(f.entry(ONE, 1, 11).join("payload")).unwrap(), RAW);
    fs::rename(&selector, f.temp.0.join("kept_previous_selector")).unwrap();

    assert!(
        f.store
            .change(
                op(ONE, 13),
                Change::Rename {
                    expected_sequence: 1,
                    name: "Not live again".into(),
                },
                || Ok(())
            )
            .is_err()
    );
    assert!(!f.entry(ONE, 2, 13).exists());
    assert_eq!(fs::read(f.entry(ONE, 1, 11).join("payload")).unwrap(), RAW);
}

fn cap(f: &Fixture, rows: usize, labels: usize) {
    f.store.names.lock().unwrap().limit_witnesses(rows, labels);
}

fn change_name(
    f: &Fixture,
    id: Id,
    request: u8,
    expected: u64,
    name: &str,
) -> Result<Receipt, super::super::Error> {
    f.store.change(
        op(id, request),
        Change::Rename {
            expected_sequence: expected,
            name: name.into(),
        },
        || Ok(()),
    )
}

fn budget(error: super::super::Error) {
    let super::super::Error::Io(error) = error else {
        panic!("expected a name budget refusal")
    };
    assert_eq!(error.kind(), io::ErrorKind::FileTooLarge);
    assert_eq!(error.to_string(), "name witness capacity exhausted");
}

#[test]
fn full_name_rows_refuse_new_route_without_poisoning_known_rename_or_recycle() {
    let f = Fixture::open(true);
    cap(&f, 1, 32);
    f.save(ONE, "Known");
    budget(
        f.store
            .save(op(TWO, 12), first("Other"), RAW, || Ok(()))
            .unwrap_err(),
    );
    assert_eq!(f.store.routes.get(TWO).unwrap(), None);
    assert!(!f.data(TWO).exists());
    assert!(f.store.operation_receipt(op(TWO, 12)).unwrap().is_none());

    change_name(&f, ONE, 13, 1, "Known again").unwrap();
    assert!(f.entry(ONE, 2, 13).is_dir());
    f.store.move_to_recycle(op(ONE, 14), 2, || Ok(())).unwrap();
    assert!(
        f.system
            .path
            .join("container-moves")
            .join(hex(ONE))
            .is_file()
    );
    assert!(!f.data(ONE).exists());
}

#[test]
fn name_preflight_counts_net_labels_and_does_not_call_wrong_token_capacity() {
    let f = Fixture::open(true);
    cap(&f, 1, 5);
    f.save(ONE, "Old");
    let wrong = f
        .store
        .names
        .lock()
        .unwrap()
        .preflight(ONE, [0xaa; 32], 99)
        .unwrap_err();
    assert_eq!(wrong.kind(), io::ErrorKind::InvalidData);
    assert!(wrong.to_string().contains("retained token"));

    budget(change_name(&f, ONE, 12, 1, "Longer").unwrap_err());
    assert!(!f.entry(ONE, 2, 12).exists());
    assert!(f.store.operation_receipt(op(ONE, 12)).unwrap().is_none());
    assert_eq!(
        f.store.detail_page(ONE, None, None).unwrap().unwrap().name,
        "Old"
    );

    let next = change_name(&f, ONE, 13, 1, "exact").unwrap();
    assert_eq!(next.sequence, 2);
    assert_eq!(
        f.store.detail_page(ONE, None, None).unwrap().unwrap().name,
        "exact"
    );
}

#[test]
fn fresh_untracked_recycle_at_capacity_never_prepares_or_selects_r() {
    let f = Fixture::open(true);
    f.save(ONE, "One");
    f.save(TWO, "Two");
    let f = f.reopen();
    cap(&f, 1, 64);
    // Both genuine CR1 routes exceed this miniature complete view. The
    // successful exact fallback records only this actually confirmed CID.
    change_name(&f, ONE, 12, 1, "Changed").unwrap();
    let archived = f.recycle_scratch("recycle").join("containers");
    assert_eq!(fs::read_dir(&archived).unwrap().count(), 0);
    budget(
        f.store
            .move_to_recycle(op(TWO, 13), 1, || Ok(()))
            .unwrap_err(),
    );
    assert_eq!(fs::read_dir(&archived).unwrap().count(), 0);
    assert!(
        !f.system
            .path
            .join("container-moves")
            .join(hex(TWO))
            .exists()
    );
    assert_eq!(fs::read(f.entry(TWO, 1, 11).join("payload")).unwrap(), RAW);
    assert!(f.store.operation_receipt(op(TWO, 13)).unwrap().is_none());
    change_name(&f, ONE, 14, 2, "Still works").unwrap();
}

#[test]
fn unretained_already_selected_r_returns_exact_receipt_without_cleaning_or_reviving_source() {
    let f = Fixture::open(true);
    f.save(ONE, "Before recycle");
    let delete = op(ONE, 12);
    let selector = f.system.path.join("container-moves").join(hex(ONE));
    let Err(super::super::Error::CleanupPending { receipt: old, .. }) =
        f.store.move_to_recycle(delete, 1, || {
            if selector.exists() {
                Err(io::ErrorKind::PermissionDenied.into())
            } else {
                Ok(())
            }
        })
    else {
        panic!("fixture must leave a durable selector with cleanup still pending");
    };
    let f = f.reopen();
    cap(&f, 0, 64);
    let Err(super::super::Error::CleanupPending { receipt, error }) =
        f.store.move_to_recycle(delete, 1, || Ok(()))
    else {
        panic!("a witnessed committed recycle stays committed and does not retire here");
    };
    assert_eq!(receipt, old);
    assert_eq!(error.kind(), io::ErrorKind::FileTooLarge);
    assert_eq!(fs::read(f.entry(ONE, 1, 11).join("payload")).unwrap(), RAW);
    assert!(selector.is_file());

    // Restoring capacity cannot repair a known selected fact that was not
    // retained. Even if the surviving DATA masquerades as active, refuse.
    cap(&f, 3, 64);
    fs::rename(&selector, f.temp.0.join("preserved_selector")).unwrap();
    assert!(change_name(&f, ONE, 13, 1, "Do not revive").is_err());
    assert!(!f.entry(ONE, 2, 13).exists());
    assert_eq!(fs::read(f.entry(ONE, 1, 11).join("payload")).unwrap(), RAW);
}
