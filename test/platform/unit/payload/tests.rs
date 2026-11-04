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
    for (name, receipt) in [("one", &first), ("two", &second)] {
        let path = root.path().join(name);
        assert_eq!(fs::metadata(&path).unwrap().nlink(), 1);
        let mut file = File::open(path).unwrap();
        assert_eq!(&verify_full(&mut file, expectation).unwrap(), receipt);
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
    // Rename collisions are Unknown; both prepared staging files survive.
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 4);
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
    assert!(matches!(
        cleanup,
        crate::fs_safety::Error::IdentityChanged(_)
    ));
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
    let held = File::open(root.path().join(name)).unwrap();
    let mut cleanup = StagingCleanup {
        file: &held,
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

#[test]
fn reader_replacement_cannot_receive_a_receipt_for_another_inode() {
    struct ReplacingReader {
        directory: std::path::PathBuf,
        done: bool,
        retain_original: bool,
    }
    impl Read for ReplacingReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.done {
                return Ok(0);
            }
            self.done = true;
            let staging = fs::read_dir(&self.directory)?.next().unwrap()?.path();
            if self.retain_original {
                fs::rename(&staging, self.directory.join("retained"))?;
            } else {
                fs::remove_file(&staging)?;
            }
            fs::write(staging, b"substitute")?;
            buffer[..4].copy_from_slice(b"good");
            Ok(4)
        }
    }
    for retain_original in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let directory = PrivateDirectory::create(temp.path().join("private")).unwrap();
        let error = write_full(
            &directory,
            &EntryName::new("output").unwrap(),
            &mut ReplacingReader {
                directory: directory.path().to_owned(),
                done: false,
                retain_original,
            },
            Expectation {
                bytes: 4,
                blake3: Some(blake3::hash(b"good")),
            },
        )
        .unwrap_err();
        let Error::Cleanup { operation, cleanup } = error else {
            panic!("replacement must survive cleanup");
        };
        assert!(matches!(
            *operation,
            Error::Publication(AtomicCommitError::NotPublished(_))
        ));
        assert!(matches!(
            cleanup,
            crate::fs_safety::Error::IdentityChanged(_)
        ));
        assert!(!directory.path().join("output").exists());
        let staging = fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .as_encoded_bytes()
                    .starts_with(b".zxs-payload-")
            })
            .unwrap();
        assert_eq!(fs::read(staging).unwrap(), b"substitute");
        if retain_original {
            assert_eq!(
                fs::read(directory.path().join("retained")).unwrap(),
                b"good"
            );
        }
    }
}

#[test]
#[ignore = "requires atomic-io.sh isolated LD_PRELOAD children"]
fn payload_io_barriers_and_failures() {
    let root = std::path::PathBuf::from(std::env::var_os("ZXS_ATOMIC_ROOT").unwrap());
    let log = std::path::PathBuf::from(std::env::var_os("ZXS_ATOMIC_LOG").unwrap());
    let stage = std::env::var("ZXS_ATOMIC_STAGE").unwrap();
    let directory = PrivateDirectory::open_existing(&root).unwrap();
    fs::write(root.join(".inject"), b"").unwrap();
    let result = write_full(
        &directory,
        &EntryName::new("destination").unwrap(),
        &mut Cursor::new(b"new"),
        Expectation {
            bytes: 3,
            blake3: Some(blake3::hash(b"new")),
        },
    );
    fs::remove_file(root.join(".inject")).unwrap();
    let events = fs::read_to_string(log).unwrap();
    if stage == "success" {
        let receipt = result.unwrap();
        let mut target = File::open(root.join("destination")).unwrap();
        assert_eq!(
            receipt,
            verify_full(
                &mut target,
                Expectation {
                    bytes: 3,
                    blake3: None
                }
            )
            .unwrap()
        );
    } else {
        let error = result.unwrap_err();
        match stage.as_str() {
            "write" => assert!(matches!(error, Error::Io(_))),
            "file-sync" => assert!(matches!(
                error,
                Error::Publication(AtomicCommitError::NotPublished(_))
            )),
            "directory-sync" => assert!(matches!(
                error,
                Error::Publication(AtomicCommitError::Published(_))
            )),
            "publish" | "rename-before" | "rename-after" | "create-race" => assert!(matches!(
                error,
                Error::Publication(AtomicCommitError::Unknown(_))
            )),
            _ => panic!("unexpected stage"),
        }
        let source = match &error {
            Error::Io(source) => source,
            Error::Publication(
                AtomicCommitError::NotPublished(source)
                | AtomicCommitError::Unknown(source)
                | AtomicCommitError::Published(source),
            ) => {
                assert!(std::error::Error::source(&error).is_some());
                match source {
                    crate::fs_safety::Error::Io(source)
                    | crate::fs_safety::Error::PublishedDurabilityUnknown(source) => source,
                    _ => panic!("original I/O error must survive"),
                }
            }
            _ => panic!("original I/O error must survive"),
        };
        let errno = source.raw_os_error();
        assert_eq!(
            errno,
            Some(if matches!(stage.as_str(), "publish" | "create-race") {
                libc::EEXIST
            } else {
                libc::EIO
            })
        );
    }
    let expected_events = match stage.as_str() {
        "success" | "directory-sync" => "file-sync\ndirectory-sync\n",
        "write" => "write-failure\n",
        "rename-before" => "file-sync\nrename-before-eio\n",
        "rename-after" => "file-sync\nrename-success-reported-eio\n",
        "create-race" => "file-sync\nrename-race-eexist\n",
        "file-sync" | "publish" => "file-sync\n",
        _ => panic!("unexpected stage"),
    };
    assert_eq!(events, expected_events);
    match stage.as_str() {
        "success" | "directory-sync" | "rename-after" => {
            assert_eq!(fs::read(root.join("destination")).unwrap(), b"new")
        }
        "create-race" => assert_eq!(fs::read(root.join("destination")).unwrap(), b"occupant"),
        "publish" => assert_eq!(fs::read(root.join("destination")).unwrap(), b""),
        _ => assert!(!root.join("destination").exists()),
    }
    let staging: Vec<_> = fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .as_encoded_bytes()
                .starts_with(b".zxs-payload-")
        })
        .collect();
    assert_eq!(
        staging.len(),
        usize::from(matches!(
            stage.as_str(),
            "publish" | "rename-before" | "create-race"
        ))
    );
    for path in staging {
        assert_eq!(fs::read(path).unwrap(), b"new");
    }
    println!("\nZXS_ATOMIC_IO_COMPLETE mode=payload stage={stage}");
}

#[test]
fn reader_directory_replacement_keeps_publication_in_the_held_directory() {
    struct MovingReader {
        original: std::path::PathBuf,
        retained: std::path::PathBuf,
        done: bool,
    }
    impl Read for MovingReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.done {
                return Ok(0);
            }
            self.done = true;
            fs::rename(&self.original, &self.retained)?;
            fs::create_dir(&self.original)?;
            fs::write(self.original.join("output"), b"substitute directory")?;
            buffer[..4].copy_from_slice(b"good");
            Ok(4)
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let directory = PrivateDirectory::create(temp.path().join("private")).unwrap();
    let retained = temp.path().join("retained");
    let expectation = Expectation {
        bytes: 4,
        blake3: Some(blake3::hash(b"good")),
    };
    let receipt = write_full(
        &directory,
        &EntryName::new("output").unwrap(),
        &mut MovingReader {
            original: directory.path().to_owned(),
            retained: retained.clone(),
            done: false,
        },
        expectation,
    )
    .unwrap();
    let mut target = File::open(retained.join("output")).unwrap();
    assert_eq!(receipt, verify_full(&mut target, expectation).unwrap());
    assert_eq!(
        fs::read(directory.path().join("output")).unwrap(),
        b"substitute directory"
    );
    assert_eq!(fs::read_dir(retained).unwrap().count(), 1);
}

#[test]
fn observed_verification_keeps_identity_digest_and_bounded_cancellation() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("observed");
    let bytes = vec![19; 150_000];
    fs::write(&path, &bytes).unwrap();
    let expectation = Expectation {
        bytes: bytes.len() as u64,
        blake3: Some(blake3::hash(&bytes)),
    };
    let mut file = File::open(&path).unwrap();
    let reference = verify_full(&mut file, expectation).unwrap();
    let mut checkpoints = 0;
    let observed = verify_full_observed(&mut file, expectation, &mut || {
        checkpoints += 1;
        Ok(())
    })
    .unwrap();
    assert_eq!(observed, reference);
    assert_eq!(checkpoints, 6); // initial, three chunks, EOF, final receipt

    for (stop, expected_position) in [(1, 7), (3, 64 * 1024), (6, 150_000)] {
        file.seek(SeekFrom::Start(7)).unwrap();
        let mut checkpoints = 0;
        let result = verify_full_observed(&mut file, expectation, &mut || {
            checkpoints += 1;
            if checkpoints == stop {
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "cancelled"))
            } else {
                Ok(())
            }
        });
        assert!(
            matches!(result, Err(Error::Io(error)) if error.kind() == io::ErrorKind::PermissionDenied)
        );
        assert_eq!(file.stream_position().unwrap(), expected_position);
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn observed_verification_still_rejects_a_multiple_link_file() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("original");
    fs::write(&path, b"bytes").unwrap();
    fs::hard_link(&path, temp.path().join("alias")).unwrap();
    let mut file = File::open(&path).unwrap();
    let mut checkpoints = 0;
    let result = verify_full_observed(
        &mut file,
        Expectation {
            bytes: 5,
            blake3: None,
        },
        &mut || {
            checkpoints += 1;
            Ok(())
        },
    );
    assert!(matches!(
        result,
        Err(Error::Filesystem(crate::fs_safety::Error::MultipleLinks(_)))
    ));
    assert_eq!(checkpoints, 1);
    assert_eq!(file.stream_position().unwrap(), 0);
}
