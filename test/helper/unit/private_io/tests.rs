use super::*;
use platform_io::publication::PublicationFailure;
use std::io::Write;
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    sync::Arc,
};

fn read_owned_bytes(path: &Path, maximum: usize) -> Result<Vec<u8>> {
    AnchoredPath::open(path)?.read_owned_bytes(maximum)
}

fn replace_private(path: &Path, bytes: &[u8]) -> Result<()> {
    DraftTransaction::open(path)?.replace_private(bytes)
}

#[test]
fn owned_draft_reads_are_bounded_and_reject_links_without_requiring_secret_mode() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("draft.json");
    fs::write(&path, b"draft").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(read_owned_bytes(&path, 5).unwrap(), b"draft");
    assert!(read_owned_bytes(&path, 4).is_err());
    symlink(&path, root.path().join("alias")).unwrap();
    assert!(read_owned_bytes(&root.path().join("alias"), 5).is_err());
    fs::hard_link(&path, root.path().join("hard")).unwrap();
    assert!(read_owned_bytes(&path, 5).is_err());
    assert!(read_owned_bytes(root.path(), 5).is_err());
}

#[test]
fn atomic_create_is_private_no_clobber_and_preserves_uncertain_staging() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("published");
    write_new(&path, b"complete").unwrap();
    let metadata = fs::metadata(&path).unwrap();
    assert_eq!(metadata.mode() & 0o7777, 0o600);
    assert_eq!(metadata.nlink(), 1);
    let error = write_new(&path, b"overwrite").unwrap_err();
    let failure = error.downcast_ref::<PublicationFailure>().unwrap();
    let AtomicCommitError::Unknown(platform_io::fs_safety::Error::Io(source)) = &failure.source
    else {
        panic!("rename collision must remain Unknown: {failure:?}");
    };
    assert_eq!(source.raw_os_error(), Some(17));
    assert_eq!(
        fs::read(directory.path().join(&failure.staging)).unwrap(),
        b"overwrite"
    );
    assert_eq!(
        FileIdentity::from_fd(File::open(directory.path().join(&failure.staging)).unwrap())
            .unwrap(),
        failure.identity
    );
    assert_eq!(fs::read(&path).unwrap(), b"complete");
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
    let alias = directory.path().join("alias");
    symlink(&path, &alias).unwrap();
    assert!(write_new(&alias, b"follow link").is_err());
    assert_eq!(fs::read(&path).unwrap(), b"complete");
    let occupied = directory.path().join("occupied");
    fs::create_dir(&occupied).unwrap();
    assert!(write_new(&occupied, b"must not publish").is_err());
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 6);
}

#[test]
fn concurrent_private_publication_has_exactly_one_complete_winner() {
    let directory = tempfile::tempdir().unwrap();
    let path = Arc::new(directory.path().join("published"));
    let threads = [b"first".as_slice(), b"second".as_slice()]
        .into_iter()
        .map(|bytes| {
            let path = path.clone();
            std::thread::spawn(move || write_new(&path, bytes).is_ok())
        })
        .collect::<Vec<_>>();
    assert_eq!(
        threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .filter(|won| *won)
            .count(),
        1
    );
    let bytes = fs::read(&*path).unwrap();
    assert!(bytes == b"first" || bytes == b"second");
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
}

#[test]
fn replacement_updates_only_owned_regular_files_without_following_links() {
    let directory = tempfile::tempdir().unwrap();
    let original = directory.path().join("draft");
    write_new(&original, b"before").unwrap();
    replace_private(&original, b"after").unwrap();
    assert_eq!(fs::read(&original).unwrap(), b"after");
    let alias = directory.path().join("alias");
    symlink(&original, &alias).unwrap();
    assert!(replace_private(&alias, b"must not follow").is_err());
    assert_eq!(fs::read(&original).unwrap(), b"after");
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
}

#[test]
fn held_parent_is_not_reopened_after_directory_rebinding() {
    let temporary = tempfile::tempdir().unwrap();
    let parent = temporary.path().join("parent");
    fs::create_dir(&parent).unwrap();
    write_new(&parent.join("draft"), b"original").unwrap();
    let held = AnchoredPath::open(&parent.join("draft")).unwrap();
    let moved = temporary.path().join("moved");
    fs::rename(&parent, &moved).unwrap();
    fs::create_dir(&parent).unwrap();
    write_new(&parent.join("draft"), b"replacement").unwrap();
    assert_eq!(held.read_owned_bytes(1024).unwrap(), b"original");
}

#[test]
fn administrator_cannot_accidentally_read_another_owners_draft() {
    if rustix::process::geteuid().as_raw() != 0 {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("foreign");
    write_new(&path, b"fixture-value").unwrap();
    let file = File::open(&path).unwrap();
    rustix::fs::fchown(&file, Some(rustix::process::Uid::from_raw(65534)), None).unwrap();
    assert!(read_owned_bytes(&path, 1024).is_err());
}

#[test]
fn draft_transaction_excludes_parent_aliases_and_survives_draft_replacement() {
    let root = tempfile::tempdir().unwrap();
    let parent = root.path().join("parent");
    fs::create_dir(&parent).unwrap();
    let alias = root.path().join("alias");
    symlink(&parent, &alias).unwrap();
    let path = parent.join("draft.json");
    write_new(&path, b"original").unwrap();
    let held = DraftTransaction::open(&path).unwrap();
    let original = held.read_owned_bytes(1024).unwrap();
    let contender = alias.join("draft.json");
    let error = std::thread::spawn(move || match DraftTransaction::open(&contender) {
        Ok(_) => panic!("a parent alias must not bypass the active transaction"),
        Err(error) => error.to_string(),
    })
    .join()
    .unwrap();
    assert!(error.contains("another configuration draft edit is active"));
    held.write_backup(OsStr::new("draft.json.before-registration"), &original)
        .unwrap();
    held.replace_private(b"replacement").unwrap();
    // Atomic rename creates a new draft inode, but must not release its lock.
    assert!(DraftTransaction::open(&path).is_err());
    // A backup used as another draft is protected by the same stable anchor.
    assert!(DraftTransaction::open(&parent.join("draft.json.before-registration")).is_err());
    drop(held);
    let next = DraftTransaction::open(&path).unwrap();
    assert_eq!(next.read_owned_bytes(1024).unwrap(), b"replacement");
    for name in ["draft.json", "draft.json.before-registration"] {
        assert_eq!(
            fs::metadata(parent.join(name)).unwrap().mode() & 0o7777,
            0o600
        );
    }
    assert_eq!(fs::read_dir(&parent).unwrap().count(), 2);
}

#[test]
fn draft_transaction_keeps_markers_backups_and_writes_on_the_held_parent() {
    let root = tempfile::tempdir().unwrap();
    let parent = root.path().join("parent");
    fs::create_dir(&parent).unwrap();
    let path = parent.join("draft.json");
    write_new(&path, b"original").unwrap();
    let held = DraftTransaction::open(&path).unwrap();
    let moved = root.path().join("moved");
    fs::rename(&parent, &moved).unwrap();
    fs::create_dir(&parent).unwrap();
    write_new(&path, b"unrelated draft").unwrap();
    fs::write(moved.join("writer.lock"), b"").unwrap();
    assert!(held.entry_exists(OsStr::new("writer.lock")).unwrap());
    assert!(!parent.join("writer.lock").exists());
    let original = held.read_owned_bytes(1024).unwrap();
    held.write_backup(OsStr::new("draft.json.before-registration"), &original)
        .unwrap();
    held.replace_private(b"replacement").unwrap();
    assert_eq!(fs::read(moved.join("draft.json")).unwrap(), b"replacement");
    assert_eq!(
        fs::read(moved.join("draft.json.before-registration")).unwrap(),
        b"original"
    );
    assert_eq!(fs::read(&path).unwrap(), b"unrelated draft");
    assert!(!parent.join("draft.json.before-registration").exists());
    assert!(DraftTransaction::open(&moved.join("draft.json")).is_err());
}

#[test]
fn dropping_draft_transaction_releases_lock_before_an_unrelated_child_execs() {
    use std::os::unix::{net::UnixStream, process::CommandExt};
    use std::time::Duration;

    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("draft.json");
    write_new(&path, b"draft").unwrap();
    let transaction = DraftTransaction::open(&path).unwrap();
    let (mut parent, child) = UnixStream::pair().unwrap();
    parent
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    child
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let spawned = std::thread::spawn(move || {
        let mut command = std::process::Command::new("/bin/true");
        // SAFETY: The child only uses async-signal-safe read/write on an already-owned socket;
        // the inherited draft descriptors deliberately remain open until the parent releases it.
        unsafe {
            command.pre_exec(move || {
                rustix::io::write(&child, b"r")?;
                let mut release = [0_u8; 1];
                if rustix::io::read(&child, &mut release)? != 1 {
                    return Err(std::io::ErrorKind::UnexpectedEof.into());
                }
                Ok(())
            });
        }
        command.spawn().unwrap().wait().unwrap()
    });
    let mut ready = [0_u8; 1];
    parent.read_exact(&mut ready).unwrap();
    drop(transaction);
    let next = DraftTransaction::open(&path);
    // Release and reap the child even when the regression assertion below fails.
    parent.write_all(b"x").unwrap();
    assert!(spawned.join().unwrap().success());
    assert!(
        next.is_ok(),
        "completed transaction left its lock in the child: {:#?}",
        next.err()
    );
}

/// Run only through publication-io.sh, in one isolated libc-backend process.
#[test]
#[ignore = "requires the isolated libc rename/fsync interposer"]
fn publication_faults_preserve_phase_identity_and_evidence() {
    let root = std::path::PathBuf::from(std::env::var_os("ZXS_ATOMIC_ROOT").unwrap());
    let mode = std::env::var("ZXS_ATOMIC_MODE").unwrap();
    let stage = std::env::var("ZXS_ATOMIC_STAGE").unwrap();
    let target = root.join("destination");
    fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
    if mode == "replace" {
        fs::write(&target, b"original").unwrap();
    }
    fs::write(root.join(".inject"), b"").unwrap();
    let result = match mode.as_str() {
        "create" => write_new(&target, b"candidate"),
        "replace" => replace_private(&target, b"candidate"),
        _ => panic!("unknown publication mode"),
    };
    fs::remove_file(root.join(".inject")).unwrap();
    let staged: Vec<_> = fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with(".zxs-helper-")
        })
        .collect();
    let events = fs::read_to_string(std::env::var_os("ZXS_ATOMIC_LOG").unwrap()).unwrap();
    match stage.as_str() {
        "success" => {
            result.unwrap();
            assert!(staged.is_empty());
            assert_eq!(fs::read(&target).unwrap(), b"candidate");
            assert_eq!(events, "file-sync\ndirectory-sync\n");
        }
        "file-sync" => {
            let error = result.unwrap_err();
            assert_eq!(
                error
                    .downcast_ref::<std::io::Error>()
                    .unwrap()
                    .raw_os_error(),
                Some(5)
            );
            assert!(staged.is_empty());
            assert_eq!(events, "file-sync\n");
            if mode == "replace" {
                assert_eq!(fs::read(&target).unwrap(), b"original");
            } else {
                assert!(!target.exists());
            }
        }
        "rename-before" | "rename-after" | "create-race" | "directory-sync" => {
            let error = result.unwrap_err();
            let failure = error.downcast_ref::<PublicationFailure>().unwrap();
            assert_eq!(
                failure.directory,
                FileIdentity::from_fd(File::open(&root).unwrap()).unwrap()
            );
            assert_eq!(failure.target, Path::new("destination"));
            let source = match (&failure.source, stage.as_str()) {
                (
                    AtomicCommitError::Published(
                        platform_io::fs_safety::Error::PublishedDurabilityUnknown(error),
                    ),
                    "directory-sync",
                ) => error,
                (
                    AtomicCommitError::Unknown(platform_io::fs_safety::Error::Io(error)),
                    "rename-before" | "rename-after" | "create-race",
                ) => error,
                _ => panic!("wrong phase: {failure:?}"),
            };
            assert_eq!(
                source.raw_os_error(),
                Some(if stage == "create-race" { 17 } else { 5 })
            );
            if stage == "rename-before" || stage == "create-race" {
                assert_eq!(staged, [root.join(&failure.staging)]);
                assert_eq!(fs::read(&staged[0]).unwrap(), b"candidate");
                assert_eq!(
                    FileIdentity::from_fd(File::open(&staged[0]).unwrap()).unwrap(),
                    failure.identity
                );
                if stage == "create-race" {
                    assert_eq!(fs::read(&target).unwrap(), b"occupant");
                } else if mode == "replace" {
                    assert_eq!(fs::read(&target).unwrap(), b"original");
                } else {
                    assert!(!target.exists());
                }
            } else {
                assert!(staged.is_empty());
                assert_eq!(fs::read(&target).unwrap(), b"candidate");
                assert_eq!(
                    FileIdentity::from_fd(File::open(&target).unwrap()).unwrap(),
                    failure.identity
                );
            }
            let event = match stage.as_str() {
                "rename-before" => "rename-before-eio",
                "rename-after" => "rename-success-reported-eio",
                "create-race" => "rename-race-eexist",
                "directory-sync" => "directory-sync",
                _ => unreachable!(),
            };
            assert_eq!(events, format!("file-sync\n{event}\n"));
        }
        _ => panic!("unknown fault stage"),
    }
    assert_eq!(fs::metadata(&root).unwrap().mode() & 0o7777, 0o755);
    println!("\nZXS_RESCUE_PUBLICATION_COMPLETE mode={mode} stage={stage}");
}

#[test]
fn program_only_publication_is_executable_no_clobber_and_retries_exact_bytes() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("zxs");
    let destination = ProgramDestination::open(&path).unwrap();
    assert!(!destination.matches_or_absent(b"exact executable").unwrap());
    assert!(destination.install(b"exact executable").unwrap());
    assert_eq!(fs::read(&path).unwrap(), b"exact executable");
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o7777, 0o555);
    assert!(!destination.install(b"exact executable").unwrap());
    assert!(destination.install(b"different bytes").is_err());
    assert_eq!(fs::read(&path).unwrap(), b"exact executable");
}

#[test]
fn program_destination_refuses_links_shared_inodes_unsafe_parents_and_competing_owners() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("zxs");
    let held = ProgramDestination::open(&path).unwrap();
    assert!(ProgramDestination::open(&path).is_err());
    drop(held);
    let other = root.path().join("untouched");
    fs::write(&other, b"untouched").unwrap();
    symlink(&other, &path).unwrap();
    assert!(
        ProgramDestination::open(&path)
            .unwrap()
            .install(b"new")
            .is_err()
    );
    assert_eq!(fs::read(&other).unwrap(), b"untouched");
    fs::remove_file(&path).unwrap();
    fs::hard_link(&other, &path).unwrap();
    assert!(
        ProgramDestination::open(&path)
            .unwrap()
            .install(b"untouched")
            .is_err()
    );
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o777)).unwrap();
    assert!(ProgramDestination::open(&path).is_err());
}

#[test]
fn verified_program_file_keeps_its_inode_through_synchronization_and_name_replacement() {
    use std::os::unix::fs::FileExt;

    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("zxs");
    let original = root.path().join("original");
    let destination = ProgramDestination::open(&path).unwrap();
    let payload = b"exact embedded program";
    assert!(destination.matching_file(payload).unwrap().is_none());
    assert!(destination.install(payload).unwrap());
    let held = destination.matching_file(payload).unwrap().unwrap();
    let identity = FileIdentity::from_fd(&held).unwrap();
    fs::rename(&path, &original).unwrap();
    fs::write(&path, payload).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o555)).unwrap();

    held.sync_all().unwrap();
    let mut bytes = [0_u8; 22];
    held.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, payload);
    assert_eq!(FileIdentity::from_fd(&held).unwrap(), identity);
    assert_eq!(
        FileIdentity::from_fd(File::open(&original).unwrap()).unwrap(),
        identity
    );
    assert_ne!(
        FileIdentity::from_fd(File::open(&path).unwrap()).unwrap(),
        identity
    );
    let error = destination
        .target
        .directory
        .verify_entry_identity(&destination.target.name, identity)
        .unwrap_err();
    assert!(matches!(
        error,
        platform_io::fs_safety::Error::IdentityChanged(_)
    ));
    // Same bytes do not make the replacement the inode synchronized through held.
    assert!(destination.matches_or_absent(payload).unwrap());
    // A fresh installation attempt may verify and synchronize that new inode itself.
    assert!(!destination.install(payload).unwrap());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    fs::write(&path, b"different program").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o555)).unwrap();
    assert!(destination.matches_or_absent(payload).is_err());
    assert!(destination.install(payload).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"different program");
    assert_eq!(fs::read(&original).unwrap(), payload);
}
