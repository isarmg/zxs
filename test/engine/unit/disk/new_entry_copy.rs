use super::*;
use platform_io::fs_safety::{PrivateDirectory, linux::FileIdentity};
use std::io::{Cursor, Read};

fn root(path: &std::path::Path) -> DiskRoot {
    PrivateDirectory::create_all(path).unwrap();
    DiskRoot::open(path).unwrap()
}
fn expectation(bytes: &[u8]) -> Expectation {
    Expectation { bytes: bytes.len() as u64, blake3: Some(blake3::hash(bytes)) }
}

#[test]
fn fresh_copy_survives_cold_reopen_and_is_an_independent_full_file() {
    let temp = tempfile::tempdir().unwrap();
    let root_path = temp.path().join("held");
    let directory = root(&root_path);
    let name = EntryName::new("copied.txt").unwrap();
    let bytes = b"complete original file";
    let receipt = directory.copy_new_entry_verified_synced(
        &name, &mut Cursor::new(bytes), expectation(bytes),
    ).unwrap();
    assert_eq!(receipt.bytes, bytes.len() as u64);
    assert_eq!(receipt.blake3, blake3::hash(bytes));
    drop(directory);
    let reopened = DiskRoot::open(&root_path).unwrap();
    assert_eq!(reopened.read_entry_bytes(&name, 1024).unwrap(), bytes);
    let file = reopened.open_regular_entry(&name).unwrap();
    assert_eq!(FileIdentity::from_fd(&file).unwrap(), receipt.identity);
    use std::os::unix::fs::MetadataExt;
    assert_eq!(file.metadata().unwrap().nlink(), 1);
}

#[test]
fn same_name_and_same_bytes_do_not_authorize_reusing_an_existing_file() {
    let temp = tempfile::tempdir().unwrap();
    let directory = root(&temp.path().join("held"));
    let name = EntryName::new("occupied").unwrap();
    let bytes = b"preserved";
    let first = directory.copy_new_entry_verified_synced(
        &name, &mut Cursor::new(bytes), expectation(bytes),
    ).unwrap();
    assert!(directory.copy_new_entry_verified_synced(
        &name, &mut Cursor::new(bytes), expectation(bytes),
    ).is_err());
    assert_eq!(directory.read_entry_bytes(&name, 1024).unwrap(), bytes);
    assert_eq!(
        FileIdentity::from_fd(directory.open_regular_entry(&name).unwrap()).unwrap(),
        first.identity,
    );
}

#[test]
fn copy_rechecks_both_length_and_hash_and_never_publishes_partial_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let directory = root(&temp.path().join("held"));
    let name = EntryName::new("frozen").unwrap();
    for bytes in [b"other".as_slice(), b"shorter".as_slice(), b"".as_slice()] {
        assert!(directory.copy_new_entry_verified_synced(
            &name, &mut Cursor::new(bytes), expectation(b"right"),
        ).is_err());
        assert!(directory.open_optional_read_entry(&name).unwrap().is_none());
    }
}

#[test]
fn a_symlink_destination_never_overwrites_its_external_target() {
    let temp = tempfile::tempdir().unwrap();
    let root_path = temp.path().join("held");
    let directory = root(&root_path);
    let outside = temp.path().join("outside");
    std::fs::write(&outside, b"outside bytes").unwrap();
    std::os::unix::fs::symlink(&outside, root_path.join("collision")).unwrap();
    let name = EntryName::new("collision").unwrap();
    assert!(directory.copy_new_entry_verified_synced(
        &name, &mut Cursor::new(b"new"), expectation(b"new"),
    ).is_err());
    assert_eq!(std::fs::read(&outside).unwrap(), b"outside bytes");
    assert!(std::fs::symlink_metadata(root_path.join("collision")).unwrap().is_symlink());
}

struct FailedRead;
impl Read for FailedRead {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("injected source read failure"))
    }
}

#[test]
fn a_failed_source_does_not_turn_into_an_empty_worktree_file() {
    let temp = tempfile::tempdir().unwrap();
    let directory = root(&temp.path().join("held"));
    let name = EntryName::new("failed").unwrap();
    assert!(directory.copy_new_entry_verified_synced(
        &name, &mut FailedRead, expectation(b"body"),
    ).is_err());
    assert!(directory.open_optional_read_entry(&name).unwrap().is_none());
}

#[test]
fn revoked_copy_stops_after_its_first_real_chunk_without_installing_any_name() {
    use std::cell::Cell;

    struct Metered<'a> {
        source: Cursor<Vec<u8>>,
        bytes: &'a Cell<usize>,
    }
    impl Read for Metered<'_> {
        fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
            let read = self.source.read(output)?;
            self.bytes.set(self.bytes.get() + read);
            Ok(read)
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let root_path = temp.path().join("held");
    let held = root(&root_path);
    let sentinel = root_path.join("preserve");
    std::fs::write(&sentinel, b"unrelated original").unwrap();
    let source = vec![71; 1_048_576];
    let expected = expectation(&source);
    let bytes = Cell::new(0usize);
    let mut input = Metered { source: Cursor::new(source), bytes: &bytes };
    let name = EntryName::new("tentative-full").unwrap();
    let result = held.copy_new_entry_verified_synced_observed(
        &name,
        &mut input,
        expected,
        &mut || {
            if bytes.get() == 0 {
                Ok(())
            } else {
                Err(crate::business::file_draft::automatic::stale())
            }
        },
    );
    assert!(result.is_err());
    assert!((1..=64 * 1024).contains(&bytes.get()));
    assert!(held.open_optional_read_entry(&name).unwrap().is_none());
    assert_eq!(std::fs::read_dir(root_path).unwrap().count(), 1);
    assert_eq!(std::fs::read(sentinel).unwrap(), b"unrelated original");
}
