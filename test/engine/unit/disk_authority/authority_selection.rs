use super::*;
use platform_io::fs_safety::{
    PrivateDirectory,
    linux::{AdvisoryLock, MountPolicy, OpenAt2Root},
};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Barrier,
    thread,
};

fn identity() -> Identity {
    Identity {
        origin: Origin {
            pool_id: Token::new("pool").unwrap(),
            disk_id: Token::new("disk").unwrap(),
            slot_id: Token::new("slot").unwrap(),
            binding_generation: 1,
        },
        incarnation: Digest::body(b"independent"),
        role: Role::Primary,
    }
}
fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, path: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                visit(root, &entry.path(), out);
            } else {
                out.insert(
                    entry.path().strip_prefix(root).unwrap().into(),
                    fs::read(entry.path()).unwrap(),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(root, root, &mut out);
    out
}

#[test]
fn legacy_never_claims_independent_data_or_system_at_any_initialization_boundary() {
    for boundary in [
        Some(fault::Boundary::InitializationIntent),
        Some(fault::Boundary::InitializationIdentity),
        Some(fault::Boundary::InitializationDirectories),
        Some(fault::Boundary::InitializationPublished),
        None,
    ] {
        let temp = tempfile::tempdir().unwrap();
        let disk = temp.path().join("independent");
        if let Some(boundary) = boundary {
            fault::inject(boundary);
            assert!(IndependentDisk::initialize(&disk, identity()).is_err());
        } else {
            IndependentDisk::initialize(&disk, identity()).unwrap();
        }
        let before = files(&disk);
        assert!(
            crate::Engine::initialize(
                &temp.path().join("legacy-system"),
                crate::Config::development([disk.clone()]).unwrap(),
            )
            .is_err()
        );
        assert_eq!(files(&disk), before, "{boundary:?}");
        let new_data = temp.path().join("legacy-data");
        assert!(
            crate::Engine::initialize(
                &disk,
                crate::Config::development([new_data.clone()]).unwrap(),
            )
            .is_err()
        );
        assert!(!new_data.exists(), "{boundary:?}");
        assert!(crate::Engine::open(&disk).is_err());
        assert!(crate::Engine::open_for_recovery(&disk).is_err());
        assert!(crate::TerminalManagement::open(&disk).is_err());
        assert_eq!(files(&disk), before, "{boundary:?}");
    }
}

#[test]
fn all_legacy_data_roots_are_admitted_before_any_writer_sidecar() {
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("empty");
    PrivateDirectory::create(&first).unwrap();
    let second = temp.path().join("independent");
    IndependentDisk::initialize(&second, identity()).unwrap();
    let before = files(&second);
    assert!(
        crate::Engine::initialize(
            &temp.path().join("system"),
            crate::Config::development([first.clone(), second.clone()]).unwrap(),
        )
        .is_err()
    );
    assert!(fs::read_dir(&first).unwrap().next().is_none());
    assert_eq!(files(&second), before);
}

#[test]
fn legacy_system_never_ignores_retained_independent_authority_markers() {
    for marker in [
        crate::authority_selection::ONLINE_SELECTION,
        crate::authority_selection::ONLINE_STATE,
        "identity.json",
        "online-owner.json",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let system = temp.path().join("system");
        let directory = PrivateDirectory::create(&system).unwrap();
        platform_io::fs_safety::AtomicFile::create(
            &directory,
            &platform_io::fs_safety::EntryName::new(marker).unwrap(),
            b"retained, even if incomplete",
        )
        .unwrap();
        let before = files(&system);
        let data = temp.path().join("data");
        assert!(
            crate::Engine::initialize(&system, crate::Config::development([data.clone()]).unwrap())
                .is_err()
        );
        assert!(crate::TerminalManagement::open(&system).is_err());
        assert!(!data.exists());
        assert_eq!(files(&system), before);
    }
}

#[test]
fn independent_initializer_preserves_existing_legacy_system_and_data() {
    let temp = tempfile::tempdir().unwrap();
    let system = temp.path().join("system");
    let data = temp.path().join("data");
    let engine =
        crate::Engine::initialize(&system, crate::Config::development([data.clone()]).unwrap())
            .unwrap();
    for root in [&system, &data] {
        let before = files(root);
        assert!(IndependentDisk::initialize(root, identity()).is_err());
        assert_eq!(files(root), before);
    }
    drop(engine);
    for root in [&system, &data] {
        let before = files(root);
        assert!(IndependentDisk::initialize(root, identity()).is_err());
        assert_eq!(files(root), before);
    }
}

#[test]
fn both_authorities_obey_the_same_directory_lock_without_creating_sidecars() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    PrivateDirectory::create(&data).unwrap();
    let held = OpenAt2Root::open(&data, MountPolicy::SameMount).unwrap();
    let lock = AdvisoryLock::directory(held.directory()).unwrap();
    assert!(IndependentDisk::initialize(&data, identity()).is_err());
    assert!(
        crate::Engine::initialize(
            &temp.path().join("system"),
            crate::Config::development([data.clone()]).unwrap()
        )
        .is_err()
    );
    assert!(fs::read_dir(&data).unwrap().next().is_none());
    drop(lock);
    IndependentDisk::initialize(&data, identity()).unwrap();
}

#[test]
fn concurrent_initializers_select_one_authority_for_data_or_system_roles() {
    for collision_role in ["data", "system"] {
        for _ in 0..8 {
            let temp = tempfile::tempdir().unwrap();
            let shared = temp.path().join("shared");
            PrivateDirectory::create(&shared).unwrap();
            let barrier = Barrier::new(3);
            thread::scope(|scope| {
                let independent = scope.spawn(|| {
                    barrier.wait();
                    IndependentDisk::initialize(&shared, identity())
                });
                let legacy = scope.spawn(|| {
                    let (system, data) = if collision_role == "data" {
                        (temp.path().join("system"), shared.clone())
                    } else {
                        (shared.clone(), temp.path().join("data"))
                    };
                    barrier.wait();
                    crate::Engine::initialize(&system, crate::Config::development([data]).unwrap())
                });
                barrier.wait();
                let independent = independent.join().unwrap();
                let legacy = legacy.join().unwrap();
                assert_ne!(independent.is_ok(), legacy.is_ok(), "{collision_role}");
                let legacy_marker = if collision_role == "data" {
                    "disk.json"
                } else {
                    "config.json"
                };
                assert_ne!(
                    shared.join("initialized.json").exists(),
                    shared.join(legacy_marker).exists(),
                    "{collision_role}"
                );
            });
        }
    }
}
