use super::*;
use platform_io::fs_safety::PrivateDirectory;
use std::{fs, io::Cursor};

struct Fixture {
    temp: tempfile::TempDir,
    source: DiskRoot,
    target: DiskRoot,
    from: EntryName,
    to: EntryName,
    expected: FrozenFileMove,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let source_path = temp.path().join("source");
        let target_path = temp.path().join("target");
        PrivateDirectory::create(&source_path).unwrap();
        PrivateDirectory::create(&target_path).unwrap();
        let source = DiskRoot::open(&source_path).unwrap();
        let target = DiskRoot::open(&target_path).unwrap();
        let from = EntryName::new("edited-source").unwrap();
        let to = EntryName::new("snapshot-target").unwrap();
        let receipt = source.copy_new_entry_verified_synced(
            &from, &mut Cursor::new(b"sealed edit"),
            Expectation { bytes: 11, blake3: Some(blake3::hash(b"sealed edit")) },
        ).unwrap();
        let expected = FrozenFileMove {
            bytes: 11, content_hash: ContentHash::from_digest(receipt.blake3),
            entity: LocalEntityIdentity::new(receipt.identity.device, receipt.identity.inode),
        };
        Self { temp, source, target, from, to, expected }
    }
    fn move_file(&self) -> Result<Receipt, FileMoveError> {
        self.source.move_new_entry_verified_synced(
            &self.from, &self.target, &self.to, self.expected,
        )
    }
    fn resume(&self) -> Result<Receipt, FileMoveError> {
        self.source.resume_decided_file_move(&self.from, &self.target, &self.to, self.expected)
    }
}

#[test]
fn move_keeps_one_inode_at_destination_and_never_copies_body_back_to_worktree() {
    let fixture = Fixture::new();
    let receipt = fixture.move_file().unwrap();
    assert_eq!(receipt.identity.device, fixture.expected.entity.device());
    assert_eq!(receipt.identity.inode, fixture.expected.entity.inode());
    assert!(fixture.source.open_optional_read_entry(&fixture.from).unwrap().is_none());
    assert_eq!(fixture.target.read_entry_bytes(&fixture.to, 100).unwrap(), b"sealed edit");
    assert_eq!(fixture.resume().unwrap(), receipt);
    assert_eq!(fixture.resume().unwrap(), receipt);
    assert!(fixture.source.open_optional_read_entry(&fixture.from).unwrap().is_none());
    assert_eq!(fixture.target.root_entries().unwrap().count(), 1);
}

#[test]
fn cold_recovery_of_an_already_renamed_inode_requires_the_original_exact_decision() {
    let fixture = Fixture::new();
    fs::rename(
        fixture.source.path().join(fixture.from.as_path()),
        fixture.target.path().join(fixture.to.as_path()),
    ).unwrap();
    let source = DiskRoot::open(fixture.source.path()).unwrap();
    let target = DiskRoot::open(fixture.target.path()).unwrap();
    assert!(matches!(source.move_new_entry_verified_synced(
        &fixture.from, &target, &fixture.to, fixture.expected,
    ), Err(FileMoveError::Rejected(FileMoveFault::ExistingDestination))));
    let recovered = source.resume_decided_file_move(
        &fixture.from, &target, &fixture.to, fixture.expected,
    ).unwrap();
    assert_eq!(recovered.identity.inode, fixture.expected.entity.inode());
    assert_eq!(target.read_entry_bytes(&fixture.to, 100).unwrap(), b"sealed edit");
}

#[test]
fn matching_destination_bytes_with_a_different_inode_never_authorize_overwrite_or_adoption() {
    let fixture = Fixture::new();
    fs::write(fixture.target.path().join(fixture.to.as_path()), b"sealed edit").unwrap();
    assert!(matches!(fixture.move_file(), Err(FileMoveError::Rejected(FileMoveFault::Identity))));
    assert!(matches!(fixture.resume(), Err(FileMoveError::Recovery(FileMoveFault::Identity))));
    assert_eq!(fixture.source.read_entry_bytes(&fixture.from, 100).unwrap(), b"sealed edit");
    assert_eq!(fixture.target.read_entry_bytes(&fixture.to, 100).unwrap(), b"sealed edit");
}

#[test]
fn absent_source_and_foreign_same_bytes_are_not_a_successful_prior_move() {
    let fixture = Fixture::new();
    fs::rename(fixture.source.path().join(fixture.from.as_path()),
        fixture.temp.path().join("retained-original")).unwrap();
    fs::write(fixture.target.path().join(fixture.to.as_path()), b"sealed edit").unwrap();
    assert!(matches!(fixture.resume(), Err(FileMoveError::Recovery(FileMoveFault::Identity))));
    assert_eq!(fs::read(fixture.temp.path().join("retained-original")).unwrap(), b"sealed edit");
}

#[test]
fn corrupt_or_substituted_source_is_not_moved_even_when_its_name_is_unchanged() {
    for substitute in [false, true] {
        let fixture = Fixture::new();
        let path = fixture.source.path().join(fixture.from.as_path());
        if substitute {
            fs::rename(&path, fixture.temp.path().join("retained-original")).unwrap();
            fs::write(&path, b"sealed edit").unwrap();
        } else {
            fs::write(&path, b"corrupt old").unwrap();
        }
        assert!(fixture.move_file().is_err());
        assert!(path.exists());
        assert!(fixture.target.open_optional_read_entry(&fixture.to).unwrap().is_none());
    }
}

#[test]
fn symlink_destination_does_not_modify_its_target_or_remove_the_source() {
    let fixture = Fixture::new();
    let outside = fixture.temp.path().join("outside");
    fs::write(&outside, b"unrelated").unwrap();
    std::os::unix::fs::symlink(&outside,
        fixture.target.path().join(fixture.to.as_path())).unwrap();
    assert!(fixture.move_file().is_err());
    assert_eq!(fs::read(&outside).unwrap(), b"unrelated");
    assert_eq!(fixture.source.read_entry_bytes(&fixture.from, 100).unwrap(), b"sealed edit");
}

#[test]
fn both_locations_absent_remains_unresolved_and_never_creates_or_reconstructs_content() {
    let fixture = Fixture::new();
    fs::rename(fixture.source.path().join(fixture.from.as_path()),
        fixture.temp.path().join("retained-original")).unwrap();
    assert!(matches!(fixture.resume(), Err(FileMoveError::Recovery(FileMoveFault::Missing))));
    assert_eq!(fixture.source.root_entries().unwrap().count(), 0);
    assert_eq!(fixture.target.root_entries().unwrap().count(), 0);
}
