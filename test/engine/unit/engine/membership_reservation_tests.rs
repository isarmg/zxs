use super::*;
use std::{fs, os::unix::fs::PermissionsExt};

// Membership reservation tests exercise independent primary roots, not backup
// topology. Every initializer below uses the public, format-selected producer.
fn primary_config(roots: impl IntoIterator<Item = PathBuf>) -> Result<Config> {
    let mut config = Config::development(roots)?;
    for disk in &mut config.disks {
        disk.level = 1;
        disk.upstream_slot_id = None;
    }
    Ok(config)
}

fn initialize_fresh(system: &Path, config: Config) -> Result<Engine> {
    // Provision only absent test directories. Never chmod or repair an existing
    // root: contention/foreign-material assertions must observe its real state.
    for root in std::iter::once(system).chain(config.disks.iter().map(|disk| disk.root.as_path())) {
        if !root.try_exists()? {
            fs::create_dir(root)?;
            fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
        }
    }
    Engine::initialize_storage(system, config, OperationId::generate()?)
}

struct Fixture {
    temp: tempfile::TempDir,
    engine: Engine,
    original: Config,
    target: Config,
    operation: OperationId,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let original =
            primary_config((0..2).map(|n| temp.path().join(format!("old-{n}")))).unwrap();
        let engine = initialize_fresh(&temp.path().join("system"), original.clone()).unwrap();
        let mut target = original.clone();
        for disk in primary_config((0..4).map(|n| temp.path().join(format!("new-{n}"))))
            .unwrap()
            .disks
            .into_iter()
            .skip(2)
        {
            private_dir(&disk.root).unwrap();
            target.disks.push(disk);
        }
        Self {
            temp,
            engine,
            original,
            target,
            operation: OperationId::generate().unwrap(),
        }
    }
    fn begin(&mut self) -> Result<MembershipFenceStatus> {
        self.engine
            .begin_membership_handoff(self.target.clone(), "admin", self.operation)
    }
    fn reopen(&mut self) {
        let placeholder = initialize_fresh(
            &self.temp.path().join("placeholder-system"),
            primary_config([self.temp.path().join("placeholder")]).unwrap(),
        )
        .unwrap();
        drop(std::mem::replace(&mut self.engine, placeholder));
        self.engine = Engine::open(&self.temp.path().join("system")).unwrap();
    }
    fn old_files(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        self.original
            .disks
            .iter()
            .flat_map(|disk| files(&disk.root))
            .collect()
    }
    fn enroll(&mut self) -> PendingMembershipStatus {
        let source = self.temp.path().join("archive");
        private_dir(&source).unwrap();
        let archive = self
            .engine
            .build_membership_control_archive(&source)
            .unwrap();
        self.engine
            .enroll_pending_members(
                &source,
                &archive.root_hash,
                self.target.disks[2..]
                    .iter()
                    .map(|disk| PendingMemberDirectory {
                        disk_id: disk.disk_id.clone(),
                        directory: File::open(&disk.root).unwrap(),
                    })
                    .collect(),
            )
            .unwrap()
    }
}
fn files(path: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    for entry in fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.extend(files(&path));
        } else {
            out.insert(path.clone(), fs::read(path).unwrap());
        }
    }
    out
}
fn write_business(engine: &mut Engine) {
    engine
        .create_repository(
            &RepositoryName::parse("still-writable").unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
}

#[test]
fn membership_preparation_obvious_bad_roots_leave_old_pool_bytes_and_state_writable() {
    for case in [
        "foreign",
        "lock",
        "not-private",
        "file",
        "symlink",
        "missing",
    ] {
        let mut f = Fixture::new();
        let root = f.target.disks[3].root.clone();
        let mut foreign = None;
        match case {
            "foreign" => {
                fs::write(root.join("user-body"), b"preserve this body").unwrap();
                foreign = Some(root.join("user-body"));
            }
            "lock" => {
                fs::write(root.join(".writer.lock"), b"unbound").unwrap();
                foreign = Some(root.join(".writer.lock"));
            }
            "not-private" => fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap(),
            "file" => {
                fs::remove_dir(&root).unwrap();
                fs::write(&root, b"not a directory").unwrap();
                foreign = Some(root.clone());
            }
            "symlink" => {
                fs::remove_dir(&root).unwrap();
                std::os::unix::fs::symlink(&f.target.disks[2].root, &root).unwrap();
            }
            "missing" => fs::remove_dir(&root).unwrap(),
            _ => unreachable!(),
        }
        let before = f.old_files();
        let state = canonical_json(f.engine.state()).unwrap();
        let bytes = foreign.as_ref().map(|path| fs::read(path).unwrap());
        assert!(f.begin().is_err(), "{case}");
        assert_eq!(f.old_files(), before, "{case}");
        assert_eq!(canonical_json(f.engine.state()).unwrap(), state);
        assert!(f.engine.membership_fence_status().is_none());
        assert!(
            fs::read_dir(&f.target.disks[2].root)
                .unwrap()
                .next()
                .is_none()
        );
        if let Some(path) = foreign {
            assert_eq!(fs::read(path).unwrap(), bytes.unwrap());
        }
        write_business(&mut f.engine);
    }
}

#[test]
fn membership_preparation_each_new_barrier_reopens_same_operation() {
    for point in [
        "after_membership_source_preparation_1",
        "after_membership_source_preparation_2",
        "before_membership_reservation_1",
        "after_membership_reservation_create_1",
        "after_membership_reservation_directory_sync_1",
        "after_membership_reservation_sync_1",
        "after_membership_writer_create_1",
        "after_membership_writer_lock_1",
        "after_membership_writer_file_sync_1",
        "after_membership_writer_sync_1",
        "before_membership_reservation_2",
        "after_membership_reservation_create_2",
        "after_membership_reservation_directory_sync_2",
        "after_membership_reservation_sync_2",
        "after_membership_writer_create_2",
        "after_membership_writer_lock_2",
        "after_membership_writer_file_sync_2",
        "after_membership_writer_sync_2",
    ] {
        let mut f = Fixture::new();
        f.engine.inject_fault_once(point);
        assert!(f.begin().is_err(), "{point}");
        assert!(f.engine.membership_fence_status().is_none(), "{point}");
        // Preparation is stable even when the eventual frozen prefix advances.
        write_business(&mut f.engine);
        let state = canonical_json(f.engine.state()).unwrap();
        f.reopen();
        assert!(!f.engine.is_read_only());
        assert_eq!(canonical_json(f.engine.state()).unwrap(), state);
        let fenced = f.begin().unwrap();
        assert!(fenced.all_members_fenced(), "{point}");
        assert_eq!((fenced.original_members, fenced.original_quorum), (2, 2));
        assert!(f.enroll().all_control_metadata_synchronized(), "{point}");
        f.engine.resume_membership_joint_decision().unwrap();
        assert_eq!(
            f.engine.activate_pending_membership().unwrap().operation_id,
            f.operation
        );
    }
}

#[test]
fn membership_preparation_source_binding_rejects_conflicts_and_replaced_roots() {
    for point in [
        "after_membership_source_preparation_1",
        "after_membership_writer_sync_1",
    ] {
        for change in ["actor", "operation", "target", "identity"] {
            let mut f = Fixture::new();
            f.engine.inject_fault_once(point);
            assert!(f.begin().is_err());
            f.reopen();
            let original_root = f.target.disks[2].root.clone();
            if change == "identity" {
                fs::rename(&original_root, original_root.with_extension("preserved")).unwrap();
                private_dir(&original_root).unwrap();
            }
            let before = f.old_files();
            let mut target = f.target.clone();
            if change == "target" {
                target.disks[2].capacity_bytes = Some(4096);
            }
            let result = f.engine.begin_membership_handoff(
                target,
                if change == "actor" {
                    "someone-else"
                } else {
                    "admin"
                },
                if change == "operation" {
                    OperationId::generate().unwrap()
                } else {
                    f.operation
                },
            );
            assert!(result.is_err(), "{point}: {change}");
            assert_eq!(f.old_files(), before);
            assert!(f.engine.membership_fence_status().is_none());
            if change == "identity" {
                assert!(fs::read_dir(&original_root).unwrap().next().is_none());
            }
        }
    }
}

#[test]
fn membership_preparation_reserved_root_is_not_available_to_initialization_or_another_writer() {
    let mut f = Fixture::new();
    f.engine.inject_fault_once("after_membership_writer_sync_1");
    assert!(f.begin().is_err());
    let disk = &f.target.disks[2];
    let before = files(&disk.root);
    let other = primary_config([disk.root.clone()]).unwrap();
    assert!(initialize_fresh(&f.temp.path().join("other-system"), other).is_err());
    assert_eq!(files(&disk.root), before);
    let lockfile = File::open(disk.root.join(".writer.lock")).unwrap();
    lock(&lockfile).unwrap();
    assert!(f.begin().is_err());
    assert!(f.engine.membership_fence_status().is_none());
    drop(lockfile);
    assert!(f.begin().unwrap().all_members_fenced());
}

#[test]
fn membership_preparation_source_conflict_and_foreign_marker_bytes_are_preserved() {
    for conflict_source in [false, true] {
        let mut f = Fixture::new();
        f.engine.inject_fault_once("after_membership_writer_sync_1");
        assert!(f.begin().is_err());
        let path = if conflict_source {
            source_path(&f.original.disks[1], f.operation)
        } else {
            fs::read_dir(&f.target.disks[2].root)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| path.is_dir())
                .unwrap()
                .join("foreign")
        };
        fs::write(&path, b"do not consume").unwrap();
        let before = f.old_files();
        assert!(f.begin().is_err());
        assert_eq!(f.old_files(), before);
        assert_eq!(fs::read(path).unwrap(), b"do not consume");
        assert!(f.engine.membership_fence_status().is_none());
    }
}

#[test]
fn membership_preparation_directory_lock_excludes_initializer_before_writer_creation() {
    let mut f = Fixture::new();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    f.engine.inject_pause_once(
        "after_membership_reservation_create_1",
        Arc::clone(&barrier),
    );
    let new_root = f.target.disks[2].root.clone();
    let other_system = f.temp.path().join("concurrent-system");
    std::thread::scope(|scope| {
        let begin = scope.spawn(|| f.begin());
        barrier.wait();
        let before = files(&new_root);
        let other = primary_config([new_root.clone()]).unwrap();
        let error = initialize_fresh(&other_system, other).err().unwrap();
        assert_eq!(error.code(), ErrorCode::WriterAlreadyActive);
        assert_eq!(files(&new_root), before);
        assert!(!new_root.join("disk.json").exists());
        assert!(!new_root.join(".writer.lock").exists());
        barrier.wait();
        assert!(begin.join().unwrap().unwrap().all_members_fenced());
    });
    assert!(f.enroll().all_control_metadata_synchronized());
}

#[test]
fn membership_preparation_original_held_resources_are_consumed_by_enrollment() {
    let mut f = Fixture::new();
    f.begin().unwrap();
    let held = f
        .engine
        .membership_fence
        .as_ref()
        .unwrap()
        .prepared_roots
        .clone();
    f.enroll();
    for (disk, pending) in f
        .engine
        .pending_membership
        .as_ref()
        .unwrap()
        .added_resources()
    {
        let prepared = &held[&disk.disk_id];
        assert!(Arc::ptr_eq(&prepared.root, &pending.root));
        assert!(Arc::ptr_eq(&prepared.writer, &pending._lock));
        // The same typed fence owns both the writer and directory reservation.
    }
}

#[test]
fn membership_preparation_cannot_mutate_roots_or_source_while_initializer_holds_directories() {
    let mut f = Fixture::new();
    let new_root = f.target.disks[2].root.clone();
    let config =
        ValidatedConfig::try_from(primary_config([new_root.clone()]).unwrap()).unwrap();
    let resources = DiskResources::open(&config, false).unwrap();
    // Membership and fresh initialization both retain directory AdvisoryLocks
    // before creating writer sidecars; this independently holds that same lock.
    let directory_locks = lock_initialization_directories(&resources).unwrap();
    let source_before = f.old_files();
    let target_before = files(&new_root);
    assert_eq!(
        f.begin().unwrap_err().code(),
        ErrorCode::WriterAlreadyActive
    );
    assert_eq!(f.old_files(), source_before);
    assert_eq!(files(&new_root), target_before);
    assert!(fs::read_dir(&new_root).unwrap().next().is_none());
    assert!(f.engine.membership_fence_status().is_none());
    drop(directory_locks);
    drop(resources);
    let other = initialize_fresh(
        &f.temp.path().join("winning-initializer"),
        (*config).clone(),
    )
    .unwrap();
    assert!(new_root.join("disk.json").is_file());
    assert!(!fs::read_dir(&new_root).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .as_encoded_bytes()
            .starts_with(PREFIX.as_bytes())
    }));
    assert!(f.begin().is_err());
    assert_eq!(f.old_files(), source_before);
    drop(other);
}
