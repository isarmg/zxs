//! Offline draft edits must never rewrite a live pool or publish invalid topology.
use super::*;
use std::{fs, path::PathBuf};
use storage_engine::Engine;

fn draft(root: &Path) -> Config {
    Config::development([root.join("p"), root.join("b"), root.join("c")])
}

fn registration(root: &Path, config: PathBuf) -> Command {
    Command::RegisterDisk {
        config,
        slot: "P02".into(),
        disk_id: storage_engine::new_id("disk"),
        path: root.join("additional"),
        level: 1,
        upstream: None,
        metadata_member: false,
    }
}

#[test]
fn unrelated_root_cannot_edit_a_live_configuration_or_its_aliases() {
    use std::os::unix::fs::symlink;

    for kind in ["direct", "parent_alias", "file_alias", "hard_link", "copy"] {
        let root = tempfile::tempdir().unwrap();
        let system = root.path().join("system");
        let engine = Engine::initialize(&system, draft(root.path())).unwrap();
        // No auth.json is needed to make an initialized pool configuration live.
        assert!(!system.join("auth.json").exists());
        let live = system.join("config.json");
        let original = fs::read(&live).unwrap();
        let selected = match kind {
            "parent_alias" => {
                let alias = root.path().join("system-alias");
                symlink(&system, &alias).unwrap();
                alias.join("config.json")
            }
            "file_alias" => {
                let alias = root.path().join("draft-alias.json");
                symlink(&live, &alias).unwrap();
                alias
            }
            "hard_link" => {
                let alias = root.path().join("draft-link.json");
                fs::hard_link(&live, &alias).unwrap();
                alias
            }
            "copy" => {
                let copy = root.path().join("copied-draft.json");
                private_io::write_new(&copy, &original).unwrap();
                copy
            }
            _ => live.clone(),
        };
        assert!(
            run(registration(root.path(), selected.clone())).is_err(),
            "{kind} must not modify an initialized pool through an unrelated --root"
        );
        assert_eq!(fs::read(&live).unwrap(), original, "{kind}");
        assert_eq!(fs::read(&selected).unwrap(), original, "{kind}");
        assert!(!registration_backup(&selected).unwrap().exists(), "{kind}");
        drop(engine);
    }
}

#[test]
fn invalid_registration_candidates_preserve_the_draft_and_backup_namespace() {
    for kind in [
        "slot",
        "slot_case",
        "disk_id",
        "relative",
        "overlap",
        "membership",
        "forked_route",
        "resource_limit",
        "level_overflow",
    ] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("draft.json");
        let mut config = draft(root.path());
        let mut args = registration(root.path(), path.clone());
        let Command::RegisterDisk {
            slot,
            disk_id,
            path: disk,
            level,
            upstream,
            metadata_member,
            ..
        } = &mut args
        else {
            unreachable!()
        };
        match kind {
            "slot" => *slot = "bad/slot".into(),
            "slot_case" => *slot = "p01".into(),
            "disk_id" => *disk_id = "not-a-disk-id".into(),
            "relative" => *disk = PathBuf::from("relative-root"),
            "overlap" => *disk = config.disks[0].root.join("nested"),
            "membership" => {
                for disk in &mut config.disks {
                    disk.metadata_member = false;
                }
                *metadata_member = false;
            }
            "forked_route" => {
                *level = 2;
                *upstream = Some("P01".into());
            }
            "resource_limit" => config.max_file_bytes = 0,
            "level_overflow" => config.disks[0].level = u32::MAX,
            _ => unreachable!(),
        }
        let original = json_codec::to_vec(&config).unwrap();
        private_io::write_new(&path, &original).unwrap();
        assert!(
            run(args).is_err(),
            "{kind} must be rejected before any write"
        );
        assert_eq!(fs::read(&path).unwrap(), original, "{kind}");
        assert!(!registration_backup(&path).unwrap().exists(), "{kind}");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1, "{kind}");
    }
}

#[test]
fn newly_added_disk_cannot_already_belong_to_another_pool() {
    let root = tempfile::tempdir().unwrap();
    let other = root.path().join("other-pool");
    fs::create_dir(&other).unwrap();
    let engine = Engine::initialize(&other.join("system"), draft(&other)).unwrap();
    let config = draft(root.path());
    let path = root.path().join("draft.json");
    let original = json_codec::to_vec(&config).unwrap();
    private_io::write_new(&path, &original).unwrap();
    let mut args = registration(root.path(), path.clone());
    if let Command::RegisterDisk { path, .. } = &mut args {
        *path = engine.config().disks[0].root.clone();
    }
    assert!(run(args).is_err());
    assert_eq!(fs::read(&path).unwrap(), original);
    assert!(!registration_backup(&path).unwrap().exists());
}

#[test]
fn aliases_overlapping_roots_and_existing_markers_preserve_a_previous_backup() {
    use std::os::unix::fs::symlink;

    for kind in [
        "alias",
        "missing_alias_child",
        "dangling_link",
        "parent_traversal",
        "system_marker",
        "seal",
        "nested_seal",
        "membership",
        "file_root",
    ] {
        let root = tempfile::tempdir().unwrap();
        let config = draft(root.path());
        let path = root.path().join("draft.json");
        let original = json_codec::to_vec(&config).unwrap();
        private_io::write_new(&path, &original).unwrap();
        let backup = registration_backup(&path).unwrap();
        private_io::write_new(&backup, b"earlier recovery draft").unwrap();
        let mut args = registration(root.path(), path.clone());
        let Command::RegisterDisk { path: new_disk, .. } = &mut args else {
            unreachable!()
        };
        match kind {
            "alias" | "missing_alias_child" => {
                fs::create_dir(&config.disks[0].root).unwrap();
                symlink(&config.disks[0].root, &*new_disk).unwrap();
                if kind == "missing_alias_child" {
                    *new_disk = new_disk.join("not-created");
                }
            }
            "dangling_link" => symlink(root.path().join("absent"), &*new_disk).unwrap(),
            "parent_traversal" => *new_disk = root.path().join("p/../b"),
            "system_marker" => fs::write(root.path().join("writer.lock"), b"").unwrap(),
            "seal" | "nested_seal" => {
                fs::create_dir(&*new_disk).unwrap();
                fs::write(new_disk.join("disk.json"), b"managed identity").unwrap();
                if kind == "nested_seal" {
                    *new_disk = new_disk.join("not-created");
                }
            }
            "membership" => {
                fs::create_dir_all(new_disk.join("metadata/pool-membership")).unwrap();
                fs::write(
                    new_disk.join("metadata/pool-membership/config.json"),
                    b"pool",
                )
                .unwrap();
            }
            "file_root" => fs::write(&*new_disk, b"not a directory").unwrap(),
            _ => unreachable!(),
        }
        assert!(run(args).is_err(), "{kind}");
        assert_eq!(fs::read(&path).unwrap(), original, "{kind}");
        assert_eq!(
            fs::read(&backup).unwrap(),
            b"earlier recovery draft",
            "{kind}"
        );
    }
}

#[test]
fn ordinary_draft_permissions_remain_supported_without_initializing_disks() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("draft.json");
    let original = json_codec::to_vec(&draft(root.path())).unwrap();
    fs::write(&path, &original).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    run(registration(root.path(), path.clone())).unwrap();
    let saved: Config = storage_engine::decode_strict(&fs::read(&path).unwrap()).unwrap();
    let saved = ValidatedConfig::try_from(saved).unwrap();
    assert!(saved.disks.iter().all(|disk| !disk.root.exists()));
    assert_eq!(
        fs::read(registration_backup(&path).unwrap()).unwrap(),
        original
    );
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 2);
}

#[test]
fn management_authority_markers_prevent_reclassifying_system_state_as_a_draft() {
    for marker in [
        "management-authority.json",
        "management-auth.json",
        "setup-intent.json",
        "setup-ready.json",
    ] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("draft.json");
        let original = json_codec::to_vec(&draft(root.path())).unwrap();
        private_io::write_new(&path, &original).unwrap();
        fs::write(root.path().join(marker), b"{}").unwrap();
        assert!(run(registration(root.path(), path.clone())).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        assert!(!registration_backup(&path).unwrap().exists());
    }
}

// Run the real registration command in a separate process while the parent
// holds a precisely staged transaction. No timing/sleep-based race is needed.
#[test]
fn registration_process_contender() {
    let Some(path) = std::env::var_os("ZXS_RESCUE_REGISTRATION_TEST_DRAFT") else {
        return;
    };
    let path = PathBuf::from(path);
    let root = path.parent().unwrap();
    let mut args = registration(root, path.clone());
    if let Command::RegisterDisk { slot, path, .. } = &mut args {
        *slot = "P03".into();
        *path = root.join("concurrent-disk");
    }
    let result = run(args);
    if std::env::var_os("ZXS_RESCUE_REGISTRATION_TEST_CONFLICT").is_some() {
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("another configuration draft edit is active")
        );
    } else {
        result.unwrap();
    }
}

fn registration_process(path: &Path, conflict: bool) {
    let mut child = std::process::Command::new(std::env::current_exe().unwrap());
    child
        .args([
            "--exact",
            "configuration::registration_tests::registration_process_contender",
            "--nocapture",
        ])
        .env("ZXS_RESCUE_REGISTRATION_TEST_DRAFT", path);
    if conflict {
        child.env("ZXS_RESCUE_REGISTRATION_TEST_CONFLICT", "1");
    } else {
        child.env_remove("ZXS_RESCUE_REGISTRATION_TEST_CONFLICT");
    }
    let output = child.output().unwrap();
    assert!(
        output.status.success(),
        "contender failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}

fn additional_disk(root: &Path) -> storage_engine::DiskConfig {
    storage_engine::DiskConfig {
        slot_id: "P02".into(),
        disk_id: storage_engine::new_id("disk"),
        root: root.join("additional"),
        level: 1,
        upstream_slot_id: None,
        metadata_member: false,
        binding_generation: 1,
        capacity_bytes: None,
    }
}

#[test]
fn concurrent_registration_rejects_stale_reads_then_preserves_the_immediate_predecessor() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("draft.json");
    // Noncanonical formatting must be retained exactly in the first backup.
    let mut original = json_codec::to_vec(&draft(root.path())).unwrap();
    original.extend_from_slice(b"\n ");
    private_io::write_new(&path, &original).unwrap();
    let backup = registration_backup(&path).unwrap();
    let transaction = private_io::DraftTransaction::open(&path).unwrap();
    assert_eq!(transaction.read_owned_bytes(1024 * 1024).unwrap(), original);
    registration_process(&path, true);
    assert_eq!(fs::read(&path).unwrap(), original);
    assert!(!backup.exists());
    register_disk(&transaction, &path, additional_disk(root.path())).unwrap();
    let predecessor = fs::read(&path).unwrap();
    assert_eq!(fs::read(&backup).unwrap(), original);
    // The second process still cannot enter after the draft inode is replaced.
    registration_process(&path, true);
    assert_eq!(fs::read(&path).unwrap(), predecessor);
    assert_eq!(fs::read(&backup).unwrap(), original);
    drop(transaction);
    registration_process(&path, false);
    let saved: Config = storage_engine::decode_strict(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved.disks.len(), 5);
    assert!(saved.disks.iter().any(|disk| disk.slot_id == "P02"));
    assert!(saved.disks.iter().any(|disk| disk.slot_id == "P03"));
    assert_eq!(fs::read(&backup).unwrap(), predecessor);
    assert!(saved.disks.iter().all(|disk| !disk.root.exists()));
}

#[test]
fn initialized_markers_cannot_be_hidden_by_rebinding_the_locked_parent() {
    let root = tempfile::tempdir().unwrap();
    let parent = root.path().join("parent");
    fs::create_dir(&parent).unwrap();
    let path = parent.join("draft.json");
    let original = json_codec::to_vec(&draft(root.path())).unwrap();
    private_io::write_new(&path, &original).unwrap();
    let transaction = private_io::DraftTransaction::open(&path).unwrap();
    fs::write(parent.join("writer.lock"), b"").unwrap();
    let moved = root.path().join("moved");
    fs::rename(&parent, &moved).unwrap();
    fs::create_dir(&parent).unwrap();
    private_io::write_new(&path, &original).unwrap();
    let error = register_disk(&transaction, &path, additional_disk(root.path())).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("only an uninitialized configuration draft")
    );
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(fs::read(moved.join("draft.json")).unwrap(), original);
    assert!(!registration_backup(&path).unwrap().exists());
    assert!(
        !registration_backup(&moved.join("draft.json"))
            .unwrap()
            .exists()
    );
}

#[test]
fn untrusted_backup_entries_fail_closed_without_changing_the_draft() {
    use std::os::unix::fs::symlink;

    for kind in ["symlink", "hard_link", "directory", "foreign_owner"] {
        if kind == "foreign_owner" && rustix::process::geteuid().as_raw() != 0 {
            continue;
        }
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("draft.json");
        let original = json_codec::to_vec(&draft(root.path())).unwrap();
        private_io::write_new(&path, &original).unwrap();
        let backup = registration_backup(&path).unwrap();
        let protected = root.path().join("protected");
        fs::write(&protected, b"must remain intact").unwrap();
        match kind {
            "symlink" => symlink(&protected, &backup).unwrap(),
            "hard_link" => fs::hard_link(&protected, &backup).unwrap(),
            "directory" => fs::create_dir(&backup).unwrap(),
            "foreign_owner" => {
                fs::write(&backup, b"must remain intact").unwrap();
                rustix::fs::fchown(
                    fs::File::open(&backup).unwrap(),
                    Some(rustix::process::Uid::from_raw(65534)),
                    None,
                )
                .unwrap();
            }
            _ => unreachable!(),
        }
        let error = run(registration(root.path(), path.clone())).unwrap_err();
        assert!(
            error.to_string().contains("the draft was not changed"),
            "{kind}"
        );
        assert_eq!(fs::read(&path).unwrap(), original, "{kind}");
        assert_eq!(
            fs::read(&protected).unwrap(),
            b"must remain intact",
            "{kind}"
        );
        if kind != "directory" {
            assert_eq!(fs::read(&backup).unwrap(), b"must remain intact", "{kind}");
        }
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 3, "{kind}");
    }
}
