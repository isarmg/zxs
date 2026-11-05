use super::*;
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
};

fn token(s: &str) -> Token {
    Token::new(s).unwrap()
}
fn identity(label: &str, role: Role) -> Identity {
    Identity {
        origin: Origin {
            pool_id: token("original_pool"),
            disk_id: token(label),
            slot_id: token(label),
            binding_generation: 1,
        },
        incarnation: Digest::body(label.as_bytes()),
        role,
    }
}
fn commit(owner: &Identity, bytes: &[u8]) -> Commit {
    Commit {
        owner: owner.clone(),
        history_id: token("shared_history_name"),
        file_id: token("shared_file_name"),
        parents: vec![],
        body: Body {
            digest: Digest::body(bytes),
            bytes: bytes.len() as u64,
        },
    }
}
fn primary_request(disk: &Disk, operation: &str, bytes: &[u8]) -> PublicationRequest {
    let mut bundle = Bundle::default();
    let head = bundle
        .add_commit(commit(disk.identity(), bytes), Some(bytes))
        .unwrap();
    PublicationRequest::new(
        &disk.verify(None).unwrap(),
        token(operation),
        Authority::Primary { heads: vec![head] },
        bundle,
    )
    .unwrap()
}
fn publish(disk: &Disk, request: &PublicationRequest) -> DiskPublicationReceipt {
    let writer = disk.writer().unwrap();
    writer.begin(request).unwrap();
    let prepared = writer.prepare(request).unwrap();
    writer.publish(&prepared).unwrap()
}
fn primary(root: &Path, name: &str, bytes: &[u8]) -> Disk {
    let disk = Disk::initialize(&root.join(name), identity(name, Role::Primary)).unwrap();
    publish(&disk, &primary_request(&disk, "first", bytes));
    disk
}
fn backup(root: &Path, name: &str, role: Role, sources: &[VerifiedDisk]) -> Disk {
    let disk = Disk::initialize(&root.join(name), identity(name, role)).unwrap();
    let mut bundle = Bundle::default();
    for source in sources {
        bundle.extend(&source.portable_bundle()).unwrap();
    }
    let sources = sources
        .iter()
        .map(|s| s.source_checkpoint().unwrap())
        .collect();
    let authority = match role {
        Role::Secondary => Authority::Secondary { sources },
        Role::Tertiary => Authority::Tertiary { sources },
        Role::Primary => unreachable!(),
    };
    let request = PublicationRequest::new(
        &disk.verify(None).unwrap(),
        token("replicate"),
        authority,
        bundle,
    )
    .unwrap();
    publish(&disk, &request);
    disk
}
fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, path: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                visit(root, &entry.path(), out);
            } else {
                out.insert(
                    entry.path().strip_prefix(root).unwrap().to_owned(),
                    fs::read(entry.path()).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}
fn copy_tree(source: &Path, target: &Path) {
    fs::create_dir(target).unwrap();
    fs::set_permissions(target, fs::Permissions::from_mode(0o700)).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let dst = target.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &dst);
        } else {
            fs::copy(entry.path(), dst).unwrap();
        }
    }
}

#[test]
fn primary_never_records_downstream_and_backup_is_independently_portable() {
    let temp = tempfile::tempdir().unwrap();
    let source = primary(temp.path(), "primary_a", b"kept content");
    let original = files(&temp.path().join("primary_a"));
    let backup = backup(
        temp.path(),
        "backup_b",
        Role::Secondary,
        &[source.verify(None).unwrap()],
    );
    assert_eq!(files(&temp.path().join("primary_a")), original);
    assert!(
        original
            .values()
            .all(|raw| !String::from_utf8_lossy(raw).contains("backup_b"))
    );
    let pin = backup.verify(None).unwrap().checkpoint();
    drop(source);
    drop(backup);
    fs::remove_dir_all(temp.path().join("primary_a")).unwrap();
    fs::rename(
        temp.path().join("backup_b"),
        temp.path().join("new_host_mount"),
    )
    .unwrap();
    let disk = Disk::open(
        &temp.path().join("new_host_mount"),
        &identity("backup_b", Role::Secondary),
    )
    .unwrap();
    let read = disk.verify(pin).unwrap();
    assert_eq!(read.copies().len(), 1);
    assert_eq!(read.copies()[0].owner, identity("primary_a", Role::Primary));
    assert_eq!(read.copies()[0].availability, Availability::Available);
    assert!(read.receipt(&token("replicate")).is_some());
}

#[test]
fn tertiary_preserves_multiple_original_primary_identities_and_missing_bodies() {
    let temp = tempfile::tempdir().unwrap();
    let a = primary(temp.path(), "a", b"a");
    let b = primary(temp.path(), "b", b"b");
    let second = backup(
        temp.path(),
        "second",
        Role::Secondary,
        &[a.verify(None).unwrap(), b.verify(None).unwrap()],
    );
    fs::remove_file(temp.path().join("second/bodies").join(format!(
            "{}.bin",
            model::Object::Commit(commit(&identity("b", Role::Primary), b"b"))
                .digest()
                .unwrap()
        )))
    .unwrap();
    let third = backup(
        temp.path(),
        "third",
        Role::Tertiary,
        &[second.verify(None).unwrap()],
    );
    for name in ["a", "b", "second"] {
        fs::remove_dir_all(temp.path().join(name)).unwrap();
    }
    let rows = third.verify(None).unwrap().copies().to_vec();
    assert_eq!(rows.len(), 2);
    let rows: BTreeMap<_, _> = rows
        .iter()
        .map(|r| (r.owner.origin.disk_id.as_str(), r.availability))
        .collect();
    assert_eq!(rows["a"], Availability::Available);
    assert_eq!(rows["b"], Availability::TemporarilyUnavailable);
}

#[test]
fn source_binding_generation_incarnation_and_level_are_exact() {
    let temp = tempfile::tempdir().unwrap();
    let a = primary(temp.path(), "a", b"a");
    let verified = a.verify(None).unwrap();
    let target = Disk::initialize(
        &temp.path().join("target"),
        identity("target", Role::Secondary),
    )
    .unwrap();
    for alteration in 0..3 {
        let mut source = verified.source_checkpoint().unwrap();
        match alteration {
            0 => source.identity.origin.binding_generation += 1,
            1 => source.identity.incarnation = Digest::body(b"replacement"),
            _ => source.identity.role = Role::Tertiary,
        }
        assert!(
            PublicationRequest::new(
                &target.verify(None).unwrap(),
                token("bad"),
                Authority::Secondary {
                    sources: vec![source]
                },
                verified.portable_bundle()
            )
            .is_err()
        );
    }
    let mut wrong = a.identity().clone();
    wrong.incarnation = Digest::body(b"different");
    assert!(matches!(
        Disk::open(&temp.path().join("a"), &wrong),
        Err(Error::Identity)
    ));
}

#[test]
fn prepared_source_has_no_publication_to_export_or_admit() {
    let temp = tempfile::tempdir().unwrap();
    let source = Disk::initialize(
        &temp.path().join("source"),
        identity("source", Role::Primary),
    )
    .unwrap();
    let request = primary_request(&source, "not_published", b"retained");
    let writer = source.writer().unwrap();
    let _prepared = writer.prepare(&request).unwrap();
    drop(writer);
    let candidate = request.publication_digest().unwrap();
    assert!(
        !temp
            .path()
            .join("source/objects")
            .join(format!("{candidate}.json"))
            .exists()
    );
    assert_eq!(source.verify(None).unwrap().checkpoint(), None);
    assert!(source.verify(None).unwrap().source_checkpoint().is_err());
    let target = Disk::initialize(
        &temp.path().join("target"),
        identity("target", Role::Secondary),
    )
    .unwrap();
    // Even copying EVERY staged object does not create a source publication.
    for entry in fs::read_dir(temp.path().join("source/objects")).unwrap() {
        let entry = entry.unwrap();
        fs::copy(
            entry.path(),
            temp.path().join("target/objects").join(entry.file_name()),
        )
        .unwrap();
    }
    let result = PublicationRequest::new(
        &target.verify(None).unwrap(),
        token("replicate"),
        Authority::Secondary {
            sources: vec![SourceCheckpoint {
                identity: source.identity().clone(),
                publication: candidate,
            }],
        },
        Bundle::default(),
    );
    assert!(matches!(result, Err(Error::Missing(hash)) if hash == candidate));
    assert_eq!(target.verify(None).unwrap().checkpoint(), None);
}

#[test]
fn moved_unpublished_execution_cannot_resume_but_restoring_original_path_can() {
    let temp = tempfile::tempdir().unwrap();
    let old = temp.path().join("original");
    let moved = temp.path().join("moved");
    let id = identity("p", Role::Primary);
    let disk = Disk::initialize(&old, id.clone()).unwrap();
    let request = primary_request(&disk, "pending", b"body");
    disk.writer().unwrap().prepare(&request).unwrap();
    drop(disk);
    fs::rename(&old, &moved).unwrap();
    let disk = Disk::open(&moved, &id).unwrap();
    let before = files(&moved);
    assert!(matches!(
        disk.writer().unwrap().resume_prepared(&token("pending")),
        Err(Error::Identity)
    ));
    assert_eq!(files(&moved), before);
    drop(disk);
    fs::rename(&moved, &old).unwrap();
    let disk = Disk::open(&old, &id).unwrap();
    let writer = disk.writer().unwrap();
    let prepared = writer.resume_prepared(&token("pending")).unwrap();
    writer.publish(&prepared).unwrap();
    assert_eq!(disk.verify(None).unwrap().sequence(), 1);
}

#[test]
fn reviewed_target_cannot_be_substituted_by_a_logically_identical_copy() {
    let temp = tempfile::tempdir().unwrap();
    let original = temp.path().join("original");
    let disk = Disk::initialize(&original, identity("p", Role::Primary)).unwrap();
    let request = primary_request(&disk, "write", b"body");
    copy_tree(&original, &temp.path().join("clone"));
    let clone = Disk::open(&temp.path().join("clone"), disk.identity()).unwrap();
    let writer = clone.writer().unwrap();
    let before = files(&temp.path().join("clone"));
    assert!(matches!(writer.begin(&request), Err(Error::Identity)));
    assert_eq!(files(&temp.path().join("clone")), before);
}

#[test]
fn held_root_replacement_is_rejected_before_publication() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("disk");
    let old = temp.path().join("renamed");
    let disk = Disk::initialize(&root, identity("p", Role::Primary)).unwrap();
    let request = primary_request(&disk, "write", b"body");
    let writer = disk.writer().unwrap();
    let prepared = writer.prepare(&request).unwrap();
    fs::rename(&root, &old).unwrap();
    copy_tree(&old, &root);
    let before = files(&root);
    let old_before = files(&old);
    assert!(writer.publish(&prepared).is_err());
    assert_eq!(files(&root), before);
    assert_eq!(files(&old), old_before);
}

#[test]
fn operation_id_is_permanent_exact_binding_and_prepared_base_can_become_stale() {
    let temp = tempfile::tempdir().unwrap();
    let disk = Disk::initialize(&temp.path().join("disk"), identity("p", Role::Primary)).unwrap();
    let first = primary_request(&disk, "first", b"one");
    let conflict = primary_request(&disk, "first", b"different");
    let second = primary_request(&disk, "second", b"two");
    let writer = disk.writer().unwrap();
    writer.begin(&first).unwrap();
    let before = files(&temp.path().join("disk"));
    assert!(matches!(
        writer.begin(&conflict),
        Err(Error::OperationConflict)
    ));
    assert_eq!(files(&temp.path().join("disk")), before);
    let prepared_first = writer.prepare(&first).unwrap();
    let prepared_second = writer.prepare(&second).unwrap();
    let receipt = writer.publish(&prepared_second).unwrap();
    assert_eq!(writer.publish(&prepared_second).unwrap(), receipt);
    assert!(matches!(
        writer.publish(&prepared_first),
        Err(Error::StaleBase)
    ));
    assert!(matches!(
        writer.resume_prepared(&token("first")),
        Err(Error::StaleBase)
    ));
    assert!(
        disk.verify(None)
            .unwrap()
            .receipt(&token("first"))
            .is_none()
    );
}

#[test]
fn missing_body_is_unavailable_while_corruption_links_and_missing_metadata_reject() {
    let temp = tempfile::tempdir().unwrap();
    let disk = primary(temp.path(), "p", b"body");
    let body = temp.path().join("p/bodies").join(format!(
        "{}.bin",
        model::Object::Commit(commit(&identity("p", Role::Primary), b"body"))
            .digest()
            .unwrap()
    ));
    fs::write(&body, b"evil").unwrap();
    assert!(matches!(disk.verify(None), Err(Error::Corrupt(_))));
    fs::remove_file(&body).unwrap();
    assert_eq!(
        disk.verify(None).unwrap().copies()[0].availability,
        Availability::TemporarilyUnavailable
    );
    let outside = temp.path().join("outside");
    fs::write(&outside, b"body").unwrap();
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o600)).unwrap();
    symlink(&outside, &body).unwrap();
    assert!(disk.verify(None).is_err());
    fs::remove_file(&body).unwrap();
    fs::hard_link(&outside, &body).unwrap();
    assert!(disk.verify(None).is_err());
    fs::remove_file(&body).unwrap();
    let object = fs::read_dir(temp.path().join("p/objects"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::remove_file(object).unwrap();
    assert!(disk.verify(None).is_err());
}

#[test]
fn external_checkpoint_rejects_rollback_but_first_observation_does_not_claim_latest() {
    let temp = tempfile::tempdir().unwrap();
    let disk = primary(temp.path(), "p", b"old");
    copy_tree(&temp.path().join("p"), &temp.path().join("old_copy"));
    let newer = publish(&disk, &primary_request(&disk, "second", b"new"));
    let old = Disk::open(&temp.path().join("old_copy"), disk.identity()).unwrap();
    assert_eq!(old.verify(None).unwrap().sequence(), 1);
    assert!(matches!(
        old.verify(Some(newer.publication())),
        Err(Error::StaleBase)
    ));
}

#[test]
fn sqlite_can_be_deleted_or_corrupted_and_rebuilt_without_touching_disks() {
    let temp = tempfile::tempdir().unwrap();
    let disk = primary(temp.path(), "p", b"body");
    let verified = disk.verify(None).unwrap();
    let before = files(&temp.path().join("p"));
    let cache_dir = temp.path().join("system_cache");
    rebuild_cache(&cache_dir, std::slice::from_ref(&verified)).unwrap();
    let expected = cache::read(&cache_dir).unwrap();
    assert_eq!(expected.len(), 1);
    assert_eq!(
        expected[0].owner_identity,
        disk.identity().digest().unwrap().to_string()
    );
    fs::remove_file(cache_dir.join("cache.db")).unwrap();
    rebuild_cache(&cache_dir, std::slice::from_ref(&verified)).unwrap();
    assert_eq!(cache::read(&cache_dir).unwrap(), expected);
    fs::write(cache_dir.join("cache.db"), b"not sqlite").unwrap();
    assert!(cache::read(&cache_dir).is_err());
    rebuild_cache(&cache_dir, &[verified]).unwrap();
    assert_eq!(cache::read(&cache_dir).unwrap(), expected);
    assert_eq!(files(&temp.path().join("p")), before);
}

#[test]
fn prepared_cache_never_replaces_old_cache_until_publication() {
    let temp = tempfile::tempdir().unwrap();
    let disk = primary(temp.path(), "p", b"old");
    let cache_dir = temp.path().join("cache");
    rebuild_cache(&cache_dir, &[disk.verify(None).unwrap()]).unwrap();
    let before = fs::read(cache_dir.join("cache.db")).unwrap();
    publish(&disk, &primary_request(&disk, "second", b"new"));
    let next = prepare_cache(&cache_dir, &[disk.verify(None).unwrap()]).unwrap();
    assert_eq!(fs::read(cache_dir.join("cache.db")).unwrap(), before);
    assert_eq!(cache::read(&cache_dir).unwrap().len(), 1);
    next.publish().unwrap();
    assert_eq!(cache::read(&cache_dir).unwrap().len(), 2);
}

#[test]
fn incarnation_conflict_is_rejected_when_independent_views_reach_system_cache() {
    let temp = tempfile::tempdir().unwrap();
    let a = primary(temp.path(), "a", b"first");
    let mut other = a.identity().clone();
    other.incarnation = Digest::body(b"another incarnation");
    let b = Disk::initialize(&temp.path().join("b"), other).unwrap();
    publish(&b, &primary_request(&b, "first", b"second"));
    let cache_dir = temp.path().join("cache");
    rebuild_cache(&cache_dir, &[a.verify(None).unwrap()]).unwrap();
    let before = fs::read(cache_dir.join("cache.db")).unwrap();
    assert!(matches!(
        prepare_cache(
            &cache_dir,
            &[a.verify(None).unwrap(), b.verify(None).unwrap()]
        ),
        Err(Error::Identity)
    ));
    assert_eq!(fs::read(cache_dir.join("cache.db")).unwrap(), before);
}

#[test]
fn bundle_budget_failure_is_atomic_and_does_not_hide_partial_material() {
    let owner = identity("p", Role::Primary);
    let mut bundle = Bundle::default();
    let first = vec![1u8; MAX_BYTES / 2];
    bundle
        .add_commit(commit(&owner, &first), Some(&first))
        .unwrap();
    let before = bundle.clone();
    let second = vec![2u8; MAX_BYTES / 2];
    assert!(matches!(
        bundle.add_commit(commit(&owner, &second), Some(&second)),
        Err(Error::Budget)
    ));
    assert_eq!(bundle, before);
    let mut another = Bundle::default();
    another
        .add_commit(commit(&owner, &second), Some(&second))
        .unwrap();
    assert!(matches!(bundle.extend(&another), Err(Error::Budget)));
    assert_eq!(bundle, before);
}

#[test]
fn no_unknown_or_downstream_fields_or_old_format_are_accepted() {
    let primary = json_codec::json!({"role":"primary", "heads":[], "sources":[]});
    assert!(json_codec::from_value::<Authority>(primary).is_err());
    assert!(json_codec::from_str::<Format>("\"old-pool-format\"").is_err());
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("existing");
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(path.join("user-data"), b"untouched").unwrap();
    assert!(Disk::initialize(&path, identity("p", Role::Primary)).is_err());
    assert_eq!(fs::read(path.join("user-data")).unwrap(), b"untouched");
}

#[test]
fn crash_child() {
    let Some(root) = std::env::var_os("ZXS_AUTHORITY_CRASH_ROOT") else {
        return;
    };
    let phase = std::env::var("ZXS_AUTHORITY_CRASH_PHASE").unwrap();
    let disk = Disk::open(Path::new(&root), &identity("p", Role::Primary)).unwrap();
    let request = primary_request(&disk, "crash_operation", b"crash body");
    let writer = disk.writer().unwrap();
    writer.begin(&request).unwrap();
    if phase == "intent" {
        std::process::exit(81);
    }
    let prepared = writer.prepare(&request).unwrap();
    if phase == "prepared" {
        std::process::exit(82);
    }
    writer.publish(&prepared).unwrap();
    std::process::exit(83);
}

#[test]
fn real_process_exit_at_every_exposed_durable_phase_recovers_without_duplicate_publication() {
    for (phase, code) in [("intent", 81), ("prepared", 82), ("published", 83)] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("disk");
        let disk = Disk::initialize(&root, identity("p", Role::Primary)).unwrap();
        drop(disk);
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("disk_authority::protocol_tests::crash_child")
            .arg("--nocapture")
            .env("ZXS_AUTHORITY_CRASH_ROOT", &root)
            .env("ZXS_AUTHORITY_CRASH_PHASE", phase)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(code));
        let disk = Disk::open(&root, &identity("p", Role::Primary)).unwrap();
        let state = disk.verify(None).unwrap();
        assert_eq!(state.sequence(), if phase == "published" { 1 } else { 0 });
        let writer = disk.writer().unwrap();
        let prepared = if phase == "intent" {
            writer
                .prepare(&primary_request(&disk, "crash_operation", b"crash body"))
                .unwrap()
        } else {
            writer.resume_prepared(&token("crash_operation")).unwrap()
        };
        let receipt = writer.publish(&prepared).unwrap();
        assert_eq!(receipt.sequence(), 1);
        assert_eq!(writer.publish(&prepared).unwrap(), receipt);
        assert_eq!(disk.verify(None).unwrap().sequence(), 1);
    }
}

#[test]
fn body_promised_by_prepare_cannot_disappear_before_publication_or_cold_resume() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("disk");
    let disk = Disk::initialize(&root, identity("p", Role::Primary)).unwrap();
    let request = primary_request(&disk, "write", b"promised");
    let writer = disk.writer().unwrap();
    let prepared = writer.prepare(&request).unwrap();
    let body_path = root.join("bodies").join(format!(
        "{}.bin",
        model::Object::Commit(commit(&identity("p", Role::Primary), b"promised"))
            .digest()
            .unwrap()
    ));
    fs::remove_file(&body_path).unwrap();
    assert!(matches!(
        writer.publish(&prepared),
        Err(Error::RequiredBodyMissing(_))
    ));
    drop(writer);
    drop(disk);
    let disk = Disk::open(&root, &identity("p", Role::Primary)).unwrap();
    let writer = disk.writer().unwrap();
    assert!(matches!(
        writer.resume_prepared(&token("write")),
        Err(Error::RequiredBodyMissing(_))
    ));
    assert_eq!(disk.verify(None).unwrap().checkpoint(), None);
    // Exact reviewed material may repair the still-unpublished staging copy.
    let prepared = writer.prepare(&request).unwrap();
    let receipt = writer.publish(&prepared).unwrap();
    fs::remove_file(&body_path).unwrap();
    let observed = disk.verify(Some(receipt.publication())).unwrap();
    assert_eq!(
        observed.copies()[0].availability,
        Availability::TemporarilyUnavailable
    );
    assert_eq!(observed.receipt(&token("write")), Some(receipt));
}

#[test]
fn required_body_set_is_part_of_the_permanent_operation_digest() {
    let temp = tempfile::tempdir().unwrap();
    let disk = Disk::initialize(&temp.path().join("disk"), identity("p", Role::Primary)).unwrap();
    let available = primary_request(&disk, "same_operation", b"body");
    let mut absent = Bundle::default();
    let head = absent
        .add_commit(commit(disk.identity(), b"body"), None)
        .unwrap();
    let unavailable = PublicationRequest::new(
        &disk.verify(None).unwrap(),
        token("same_operation"),
        Authority::Primary { heads: vec![head] },
        absent,
    )
    .unwrap();
    assert_ne!(available.request_digest(), unavailable.request_digest());
    let writer = disk.writer().unwrap();
    writer.begin(&available).unwrap();
    let before = files(&temp.path().join("disk"));
    assert!(matches!(
        writer.begin(&unavailable),
        Err(Error::OperationConflict)
    ));
    assert_eq!(files(&temp.path().join("disk")), before);
}

#[test]
fn divergent_primary_forks_cannot_be_merged_through_different_secondary_disks() {
    let temp = tempfile::tempdir().unwrap();
    let a = primary(temp.path(), "a", b"common");
    copy_tree(&temp.path().join("a"), &temp.path().join("fork"));
    let fork = Disk::open(&temp.path().join("fork"), a.identity()).unwrap();
    publish(&a, &primary_request(&a, "left", b"left"));
    publish(&fork, &primary_request(&fork, "right", b"right"));
    let left = backup(
        temp.path(),
        "left_backup",
        Role::Secondary,
        &[a.verify(None).unwrap()],
    );
    let right = backup(
        temp.path(),
        "right_backup",
        Role::Secondary,
        &[fork.verify(None).unwrap()],
    );
    let l = left.verify(None).unwrap();
    let r = right.verify(None).unwrap();
    assert!(prepare_cache(&temp.path().join("cache"), &[l.clone(), r.clone()]).is_err());
    let target = Disk::initialize(
        &temp.path().join("tertiary"),
        identity("tertiary", Role::Tertiary),
    )
    .unwrap();
    let mut bundle = l.portable_bundle();
    bundle.extend(&r.portable_bundle()).unwrap();
    let request = PublicationRequest::new(
        &target.verify(None).unwrap(),
        token("combine"),
        Authority::Tertiary {
            sources: vec![
                l.source_checkpoint().unwrap(),
                r.source_checkpoint().unwrap(),
            ],
        },
        bundle,
    );
    assert!(matches!(
        request,
        Err(Error::Invalid(
            "conflicting publication forks for one disk identity"
        ))
    ));
    assert_eq!(target.verify(None).unwrap().checkpoint(), None);
}

#[test]
fn cache_crash_child() {
    let Some(root) = std::env::var_os("ZXS_AUTHORITY_CACHE_CRASH_ROOT") else {
        return;
    };
    let root = Path::new(&root);
    let disk = Disk::open(&root.join("disk"), &identity("p", Role::Primary)).unwrap();
    let _prepared = prepare_cache(&root.join("cache"), &[disk.verify(None).unwrap()]).unwrap();
    std::process::exit(84);
}

#[test]
fn real_process_exit_before_cache_publication_retains_old_cache_and_all_disk_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("disk");
    let disk = Disk::initialize(&root, identity("p", Role::Primary)).unwrap();
    publish(&disk, &primary_request(&disk, "first", b"old"));
    rebuild_cache(&temp.path().join("cache"), &[disk.verify(None).unwrap()]).unwrap();
    let old_cache = fs::read(temp.path().join("cache/cache.db")).unwrap();
    publish(&disk, &primary_request(&disk, "second", b"new"));
    let before = files(&root);
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("disk_authority::protocol_tests::cache_crash_child")
        .arg("--nocapture")
        .env("ZXS_AUTHORITY_CACHE_CRASH_ROOT", temp.path())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(84));
    assert_eq!(
        fs::read(temp.path().join("cache/cache.db")).unwrap(),
        old_cache
    );
    assert_eq!(files(&root), before);
    assert!(
        fs::read_dir(temp.path().join("cache"))
            .unwrap()
            .any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("cache-build-"))
    );
    rebuild_cache(&temp.path().join("cache"), &[disk.verify(None).unwrap()]).unwrap();
    assert_eq!(cache::read(&temp.path().join("cache")).unwrap().len(), 2);
    assert_eq!(files(&root), before);
}

#[test]
fn cache_read_rejects_oversized_files_and_row_sets_without_trusting_the_cache() {
    use sqlite_access::{Connection, Executor};
    let temp = tempfile::tempdir().unwrap();
    let disk = primary(temp.path(), "p", b"body");
    let dir = temp.path().join("cache");
    rebuild_cache(&dir, &[disk.verify(None).unwrap()]).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(dir.join("cache.db"))
        .unwrap()
        .set_len(MAX_BYTES as u64 + 1)
        .unwrap();
    assert!(matches!(cache::read(&dir), Err(Error::Budget)));
    rebuild_cache(&dir, &[disk.verify(None).unwrap()]).unwrap();
    platform_io::sqlite::block_on_sqlite_connection(async {
        let options = sqlite_access::sqlite::SqliteConnectOptions::new()
            .filename(dir.join("cache.db"))
            .create_if_missing(false);
        let mut connection = sqlite_access::SqliteConnection::connect_with(&options)
            .await
            .unwrap();
        connection.execute("WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<4097) INSERT INTO copies SELECT observer,owner,pool,disk,slot,binding,history,file,printf('%064x',x),body_hash,available FROM copies,n WHERE commit_hash NOT LIKE '0000%' ").await.unwrap();
        connection.close().await.unwrap();
    });
    assert!(fs::metadata(dir.join("cache.db")).unwrap().len() < MAX_BYTES as u64);
    assert!(matches!(cache::read(&dir), Err(Error::Budget)));
    assert_eq!(disk.verify(None).unwrap().copies().len(), 1);
}

#[test]
fn operation_status_is_read_only_and_never_promotes_prepared_to_published() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("disk");
    let disk = Disk::initialize(&root, identity("p", Role::Primary)).unwrap();
    let before = files(&root);
    assert!(disk.operation_evidence(&token("absent")).unwrap().is_none());
    assert_eq!(files(&root), before);
    let request = primary_request(&disk, "write", b"body");
    let writer = disk.writer().unwrap();
    writer.begin(&request).unwrap();
    let before = files(&root);
    assert!(matches!(
        disk.operation_evidence(&token("write")).unwrap(),
        Some(OperationEvidence::Intent {
            local_target_matches: true,
            base_matches: true,
            ..
        })
    ));
    assert_eq!(files(&root), before);
    let prepared = writer.prepare(&request).unwrap();
    fs::remove_file(root.join("bodies").join(format!(
            "{}.bin",
            model::Object::Commit(commit(&identity("p", Role::Primary), b"body"))
                .digest()
                .unwrap()
        )))
    .unwrap();
    let before = files(&root);
    assert!(
        matches!(disk.operation_evidence(&token("write")).unwrap(), Some(OperationEvidence::Prepared { missing_required_bodies, .. }) if missing_required_bodies == vec![model::Object::Commit(commit(&identity("p", Role::Primary), b"body")).digest().unwrap()])
    );
    assert_eq!(files(&root), before);
    assert!(writer.publish(&prepared).is_err());
    let prepared = writer.prepare(&request).unwrap();
    let receipt = writer.publish(&prepared).unwrap();
    let before = files(&root);
    assert!(
        matches!(disk.operation_evidence(&token("write")).unwrap(), Some(OperationEvidence::Published(observed)) if observed == receipt)
    );
    assert_eq!(files(&root), before);
}

#[test]
fn partial_begin_evidence_is_recovery_required_not_an_absent_operation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("disk");
    let disk = Disk::initialize(&root, identity("p", Role::Primary)).unwrap();
    let empty = root.join("operations/empty");
    fs::create_dir(&empty).unwrap();
    fs::set_permissions(&empty, fs::Permissions::from_mode(0o700)).unwrap();
    let before = files(&root);
    assert!(matches!(
        disk.operation_evidence(&token("empty")).unwrap(),
        Some(OperationEvidence::Incomplete {
            local_target_matches: None,
            ..
        })
    ));
    assert_eq!(files(&root), before);
    let request = primary_request(&disk, "local_only", b"body");
    let different = primary_request(&disk, "local_only", b"other");
    let writer = disk.writer().unwrap();
    writer.begin(&request).unwrap();
    // The durable prefix after local.json and before intent.json. This is a
    // simulated internal write cut, separate from the real completed-phase exits.
    fs::remove_file(root.join("operations/local_only/intent.json")).unwrap();
    let before = files(&root);
    assert!(matches!(
        disk.operation_evidence(&token("local_only")).unwrap(),
        Some(OperationEvidence::Incomplete {
            local_target_matches: Some(true),
            ..
        })
    ));
    assert_eq!(files(&root), before);
    assert!(matches!(
        writer.begin(&different),
        Err(Error::OperationConflict)
    ));
    assert_eq!(files(&root), before);
    writer.begin(&request).unwrap();
    assert!(matches!(
        disk.operation_evidence(&token("local_only")).unwrap(),
        Some(OperationEvidence::Intent { .. })
    ));
}

#[test]
fn cache_reader_rejects_external_wal_layout_instead_of_reopening_sidecars() {
    use sqlite_access::{Connection, Executor};
    let temp = tempfile::tempdir().unwrap();
    let disk = primary(temp.path(), "p", b"body");
    let dir = temp.path().join("cache");
    rebuild_cache(&dir, &[disk.verify(None).unwrap()]).unwrap();
    let expected = cache::read(&dir).unwrap();
    let options = sqlite_access::sqlite::SqliteConnectOptions::new()
        .filename(dir.join("cache.db"))
        .create_if_missing(false)
        .journal_mode(sqlite_access::sqlite::SqliteJournalMode::Wal);
    let mut connection = platform_io::sqlite::block_on_sqlite_connection(
        sqlite_access::SqliteConnection::connect_with(&options),
    )
    .unwrap();
    platform_io::sqlite::block_on_sqlite_connection(connection.execute("INSERT INTO copies SELECT observer,owner,pool,disk,slot,binding,history,file,'0000000000000000000000000000000000000000000000000000000000000000',body_hash,available FROM copies")).unwrap();
    assert!(dir.join("cache.db-wal").exists());
    let before = files(&dir);
    assert!(matches!(cache::read(&dir), Err(Error::Sqlite(_))));
    assert_eq!(files(&dir), before);
    platform_io::sqlite::block_on_sqlite_connection(connection.close()).unwrap();
    rebuild_cache(&dir, &[disk.verify(None).unwrap()]).unwrap();
    assert_eq!(cache::read(&dir).unwrap(), expected);
}

fn prepare_cache(path: &Path, disks: &[VerifiedDisk]) -> Result<cache::PreparedCache> {
    cache::prepare(
        platform_io::fs_safety::PrivateDirectory::create(path)?,
        disks,
    )
}
fn rebuild_cache(path: &Path, disks: &[VerifiedDisk]) -> Result<()> {
    prepare_cache(path, disks)?.publish().map(drop)
}

#[test]
fn cache_observation_identity_rejects_empty_incarnation_and_role_conflicts() {
    for published in [false, true] {
        for different_role in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let id = identity("same-origin", Role::Primary);
            let first = Disk::initialize(&temp.path().join("first"), id.clone()).unwrap();
            let mut changed = id;
            if different_role {
                changed.role = Role::Secondary;
            } else {
                changed.incarnation = Digest::body(b"other-incarnation");
            }
            let empty = Disk::initialize(&temp.path().join("empty"), changed).unwrap();
            if published {
                publish(&first, &primary_request(&first, "publish", b"body"));
            }
            let cache = temp.path().join("cache");
            rebuild_cache(&cache, &[first.verify(None).unwrap()]).unwrap();
            let before = files(temp.path());
            assert!(matches!(
                prepare_cache(
                    &cache,
                    &[first.verify(None).unwrap(), empty.verify(None).unwrap()]
                ),
                Err(Error::Identity)
            ));
            assert_eq!(files(temp.path()), before);
        }
    }
}
