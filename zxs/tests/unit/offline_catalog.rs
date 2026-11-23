use super::*;
use crate::{
    container_store::{Body, Change, Mutation, Store, Target, create_file, ensure_dir, entry_leaf},
    hash::Sha256,
    test_support::PrivateTemp,
};
use std::{
    fs::{self, File, FileTimes},
    io::Write,
    os::unix::fs::MetadataExt,
    path::Path,
    time::{Duration, UNIX_EPOCH},
};

const POOL: Id = [9; 16];
const TOKEN: [u8; 32] = [8; 32];

fn data(root: &Path) -> DataRoot {
    DataRoot {
        root: HeldDir::trusted_storage(root).unwrap(),
        token: TOKEN,
    }
}

struct Fixture {
    _temporary: PrivateTemp,
    system: HeldDir,
    recycle: HeldDir,
    data: Vec<DataRoot>,
}

impl Fixture {
    fn new() -> Self {
        let temporary = PrivateTemp::under(&std::env::temp_dir());
        let [system, recycle, location] = ["system", "recycle", "data"].map(|leaf| {
            let path = temporary.0.join(leaf);
            ensure_dir(&path).unwrap();
            path
        });
        Self {
            _temporary: temporary,
            system: HeldDir::trusted_storage(&system).unwrap(),
            recycle: HeldDir::trusted_storage(&recycle).unwrap(),
            data: vec![data(&location)],
        }
    }

    fn store(&self) -> Store {
        let owner = self.system.opaque_owner(c"container-owner").unwrap();
        Store::open_roster(&self.system, vec![data(&self.data[0].root.path)], owner).unwrap()
    }

    fn names(&self) -> io::Result<BTreeSet<String>> {
        // Sibling directories qualify local reads, not multi-device admission.
        read_live_names(&self.system, &self.recycle, &self.data, POOL)
    }
}

fn save(store: &Store, container: Id, name: &str) {
    store
        .save(
            Mutation {
                container,
                request: [2; 16],
            },
            Change::Save {
                target: Target::FirstMain {
                    name: name.to_owned(),
                },
                node: [3; 16],
                body: Body {
                    length: 0,
                    sha256: Sha256::new().finish(),
                },
            },
            &b""[..],
            || Ok(()),
        )
        .unwrap();
}

#[test]
fn fresh_marker_only_paths_are_not_initialized_by_the_preview() {
    let fixture = Fixture::new();
    assert!(fixture.names().unwrap().is_empty());
    for root in [&fixture.system, &fixture.recycle, &fixture.data[0].root] {
        assert_eq!(fs::read_dir(&root.path).unwrap().count(), 0);
    }
}

#[test]
fn preview_uses_exact_names_and_does_not_touch_access_times() {
    let fixture = Fixture::new();
    let store = fixture.store();
    save(&store, [1; 16], "Report");
    save(&store, [2; 16], "report");
    store.routes.reserve([4; 16], TOKEN).unwrap();
    drop(store);
    let container = fixture.data[0].root.path.join("data").join(hex([1; 16]));
    let generation = container.join(entry_leaf(1, [2; 16]));
    let touched = [
        fixture.system.path.join("container-routes"),
        fixture
            .system
            .path
            .join("container-routes")
            .join(hex([1; 16])),
        container,
        generation.join("receipt"),
        generation.join("payload"),
    ];
    for path in &touched {
        File::open(path)
            .unwrap()
            .set_times(FileTimes::new().set_accessed(UNIX_EPOCH + Duration::from_secs(1)))
            .unwrap();
    }
    let before = touched
        .each_ref()
        .map(|p| fs::metadata(p).unwrap().accessed().unwrap());
    let expected = BTreeSet::from([String::from("Report"), String::from("report")]);
    assert_eq!(fixture.names().unwrap(), expected);
    let after = touched
        .each_ref()
        .map(|p| fs::metadata(p).unwrap().accessed().unwrap());
    assert_eq!(after, before);
}

#[test]
fn a_selected_recycle_receipt_must_match_its_exact_archived_copy() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let id = [1; 16];
    let request = [5; 16];
    save(&store, id, "report");
    drop(store);

    let moves = fixture.system.path.join("container-moves");
    let arrived = fixture.recycle.path.join("containers");
    let arrival = arrived.join(format!("{}-{}", hex(id), hex(request)));
    for directory in [&moves, &arrived, &arrival, &arrival.join("container")] {
        ensure_dir(directory).unwrap();
    }
    let meta = fs::metadata(&arrival).unwrap();
    let mut receipt = [0; 108];
    receipt[..4].copy_from_slice(b"RC1\0");
    receipt[4..20].copy_from_slice(&id);
    receipt[20..36].copy_from_slice(&request);
    for (slot, number) in receipt[36..76].as_chunks_mut::<8>().0.iter_mut().zip([
        1_u64,
        meta.dev() ^ 1, // Synthetic distinct coordinate for pure parser coverage.
        21,
        meta.dev(),
        meta.ino(),
    ]) {
        *slot = number.to_be_bytes();
    }
    create_file(&moves.join(hex(id)))
        .unwrap()
        .write_all(&receipt)
        .unwrap();
    assert!(fixture.names().is_err());
    create_file(&arrival.join("ready"))
        .unwrap()
        .write_all(&receipt)
        .unwrap();
    assert!(fixture.names().unwrap().is_empty());
}

#[test]
fn unknown_routed_data_cannot_return_a_partial_live_catalog() {
    let fixture = Fixture::new();
    let store = fixture.store();
    save(&store, [1; 16], "first");
    store.routes.reserve([4; 16], [7; 32]).unwrap();
    drop(store);
    assert!(fixture.names().is_err());
}
