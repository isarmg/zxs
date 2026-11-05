use super::*;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};
use storage_contract::OperationId;

fn token(text: &str) -> Token {
    Token::new(text).unwrap()
}
fn identity(label: &str, role: Role) -> Identity {
    Identity {
        origin: Origin {
            pool_id: token("pool"),
            disk_id: token(label),
            slot_id: token(label),
            binding_generation: 1,
        },
        incarnation: Digest::body(label.as_bytes()),
        role,
    }
}
fn selection(path: &Path, id: &Identity) -> DiskSelection {
    DiskSelection {
        path: path.into(),
        identity: id.clone(),
        known_checkpoint: None,
    }
}
fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                visit(root, &entry.path(), out);
            } else {
                out.insert(
                    entry.path().strip_prefix(root).unwrap().into(),
                    fs::read(entry.path()).unwrap(),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(root, root, &mut out);
    out
}
fn review(
    disk: &IndependentDisk,
    operation: OperationId,
    base: Option<Digest>,
    file: &str,
    bytes: &[u8],
) -> ReviewedPublication {
    disk.review_file(FileWrite {
        operation,
        expected_base: base,
        history: &token("history"),
        file: &token(file),
        bytes,
    })
    .unwrap()
}

#[test]
fn primary_append_preserves_other_files_and_links_the_exact_previous_file_head() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("primary");
    let disk = IndependentDisk::initialize(&path, identity("primary", Role::Primary)).unwrap();
    let a = disk
        .execute(&review(
            &disk,
            OperationId::generate().unwrap(),
            None,
            "a",
            b"first",
        ))
        .unwrap();
    let b = disk
        .execute(&review(
            &disk,
            OperationId::generate().unwrap(),
            Some(a.publication()),
            "b",
            b"second",
        ))
        .unwrap();
    let c = disk
        .execute(&review(
            &disk,
            OperationId::generate().unwrap(),
            Some(b.publication()),
            "a",
            b"new first",
        ))
        .unwrap();
    let report = disk.report().unwrap();
    assert_eq!(report.sequence, 3);
    assert_eq!(report.copies.len(), 3);
    let raw = Disk::open(&path, &identity("primary", Role::Primary))
        .unwrap()
        .verify(None)
        .unwrap();
    let publication = &raw.publications[&c.publication()];
    let model::Object::Manifest(manifest) =
        codec::decode(&raw.bundle.objects[&publication.manifest]).unwrap()
    else {
        panic!()
    };
    let Authority::Primary { heads } = manifest.authority else {
        panic!()
    };
    assert_eq!(heads.len(), 2);
    let mut linked = false;
    for head in heads {
        let model::Object::Commit(commit) = codec::decode(&raw.bundle.objects[&head]).unwrap()
        else {
            panic!()
        };
        if commit.file_id == token("a") {
            assert_eq!(commit.parents.len(), 1);
            linked = true;
        } else {
            assert_eq!(commit.file_id, token("b"));
            assert!(commit.parents.is_empty());
        }
        assert_eq!(
            disk.read_content(head).unwrap(),
            if commit.file_id == token("a") {
                b"new first".as_slice()
            } else {
                b"second".as_slice()
            }
        );
    }
    assert!(linked);
}

#[test]
fn review_is_read_only_and_stale_or_wrong_target_cannot_publish() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("primary");
    let id = identity("primary", Role::Primary);
    let disk = IndependentDisk::initialize(&path, id.clone()).unwrap();
    let before = files(&path);
    let stale = review(&disk, OperationId::generate().unwrap(), None, "a", b"stale");
    assert_eq!(files(&path), before);
    let other = IndependentDisk::initialize(&temp.path().join("other"), id).unwrap();
    assert!(matches!(other.execute(&stale), Err(Error::Identity)));
    disk.execute(&review(
        &disk,
        OperationId::generate().unwrap(),
        None,
        "a",
        b"new",
    ))
    .unwrap();
    let saved = files(&path);
    assert!(matches!(disk.execute(&stale), Err(Error::StaleBase)));
    assert_eq!(files(&path), saved);
}

#[test]
fn initializer_durable_boundaries_resume_only_the_exact_identity() {
    use fault::Boundary::*;
    for boundary in [
        InitializationIntent,
        InitializationIdentity,
        InitializationDirectories,
        InitializationPublished,
    ] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("primary");
        let id = identity("primary", Role::Primary);
        fault::inject(boundary);
        assert!(
            IndependentDisk::initialize(&path, id.clone()).is_err(),
            "{boundary:?}"
        );
        let saved = files(&path);
        assert!(IndependentDisk::initialize(&path, identity("other", Role::Primary)).is_err());
        assert_eq!(files(&path), saved);
        if boundary != InitializationPublished {
            assert!(IndependentDisk::open(&selection(&path, &id)).is_err());
        }
        let disk = IndependentDisk::initialize(&path, id.clone()).unwrap();
        assert_eq!(disk.report().unwrap().sequence, 0);
        let saved = files(&path);
        IndependentDisk::initialize(&path, id).unwrap();
        assert_eq!(files(&path), saved);
    }
}

#[test]
fn unfinished_initialization_cannot_be_rebound_by_moving_its_directory() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("primary");
    let moved = temp.path().join("moved");
    let id = identity("primary", Role::Primary);
    fault::inject(fault::Boundary::InitializationIdentity);
    assert!(IndependentDisk::initialize(&path, id.clone()).is_err());
    fs::rename(&path, &moved).unwrap();
    let saved = files(&moved);
    assert!(matches!(
        IndependentDisk::initialize(&moved, id.clone()),
        Err(Error::Identity)
    ));
    assert_eq!(files(&moved), saved);
    fs::rename(&moved, &path).unwrap();
    IndependentDisk::initialize(&path, id.clone()).unwrap();
    fs::rename(&path, &moved).unwrap();
    IndependentDisk::open(&selection(&moved, &id)).unwrap();
}

#[test]
fn publication_faults_cold_recover_without_duplicate_or_false_success() {
    use fault::Boundary::*;
    for boundary in [Intent, Material, Prepared, Published] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("primary");
        let id = identity("primary", Role::Primary);
        let operation = OperationId::generate().unwrap();
        let disk = IndependentDisk::initialize(&path, id.clone()).unwrap();
        let prepared = review(&disk, operation, None, "a", b"retained");
        let digest = prepared.request_digest();
        fault::inject(boundary);
        assert!(disk.execute(&prepared).is_err(), "{boundary:?}");
        drop(prepared);
        drop(disk);
        let disk = IndependentDisk::open(&selection(&path, &id)).unwrap();
        let before = files(&path);
        let status = disk.operation_status(operation).unwrap();
        assert_eq!(files(&path), before);
        match boundary {
            Intent | Material => {
                assert!(matches!(status, OperationStatus::Intent { .. }));
                assert!(disk.resume(operation, digest).is_err());
                assert_eq!(files(&path), before);
                let repeated = review(&disk, operation, None, "a", b"retained");
                assert_eq!(repeated.request_digest(), digest);
                disk.execute(&repeated).unwrap();
            }
            Prepared => {
                assert!(matches!(status, OperationStatus::Prepared { .. }));
                disk.resume(operation, digest).unwrap();
            }
            Published => {
                assert!(matches!(status, OperationStatus::Published { .. }));
                disk.resume(operation, digest).unwrap();
            }
            _ => unreachable!(),
        }
        let saved = files(&path);
        assert!(matches!(
            disk.resume(operation, Digest::body(b"wrong")),
            Err(Error::OperationConflict)
        ));
        assert_eq!(files(&path), saved);
        assert_eq!(disk.resume(operation, digest).unwrap().sequence(), 1);
        assert_eq!(disk.report().unwrap().sequence, 1);
    }
}

#[test]
fn retained_orphan_bytes_are_charged_before_another_operation_is_created() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("primary");
    let disk = IndependentDisk::initialize(&path, identity("primary", Role::Primary)).unwrap();
    let retained = fs::File::create(path.join("retained-residue")).unwrap();
    retained.set_len(49 << 20).unwrap();
    retained.sync_all().unwrap();
    let request = review(&disk, OperationId::generate().unwrap(), None, "a", b"new");
    assert!(matches!(disk.execute(&request), Err(Error::Budget)));
    assert_eq!(fs::read_dir(path.join("operations")).unwrap().count(), 0);
    assert_eq!(retained.metadata().unwrap().len(), 49 << 20);
    assert_eq!(disk.report().unwrap().sequence, 0);
}

#[test]
fn multi_source_backup_and_deleted_sqlite_rebuild_never_modify_the_primary_disks() {
    let temp = tempfile::tempdir().unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut sources = Vec::new();
    let mut saved = Vec::new();
    for label in ["a", "b"] {
        let path = temp.path().join(label);
        let id = identity(label, Role::Primary);
        let disk = IndependentDisk::initialize(&path, id.clone()).unwrap();
        disk.execute(&review(
            &disk,
            OperationId::generate().unwrap(),
            None,
            "same_name",
            label.as_bytes(),
        ))
        .unwrap();
        saved.push(files(&path));
        sources.push(selection(&path, &id));
    }
    let path = temp.path().join("backup");
    let id = identity("backup", Role::Secondary);
    let disk = IndependentDisk::initialize(&path, id.clone()).unwrap();
    let review = disk
        .review_backup(OperationId::generate().unwrap(), None, &sources)
        .unwrap();
    disk.execute(&review).unwrap();
    for (source, saved) in sources.iter().zip(saved) {
        assert_eq!(files(&source.path), saved);
    }
    let before = files(&path);
    assert!(rebuild_projection(&path.join("cache"), &[selection(&path, &id)]).is_err());
    assert_eq!(files(&path), before);
    let moved = temp.path().join("portable");
    drop(disk);
    fs::rename(&path, &moved).unwrap();
    for source in sources {
        fs::remove_dir_all(source.path).unwrap();
    }
    let selections = [selection(&moved, &id)];
    let cache = temp.path().join("system-cache");
    let rows = rebuild_projection(&cache, &selections).unwrap();
    assert_eq!(rows.len(), 2);
    fs::remove_file(cache.join("cache.db")).unwrap();
    assert_eq!(rebuild_projection(&cache, &selections).unwrap(), rows);
    assert_eq!(files(&moved), before);
}

#[test]
fn projection_refuses_an_unrelated_existing_cache_directory() {
    let temp = tempfile::tempdir().unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let root = temp.path().join("cache");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(root.join("cache.db"), b"old engine index").unwrap();
    let before = files(&root);
    assert!(matches!(
        rebuild_projection(&root, &[]),
        Err(Error::Invalid("new projection directory must be empty"))
    ));
    assert_eq!(files(&root), before);
}

#[test]
fn nested_source_and_backup_targets_are_rejected_without_any_write() {
    for target_inside in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let (source_path, target_path) = if target_inside {
            (
                temp.path().join("source"),
                temp.path().join("source/target"),
            )
        } else {
            (
                temp.path().join("target/source"),
                temp.path().join("target"),
            )
        };
        let source_id = identity("source", Role::Primary);
        let target_id = identity("target", Role::Secondary);
        if !target_inside {
            IndependentDisk::initialize(&target_path, target_id.clone()).unwrap();
        }
        let source = IndependentDisk::initialize(&source_path, source_id.clone()).unwrap();
        source
            .execute(&review(
                &source,
                OperationId::generate().unwrap(),
                None,
                "file",
                b"body",
            ))
            .unwrap();
        let target = if target_inside {
            IndependentDisk::initialize(&target_path, target_id.clone()).unwrap()
        } else {
            IndependentDisk::open(&selection(&target_path, &target_id)).unwrap()
        };
        let before = files(temp.path());
        assert!(
            target
                .review_backup(
                    OperationId::generate().unwrap(),
                    None,
                    &[selection(&source_path, &source_id)]
                )
                .is_err()
        );
        assert_eq!(files(temp.path()), before);
    }
}

#[test]
fn identical_source_bytes_at_another_path_change_the_exact_review_digest() {
    fn copy(source: &Path, destination: &Path) {
        use std::os::unix::fs::PermissionsExt;
        fs::create_dir(destination).unwrap();
        fs::set_permissions(destination, fs::Permissions::from_mode(0o700)).unwrap();
        for entry in fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let target = destination.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), target).unwrap();
            }
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let source_path = temp.path().join("source");
    let id = identity("source", Role::Primary);
    let source = IndependentDisk::initialize(&source_path, id.clone()).unwrap();
    source
        .execute(&review(
            &source,
            OperationId::generate().unwrap(),
            None,
            "file",
            b"body",
        ))
        .unwrap();
    let clone = temp.path().join("clone");
    copy(&source_path, &clone);
    let target = IndependentDisk::initialize(
        &temp.path().join("target"),
        identity("target", Role::Secondary),
    )
    .unwrap();
    let operation = OperationId::generate().unwrap();
    let a = target
        .review_backup(operation, None, &[selection(&source_path, &id)])
        .unwrap();
    let b = target
        .review_backup(operation, None, &[selection(&clone, &id)])
        .unwrap();
    assert_ne!(a.request_digest(), b.request_digest());
    target.execute(&a).unwrap();
    assert!(matches!(target.execute(&b), Err(Error::OperationConflict)));
}

#[test]
fn initialization_lock_only_and_temporary_only_residue_remain_blocked_and_untouched() {
    use std::os::unix::fs::PermissionsExt;
    for residue in ["writer.lock", ".state-atomic-interrupted.tmp"] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("disk");
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(path.join(residue), b"retained").unwrap();
        let before = files(&path);
        assert!(IndependentDisk::initialize(&path, identity("p", Role::Primary)).is_err());
        assert_eq!(files(&path), before);
    }
}

#[test]
fn physical_containment_uses_ancestor_handles_and_refuses_unproved_mount_aliases() {
    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path().join("parent");
    let child = parent.join("child");
    let sibling = temp.path().join("sibling");
    fs::create_dir(&parent).unwrap();
    fs::create_dir(&child).unwrap();
    fs::create_dir(&sibling).unwrap();
    let parent = location::Location::open(&parent).unwrap();
    let child = location::Location::open(&child).unwrap();
    let sibling = location::Location::open(&sibling).unwrap();
    assert!(parent.overlaps(&child).unwrap());
    assert!(child.overlaps(&parent).unwrap());
    assert!(!sibling.overlaps(&child).unwrap());
    assert!(matches!(
        child.descends_from(
            parent.identity(),
            location::MountRoot::from(platform_io::fs_safety::linux::FileIdentity {
                device: parent.identity().device,
                inode: parent.identity().inode.checked_add(1).unwrap()
            })
        ),
        Err(Error::Invalid(
            "same filesystem selected through different physical mount roots"
        ))
    ));
}

#[test]
#[ignore = "requires isolated real fsync failure interposer; run publication-io.sh"]
fn published_replay_repeats_the_real_file_and_directory_barriers() {
    use std::os::unix::fs::MetadataExt;
    let root =
        PathBuf::from(std::env::var_os("ZXS_AUTHORITY_TEST_DISK").expect("isolated test disk"));
    let trigger = PathBuf::from(std::env::var_os("ZXS_ATOMIC_TRIGGER").expect("fault trigger"));
    let recovery =
        PathBuf::from(std::env::var_os("ZXS_ATOMIC_RECOVERY_TRIGGER").expect("recovery trigger"));
    let log = PathBuf::from(std::env::var_os("ZXS_ATOMIC_LOG").expect("event log"));
    let identity = identity("primary", Role::Primary);
    let operation = OperationId::generate().unwrap();
    let disk = IndependentDisk::initialize(&root, identity.clone()).unwrap();
    let reviewed = review(&disk, operation, None, "file", b"complete body");
    let digest = reviewed.request_digest();
    fs::write(&trigger, b"active").unwrap();
    assert!(
        matches!(disk.execute(&reviewed), Err(Error::Filesystem(platform_io::fs_safety::Error::PublishedDurabilityUnknown(ref error))) if error.raw_os_error() == Some(5))
    );
    let decision = root.join("publications/00000000000000000001.json");
    let metadata = fs::metadata(&decision).unwrap();
    let decided_bytes = fs::read(&decision).unwrap();
    assert_eq!(
        fs::read_to_string(&log).unwrap(),
        "file-sync\ndirectory-sync\n"
    );
    fs::write(&log, b"").unwrap();
    let OperationStatus::Published { receipt } = disk.operation_status(operation).unwrap() else {
        panic!("real decision must be observable")
    };
    assert_eq!(fs::read(&log).unwrap(), b"");
    let replay_error = disk.resume(operation, digest).unwrap_err();
    assert!(
        matches!(replay_error, Error::Filesystem(platform_io::fs_safety::Error::Io(ref error)) if error.raw_os_error() == Some(5)),
        "{replay_error:?}"
    );
    assert_eq!(
        fs::read_to_string(&log).unwrap(),
        "published-file-sync\ndirectory-sync\n"
    );
    fs::write(&log, b"").unwrap();
    let repeated_error = disk.execute(&reviewed).unwrap_err();
    assert!(
        matches!(repeated_error, Error::Filesystem(platform_io::fs_safety::Error::Io(ref error)) if error.raw_os_error() == Some(5)),
        "{repeated_error:?}"
    );
    assert_eq!(
        fs::read_to_string(&log).unwrap(),
        "published-file-sync\ndirectory-sync\n"
    );
    drop(disk);
    let disk = IndependentDisk::open(&selection(&root, &identity)).unwrap();
    fs::write(&log, b"").unwrap();
    fs::write(&recovery, b"allow").unwrap();
    assert_eq!(disk.resume(operation, digest).unwrap(), receipt);
    assert_eq!(
        fs::read_to_string(&log).unwrap(),
        "published-file-sync\ndirectory-sync\n"
    );
    let now = fs::metadata(&decision).unwrap();
    assert_eq!((metadata.dev(), metadata.ino()), (now.dev(), now.ino()));
    assert_eq!(fs::read(&decision).unwrap(), decided_bytes);
    assert_eq!(fs::read_dir(root.join("publications")).unwrap().count(), 1);
    println!("ZXS_INDEPENDENT_DURABILITY_REPLAY_COMPLETE");
}

#[test]
#[ignore = "requires isolated replay mutation interposer; run replay-mutation-io.sh"]
fn published_replay_rejects_same_inode_changes_and_lost_final_chain() {
    use std::os::unix::fs::MetadataExt;
    let root = PathBuf::from(std::env::var_os("ZXS_AUTHORITY_TEST_DISK").expect("isolated disk"));
    let trigger =
        PathBuf::from(std::env::var_os("ZXS_AUTHORITY_MUTATION_TRIGGER").expect("trigger"));
    let selected =
        PathBuf::from(std::env::var_os("ZXS_AUTHORITY_MUTATION_SELECTION").expect("selection"));
    let log = PathBuf::from(std::env::var_os("ZXS_AUTHORITY_MUTATION_LOG").expect("log"));
    let kind = std::env::var("ZXS_AUTHORITY_MUTATION_KIND").unwrap();
    let method = std::env::var("ZXS_AUTHORITY_REPLAY_METHOD").unwrap();
    let id = identity("primary", Role::Primary);
    let disk = IndependentDisk::initialize(&root, id.clone()).unwrap();
    let operation = OperationId::generate().unwrap();
    let request = review(&disk, operation, None, "file", b"retained FULL body");
    let receipt = disk.execute(&request).unwrap();
    let verified = Disk::open(&root, &id).unwrap().verify(None).unwrap();
    let publication = &verified.publications[&receipt.publication()];
    let first_decision = root.join("publications/00000000000000000001.json");
    let mutation = match kind.as_str() {
        "decision" => first_decision.clone(),
        "manifest" => root.join(format!("objects/{}.json", publication.manifest)),
        "body" | "missing-body" => root.join(format!("bodies/{}.bin", verified.rows[0].commit)),
        "rollback" => {
            disk.execute(&review(
                &disk,
                OperationId::generate().unwrap(),
                Some(receipt.publication()),
                "other-file",
                b"newer checkpoint",
            ))
            .unwrap();
            root.join("publications/00000000000000000002.json")
        }
        _ => panic!("unknown mutation fixture"),
    };
    let metadata = fs::metadata(&mutation).unwrap();
    let original = fs::read(&mutation).unwrap();
    fs::write(&selected, mutation.as_os_str().as_encoded_bytes()).unwrap();
    fs::write(&trigger, b"mutate only this replay").unwrap();
    let result = match method.as_str() {
        "execute" => disk.execute(&request),
        "resume" => disk.resume(operation, request.request_digest()),
        _ => panic!("unknown replay fixture"),
    };
    match kind.as_str() {
        "decision" | "manifest" | "body" => {
            assert!(matches!(result, Err(Error::Corrupt(_))), "{result:?}");
            let now = fs::metadata(&mutation).unwrap();
            assert_eq!(
                (metadata.dev(), metadata.ino(), metadata.len()),
                (now.dev(), now.ino(), now.len())
            );
            assert_ne!(fs::read(&mutation).unwrap(), original);
            assert_eq!(fs::read_to_string(&log).unwrap(), "same-inode-rewrite\n");
        }
        "missing-body" => {
            assert!(
                matches!(result, Err(Error::RequiredBodyMissing(_))),
                "{result:?}"
            );
            assert_eq!(fs::read_to_string(&log).unwrap(), "removed-selected-file\n");
        }
        "rollback" => {
            assert!(matches!(result, Err(Error::StaleBase)), "{result:?}");
            assert_eq!(fs::read_to_string(&log).unwrap(), "removed-selected-file\n");
        }
        _ => unreachable!(),
    }
    println!("ZXS_INDEPENDENT_REPLAY_MUTATION_REJECTED");
}

#[test]
fn projection_parent_replacement_cannot_create_a_cache_in_a_verified_source() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let source_path = temp.path().join("source");
    let parent = temp.path().join("cache-parent");
    let id = identity("source", Role::Primary);
    let source = Disk::initialize(&source_path, id).unwrap();
    fs::create_dir(&parent).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
    let mut destination = projection::Destination::open(&parent.join("projection")).unwrap();
    destination.admit(&source).unwrap();
    fs::rename(&parent, temp.path().join("old-parent")).unwrap();
    fs::rename(&source_path, &parent).unwrap();
    let before = files(temp.path());
    assert!(destination.open_for_write().is_err());
    assert_eq!(files(temp.path()), before);
}

#[test]
fn identical_content_has_independent_full_copies_per_history_file_and_version() {
    use std::os::unix::fs::MetadataExt;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("primary");
    let id = identity("primary", Role::Primary);
    let disk = IndependentDisk::initialize(&path, id.clone()).unwrap();
    let mut base = None;
    for (history, file) in [("h1", "f1"), ("h1", "f2"), ("h2", "f1"), ("h1", "f1")] {
        let reviewed = disk
            .review_file(FileWrite {
                operation: OperationId::generate().unwrap(),
                expected_base: base,
                history: &token(history),
                file: &token(file),
                bytes: b"identical full contents",
            })
            .unwrap();
        base = Some(disk.execute(&reviewed).unwrap().publication());
    }
    let report = disk.report().unwrap();
    assert_eq!(report.copies.len(), 4);
    let mut physical = std::collections::BTreeSet::new();
    for row in &report.copies {
        assert_eq!(row.body, Digest::body(b"identical full contents"));
        let metadata =
            fs::metadata(path.join("bodies").join(format!("{}.bin", row.commit))).unwrap();
        assert_eq!(metadata.nlink(), 1);
        assert!(physical.insert((metadata.dev(), metadata.ino())));
    }
    let backup_path = temp.path().join("backup");
    let backup =
        IndependentDisk::initialize(&backup_path, identity("backup", Role::Secondary)).unwrap();
    backup
        .execute(
            &backup
                .review_backup(
                    OperationId::generate().unwrap(),
                    None,
                    &[selection(&path, &id)],
                )
                .unwrap(),
        )
        .unwrap();
    for row in &report.copies {
        let metadata = fs::metadata(
            backup_path
                .join("bodies")
                .join(format!("{}.bin", row.commit)),
        )
        .unwrap();
        assert_eq!(metadata.nlink(), 1);
        assert!(physical.insert((metadata.dev(), metadata.ino())));
    }
    let missing = report.copies[0].commit;
    fs::remove_file(path.join("bodies").join(format!("{missing}.bin"))).unwrap();
    let after = disk.report().unwrap();
    assert_eq!(
        after
            .copies
            .iter()
            .filter(|row| row.availability == Availability::TemporarilyUnavailable)
            .count(),
        1
    );
    assert!(after.copies.iter().any(
        |row| row.commit == missing && row.availability == Availability::TemporarilyUnavailable
    ));
    assert!(
        backup
            .report()
            .unwrap()
            .copies
            .iter()
            .all(|row| row.availability == Availability::Available)
    );
    let corrupt = report.copies[1].commit;
    fs::write(path.join("bodies").join(format!("{corrupt}.bin")), b"bad").unwrap();
    assert!(matches!(disk.report(), Err(Error::Corrupt(key)) if key == corrupt.to_string()));
    let reopened = IndependentDisk::open(&selection(&path, &id)).unwrap();
    assert!(
        matches!(reopened.read_content(missing), Err(Error::ContentUnavailable(key)) if key == missing)
    );
    assert!(
        matches!(reopened.read_content(corrupt), Err(Error::Corrupt(key)) if key == corrupt.to_string())
    );
    let unknown = Digest::body(b"not a published commit");
    assert!(matches!(reopened.read_content(unknown), Err(Error::Missing(key)) if key == unknown));
    assert!(matches!(reopened.review_file(FileWrite {
        operation: OperationId::generate().unwrap(),
        expected_base: base,
        history: &token("new-history"),
        file: &token("new-file"),
        bytes: b"new body",
    }), Err(Error::Corrupt(key)) if key == corrupt.to_string()));
    let before = files(temp.path());
    assert!(matches!(backup.review_backup(
        OperationId::generate().unwrap(), backup.report().unwrap().checkpoint,
        &[selection(&path, &id)],
    ), Err(Error::Corrupt(key)) if key == corrupt.to_string()));
    assert_eq!(files(temp.path()), before);
    for row in report.copies.iter().skip(2) {
        assert_eq!(
            reopened.read_content(row.commit).unwrap(),
            b"identical full contents"
        );
        assert_eq!(
            fs::read(path.join("bodies").join(format!("{}.bin", row.commit))).unwrap(),
            b"identical full contents"
        );
    }
    assert!(
        backup
            .report()
            .unwrap()
            .copies
            .iter()
            .all(|row| row.availability == Availability::Available)
    );
}

#[test]
fn held_selection_keeps_its_known_checkpoint_across_read_write_and_recovery() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("primary");
    let id = identity("primary", Role::Primary);
    let disk = IndependentDisk::initialize(&path, id.clone()).unwrap();
    let first = disk
        .execute(&review(
            &disk,
            OperationId::generate().unwrap(),
            None,
            "a",
            b"first",
        ))
        .unwrap();
    let operation = OperationId::generate().unwrap();
    let second_review = review(&disk, operation, Some(first.publication()), "b", b"second");
    let second = disk.execute(&second_review).unwrap();
    let mut pinned = selection(&path, &id);
    pinned.known_checkpoint = Some(second.publication());
    let held = IndependentDisk::open(&pinned).unwrap();
    let first_copy = held
        .report()
        .unwrap()
        .copies
        .into_iter()
        .find(|row| row.file_id == token("a"))
        .unwrap()
        .commit;
    fs::remove_file(path.join("publications/00000000000000000002.json")).unwrap();
    let before = files(temp.path());
    assert!(matches!(held.report(), Err(Error::StaleBase)));
    assert!(matches!(
        held.read_content(first_copy),
        Err(Error::StaleBase)
    ));
    assert!(matches!(
        held.operation_status(operation),
        Err(Error::StaleBase)
    ));
    assert!(matches!(
        held.execute(&second_review),
        Err(Error::StaleBase)
    ));
    assert!(matches!(
        held.resume(operation, second_review.request_digest()),
        Err(Error::StaleBase)
    ));
    assert_eq!(files(temp.path()), before);
}

#[test]
fn projection_absent_child_never_adopts_an_appeared_source_or_source_subdirectory() {
    use std::os::unix::fs::PermissionsExt;
    for whole_source in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let source_path = temp.path().join("source");
        let source = Disk::initialize(&source_path, identity("source", Role::Primary)).unwrap();
        let replacement = if whole_source {
            source_path.clone()
        } else {
            let child = source_path.join("retained-projection");
            fs::create_dir(&child).unwrap();
            fs::set_permissions(&child, fs::Permissions::from_mode(0o700)).unwrap();
            child
        };
        fs::write(
            replacement.join("projection-kind.json"),
            br#"{"format":"independent-disk-projection"}"#,
        )
        .unwrap();
        fs::set_permissions(
            replacement.join("projection-kind.json"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        let target = temp.path().join("projection");
        let mut destination = projection::Destination::open(&target).unwrap();
        destination.admit(&source).unwrap();
        source.verify(None).unwrap();
        fs::rename(&replacement, &target).unwrap();
        let before = files(temp.path());
        assert!(
            matches!(destination.open_for_write(), Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists)
        );
        assert_eq!(files(temp.path()), before);
        assert!(target.join("projection-kind.json").exists());
        assert!(!target.join("cache.db").exists());
    }
}

#[test]
fn projection_rechecks_the_actual_child_against_retained_source_handles() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = temp.path().join("source");
    let source = Disk::initialize(&path, identity("source", Role::Primary)).unwrap();
    let selected = source.location().unwrap();
    let target = temp.path().join("appeared");
    fs::rename(&path, &target).unwrap();
    let actual = location::Location::open(&target).unwrap();
    assert!(actual.overlaps(&selected.refreshed().unwrap()).unwrap());
}
