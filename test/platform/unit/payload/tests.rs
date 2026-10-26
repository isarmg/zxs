use super::*;
use std::{fs, io::Cursor, os::unix::fs::MetadataExt};

#[test]
fn independent_complete_files_have_matching_content_and_distinct_inodes() {
    let temp = tempfile::tempdir().unwrap();
    let root = PrivateDirectory::create(temp.path().join("payloads")).unwrap();
    let content = vec![42u8; 130_001];
    let expectation = Expectation {
        bytes: content.len() as u64,
        blake3: Some(blake3::hash(&content)),
    };
    let first = write_full(
        &root,
        &EntryName::new("one").unwrap(),
        &mut Cursor::new(&content),
        expectation,
    )
    .unwrap();
    let second = write_full(
        &root,
        &EntryName::new("two").unwrap(),
        &mut Cursor::new(&content),
        expectation,
    )
    .unwrap();
    assert_eq!(first.blake3, second.blake3);
    assert_ne!(first.identity.inode, second.identity.inode);
    for name in ["one", "two"] {
        let path = root.path().join(name);
        assert_eq!(fs::metadata(&path).unwrap().nlink(), 1);
        let mut file = File::open(path).unwrap();
        assert_eq!(
            verify_full(&mut file, expectation).unwrap().blake3,
            first.blake3
        );
    }
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 2);
}

#[test]
fn bad_length_or_digest_leaves_no_published_or_staging_file() {
    let temp = tempfile::tempdir().unwrap();
    let root = PrivateDirectory::create(temp.path().join("payloads")).unwrap();
    let name = EntryName::new("immutable").unwrap();
    for expectation in [
        Expectation {
            bytes: 2,
            blake3: None,
        },
        Expectation {
            bytes: 4,
            blake3: None,
        },
        Expectation {
            bytes: 3,
            blake3: Some(blake3::hash(b"bad")),
        },
    ] {
        assert!(write_full(&root, &name, &mut Cursor::new(b"abc"), expectation).is_err());
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }
}

#[test]
fn publishing_never_clobbers_existing_payload_or_symlink() {
    let temp = tempfile::tempdir().unwrap();
    let root = PrivateDirectory::create(temp.path().join("payloads")).unwrap();
    let name = EntryName::new("immutable").unwrap();
    let expectation = Expectation {
        bytes: 3,
        blake3: None,
    };
    write_full(&root, &name, &mut Cursor::new(b"old"), expectation).unwrap();
    assert!(write_full(&root, &name, &mut Cursor::new(b"new"), expectation).is_err());
    assert_eq!(fs::read(root.path().join("immutable")).unwrap(), b"old");
    std::os::unix::fs::symlink("immutable", root.path().join("link")).unwrap();
    assert!(
        write_full(
            &root,
            &EntryName::new("link").unwrap(),
            &mut Cursor::new(b"new"),
            expectation
        )
        .is_err()
    );
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 2);
}

#[test]
fn randomness_failure_preserves_its_entropy_source() {
    let error = Error::Randomness(system_entropy::Error::UNSUPPORTED);
    let source = std::error::Error::source(&error).expect("entropy source must survive");
    assert!(source.downcast_ref::<system_entropy::Error>().is_some());
}

#[test]
fn read_and_cleanup_failures_are_both_preserved_without_deleting_a_replacement() {
    struct ReplacedStaging(std::path::PathBuf);
    impl Read for ReplacedStaging {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            let temporary = fs::read_dir(&self.0)?.next().unwrap()?.path();
            fs::remove_file(&temporary)?;
            fs::create_dir(&temporary)?;
            Err(io::Error::from_raw_os_error(libc::EIO))
        }
    }
    let root = tempfile::tempdir().unwrap();
    let directory = PrivateDirectory::create(root.path().join("private")).unwrap();
    let error = write_full(
        &directory,
        &EntryName::new("output").unwrap(),
        &mut ReplacedStaging(directory.path().to_owned()),
        Expectation {
            bytes: 1,
            blake3: None,
        },
    )
    .unwrap_err();
    let Error::Cleanup { operation, cleanup } = error else {
        panic!("both failures must survive");
    };
    assert!(
        matches!(*operation, Error::Io(ref source) if source.raw_os_error() == Some(libc::EIO))
    );
    assert_eq!(cleanup.raw_os_error(), Some(libc::EISDIR));
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    assert!(
        fs::read_dir(directory.path())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .file_type()
            .unwrap()
            .is_dir()
    );
}

#[test]
fn published_payload_cleanup_does_not_remove_reused_name() {
    let root = tempfile::tempdir().unwrap();
    let name = "staged";
    fs::write(root.path().join(name), b"published").unwrap();
    let mut cleanup = StagingCleanup {
        directory: File::open(root.path()).unwrap(),
        name: name.into(),
        armed: true,
    };
    fs::rename(root.path().join(name), root.path().join("final")).unwrap();
    cleanup.armed = false;
    fs::write(root.path().join(name), b"replacement").unwrap();
    drop(cleanup);
    assert_eq!(fs::read(root.path().join(name)).unwrap(), b"replacement");
}
