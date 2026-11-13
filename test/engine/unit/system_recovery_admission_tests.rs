//! Real publication sources; admission never writes replacement SYSTEM state.
use crate::{Config, Engine, OperationId};
use platform_io::fs_safety::PrivateDirectory;
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub(super) fn fixture() -> (tempfile::TempDir, PathBuf, Config) {
    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let system = temp.path().join("system");
    fs::create_dir(&system).unwrap();
    fs::set_permissions(&system, fs::Permissions::from_mode(0o700)).unwrap();
    let paths = (0..3).map(|index| temp.path().join(format!("disk-{index}")))
        .collect::<Vec<_>>();
    for path in &paths {
        fs::create_dir(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let mut config = Config::development(paths).unwrap();
    for disk in &mut config.disks {
        disk.level = 1;
        disk.upstream_slot_id = None;
    }
    (temp, system, config)
}

pub(super) fn image(root: &Path) -> BTreeMap<PathBuf, (u64, u64, u32, Vec<u8>)> {
    fn visit(root: &Path, path: &Path, rows: &mut BTreeMap<PathBuf, (u64, u64, u32, Vec<u8>)>) {
        let metadata = fs::symlink_metadata(path).unwrap();
        let bytes = if metadata.is_dir() { Vec::new() } else { fs::read(path).unwrap() };
        rows.insert(path.strip_prefix(root).unwrap().to_owned(),
            (metadata.dev(), metadata.ino(), metadata.mode(), bytes));
        if metadata.is_dir() {
            for child in fs::read_dir(path).unwrap() {
                visit(root, &child.unwrap().path(), rows);
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}

#[test]
fn original_quorum_admission_retains_sources_after_system_loss_without_writes() {
    let (temp, system, config) = fixture();
    let mut engine = Engine::initialize_storage(
        &system, config.clone(), OperationId::generate().unwrap(),
    ).unwrap();
    engine.create_repository(
        &storage_contract::RepositoryName::parse("retained source").unwrap(),
        "P01", 1, "admin", OperationId::generate().unwrap(),
    ).unwrap();
    let state = engine.state().clone();
    drop(engine);
    fs::rename(&system, temp.path().join("preserved-lost-system")).unwrap();
    let before = image(temp.path());
    let handles = config.disks.iter().take(2)
        .map(|disk| PrivateDirectory::open_existing(&disk.root).unwrap()).collect();
    let admitted = Engine::admit_storage_recovery(handles).unwrap();
    assert_eq!(&admitted.state, &state);
    let commitment = admitted.evidence_commitment();
    assert_eq!(image(temp.path()), before);
    // Retained source writers prevent a competing readmission owner.
    let handles = config.disks.iter().take(2)
        .map(|disk| PrivateDirectory::open_existing(&disk.root).unwrap()).collect();
    assert!(Engine::admit_storage_recovery(handles).is_err());
    assert_eq!(image(temp.path()), before);
    drop(admitted);
    let handles = config.disks.iter().take(2)
        .map(|disk| PrivateDirectory::open_existing(&disk.root).unwrap()).collect();
    let retry = Engine::admit_storage_recovery(handles).unwrap();
    assert_eq!(retry.evidence_commitment(), commitment);
    assert_eq!(image(temp.path()), before);
}

#[test]
fn insufficient_quorum_and_torn_member_preserve_every_source_byte() {
    let (temp, system, config) = fixture();
    drop(Engine::initialize_storage(
        &system, config.clone(), OperationId::generate().unwrap(),
    ).unwrap());
    let before = image(temp.path());
    let one = vec![PrivateDirectory::open_existing(&config.disks[0].root).unwrap()];
    assert!(Engine::admit_storage_recovery(one).is_err());
    assert_eq!(image(temp.path()), before);
    let log = crate::journal::log_path(&config.disks[1]);
    let length = fs::metadata(&log).unwrap().len();
    fs::OpenOptions::new().write(true).open(log).unwrap().set_len(length - 1).unwrap();
    let before = image(temp.path());
    let handles = config.disks.iter().take(2)
        .map(|disk| PrivateDirectory::open_existing(&disk.root).unwrap()).collect();
    assert!(Engine::admit_storage_recovery(handles).is_err());
    assert_eq!(image(temp.path()), before);
}

#[test]
fn missing_barrier_foreign_root_and_older_prefix_cannot_authorize_recovery() {
    for attack in 0..3 {
        let (temp, system, config) = fixture();
        let mut engine = Engine::initialize_storage(
            &system, config.clone(), OperationId::generate().unwrap(),
        ).unwrap();
        let admitted_config = engine.config.clone();
        engine.create_repository(
            &storage_contract::RepositoryName::parse("newer published state").unwrap(),
            "P01", 1, "admin", OperationId::generate().unwrap(),
        ).unwrap();
        drop(engine);
        let root = crate::disk::DiskRoot::open(&config.disks[0].root).unwrap();
        let scan = crate::journal::scan_configured_at(
            &root, &config.disks[0], &admitted_config,
        ).unwrap();
        assert_eq!(scan.records.len(), 8);
        let foreign = fixture();
        drop(Engine::initialize_storage(
            &foreign.1, foreign.2.clone(), OperationId::generate().unwrap(),
        ).unwrap());
        match attack {
            0 => fs::remove_file(crate::journal::publication_seal_path(
                &config.disks[0], &scan.records[0].0.candidate_hash,
            )).unwrap(),
            1 => {}
            2 => {
                // Every selected member retains an internally complete old
                // prefix, but the newer actual publication barriers remain.
                for disk in config.disks.iter().take(2) {
                    let root = crate::disk::DiskRoot::open(&disk.root).unwrap();
                    let scan = crate::journal::scan_configured_at(
                        &root, disk, &admitted_config,
                    ).unwrap();
                    let length = scan.records.iter().take(4).map(|(record, _)| {
                        12 + storage_contract::canonical_json(record).unwrap().len() as u64 + 32
                    }).sum::<u64>();
                    fs::OpenOptions::new().write(true).open(crate::journal::log_path(disk))
                        .unwrap().set_len(length).unwrap();
                }
            }
            _ => unreachable!(),
        }
        let before = image(temp.path());
        let foreign_before = image(foreign.0.path());
        let second = if attack == 1 { &foreign.2.disks[1] } else { &config.disks[1] };
        let handles = vec![
            PrivateDirectory::open_existing(&config.disks[0].root).unwrap(),
            PrivateDirectory::open_existing(&second.root).unwrap(),
        ];
        assert!(Engine::admit_storage_recovery(handles).is_err());
        assert_eq!(image(temp.path()), before);
        assert_eq!(image(foreign.0.path()), foreign_before);
    }
}

#[test]
fn prior_unified_reader_format_is_rejected_without_migration() {
    for prior in ["v1", "v2"] {
    let (temp, system, config) = fixture();
    drop(Engine::initialize_storage(
        &system, config.clone(), OperationId::generate().unwrap(),
    ).unwrap());
    let marker = system.join("initialized.json");
    let bytes = fs::read_to_string(&marker).unwrap();
    assert!(bytes.contains("unified-storage-system-v3"));
    fs::write(&marker, bytes.replace("unified-storage-system-v3", &format!("unified-storage-system-{prior}")))
        .unwrap();
    let seal = config.disks[0].root.join(crate::persist::layout::DISK_IDENTITY.as_path());
    let bytes = fs::read_to_string(&seal).unwrap();
    assert!(bytes.contains("unified-storage-managed-directory-v3"));
    fs::set_permissions(&seal, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&seal, bytes.replace(
        "unified-storage-managed-directory-v3", &format!("unified-storage-managed-directory-{prior}"),
    )).unwrap();
    let before = image(temp.path());
    assert!(Engine::open(&system).is_err());
    let handles = config.disks.iter().take(2)
        .map(|disk| PrivateDirectory::open_existing(&disk.root).unwrap()).collect();
    assert!(Engine::admit_storage_recovery(handles).is_err());
    assert_eq!(image(temp.path()), before);
}
}
