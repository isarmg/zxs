use super::*;
use crate::{container_store::ensure_dir, test_support::PrivateTemp};
use std::{fs, path::Path};

const POOL: Id = [9; 16];
const DATA_A: [u8; 32] = [3; 32];
const DATA_B: [u8; 32] = [4; 32];
const RECYCLE: [u8; 32] = [6; 32];

struct Fixture {
    _temp: PrivateTemp,
    system: HeldDir,
    recycle: HeldDir,
    data: [HeldDir; 2],
}

fn hold(path: &Path) -> HeldDir {
    HeldDir::trusted_storage(path).unwrap()
}

impl Fixture {
    fn new() -> Self {
        let temp = PrivateTemp::under(&std::env::temp_dir());
        let [system, recycle, a, b] = ["system", "recycle", "a", "b"].map(|leaf| {
            let path = temp.0.join(leaf);
            ensure_dir(&path).unwrap();
            hold(&path)
        });
        Self {
            _temp: temp,
            system,
            recycle,
            data: [a, b],
        }
    }

    fn roster(&self) -> Vec<DataRoot> {
        self.data
            .iter()
            .zip([DATA_A, DATA_B])
            .map(|(root, token)| DataRoot {
                root: hold(&root.path),
                token,
            })
            .collect()
    }

    fn initialize(&self) {
        let owner = self.system.opaque_owner(c"container-owner").unwrap();
        let store = Store::open_roster(&self.system, self.roster(), owner).unwrap();
        let archive =
            Recycle::open(hold(&self.system.path), hold(&self.recycle.path), RecycleRole { pool: POOL, token: RECYCLE }).unwrap();
        drop((store, archive));
    }

    fn writer(&self) -> io::Result<OfflineImport> {
        // These local siblings test namespace and write selection, not
        // the operational predicate for three distinct storage mounts.
        OfflineImport::open_existing(
            hold(&self.system.path),
            hold(&self.recycle.path),
            self.roster(),
            POOL,
            RECYCLE,
        )
    }

    fn intent(&self) -> ImportIntent {
        let mut hash = Sha256::new();
        hash.update(b"hello").unwrap();
        ImportIntent {
            seed: [1; 16],
            pool: POOL,
            data_token: DATA_B,
            data_path: self.data[1].path.as_os_str().as_bytes().to_vec(),
            source_root: b"/external".to_vec(),
            source_relative: b"folder/report".to_vec(),
            source: SourceWitness {
                device: 41,
                inode: 42,
                uid: 1000,
                mode: 0o100_600,
                links: 1,
                modified: (1, 2),
                changed: (3, 4),
            },
            name: String::from("Report"),
            body: Body {
                length: 5,
                sha256: hash.finish(),
            },
        }
    }
}

struct MustNotRead;

impl Read for MustNotRead {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        panic!("an exact replay or wrong target must not read the source body");
    }
}

#[test]
fn existing_import_placement_and_receipt_replay_never_touch_other_data_stages() {
    let fixture = Fixture::new();
    fixture.initialize();
    let unrelated = fixture.data[0]
        .path
        .join("staging")
        .join("0000000000000000");
    ensure_dir(&unrelated).unwrap();
    let writer = fixture.writer().unwrap();
    let mut intent = fixture.intent();
    intent.seed = (1..=u8::MAX)
        .map(|n| [n; 16])
        .find(|seed| {
            intent.seed = *seed;
            intent.identity().unwrap().0.container[0].is_multiple_of(2)
        })
        .unwrap();
    let (mutation, node) = intent.identity().unwrap();
    let chosen = writer.save(&intent, &b"hello"[..], || Ok(())).unwrap();
    assert_eq!(chosen.sequence, 1);
    assert_eq!(chosen.mutation, mutation);
    assert_eq!(chosen.change, intent.change(node));
    assert_eq!(
        writer.store.routes.get(mutation.container).unwrap(),
        Some(DATA_B)
    );
    assert!(
        unrelated.is_dir(),
        "opening the writer must not sweep other DATA"
    );
    drop(writer);
    let writer = fixture.writer().unwrap();
    assert_eq!(
        writer.receipt(&intent, || Ok(())).unwrap(),
        Some(chosen.clone())
    );
    assert_eq!(
        writer.save(&intent, MustNotRead, || Ok(())).unwrap(),
        chosen
    );
}

#[test]
fn existing_writer_refuses_marker_only_or_missing_initialized_areas() {
    let fixture = Fixture::new();
    assert!(fixture.writer().is_err());
    assert!(!fixture.system.path.join("container-owner").exists());
    fixture.initialize();
    let staging = fixture.data[1].path.join("staging");
    fs::remove_dir(&staging).unwrap();
    assert!(fixture.writer().is_err());
    assert!(!staging.exists());
}

#[test]
fn route_reservation_cannot_be_reused_by_an_edited_intent_or_to_a_different_disk() {
    let fixture = Fixture::new();
    fixture.initialize();
    let writer = fixture.writer().unwrap();
    let mut intent = fixture.intent();
    let (initial, initial_node) = intent.identity().unwrap();
    assert_eq!(initial.request, intent.seed);

    writer
        .store
        .routes
        .reserve(initial.container, DATA_A)
        .unwrap();
    assert!(writer.save(&intent, MustNotRead, || Ok(())).is_err());
    intent.name = String::from("report");
    let (renamed, renamed_node) = intent.identity().unwrap();
    assert_ne!(renamed.container, initial.container);
    assert_ne!(renamed_node, initial_node);
    intent.body.length += 1;
    let (larger, _) = intent.identity().unwrap();
    assert_ne!(larger.container, renamed.container);
    intent.source_relative = b"folder/../report".to_vec();
    assert!(intent.identity().is_err());
}

#[test]
fn an_exact_reserved_route_retries_only_the_planned_complete_body() {
    let fixture = Fixture::new();
    fixture.initialize();
    let writer = fixture.writer().unwrap();
    let intent = fixture.intent();
    let (mutation, _) = intent.identity().unwrap();
    writer
        .store
        .routes
        .reserve(mutation.container, DATA_B)
        .unwrap();
    assert!(matches!(
        writer.save(&intent, &b"wrong"[..], || Ok(())),
        Err(Error::WrongDigest)
    ));
    assert_eq!(writer.receipt(&intent, || Ok(())).unwrap(), None);
    let first = writer.save(&intent, &b"hello"[..], || Ok(())).unwrap();
    assert_eq!(first.sequence, 1);
    assert_eq!(writer.receipt(&intent, || Ok(())).unwrap(), Some(first));
}

#[test]
fn lost_source_proof_prevents_initial_route_publication() {
    use std::cell::Cell;

    struct HeldBody<'a>(&'a Cell<bool>, &'static [u8]);
    impl Read for HeldBody<'_> {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            self.0.set(true);
            self.1.read(bytes)
        }
    }

    let fixture = Fixture::new();
    fixture.initialize();
    let writer = fixture.writer().unwrap();
    let intent = fixture.intent();
    let (mutation, _) = intent.identity().unwrap();
    let read = Cell::new(false);
    let body = HeldBody(&read, b"hello");
    assert!(
        writer
            .save(&intent, body, || {
                if read.get() {
                    Err(io::ErrorKind::PermissionDenied.into())
                } else {
                    Ok(())
                }
            })
            .is_err()
    );
    assert!(read.get());
    assert_eq!(writer.store.routes.get(mutation.container).unwrap(), None);
}
