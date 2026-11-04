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
    fn new(old: usize, added: usize, added_first: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let mut original =
            Config::development((0..old).map(|n| temp.path().join(format!("old-{n}")))).unwrap();
        for (n, disk) in original.disks.iter_mut().enumerate() {
            disk.disk_id = format!("disk_{:032x}", n + if added_first { added + 1 } else { 1 });
        }
        let mut engine = Engine::initialize(&temp.path().join("system"), original.clone()).unwrap();
        engine
            .create_repository(
                &RepositoryName::parse("preserved").unwrap(),
                "P01",
                1,
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        let mut target = original.clone();
        let additional =
            Config::development((0..old + added).map(|n| temp.path().join(format!("new-{n}"))))
                .unwrap();
        for (n, mut disk) in additional.disks.into_iter().skip(old).enumerate() {
            disk.disk_id = format!("disk_{:032x}", n + if added_first { 1 } else { old + 1 });
            private_dir(&disk.root).unwrap();
            target.disks.push(disk);
        }
        engine
            .begin_membership_handoff(target.clone(), "admin", OperationId::generate().unwrap())
            .unwrap();
        let source = temp.path().join("archive");
        private_dir(&source).unwrap();
        let archive = engine.build_membership_control_archive(&source).unwrap();
        let mut fixture = Self {
            temp,
            original,
            target,
            engine,
            source,
            archive,
        };
        fixture.enroll();
        fixture
    }
    fn enroll(&mut self) {
        let directories = self.target.disks[self.original.disks.len()..]
            .iter()
            .map(|disk| PendingMemberDirectory {
                disk_id: disk.disk_id.clone(),
                directory: File::open(&disk.root).unwrap(),
            })
            .collect();
        self.engine
            .enroll_pending_members(&self.source, &self.archive.root_hash, directories)
            .unwrap();
    }
    fn reopen(&mut self, lose_system: bool) {
        let placeholder = Engine::initialize(
            &self.temp.path().join("placeholder-system"),
            Config::development([self.temp.path().join("placeholder-disk")]).unwrap(),
        )
        .unwrap();
        drop(std::mem::replace(&mut self.engine, placeholder));
        let system = self.temp.path().join("system");
        if lose_system {
            fs::rename(&system, self.temp.path().join("lost-system")).unwrap();
            self.engine = Engine::restore_from_disks(
                &system,
                &self
                    .original
                    .disks
                    .iter()
                    .map(|disk| disk.root.clone())
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        } else {
            self.engine = Engine::open(&system).unwrap();
        }
        assert!(self.engine.membership_joint_decision_status().is_none());
        self.enroll();
    }
    fn path(&self, index: usize) -> PathBuf {
        member_directory(&self.target.disks[index], &self.archive.transition_hash).join("joint")
    }
    fn run(&mut self) -> MembershipJointDecisionStatus {
        self.engine.resume_membership_joint_decision().unwrap()
    }
    fn fault(&mut self, index: usize, stage: JointFaultStage) {
        self.engine.pending_membership.as_mut().unwrap().joint_fault = Some(JointFault {
            disk_id: self.target.disks[index].disk_id.clone(),
            stage,
        });
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
    assert!(engine.is_read_only());
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
fn membership_joint_two_to_four_preserves_original_bytes_and_never_activates() {
    let mut fixture = Fixture::new(2, 2, false);
    let before = fixture
        .target
        .disks
        .iter()
        .flat_map(|disk| files(&disk.root))
        .collect::<BTreeMap<_, _>>();
    let state = canonical_json(fixture.engine.state()).unwrap();
    let config = canonical_json(fixture.engine.config()).unwrap();
    let result = fixture.run();
    assert_eq!(
        (
            result.original_members,
            result.original_quorum,
            result.synchronized_original_votes
        ),
        (2, 2, 2)
    );
    assert_eq!(
        (
            result.target_members,
            result.target_quorum,
            result.synchronized_target_votes
        ),
        (4, 3, 4)
    );
    assert!(result.all_certificates_synchronized());
    assert_eq!(
        result
            .votes
            .iter()
            .map(|vote| &vote.acceptance_hash)
            .collect::<BTreeSet<_>>()
            .len(),
        4
    );
    assert_eq!(
        result
            .votes
            .iter()
            .map(|vote| &vote.frame_hash)
            .collect::<BTreeSet<_>>()
            .len(),
        4
    );
    for (path, bytes) in before {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
    assert_eq!(fixture.run(), result);
    assert_eq!(
        fixture.engine.verify_membership_joint_decision().unwrap(),
        result
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), state);
    assert_eq!(canonical_json(fixture.engine.config()).unwrap(), config);
    assert_eq!(fixture.engine.disk_resources.handles().count(), 2);
    assert_blocked(&mut fixture.engine);
}

#[test]
fn membership_joint_independent_quorums_do_not_imply_all_target_completion() {
    for (old, added, new_first, stop, expected) in [
        (2, 2, false, 2, (true, false)),
        (3, 3, true, 4, (false, true)),
        (3, 2, false, 3, (true, true)),
    ] {
        let mut fixture = Fixture::new(old, added, new_first);
        fixture
            .engine
            .inject_fault_once(&format!("after_membership_joint_vote_{stop}"));
        assert!(fixture.engine.resume_membership_joint_decision().is_err());
        let observed = fixture.engine.membership_joint_decision_status().unwrap();
        assert_eq!(
            (
                observed.original_quorum_accepted(),
                observed.target_quorum_accepted()
            ),
            expected
        );
        assert!(!observed.all_certificates_synchronized());
        assert!(observed.certificate_members.is_empty());
        assert_blocked(&mut fixture.engine);
        assert!(fixture.run().all_certificates_synchronized());
    }
}

#[test]
fn membership_joint_each_member_partial_write_sync_and_opening_failure_resume_same_proposal() {
    for index in 0..4 {
        for stage in [
            JointFaultStage::Journal(JournalFault::PartialWrite),
            JointFaultStage::Journal(JournalFault::Sync),
            JointFaultStage::Opening(OpeningFault::FileSync),
            JointFaultStage::Opening(OpeningFault::DirectorySync),
        ] {
            let mut fixture = Fixture::new(2, 2, false);
            fixture.fault(index, stage);
            assert!(fixture.engine.resume_membership_joint_decision().is_err());
            let status = fixture.engine.membership_joint_decision_status().unwrap();
            assert_eq!(status.synchronized_target_votes, index);
            assert!(!status.all_certificates_synchronized());
            let old = (0..index)
                .map(|i| fs::read(fixture.path(i).join("journal.bin")).unwrap())
                .collect::<Vec<_>>();
            fixture.reopen(false);
            let completed = fixture.run();
            assert_eq!(completed.proposal_hash, status.proposal_hash);
            assert!(completed.all_certificates_synchronized());
            for (i, bytes) in old.into_iter().enumerate() {
                assert_eq!(
                    fs::read(fixture.path(i).join("journal.bin")).unwrap(),
                    bytes
                );
            }
            assert_blocked(&mut fixture.engine);
        }
    }
}

#[test]
fn membership_joint_visible_completion_root_failed_confirmation_sync_is_not_counted() {
    for index in 0..4 {
        for fault in [OpeningFault::FileSync, OpeningFault::DirectorySync] {
            let mut fixture = Fixture::new(2, 2, false);
            fixture.fault(index, JointFaultStage::Completion(fault));
            assert!(fixture.engine.resume_membership_joint_decision().is_err());
            assert!(fixture.path(index).join(CERTIFICATE_FILE).is_file());
            let status = fixture.engine.membership_joint_decision_status().unwrap();
            assert!(status.joint_quorum_accepted());
            assert_eq!(status.certificate_members.len(), index);
            assert!(!status.all_certificates_synchronized());
            fixture.reopen(false);
            let completed = fixture.engine.verify_membership_joint_decision();
            if index == 3 {
                assert!(completed.unwrap().all_certificates_synchronized());
            } else {
                assert!(completed.is_err());
            }
            assert!(fixture.run().all_certificates_synchronized());
        }
    }
}

#[test]
fn membership_joint_bound_proposal_and_unbound_unknown_material_are_preserved() {
    let mut fixture = Fixture::new(2, 2, false);
    fixture
        .engine
        .inject_fault_once("after_membership_joint_vote_1");
    assert!(fixture.engine.resume_membership_joint_decision().is_err());
    let path = fixture.path(0);
    let before = files(&path);
    let root = DiskRoot::open(path.parent().unwrap()).unwrap();
    let identity = fixture.engine.pending_membership.as_ref().unwrap().members[0]
        .enrollment
        .identity
        .clone();
    assert!(JointStore::open(&root, &identity, &"e".repeat(64), true).is_err());
    assert_eq!(files(&path), before);
    let unknown = fixture.path(1);
    private_dir(&unknown).unwrap();
    fs::write(unknown.join("unknown"), b"not ours").unwrap();
    assert!(fixture.engine.resume_membership_joint_decision().is_err());
    assert_eq!(fs::read(unknown.join("unknown")).unwrap(), b"not ours");
    assert!(!unknown.join(BINDING_FILE).exists());
}

#[test]
fn membership_joint_foreign_or_modified_vote_and_suffix_fail_without_repair() {
    let mut fixture = Fixture::new(2, 2, false);
    fixture.run();
    let first = fs::read(fixture.path(0).join("journal.bin")).unwrap();
    let path = fixture.path(1).join("journal.bin");
    let original = fs::read(&path).unwrap();
    for bytes in [
        first,
        [original.clone(), b"Z".to_vec()].concat(),
        [original.clone(), original.clone()].concat(),
    ] {
        fs::write(&path, &bytes).unwrap();
        assert!(fixture.engine.resume_membership_joint_decision().is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
    fs::write(&path, &original).unwrap();
    assert!(fixture.run().all_certificates_synchronized());
}

#[test]
fn membership_joint_certificate_missing_local_page_cannot_use_other_target() {
    let mut fixture = Fixture::new(2, 2, false);
    let status = fixture.run();
    let store = JointStore {
        root: DiskRoot::open(&fixture.path(2)).unwrap(),
    };
    let certificate: Certificate = store
        .typed(CERTIFICATE_KIND, status.certificate_hash.as_ref().unwrap())
        .unwrap();
    let page = store
        .object_path(certificate.votes.kind().domain(), certificate.votes.hash())
        .unwrap();
    fs::remove_file(&page).unwrap();
    assert!(fixture.engine.verify_membership_joint_decision().is_err());
    assert!(
        !fixture
            .engine
            .membership_joint_decision_status()
            .unwrap()
            .all_certificates_synchronized()
    );
    assert!(!page.exists());
    // Explicit same-proposal repair can reconstruct the exact hash from all
    // actual local votes, then independently reverify this target again.
    assert!(fixture.run().all_certificates_synchronized());
    assert!(page.is_file());
}

#[test]
fn membership_joint_system_loss_restores_original_fence_then_explicit_joint_resume() {
    let mut fixture = Fixture::new(3, 2, false);
    fixture
        .engine
        .inject_fault_once("after_membership_joint_vote_2");
    assert!(fixture.engine.resume_membership_joint_decision().is_err());
    let proposal = fixture
        .engine
        .membership_joint_decision_status()
        .unwrap()
        .proposal_hash;
    let state = canonical_json(fixture.engine.state()).unwrap();
    fixture.reopen(true);
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), state);
    let status = fixture.run();
    assert_eq!(status.proposal_hash, proposal);
    assert!(status.all_certificates_synchronized());
    assert_blocked(&mut fixture.engine);
}

#[test]
fn membership_joint_control_revalidation_clears_previous_success() {
    let mut fixture = Fixture::new(2, 2, false);
    fixture.run();
    let path = fixture.path(2).parent().unwrap().join("journal.bin");
    let bytes = fs::read(&path).unwrap();
    fs::write(&path, b"torn").unwrap();
    assert!(fixture.engine.resume_membership_joint_decision().is_err());
    assert!(fixture.engine.membership_joint_decision_status().is_none());
    assert_eq!(fs::read(&path).unwrap(), b"torn");
    fs::write(&path, bytes).unwrap();
    assert!(fixture.run().all_certificates_synchronized());
    fixture.engine.resume_pending_members().unwrap();
    assert!(fixture.engine.membership_joint_decision_status().is_none());
}

#[test]
fn membership_joint_member_archive_and_joint_rename_never_reports_false_success() {
    for target in ["member", "archive", "joint"] {
        for stage in ["dependencies", "completion"] {
            let mut fixture = Fixture::new(2, 2, false);
            let disk = &fixture.target.disks[0];
            let point = if stage == "dependencies" {
                format!("after_membership_joint_dependencies_{}", disk.disk_id)
            } else {
                format!("before_membership_joint_completion_{}", disk.disk_id)
            };
            let barrier = Arc::new(std::sync::Barrier::new(2));
            fixture
                .engine
                .inject_pause_once(&point, Arc::clone(&barrier));
            let member = member_directory(disk, &fixture.archive.transition_hash);
            let path = match target {
                "member" => member,
                "archive" => member.join("control-archive"),
                "joint" => member.join("joint"),
                _ => unreachable!(),
            };
            let displaced = path.with_extension("displaced");
            std::thread::scope(|scope| {
                let run = scope.spawn(|| fixture.engine.resume_membership_joint_decision());
                barrier.wait();
                fs::rename(&path, &displaced).unwrap();
                private_dir(&path).unwrap();
                barrier.wait();
                assert!(run.join().unwrap().is_err());
            });
            let status = fixture.engine.membership_joint_decision_status().unwrap();
            assert!(!status.all_certificates_synchronized());
            assert!(status.certificate_members.is_empty());
            if stage == "dependencies" {
                assert!(status.votes.is_empty());
            }
            assert!(files(&path).is_empty());
            assert_blocked(&mut fixture.engine);
        }
    }
}

#[test]
fn membership_joint_each_member_bound_phase_failure_and_real_io_conflict_resume() {
    for index in 0..4 {
        for phase in ["binding", "dependencies"] {
            let mut fixture = Fixture::new(2, 2, false);
            fixture.engine.inject_fault_once(&format!(
                "after_membership_joint_{phase}_{}",
                fixture.target.disks[index].disk_id
            ));
            assert!(fixture.engine.resume_membership_joint_decision().is_err());
            let status = fixture.engine.membership_joint_decision_status().unwrap();
            assert_eq!(status.synchronized_target_votes, index);
            fixture.reopen(false);
            assert_eq!(fixture.run().proposal_hash, status.proposal_hash);
        }
        let mut fixture = Fixture::new(2, 2, false);
        fixture.engine.inject_fault_once(&format!(
            "after_membership_joint_binding_{}",
            fixture.target.disks[index].disk_id
        ));
        assert!(fixture.engine.resume_membership_joint_decision().is_err());
        let obstruction = fixture.path(index).join("objects");
        fs::write(&obstruction, b"directory name occupied").unwrap();
        assert!(fixture.engine.resume_membership_joint_decision().is_err());
        assert_eq!(fs::read(&obstruction).unwrap(), b"directory name occupied");
        // The test removes only its own injected temporary-directory obstruction.
        fs::remove_file(obstruction).unwrap();
        assert!(fixture.run().all_certificates_synchronized());
    }
}

#[test]
fn membership_joint_certificate_rejects_relabelled_duplicate_foreign_and_modified_votes() {
    let mut fixture = Fixture::new(2, 2, false);
    let status = fixture.run();
    let store = JointStore {
        root: DiskRoot::open(&fixture.path(0)).unwrap(),
    };
    let certificate: Certificate = store
        .typed(CERTIFICATE_KIND, status.certificate_hash.as_ref().unwrap())
        .unwrap();
    let original =
        crate::metadata_map::read_map(&certificate.votes, |kind, hash| store.load(kind, hash))
            .unwrap();
    let first = &fixture.target.disks[0].disk_id;
    let second = &fixture.target.disks[1].disk_id;
    for corruption in [
        "duplicate",
        "foreign",
        "slot",
        "generation",
        "acceptance",
        "frame",
        "missing",
    ] {
        let mut votes = original.clone();
        match corruption {
            "duplicate" => {
                votes.insert(second.clone(), votes[first].clone());
            }
            "missing" => {
                votes.remove(second);
            }
            _ => {
                let value = votes.get_mut(second).unwrap();
                match corruption {
                    "foreign" => {
                        value["identity"]["disk_id"] =
                            json_codec::to_value("disk_ffffffffffffffffffffffffffffffff").unwrap()
                    }
                    "slot" => value["identity"]["slot_id"] = json_codec::to_value("P99").unwrap(),
                    "generation" => {
                        value["identity"]["binding_generation"] = json_codec::to_value("2").unwrap()
                    }
                    "acceptance" => {
                        value["acceptance_hash"] = original[first]["acceptance_hash"].clone()
                    }
                    "frame" => {
                        value["receipt"]["frame_hash"] =
                            json_codec::to_value("f".repeat(64)).unwrap()
                    }
                    _ => unreachable!(),
                }
            }
        }
        let mut encoder = Encoder::new(MAX_METADATA);
        let bad = Certificate {
            format: JointFormat::FencedJointDecision,
            proposal_hash: status.proposal_hash.clone(),
            votes: encoder
                .map_root(votes, ProofMeasure::Plain)
                .unwrap()
                .reference()
                .clone(),
        };
        let hash = encoder.object(CERTIFICATE_KIND, &bad).unwrap();
        store.install(&encoder.finish()).unwrap();
        assert!(
            store
                .verify_certificate(&hash, &status.proposal_hash, &fixture.target.disks[0])
                .is_err(),
            "{corruption}"
        );
    }
    assert!(
        fixture
            .engine
            .verify_membership_joint_decision()
            .unwrap()
            .all_certificates_synchronized()
    );
}

#[test]
fn membership_joint_final_sweep_rejects_member_moved_after_its_completion() {
    for target in ["member", "archive", "joint"] {
        let mut fixture = Fixture::new(2, 2, false);
        let barrier = Arc::new(std::sync::Barrier::new(2));
        fixture
            .engine
            .inject_pause_once("after_membership_joint_certificate_1", Arc::clone(&barrier));
        let member = member_directory(&fixture.target.disks[0], &fixture.archive.transition_hash);
        let path = match target {
            "member" => member,
            "archive" => member.join("control-archive"),
            "joint" => member.join("joint"),
            _ => unreachable!(),
        };
        std::thread::scope(|scope| {
            let run = scope.spawn(|| fixture.engine.resume_membership_joint_decision());
            barrier.wait();
            fs::rename(&path, path.with_extension("displaced")).unwrap();
            private_dir(&path).unwrap();
            barrier.wait();
            assert!(run.join().unwrap().is_err());
        });
        assert!(
            !fixture
                .engine
                .membership_joint_decision_status()
                .unwrap()
                .all_certificates_synchronized()
        );
        assert_blocked(&mut fixture.engine);
    }
}

#[test]
fn membership_joint_certificate_manifest_pages_large_synthetic_lists() {
    let mut fixture = Fixture::new(2, 2, false);
    let status = fixture.run();
    let store = JointStore {
        root: DiskRoot::open(&fixture.path(0)).unwrap(),
    };
    let certificate: Certificate = store
        .typed(CERTIFICATE_KIND, status.certificate_hash.as_ref().unwrap())
        .unwrap();
    let votes =
        crate::metadata_map::read_map(&certificate.votes, |kind, hash| store.load(kind, hash))
            .unwrap();
    let value = votes.values().next().unwrap();
    let synthetic = (0..257)
        .map(|n| (format!("synthetic-{n}"), value.clone()))
        .collect();
    let mut encoder = Encoder::new(MAX_METADATA);
    let reference = encoder
        .map_root(synthetic, ProofMeasure::Plain)
        .unwrap()
        .reference()
        .clone();
    let objects = encoder.finish();
    assert!(
        objects
            .iter()
            .filter(|object| object.kind() == "state-page")
            .count()
            > 1
    );
    assert!(
        objects
            .iter()
            .all(|object| object.bytes().len() <= MAX_METADATA)
    );
    store.install(&objects).unwrap();
    assert_eq!(
        crate::metadata_map::read_map(&reference, |kind, hash| store.load(kind, hash))
            .unwrap()
            .len(),
        257
    );
    // This is a bounded-layout test, not 257 real votes or admitted members.
}

#[test]
fn membership_joint_corrupt_binding_dependency_and_completion_are_preserved() {
    let mut fixture = Fixture::new(2, 2, false);
    let status = fixture.run();
    let store = JointStore {
        root: DiskRoot::open(&fixture.path(1)).unwrap(),
    };
    let vote = &status.votes[1];
    for path in [
        store.root.path().join(BINDING_FILE),
        store
            .object_path(PROPOSAL_KIND, &status.proposal_hash)
            .unwrap(),
        store
            .object_path(ACCEPTANCE_KIND, &vote.acceptance_hash)
            .unwrap(),
        store.root.path().join(CERTIFICATE_FILE),
    ] {
        let original = fs::read(&path).unwrap();
        let partial = original[..original.len() / 2].to_vec();
        fs::write(&path, &partial).unwrap();
        let error = fixture
            .engine
            .resume_membership_joint_decision()
            .unwrap_err();
        assert!(error.to_string().contains(&fixture.target.disks[1].disk_id));
        assert!(
            !fixture
                .engine
                .membership_joint_decision_status()
                .unwrap()
                .all_certificates_synchronized()
        );
        assert_eq!(fs::read(&path).unwrap(), partial);
        fs::write(path, original).unwrap();
        assert_eq!(fixture.run(), status);
    }
}
