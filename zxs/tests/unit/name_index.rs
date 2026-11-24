use super::*;
use crate::{
    container_store::{
        Body, Change, Mutation, Receipt, Target, ensure_dir, entry_leaf, hex, write_file,
    },
    test_support::PrivateTemp,
};
use std::{fs, path::PathBuf};

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
}

impl Fixture {
    fn open(with_recycle: bool) -> Self {
        let temp = PrivateTemp::under(&std::env::temp_dir());
        let dir = |name| {
            let at = temp.0.join(name);
            ensure_dir(&at).unwrap();
            HeldDir::trusted_storage(&at).unwrap()
        };
        let system = dir("system");
        let owner = system.opaque_owner(c"container-owner").unwrap();
        let data = dir("data_disk");
        let mut store = Store::open_roster(
            &system,
            vec![super::super::DataRoot {
                token: TOKEN,
                root: data,
            }],
            owner,
        )
        .unwrap();
        if with_recycle {
            // Real post-admission namespace semantics on sibling fake roots,
            // not qualification of independently mounted media.
            store.archive = Some(
                super::super::recycle::Recycle::open(
                    HeldDir::trusted_storage(&system.path).unwrap(),
                    dir("recycle"),
                    super::super::recycle_receipt::RecycleRole {
                        pool: POOL,
                        token: RECYCLE,
                    },
                )
                .unwrap(),
            );
        }
        Self {
            store,
            system,
            temp,
        }
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
    let keep = f.temp.0.join("archive_child");
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
    fs::rename(&at.path, f.temp.0.join("archive_substitute")).unwrap();
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
    fs::rename(&control, f.temp.0.join("original_control")).unwrap();
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
