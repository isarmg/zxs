use super::*;
use crate::{
    container_store::{Body, Mutation, Store, Target, create_file, ensure_dir},
    hash::Sha256,
    test_support::PrivateTemp,
};
use std::{
    fs::{self, File, FileTimes},
    io::Write,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    time::{Duration, UNIX_EPOCH},
};

const CID: Id = [1; 16];
const SAVE: Id = [2; 16];
const NODE: Id = [3; 16];
const MOVE: Id = [4; 16];
const TOKEN: [u8; 32] = [8; 32];
const ROLE: RecycleRole = RecycleRole {
    pool: [9; 16],
    token: [7; 32],
};

fn held(path: &Path) -> HeldDir {
    HeldDir::trusted_storage(path).unwrap()
}

fn volume(path: &Path) -> DataRoot {
    DataRoot {
        root: held(path),
        token: TOKEN,
    }
}

struct Fixture {
    temporary: PrivateTemp,
    system: HeldDir,
    recycle: HeldDir,
    data: Vec<DataRoot>,
}

impl Fixture {
    fn new() -> Self {
        let temporary = PrivateTemp::under(&std::env::temp_dir());
        let [system, recycle, data] = ["system", "recycle", "data"].map(|name| {
            let path = temporary.0.join(name);
            ensure_dir(&path).unwrap();
            path
        });
        Self {
            temporary,
            system: held(&system),
            recycle: held(&recycle),
            data: vec![volume(&data)],
        }
    }

    fn store(&self) -> Store {
        let owner = self.system.opaque_owner(c"container-owner").unwrap();
        Store::open_roster(&self.system, vec![volume(&self.data[0].root.path)], owner).unwrap()
    }

    fn verify(&self, id: Id) -> io::Result<Verification> {
        // Filesystem-shaped fixture only. The production wrapper also proves
        // positive native separation of each enrolled filesystem.
        let catalog = Catalog::open(&self.system, &self.recycle, &self.data, ROLE)?;
        inspect(&catalog, id, &|| Ok(()))
    }

    fn ordinary(&self) -> PathBuf {
        self.data[0].root.path.join("data").join(hex(CID))
    }
}

fn save(store: &Store, bytes: &[u8]) {
    let mut hash = Sha256::new();
    hash.update(bytes).unwrap();
    store
        .save(
            Mutation {
                container: CID,
                request: SAVE,
            },
            Change::Save {
                target: Target::FirstMain {
                    name: "report".into(),
                },
                node: NODE,
                body: Body {
                    length: u64::try_from(bytes.len()).unwrap(),
                    sha256: hash.finish(),
                },
            },
            bytes,
            || Ok(()),
        )
        .unwrap();
}

#[test]
fn verifies_whole_exact_selected_data_without_touching_access_times_or_staging() {
    let fixture = Fixture::new();
    let store = fixture.store();
    save(&store, b"body");
    store
        .change(
            Mutation {
                container: CID,
                request: [5; 16],
            },
            Change::Tags {
                expected_sequence: 1,
                tags: vec!["fixture".into()],
            },
            || Ok(()),
        )
        .unwrap();
    store.routes.reserve([6; 16], TOKEN).unwrap();
    drop(store);
    let staging = fixture.data[0].root.path.join("staging/0000000000000001");
    ensure_dir(&staging).unwrap();
    let remnant = staging.join("payload");
    create_file(&remnant)
        .unwrap()
        .write_all(b"partial")
        .unwrap();
    let entry = fixture.ordinary().join(entry_leaf(1, SAVE));
    let tracked = [
        fixture.system.path.join("container-routes").join(hex(CID)),
        fixture.ordinary(),
        entry.clone(),
        entry.join("receipt"),
        entry.join("payload"),
    ];
    for path in &tracked {
        File::open(path)
            .unwrap()
            .set_times(FileTimes::new().set_accessed(UNIX_EPOCH + Duration::from_secs(1)))
            .unwrap();
    }
    let check_access = || {
        tracked
            .each_ref()
            .map(|path| fs::metadata(path).unwrap().accessed().unwrap())
    };
    let before = check_access();
    assert_eq!(
        fixture.verify(CID).unwrap(),
        Verification {
            location: VerifiedLocation::Data,
            receipts: 2,
            bodies: 1,
            bytes: 4
        }
    );
    assert_eq!(check_access(), before);
    assert_eq!(fs::read(remnant).unwrap(), b"partial");
    assert!(fixture.verify([6; 16]).is_err());
}

#[test]
fn rejects_absent_short_changed_trailing_and_unknown_members() {
    let fixture = Fixture::new();
    let store = fixture.store();
    save(&store, b"one");
    drop(store);
    let original = fixture.ordinary().join(entry_leaf(1, SAVE));
    let body = original.join("payload");
    for bad in [b"x".as_slice(), b"two", b"one!"] {
        fs::write(&body, bad).unwrap();
        assert!(fixture.verify(CID).is_err());
        assert_eq!(fs::read(&body).unwrap(), bad);
    }
    fs::write(&body, b"one").unwrap();
    let outside = fixture.temporary.0.join("retained-body");
    fs::rename(&body, &outside).unwrap();
    assert!(fixture.verify(CID).is_err());
    assert_eq!(fs::read(&outside).unwrap(), b"one");
    fs::rename(&outside, &body).unwrap();
    let other = original.join("extra");
    create_file(&other).unwrap().write_all(b"retain").unwrap();
    assert!(fixture.verify(CID).is_err());
    assert_eq!(fs::read(other).unwrap(), b"retain");
}

#[test]
fn selected_archive_remains_the_only_target_even_when_data_body_is_healthy() {
    let fixture = Fixture::new();
    let store = fixture.store();
    save(&store, b"body");
    drop(store);

    let moves = fixture.system.path.join("container-moves");
    let arrived = fixture.recycle.path.join("containers");
    let envelope = arrived.join(format!("{}-{}", hex(CID), hex(MOVE)));
    let root = envelope.join("container");
    let copy = root.join(entry_leaf(1, SAVE));
    for directory in [&moves, &arrived, &envelope, &root, &copy] {
        ensure_dir(directory).unwrap();
    }
    let src = fixture.ordinary().join(entry_leaf(1, SAVE));
    for name in ["receipt", "payload"] {
        create_file(&copy.join(name))
            .unwrap()
            .write_all(&fs::read(src.join(name)).unwrap())
            .unwrap();
    }
    let selected = RecycleReceipt::enrolled(
        Mutation {
            container: CID,
            request: MOVE,
        },
        1,
        ROLE,
        TOKEN,
        (
            fs::metadata(fixture.ordinary()).unwrap().ino(),
            fs::metadata(&envelope).unwrap().ino(),
        ),
        [7; 32],
    )
    .unwrap()
    .encode();
    for place in [moves.join(hex(CID)), envelope.join("ready")] {
        create_file(&place).unwrap().write_all(&selected).unwrap();
    }
    assert_eq!(
        fixture.verify(CID).unwrap(),
        Verification {
            location: VerifiedLocation::Recycle,
            receipts: 1,
            bodies: 1,
            bytes: 4
        }
    );
    fs::write(copy.join("payload"), b"bad!").unwrap();
    assert!(fixture.verify(CID).is_err());
    assert_eq!(fs::read(src.join("payload")).unwrap(), b"body");
    assert_eq!(fs::read(copy.join("payload")).unwrap(), b"bad!");
}

#[test]
#[cfg(target_arch = "x86_64")]
fn a_same_name_twin_with_exact_ledger_cannot_match_the_parent_live_binding() {
    use crate::headless_import_worker::verify::{fixture_binding, fixture_order};

    let fixture = Fixture::new();
    let store = fixture.store();
    save(&store, b"body");
    drop(store);
    let descriptor = [0x42; 32]; // Identical conf bytes across a transient root swap.
    let original = &fixture.data[0].root;
    let coordinates = |data: &HeldDir| {
        [
            fixture.system.live_identity().unwrap(),
            fixture.recycle.live_identity().unwrap(),
            data.live_identity().unwrap(),
        ]
    };
    let old = fixture_binding(descriptor, &coordinates(original)).unwrap();
    let mut raw = b"ZXOVF1\0\0".to_vec();
    raw.extend(old);
    raw.extend([0x72; 32]);
    raw.extend(CID);
    assert_eq!(fixture_order(&raw, old).unwrap(), ([0x72; 32], CID));

    // The old path, route, receipt and bytes all look identical to a fresh
    // exact name lookup during the swap. Only the held role proof distinguishes it.
    let twin_root = fixture.temporary.0.join("twin-data");
    let twin_data = twin_root.join("data");
    let twin_container = twin_data.join(hex(CID));
    let twin_entry = twin_container.join(entry_leaf(1, SAVE));
    for path in [&twin_root, &twin_data, &twin_container, &twin_entry] {
        ensure_dir(path).unwrap();
    }
    let original_entry = fixture.ordinary().join(entry_leaf(1, SAVE));
    for name in ["receipt", "payload"] {
        create_file(&twin_entry.join(name))
            .unwrap()
            .write_all(&fs::read(original_entry.join(name)).unwrap())
            .unwrap();
    }
    let kept = fixture.temporary.0.join("held-original");
    fs::rename(&original.path, &kept).unwrap();
    fs::rename(&twin_root, &original.path).unwrap();
    assert!(original.live_identity().is_err());
    {
        let child_roles = [volume(&original.path)];
        let child = Catalog::open(&fixture.system, &fixture.recycle, &child_roles, ROLE).unwrap();
        assert_eq!(inspect(&child, CID, &|| Ok(())).unwrap().bytes, 4);
        let wrong = fixture_binding(descriptor, &coordinates(&child_roles[0].root)).unwrap();
        assert_ne!(wrong, old);
        assert!(fixture_order(&raw, wrong).is_err());
    }
    fs::rename(&original.path, &twin_root).unwrap();
    fs::rename(&kept, &original.path).unwrap();
    assert_eq!(
        fixture_binding(descriptor, &coordinates(original)).unwrap(),
        old
    );
    assert_eq!(fixture.verify(CID).unwrap().bytes, 4);
}
