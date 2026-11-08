use super::*;
use std::{fs, os::unix::fs::symlink};

const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn owner() -> (u32, u32) {
    (rustix::process::geteuid().as_raw(), rustix::process::getegid().as_raw())
}

fn root() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o755)).unwrap();
    root
}

#[test]
fn private_runtime_directories_publish_only_empty_matching_inodes() {
    let root = root();
    let host = BundleDestination::open(root.path()).unwrap();
    let directories = Directories::open(root.path(), &host, owner()).unwrap();
    for spec in &SPECS[..6] {
        assert!(directories.publish_one(spec, DIGEST, &|| Ok(())).unwrap());
        let metadata = fs::metadata(root.path().join(spec.name)).unwrap();
        assert_eq!((metadata.uid(), metadata.gid()), owner());
        assert_eq!(metadata.mode() & 0o7777, 0o700);
        let inode = metadata.ino();
        assert!(!directories.publish_one(spec, DIGEST, &|| Ok(())).unwrap());
        assert_eq!(fs::metadata(root.path().join(spec.name)).unwrap().ino(), inode);
    }
    fs::write(root.path().join("log/retained.log"), b"preserve").unwrap();
    assert!(directories.publish(DIGEST, || Ok(())).is_err());
    assert_eq!(fs::read(root.path().join("log/retained.log")).unwrap(), b"preserve");
}

#[test]
fn all_destinations_are_inspected_before_first_directory_is_created() {
    let root = root();
    fs::create_dir(root.path().join("tmp")).unwrap();
    fs::set_permissions(root.path().join("tmp"), fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(root.path().join("tmp/keep"), b"unrelated").unwrap();
    let host = BundleDestination::open(root.path()).unwrap();
    let directories = Directories::open(root.path(), &host, owner()).unwrap();
    assert!(directories.publish(DIGEST, || Ok(())).is_err());
    assert!(!root.path().join("cache").exists());
    assert!(!root.path().join(pending_name(&SPECS[0], DIGEST).unwrap().as_os_str()).exists());
    assert_eq!(fs::read(root.path().join("tmp/keep")).unwrap(), b"unrelated");
}

#[test]
fn unsafe_final_permissions_are_not_repaired_and_symlinks_are_rejected() {
    let root = root();
    let run = root.path().join("run");
    fs::create_dir(&run).unwrap();
    fs::set_permissions(&run, fs::Permissions::from_mode(0o755)).unwrap();
    let host = BundleDestination::open(root.path()).unwrap();
    let directories = Directories::open(root.path(), &host, owner()).unwrap();
    assert!(directories.inspect(DIGEST).is_err());
    assert_eq!(fs::metadata(&run).unwrap().mode() & 0o7777, 0o755);
    fs::remove_dir(&run).unwrap();
    symlink(root.path(), &run).unwrap();
    assert!(directories.inspect(DIGEST).is_err());
    assert!(fs::symlink_metadata(&run).unwrap().file_type().is_symlink());
}

#[test]
fn same_pending_inode_resumes_but_both_names_refuse_without_deleting_either() {
    let root = root();
    let host = BundleDestination::open(root.path()).unwrap();
    let directories = Directories::open(root.path(), &host, owner()).unwrap();
    let spec = &SPECS[0];
    let pending = root.path().join(pending_name(spec, DIGEST).unwrap().as_os_str());
    fs::create_dir(&pending).unwrap();
    fs::set_permissions(&pending, fs::Permissions::from_mode(0o700)).unwrap();
    let inode = fs::metadata(&pending).unwrap().ino();
    assert!(directories.publish_one(spec, DIGEST, &|| Err("account changed".into())).is_err());
    assert!(!root.path().join(spec.name).exists());
    assert!(directories.publish_one(spec, DIGEST, &|| Ok(())).unwrap());
    assert_eq!(fs::metadata(root.path().join(spec.name)).unwrap().ino(), inode);
    fs::create_dir(&pending).unwrap();
    fs::set_permissions(&pending, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(directories.publish_one(spec, DIGEST, &|| Ok(())).is_err());
    assert!(pending.is_dir());
    assert!(root.path().join(spec.name).is_dir());
}

#[test]
fn completed_sync_never_recreates_a_missing_directory() {
    let root = root();
    let host = BundleDestination::open(root.path()).unwrap();
    let directories = Directories::open(root.path(), &host, owner()).unwrap();
    assert!(directories.sync_completed(DIGEST).is_err());
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    let name = format!(".zxs-helper-runtime-complete-{DIGEST}");
    assert!(directories.completion_phase(&name).unwrap() == CompletionPhase::Absent);
    fs::create_dir(root.path().join(format!("{name}.pending"))).unwrap();
    assert!(directories.completion_phase(&name).unwrap() == CompletionPhase::PendingBundle);
    assert!(directories.sync_completed(DIGEST).is_err());
    assert!(!root.path().join("cache").exists());
}

#[test]
fn changed_host_name_is_not_accepted_while_old_directory_remains_locked() {
    let parent = tempfile::tempdir().unwrap();
    let path = parent.path().join("host");
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    let host = BundleDestination::open(&path).unwrap();
    let directories = Directories::open(&path, &host, owner()).unwrap();
    assert!(BundleDestination::open(&path).is_err());
    fs::rename(&path, parent.path().join("old-host")).unwrap();
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(directories.publish(DIGEST, || Ok(())).is_err());
    assert_eq!(fs::read_dir(&path).unwrap().count(), 0);
}

#[test]
fn export_policy_never_grants_the_service_account_ownership() {
    let root = root();
    let host = BundleDestination::open(root.path()).unwrap();
    let directories = Directories::open(root.path(), &host, (40123, 40124)).unwrap();
    let policies = directories.policies();
    assert_eq!(policies.len(), 7);
    let export = policies.last().unwrap();
    assert_eq!((export.uid, export.gid, export.mode), (0, 0, "0755"));
    assert_eq!(export.destination, "/zxs/smb-export");
    assert!(export.empty_only);
}

#[test]
#[ignore = "requires an explicitly authorized root test runner; temporary directories only"]
fn root_handoff_prepares_all_seven_directories_without_touching_system_paths() {
    assert_eq!(rustix::process::geteuid().as_raw(), 0);
    let root = root();
    let host = BundleDestination::open(root.path()).unwrap();
    let directories = Directories::open(root.path(), &host, (40123, 40124)).unwrap();
    assert_eq!(directories.publish(DIGEST, || Ok(())).unwrap().len(), 7);
    let first = json_codec::to_vec(&directories.completed(DIGEST).unwrap()).unwrap();
    directories.sync_completed(DIGEST).unwrap();
    assert!(directories.publish(DIGEST, || Ok(())).unwrap().is_empty());
    assert_eq!(first, json_codec::to_vec(&directories.completed(DIGEST).unwrap()).unwrap());
    for spec in SPECS {
        let metadata = fs::metadata(root.path().join(spec.name)).unwrap();
        let (owner, mode) = directories.attributes(spec);
        assert_eq!((metadata.uid(), metadata.gid(), metadata.mode() & 0o7777), (owner.0, owner.1, mode));
    }
}
