use super::*;
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
fn atomic_create_is_private_no_clobber_and_cleans_failed_staging() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("published");
    write_new(&path, b"complete").unwrap();
    let metadata = fs::metadata(&path).unwrap();
    assert_eq!(metadata.mode() & 0o7777, 0o600);
    assert_eq!(metadata.nlink(), 1);
    assert!(write_new(&path, b"overwrite").is_err());
    assert_eq!(fs::read(&path).unwrap(), b"complete");
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    let alias = directory.path().join("alias");
    symlink(&path, &alias).unwrap();
    assert!(write_new(&alias, b"follow link").is_err());
    assert_eq!(fs::read(&path).unwrap(), b"complete");
    let occupied = directory.path().join("occupied");
    fs::create_dir(&occupied).unwrap();
    assert!(write_new(&occupied, b"must not publish").is_err());
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 3);
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
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn published_staging_cleanup_preserves_a_reused_temporary_name() {
    let temporary = tempfile::tempdir().unwrap();
    let directory = OpenAt2Root::open(temporary.path(), MountPolicy::AllowNestedMounts).unwrap();
    let name = "temporary";
    fs::write(temporary.path().join(name), b"published").unwrap();
    let mut cleanup = StagingCleanup {
        directory: directory.directory(),
        name: name.to_owned(),
        armed: true,
    };
    renameat(directory.directory(), name, directory.directory(), "final").unwrap();
    cleanup.armed = false;
    fs::write(temporary.path().join(name), b"another creator").unwrap();
    drop(cleanup);
    assert_eq!(
        fs::read(temporary.path().join(name)).unwrap(),
        b"another creator"
    );
    assert_eq!(
        fs::read(temporary.path().join("final")).unwrap(),
        b"published"
    );
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
fn failed_explicit_staging_cleanup_preserves_error_and_never_retries_on_drop() {
    let root = tempfile::tempdir().unwrap();
    let directory = File::open(root.path()).unwrap();
    fs::create_dir(root.path().join("staged")).unwrap();
    let mut cleanup = StagingCleanup {
        directory: &directory,
        name: "staged".into(),
        armed: true,
    };
    assert_eq!(
        cleanup.cleanup().unwrap_err().kind(),
        std::io::ErrorKind::IsADirectory
    );
    assert!(!cleanup.armed);
    drop(cleanup);
    assert!(root.path().join("staged").is_dir());
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

#[test]
fn failed_drop_cleanup_does_not_panic_when_the_diagnostic_sink_is_full() {
    let root = tempfile::tempdir().unwrap();
    let directory = File::open(root.path()).unwrap();
    fs::create_dir(root.path().join("staged")).unwrap();
    let full = fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let subscriber = tracing::collector::fmt()
        .with_writer(full)
        .log_internal_errors(false)
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        drop(StagingCleanup {
            directory: &directory,
            name: "staged".into(),
            armed: true,
        });
    });
    assert!(root.path().join("staged").is_dir());
}
