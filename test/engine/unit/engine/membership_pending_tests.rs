use super::*;
use std::fs;

struct Fixture {
    temp: tempfile::TempDir,
    original: Config,
    target: Config,
    engine: Engine,
    source: PathBuf,
    archive: MembershipControlArchive,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let mut original =
            Config::development((0..2).map(|n| temp.path().join(format!("old-{n}")))).unwrap();
        for (n, disk) in original.disks.iter_mut().enumerate() {
            disk.disk_id = format!("disk_{:032x}", n + 1);
        }
        let mut engine = Engine::initialize(&temp.path().join("system"), original.clone()).unwrap();
        for name in ["first", "second"] {
            engine
                .create_repository(
                    &RepositoryName::parse(name).unwrap(),
                    "P01",
                    1,
                    "admin",
                    OperationId::generate().unwrap(),
                )
                .unwrap();
        }
        let mut target = original.clone();
        let additional =
            Config::development((0..4).map(|n| temp.path().join(format!("new-{n}")))).unwrap();
        for (n, mut disk) in additional.disks.into_iter().skip(2).enumerate() {
            disk.disk_id = format!("disk_{:032x}", n + 3);
            private_dir(&disk.root).unwrap();
            target.disks.push(disk);
        }
        engine
            .begin_membership_handoff(target.clone(), "admin", OperationId::generate().unwrap())
            .unwrap();
        let source = temp.path().join("archive");
        private_dir(&source).unwrap();
        let archive = engine.build_membership_control_archive(&source).unwrap();
        Self {
            temp,
            original,
            target,
            engine,
            source,
            archive,
        }
    }
    fn directories(&self) -> Vec<PendingMemberDirectory> {
        self.target.disks[2..]
            .iter()
            .map(|disk| PendingMemberDirectory {
                disk_id: disk.disk_id.clone(),
                directory: File::open(&disk.root).unwrap(),
            })
            .collect()
    }
    fn enroll(&mut self) -> Result<PendingMembershipStatus> {
        self.engine.enroll_pending_members(
            &self.source,
            &self.archive.root_hash,
            self.directories(),
        )
    }
    fn reopen(&mut self) {
        let placeholder = Engine::initialize(
            &self.temp.path().join("placeholder-system"),
            Config::development([self.temp.path().join("placeholder-disk")]).unwrap(),
        )
        .unwrap();
        drop(std::mem::replace(&mut self.engine, placeholder));
        self.engine = Engine::open(&self.temp.path().join("system")).unwrap();
    }
    fn pending_path(&self, index: usize) -> PathBuf {
        member_directory(&self.target.disks[index], &self.archive.transition_hash)
    }
}
fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.extend(files(&path));
        } else {
            out.insert(path.clone(), fs::read(path).unwrap());
        }
    }
    out
}
fn assert_blocked(engine: &mut Engine) {
    assert!(
        engine
            .create_repository(
                &RepositoryName::parse("blocked").unwrap(),
                "P01",
                1,
                "admin",
                OperationId::generate().unwrap()
            )
            .is_err()
    );
}

#[test]
fn membership_pending_each_target_has_own_control_receipt_and_preserves_old_bytes() {
    let mut f = Fixture::new();
    let originals = f
        .original
        .disks
        .iter()
        .flat_map(|disk| files(&disk.root))
        .collect::<BTreeMap<_, _>>();
    let status = f.enroll().unwrap();
    assert_eq!(status.control_receipts.len(), 4);
    assert!(status.all_control_metadata_synchronized());
    assert_eq!(
        status
            .control_receipts
            .iter()
            .map(|r| &r.bootstrap_hash)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        4
    );
    assert_eq!(f.engine.disk_resources.handles().count(), 2);
    assert_eq!(f.engine.journals.len(), 2);
    for (path, bytes) in originals {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
    for (n, disk) in f.target.disks.iter().enumerate() {
        let path = f.pending_path(n);
        let inspection = verify_membership_control_archive(
            &path.join("control-archive"),
            &f.archive.root_hash,
            &f.archive.transition_hash,
        )
        .unwrap();
        assert_eq!(inspection.published_candidates, 2);
        let receipt = &status.control_receipts[n];
        assert_eq!(receipt.disk_id, disk.disk_id);
        assert!(
            scan_at(
                &DiskRoot::open(&path).unwrap(),
                &DiskConfig {
                    root: path.clone(),
                    ..disk.clone()
                }
            )
            .is_err()
        );
        if n >= 2 {
            assert!(!log_path(disk).exists());
            let bytes = fs::read(disk.root.join("disk.json")).unwrap();
            assert!(decode_seal(&bytes).is_err());
            assert!(
                Engine::initialize(
                    &f.temp.path().join(format!("reinit-{n}")),
                    Config::development([disk.root.clone()]).unwrap()
                )
                .is_err()
            );
        }
    }
    let first = f
        .target
        .disks
        .iter()
        .flat_map(|disk| files(&disk.root))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(f.engine.resume_pending_members().unwrap(), status);
    assert_eq!(
        f.target
            .disks
            .iter()
            .flat_map(|disk| files(&disk.root))
            .collect::<BTreeMap<_, _>>(),
        first
    );
    assert_blocked(&mut f.engine);
}

#[test]
fn membership_pending_reopens_only_through_original_fence_and_explicit_resume() {
    let mut f = Fixture::new();
    let status = f.enroll().unwrap();
    let before = f
        .target
        .disks
        .iter()
        .flat_map(|disk| files(&disk.root))
        .collect::<BTreeMap<_, _>>();
    f.reopen();
    assert!(f.engine.pending_membership_status().is_none());
    assert!(
        f.engine
            .membership_fence_status()
            .unwrap()
            .all_members_fenced()
    );
    assert_blocked(&mut f.engine);
    assert_eq!(f.enroll().unwrap(), status);
    assert_eq!(
        f.target
            .disks
            .iter()
            .flat_map(|disk| files(&disk.root))
            .collect::<BTreeMap<_, _>>(),
        before
    );
}

#[test]
fn membership_pending_unsupported_targets_fail_before_target_writes() {
    let mut f = Fixture::new();
    let target = f.target.clone();
    for change in 0..9 {
        let mut unsupported = target.clone();
        match change {
            0 => {
                let mut added = unsupported.disks[2].clone();
                added.level = 2;
                added.upstream_slot_id = Some("P01".into());
                unsupported.disks = vec![unsupported.disks[0].clone(), added];
                unsupported.disks[0].clone()
            }
            1 => {
                unsupported.disks[0].root = f.temp.path().join("changed-root");
                unsupported.disks[0].clone()
            }
            2 => {
                unsupported.disks[0].binding_generation += 1;
                unsupported.disks[0].clone()
            }
            3 => {
                unsupported.disks[0].metadata_member = false;
                unsupported.disks[0].clone()
            }
            4 => {
                unsupported.disks[0].capacity_bytes = Some(10);
                unsupported.disks[0].clone()
            }
            5 => {
                unsupported.max_entries += 1;
                unsupported.disks[0].clone()
            }
            6 => {
                unsupported.mode = Mode::Production;
                unsupported.disks[0].clone()
            }
            7 => {
                unsupported.disks[2].binding_generation = 2;
                unsupported.disks[0].clone()
            }
            8 => {
                // New nonvoters are supported, but still require a fresh binding.
                unsupported.disks[2].metadata_member = false;
                unsupported.disks[2].binding_generation = 2;
                unsupported.disks[0].clone()
            }
            _ => unreachable!(),
        };
        f.engine.membership_fence.as_mut().unwrap().intent.target =
            ValidatedConfig::try_from(unsupported).unwrap();
        assert!(f.enroll().is_err(), "case {change}");
        for disk in &f.target.disks[2..] {
            assert!(files(&disk.root).is_empty());
        }
        assert_blocked(&mut f.engine);
    }
}

#[test]
fn membership_pending_rejects_missing_changed_duplicate_and_locked_roots() {
    for case in 0..4 {
        let mut f = Fixture::new();
        let mut directories = f.directories();
        let moved = f.target.disks[2].root.with_extension("moved");
        let _lock;
        match case {
            0 => {
                fs::rename(&f.target.disks[2].root, &moved).unwrap();
            }
            1 => {
                fs::rename(&f.target.disks[2].root, &moved).unwrap();
                private_dir(&f.target.disks[2].root).unwrap();
            }
            2 => {
                directories[1].directory = directories[0].directory.try_clone().unwrap();
            }
            3 => {
                _lock = DiskRoot::open(&f.target.disks[2].root)
                    .unwrap()
                    .lock_file()
                    .unwrap();
                rustix::fs::flock(&_lock, rustix::fs::FlockOperation::NonBlockingLockExclusive)
                    .unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            f.engine
                .enroll_pending_members(&f.source, &f.archive.root_hash, directories)
                .is_err()
        );
        assert!(!f.target.disks[3].root.join("disk.json").exists());
    }
}

#[test]
fn membership_pending_rejects_untrusted_archive_before_any_seal() {
    let mut f = Fixture::new();
    let wrong = "a".repeat(64);
    assert!(
        f.engine
            .enroll_pending_members(&f.source, &wrong, f.directories())
            .is_err()
    );
    for disk in &f.target.disks[2..] {
        assert!(files(&disk.root).is_empty());
    }
}

#[test]
fn membership_pending_cannot_copy_another_members_seal_object_or_log() {
    for artifact in ["seal", "member", "object", "log"] {
        let mut f = Fixture::new();
        f.enroll().unwrap();
        let a = f.pending_path(2);
        let b = f.pending_path(3);
        match artifact {
            "seal" => {
                fs::copy(
                    f.target.disks[2].root.join("disk.json"),
                    f.target.disks[3].root.join("disk.json"),
                )
                .unwrap();
            }
            "member" => {
                fs::copy(a.join("member.json"), b.join("member.json")).unwrap();
            }
            "object" => {
                let object = fs::read_dir(a.join("objects").join(BOOTSTRAP_KIND))
                    .unwrap()
                    .next()
                    .unwrap()
                    .unwrap()
                    .path();
                let target = fs::read_dir(b.join("objects").join(BOOTSTRAP_KIND))
                    .unwrap()
                    .next()
                    .unwrap()
                    .unwrap()
                    .path();
                fs::copy(object, target).unwrap();
            }
            "log" => {
                fs::copy(a.join("journal.bin"), b.join("journal.bin")).unwrap();
            }
            _ => unreachable!(),
        }
        let before = files(&f.target.disks[3].root);
        assert!(f.engine.resume_pending_members().is_err());
        assert_eq!(files(&f.target.disks[3].root), before);
        assert_blocked(&mut f.engine);
    }
}

#[test]
fn membership_pending_missing_local_closure_is_never_borrowed_from_another_target() {
    let mut f = Fixture::new();
    f.enroll().unwrap();
    let archive = f.pending_path(2).join("control-archive");
    let missing = files(&archive)
        .keys()
        .find(|path| path.to_string_lossy().contains("objects/snapshot/"))
        .unwrap()
        .clone();
    fs::remove_file(&missing).unwrap();
    assert!(
        verify_membership_control_archive(
            &archive,
            &f.archive.root_hash,
            &f.archive.transition_hash
        )
        .is_err()
    );
    // Same-hash resume may reinstall from the separately trusted source archive.
    f.engine.resume_pending_members().unwrap();
    verify_membership_control_archive(&archive, &f.archive.root_hash, &f.archive.transition_hash)
        .unwrap();
    fs::remove_file(&missing).unwrap();
    fs::remove_file(f.source.join(missing.strip_prefix(&archive).unwrap())).unwrap();
    assert!(f.engine.resume_pending_members().is_err());
    assert!(!missing.exists());
    assert_blocked(&mut f.engine);
}

#[test]
fn membership_pending_staged_failures_resume_same_intent_without_duplicate_frames() {
    for point in [
        "after_membership_pending_member",
        "after_membership_pending_archive",
        "after_membership_pending_object",
        "after_membership_pending_receipt_1",
    ] {
        let mut f = Fixture::new();
        f.engine.inject_fault_once(point);
        assert!(f.enroll().is_err());
        let observed = f.engine.pending_membership_status().unwrap();
        assert_eq!(
            observed.control_receipts.len(),
            usize::from(point.ends_with("_1"))
        );
        assert!(!observed.all_control_metadata_synchronized());
        assert_blocked(&mut f.engine);
        f.reopen();
        let result = f.enroll().unwrap();
        assert!(result.all_control_metadata_synchronized());
        let before = f
            .target
            .disks
            .iter()
            .flat_map(|disk| files(&disk.root))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(f.engine.resume_pending_members().unwrap(), result);
        assert_eq!(
            f.target
                .disks
                .iter()
                .flat_map(|disk| files(&disk.root))
                .collect::<BTreeMap<_, _>>(),
            before
        );
    }
}

#[test]
fn membership_pending_log_failures_reopen_and_report_only_synchronized_receipts() {
    for fault in [JournalFault::PartialWrite, JournalFault::Sync] {
        let mut f = Fixture::new();
        f.engine
            .inject_fault_once("after_membership_pending_object");
        assert!(f.enroll().is_err());
        f.engine.pending_membership.as_mut().unwrap().journal_fault = Some(fault);
        assert!(f.engine.resume_pending_members().is_err());
        assert!(
            f.engine
                .pending_membership_status()
                .unwrap()
                .control_receipts
                .is_empty()
        );
        let done = f.engine.resume_pending_members().unwrap();
        assert_eq!(done.control_receipts.len(), 4);
        assert_blocked(&mut f.engine);
    }
}

#[test]
fn membership_pending_unbound_and_partial_seal_material_is_preserved() {
    for artifact in ["disk.json", ".writer.lock", ".unknown"] {
        let mut f = Fixture::new();
        fs::write(f.target.disks[2].root.join(artifact), b"partial").unwrap();
        let before = files(&f.target.disks[2].root);
        assert!(f.enroll().is_err());
        assert_eq!(files(&f.target.disks[2].root), before);
        assert!(!f.target.disks[3].root.join("disk.json").exists());
    }
}

#[test]
fn membership_pending_rejects_active_namespace_on_added_disk() {
    let mut f = Fixture::new();
    f.enroll().unwrap();
    let active = log_path(&f.target.disks[2]);
    private_dir(active.parent().unwrap()).unwrap();
    fs::copy(f.pending_path(2).join("journal.bin"), &active).unwrap();
    let before = files(&f.target.disks[2].root);
    assert!(scan(&f.target.disks[2]).is_err());
    assert!(f.engine.resume_pending_members().is_err());
    assert_eq!(files(&f.target.disks[2].root), before);
}

#[test]
fn membership_pending_seal_failure_boundaries_distinguish_bound_and_unbound_roots() {
    for completed in [1, 2] {
        for reopen in [false, true] {
            let mut f = Fixture::new();
            f.engine
                .inject_fault_once(&format!("after_membership_pending_seal_{completed}"));
            assert!(f.enroll().is_err());
            for n in 0..2 {
                assert_eq!(
                    f.target.disks[n + 2].root.join("disk.json").exists(),
                    n < completed
                );
            }
            assert!(
                f.engine
                    .pending_membership_status()
                    .unwrap()
                    .control_receipts
                    .is_empty()
            );
            if reopen {
                let before = f.target.disks[2..]
                    .iter()
                    .flat_map(|disk| files(&disk.root))
                    .collect::<BTreeMap<_, _>>();
                f.reopen();
                let result = f.enroll();
                if completed == 1 {
                    assert!(result.is_err());
                    assert_eq!(
                        f.target.disks[2..]
                            .iter()
                            .flat_map(|disk| files(&disk.root))
                            .collect::<BTreeMap<_, _>>(),
                        before
                    );
                } else {
                    assert!(result.unwrap().all_control_metadata_synchronized());
                }
            } else {
                assert!(
                    f.engine
                        .resume_pending_members()
                        .unwrap()
                        .all_control_metadata_synchronized()
                );
            }
        }
    }
}

#[test]
fn membership_pending_directory_switch_during_install_never_reports_a_receipt() {
    for (point, replace_archive) in [
        ("after_membership_pending_member", false),
        ("after_membership_pending_archive", false),
        ("after_membership_pending_archive", true),
    ] {
        let mut f = Fixture::new();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        f.engine.inject_pause_once(point, Arc::clone(&barrier));
        let member = f.pending_path(0);
        let switched = if replace_archive {
            member.join("control-archive")
        } else {
            member.clone()
        };
        let displaced = switched.with_extension("displaced");
        std::thread::scope(|scope| {
            let join = scope.spawn(|| f.enroll());
            barrier.wait();
            fs::rename(&switched, &displaced).unwrap();
            private_dir(&switched).unwrap();
            barrier.wait();
            assert!(join.join().unwrap().is_err());
        });
        assert!(
            f.engine
                .pending_membership_status()
                .unwrap()
                .control_receipts
                .is_empty()
        );
        assert!(!member.join("journal.bin").exists());
        assert!(files(&switched).is_empty());
        assert_blocked(&mut f.engine);
    }
}

#[test]
fn membership_pending_system_loss_recovers_old_fence_then_explicit_control_resume() {
    let mut f = Fixture::new();
    let status = f.enroll().unwrap();
    let expected_state = canonical_json(f.engine.state()).unwrap();
    let original_paths = f
        .original
        .disks
        .iter()
        .map(|disk| disk.root.clone())
        .collect::<Vec<_>>();
    let new_paths = f.target.disks[2..]
        .iter()
        .map(|disk| disk.root.clone())
        .collect::<Vec<_>>();
    let Fixture {
        temp,
        original: _,
        target,
        engine,
        source,
        archive,
    } = f;
    drop(engine);
    fs::rename(temp.path().join("system"), temp.path().join("lost-system")).unwrap();
    assert!(
        Engine::restore_from_disks(&temp.path().join("unsupported-system"), &new_paths).is_err()
    );
    let mut restored =
        Engine::restore_from_disks(&temp.path().join("replacement-system"), &original_paths)
            .unwrap();
    assert_eq!(canonical_json(restored.state()).unwrap(), expected_state);
    assert!(
        restored
            .membership_fence_status()
            .unwrap()
            .all_members_fenced()
    );
    assert!(restored.pending_membership_status().is_none());
    let directories = target.disks[2..]
        .iter()
        .map(|disk| PendingMemberDirectory {
            disk_id: disk.disk_id.clone(),
            directory: File::open(&disk.root).unwrap(),
        })
        .collect();
    assert_eq!(
        restored
            .enroll_pending_members(&source, &archive.root_hash, directories)
            .unwrap(),
        status
    );
    assert_blocked(&mut restored);
}

#[test]
fn membership_pending_bound_member_uses_same_writer_lock_as_ordinary_engine() {
    let mut f = Fixture::new();
    f.enroll().unwrap();
    f.reopen();
    let root = DiskRoot::open(&f.target.disks[2].root).unwrap();
    let lock = root.lock_file().unwrap();
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive).unwrap();
    let before = files(root.path());
    let error = f.enroll().unwrap_err();
    assert_eq!(error.code(), ErrorCode::WriterAlreadyActive);
    assert_eq!(files(root.path()), before);
    drop(lock);
    f.enroll().unwrap();
}

#[test]
fn membership_pending_partial_control_object_is_preserved_and_never_receipted() {
    for artifact in ["member", "object", "archive"] {
        let mut f = Fixture::new();
        f.engine
            .inject_fault_once("after_membership_pending_object");
        assert!(f.enroll().is_err());
        let member = f.pending_path(0);
        let path = match artifact {
            "member" => member.join("member.json"),
            "object" => fs::read_dir(member.join("objects").join(BOOTSTRAP_KIND))
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path(),
            "archive" => files(&member.join("control-archive"))
                .keys()
                .find(|path| path.to_string_lossy().contains("objects/snapshot/"))
                .unwrap()
                .clone(),
            _ => unreachable!(),
        };
        let bytes = fs::read(&path).unwrap();
        fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
        let before = files(&member);
        assert!(f.engine.resume_pending_members().is_err());
        assert!(
            f.engine
                .pending_membership_status()
                .unwrap()
                .control_receipts
                .is_empty()
        );
        assert_eq!(files(&member), before);
        assert!(!member.join("journal.bin").exists());
    }
}

#[test]
#[ignore = "explicit cross-build evidence fixture persists temporary directories"]
fn membership_pending_export_cross_build_fixture() {
    let temp = tempfile::tempdir().unwrap();
    let original = Config::development([temp.path().join("old-primary")]).unwrap();
    let mut engine = Engine::initialize(&temp.path().join("old-system"), original.clone()).unwrap();
    let probe = temp.path().join("probe");
    let pending_root = probe.join("primary");
    private_dir(&pending_root).unwrap();
    let mut target = original;
    let mut added = Config::development([pending_root.clone()])
        .unwrap()
        .disks
        .remove(0);
    added.slot_id = "B01".into();
    added.level = 2;
    added.upstream_slot_id = Some("P01".into());
    target.disks.push(added.clone());
    engine
        .begin_membership_handoff(target.clone(), "admin", OperationId::generate().unwrap())
        .unwrap();
    let source = temp.path().join("source");
    private_dir(&source).unwrap();
    let archive = engine.build_membership_control_archive(&source).unwrap();
    engine
        .enroll_pending_members(
            &source,
            &archive.root_hash,
            vec![PendingMemberDirectory {
                disk_id: added.disk_id,
                directory: File::open(&pending_root).unwrap(),
            }],
        )
        .unwrap();
    let mut single = target;
    single.disks.remove(0);
    single.disks[0].level = 1;
    single.disks[0].upstream_slot_id = None;
    let single = ValidatedConfig::try_from(single).unwrap();
    fs::write(
        probe.join("pending-open-config.json"),
        canonical_json(&single).unwrap(),
    )
    .unwrap();
    drop(engine);
    println!("PENDING_CROSS_BUILD_ROOT={}", temp.keep().display());
}

#[test]
fn membership_pending_real_directory_conflict_keeps_progress_incomplete() {
    let mut f = Fixture::new();
    f.engine
        .inject_fault_once("after_membership_pending_member");
    assert!(f.enroll().is_err());
    let member = f.pending_path(0);
    let blocker = member.join("control-archive");
    fs::write(&blocker, b"not a directory").unwrap();
    let before = files(&member);
    assert!(f.engine.resume_pending_members().is_err());
    assert!(
        f.engine
            .pending_membership_status()
            .unwrap()
            .control_receipts
            .is_empty()
    );
    assert_eq!(files(&member), before);
    assert!(!member.join("journal.bin").exists());
    // The fixture owner removes its deliberate blocker; production code never does.
    fs::remove_file(blocker).unwrap();
    assert!(
        f.engine
            .resume_pending_members()
            .unwrap()
            .all_control_metadata_synchronized()
    );
}
