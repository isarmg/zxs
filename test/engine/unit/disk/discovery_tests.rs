use super::*;
use std::os::unix::fs::{PermissionsExt, symlink};

fn recognize(path: &Path) -> Result<RecognizedDisk> {
    recognize_disk(File::open(path).unwrap(), path.into())
}

#[test]
fn current_disk_recognition_binds_physical_identity_without_scanning_contents() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("disk");
    let config = Config::development([root.clone()]).unwrap();
    let engine = Engine::initialize(&temp.path().join("system"), config.clone()).unwrap();
    let before = std::fs::read(root.join("disk.json")).unwrap();
    let metadata = std::fs::metadata(root.join("disk.json")).unwrap();
    // A nested ordinary directory is content, not another discovery candidate.
    std::fs::create_dir(root.join("ordinary-child")).unwrap();
    std::fs::write(root.join("ordinary-child/preserve"), b"ordinary bytes").unwrap();
    let recognized = recognize(&root).unwrap();
    assert_eq!(recognized.identity().pool_id, config.pool_id);
    assert_eq!(recognized.identity().disk_id, config.disks[0].disk_id);
    assert!(recognized.simulation());
    assert_eq!(recognized.role().level(), 1);
    assert_eq!(recognized.role().upstream_slot_id(), None);
    assert!(recognized.role().metadata_member());
    assert_eq!(
        recognized.role().evidence(),
        &DiskRoleEvidence::InitialConfiguration
    );
    assert_eq!(std::fs::read(root.join("disk.json")).unwrap(), before);
    assert_eq!(
        std::fs::metadata(root.join("disk.json")).unwrap().mtime(),
        metadata.mtime()
    );
    assert_eq!(
        std::fs::read(root.join("ordinary-child/preserve")).unwrap(),
        b"ordinary bytes"
    );
    drop(engine);
}

#[test]
fn recorded_roles_are_not_inferred_from_names_and_never_open_other_roots() {
    let temp = tempfile::tempdir().unwrap();
    let config =
        Config::development((0..3).map(|index| temp.path().join(format!("root-{index}")))).unwrap();
    let engine = Engine::initialize(&temp.path().join("system"), config.clone()).unwrap();
    drop(engine);
    // The source anchor's foreign locators may be absent. Recognition reads
    // only the explicitly held disk, not another disk or the old system root.
    for disk in config.disks.iter().take(2) {
        std::fs::rename(&disk.root, disk.root.with_extension("absent")).unwrap();
    }
    let third = &config.disks[2];
    let relocated = temp.path().join("not-a-level-name");
    std::fs::rename(&third.root, &relocated).unwrap();
    let before = std::fs::read(relocated.join("metadata/pool-membership/config.json")).unwrap();
    let role = recognize(&relocated).unwrap();
    assert_eq!(role.role().level(), 3);
    assert_eq!(role.role().upstream_slot_id(), Some("B01"));
    assert_eq!(role.identity().disk_id, third.disk_id);
    assert_eq!(
        std::fs::read(relocated.join("metadata/pool-membership/config.json")).unwrap(),
        before
    );
    assert!(config.disks.iter().all(|disk| !disk.root.exists()));
}

#[test]
fn missing_or_mismatched_role_evidence_is_rejected_without_repair() {
    for attack in 0..7 {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("disk");
        let engine = Engine::initialize(
            &temp.path().join("system"),
            Config::development([root.clone()]).unwrap(),
        )
        .unwrap();
        drop(engine);
        let path = root.join("metadata/pool-membership/config.json");
        let mut config: Config = read_json(&path).unwrap();
        match attack {
            0 => config.pool_id = new_id("pool").unwrap(),
            1 => config.disks[0].disk_id = new_id("disk").unwrap(),
            2 => config.disks[0].slot_id = "X01".into(),
            3 => config.disks[0].binding_generation = 2,
            4 => config.mode = Mode::Production,
            5 => config.disks[0].level = 2,
            6 => {}
            _ => unreachable!(),
        }
        std::fs::write(&path, json_codec::to_vec(&config).unwrap()).unwrap();
        let damaged = std::fs::read(&path).unwrap();
        if attack == 6 {
            std::fs::remove_file(&path).unwrap();
        }
        let seal = std::fs::read(root.join("disk.json")).unwrap();
        assert!(recognize(&root).is_err(), "attack {attack}");
        assert_eq!(std::fs::read(root.join("disk.json")).unwrap(), seal);
        if attack == 6 {
            assert!(!path.exists());
        } else {
            assert_eq!(std::fs::read(&path).unwrap(), damaged);
        }
    }
}

#[test]
fn initial_seal_never_hides_an_unpromoted_generation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("disk");
    let engine = Engine::initialize(
        &temp.path().join("system"),
        Config::development([root.clone()]).unwrap(),
    )
    .unwrap();
    let marker = root.join("metadata/pool-membership/generation.json");
    write_json(&marker, &json_codec::json!({}), false).unwrap();
    assert_eq!(
        recognize(&root).unwrap_err().code(),
        ErrorCode::RecoveryRequired
    );
    assert_eq!(std::fs::read(&marker).unwrap(), b"{}");
    drop(engine);
}

#[test]
fn generation_role_requires_its_seal_hash_and_local_data_enrollment() {
    let temp = tempfile::tempdir().unwrap();
    let original = Config::development([temp.path().join("original")]).unwrap();
    let mut engine = Engine::initialize(&temp.path().join("system"), original.clone()).unwrap();
    engine
        .create_repository(
            &RepositoryName::parse("retained").unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let root = temp.path().join("new-backup");
    private_dir(&root).unwrap();
    let mut addition = Config::development([root.clone()]).unwrap().disks.remove(0);
    addition.slot_id = "B01".into();
    addition.level = 2;
    addition.upstream_slot_id = Some("P01".into());
    addition.metadata_member = false;
    let mut target = original;
    target.disks.push(addition.clone());
    engine
        .begin_membership_handoff(target, "admin", OperationId::generate().unwrap())
        .unwrap();
    let archive = engine.build_managed_membership_control_archive().unwrap();
    engine
        .enroll_pending_members(
            &archive.archive_path,
            &archive.root_hash,
            vec![PendingMemberDirectory {
                disk_id: addition.disk_id,
                directory: File::open(&root).unwrap(),
            }],
        )
        .unwrap();
    engine.resume_membership_joint_decision().unwrap();
    engine.activate_pending_membership().unwrap();
    let recognized = recognize(&root).unwrap();
    assert_eq!(recognized.role().level(), 2);
    assert_eq!(recognized.role().upstream_slot_id(), Some("P01"));
    assert!(!recognized.role().metadata_member());
    assert!(matches!(
        recognized.role().evidence(),
        DiskRoleEvidence::MembershipGeneration { .. }
    ));
    let seal_path = root.join("disk.json");
    let seal_bytes = std::fs::read(&seal_path).unwrap();
    let mut seal = decode_seal(&seal_bytes).unwrap();
    seal.format = DiskSealFormat::MembershipGeneration {
        authority_hash: "f".repeat(64),
    };
    std::fs::write(&seal_path, json_codec::to_vec(&seal).unwrap()).unwrap();
    assert!(recognize(&root).is_err());
    std::fs::write(&seal_path, &seal_bytes).unwrap();
    let ready = root
        .join("metadata/membership-pending")
        .join(&archive.transition_hash)
        .join("data-ready.json");
    let bytes = std::fs::read(&ready).unwrap();
    std::fs::write(&ready, b"{}").unwrap();
    assert!(recognize(&root).is_err());
    assert_eq!(std::fs::read(&ready).unwrap(), b"{}");
    std::fs::write(&ready, bytes).unwrap();
    assert_eq!(recognize(&root).unwrap(), recognized);
}

#[test]
fn ordinary_unknown_and_rebound_roots_are_rejected_without_changes() {
    let temp = tempfile::tempdir().unwrap();
    let plain = temp.path().join("plain");
    std::fs::create_dir(&plain).unwrap();
    std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(plain.join("notes.txt"), b"preserve").unwrap();
    assert!(recognize(&plain).is_err());
    assert_eq!(std::fs::read_dir(&plain).unwrap().count(), 1);
    assert_eq!(std::fs::read(plain.join("notes.txt")).unwrap(), b"preserve");

    let root = temp.path().join("disk");
    let engine = Engine::initialize(
        &temp.path().join("system"),
        Config::development([root.clone()]).unwrap(),
    )
    .unwrap();
    let seal = std::fs::read(root.join("disk.json")).unwrap();
    std::fs::write(plain.join("disk.json"), &seal).unwrap();
    std::fs::set_permissions(
        plain.join("disk.json"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    assert_eq!(
        recognize(&plain).unwrap_err().code(),
        ErrorCode::DiskIdentityMismatch
    );
    let mut unknown: json_codec::Value = json_codec::from_slice(&seal).unwrap();
    unknown["format"] = json_codec::json!("unrecognized-format");
    let unknown = json_codec::to_vec(&unknown).unwrap();
    std::fs::write(plain.join("disk.json"), &unknown).unwrap();
    assert!(recognize(&plain).is_err());
    assert_eq!(std::fs::read(plain.join("disk.json")).unwrap(), unknown);
    std::fs::remove_file(plain.join("disk.json")).unwrap();
    symlink(root.join("disk.json"), plain.join("disk.json")).unwrap();
    assert!(recognize(&plain).is_err());
    assert_eq!(std::fs::read(root.join("disk.json")).unwrap(), seal);
    drop(engine);
}

#[test]
fn fresh_snapshot_seal_is_not_a_legacy_role_or_generation_recovery_candidate() {
    fn image(root: &Path) -> std::collections::BTreeMap<PathBuf, (u64, u32, Vec<u8>)> {
        use std::os::unix::fs::MetadataExt;
        fn visit(path: &Path, rows: &mut std::collections::BTreeMap<PathBuf,
            (u64, u32, Vec<u8>)>)
        {
            let metadata = std::fs::symlink_metadata(path).unwrap();
            assert!(metadata.is_file() || metadata.is_dir());
            let bytes = if metadata.is_file() { std::fs::read(path).unwrap() } else { Vec::new() };
            rows.insert(path.to_path_buf(), (metadata.ino(), metadata.mode(), bytes));
            if metadata.is_dir() {
                for entry in std::fs::read_dir(path).unwrap() {
                    visit(&entry.unwrap().path(), rows);
                }
            }
        }
        let mut rows = std::collections::BTreeMap::new();
        visit(root, &mut rows);
        rows
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("disk");
    let engine = Engine::initialize(
        &temp.path().join("system"), Config::development([root.clone()]).unwrap(),
    ).unwrap();
    drop(engine);
    let seal_path = root.join("disk.json");
    let config_path = root.join("metadata/pool-membership/config.json");
    let original_config = std::fs::read(&config_path).unwrap();
    let mut seal = decode_seal(&std::fs::read(&seal_path).unwrap()).unwrap();
    // Reader-boundary fixture only, not an admitted fresh bootstrap. The new
    // seal format must never fall through to the retained old configuration.
    seal.format = DiskSealFormat::FreshDirectorySnapshot {
        bootstrap: storage_contract::working_tree::MaterialCommitment::from_digest(
            blake3::hash(b"fresh reader boundary"),
        ),
    };
    // Simulate external bytes at the reader boundary. The production immutable
    // writer correctly refuses to replace the original seal with different bytes.
    std::fs::write(&seal_path, crate::canonical_json(&seal).unwrap()).unwrap();
    let fresh_bytes = std::fs::read(&seal_path).unwrap();
    let marker = root.join("metadata/pool-membership/generation.json");
    std::fs::write(&marker, b"retained unpromoted bytes, not JSON").unwrap();
    let before = image(temp.path());
    let error = recognize(&root).unwrap_err();
    assert_eq!(image(temp.path()), before);
    assert_eq!(error.code(), ErrorCode::FormatUnsupported);
    assert!(matches!(error, Error::FreshBootstrap(source)
        if matches!(*source, crate::authority_selection::FreshBootstrapError::UnsupportedReader)));
    assert_eq!(std::fs::read(&seal_path).unwrap(), fresh_bytes);
    assert_eq!(std::fs::read(&config_path).unwrap(), original_config);
    assert_eq!(std::fs::read(&marker).unwrap(), b"retained unpromoted bytes, not JSON");

    // Existing held-identity rejection still precedes unsupported-reader status.
    seal.inode ^= 1;
    std::fs::write(&seal_path, crate::canonical_json(&seal).unwrap()).unwrap();
    let stale_bytes = std::fs::read(&seal_path).unwrap();
    let before = image(temp.path());
    assert_eq!(recognize(&root).unwrap_err().code(), ErrorCode::DiskIdentityMismatch);
    assert_eq!(image(temp.path()), before);
    assert_eq!(std::fs::read(&seal_path).unwrap(), stale_bytes);
    assert_eq!(std::fs::read(&config_path).unwrap(), original_config);
}
