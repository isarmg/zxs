use super::*;
use std::io::Read;

struct ExitFixture {
    temp: tempfile::TempDir,
    engine: Engine,
    original: Config,
    source: PathBuf,
    archive: MembershipControlArchive,
    operation: OperationId,
    version: String,
}
impl ExitFixture {
    fn new(voters: usize, action: MembershipExitAction) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let original =
            Config::development((0..voters).map(|n| temp.path().join(format!("disk-{n}"))))
                .unwrap();
        let mut engine = Engine::initialize(&temp.path().join("system"), original.clone()).unwrap();
        let repository = engine
            .create_repository(
                &RepositoryName::parse("preserved").unwrap(),
                "P01",
                1,
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        let workspace = engine
            .create_workspace(&repository.main_branch_id, "admin")
            .unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, "body.txt", b"retained exit body")
            .unwrap();
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: OperationId::generate().unwrap(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: String::new(),
                request_digest: None,
            })
            .unwrap();
        let version = engine.list_tree(&repository.main_branch_id).unwrap()[0]
            .version_id
            .clone();
        let operation = OperationId::generate().unwrap();
        engine
            .begin_membership_exit_preparation(
                &original.disks[0].disk_id,
                action,
                "admin",
                operation,
            )
            .unwrap();
        let source = temp.path().join("archive");
        private_dir(&source).unwrap();
        let archive = engine.build_membership_control_archive(&source).unwrap();
        let mut fixture = Self {
            temp,
            engine,
            original,
            source,
            archive,
            operation,
            version,
        };
        fixture.enroll();
        fixture
    }
    fn enroll(&mut self) {
        let status = self
            .engine
            .enroll_pending_members(&self.source, &self.archive.root_hash, vec![])
            .unwrap();
        assert_eq!(status.control_participants, self.original.disks.len());
        assert_eq!(
            status.target_metadata_members,
            self.original.disks.len() - 1
        );
        assert!(status.all_control_metadata_synchronized());
    }
    fn store(&self, index: usize) -> JointStore {
        let pending = self.engine.pending_membership.as_ref().unwrap();
        let member = pending
            .members
            .iter()
            .find(|member| member.disk.disk_id == self.original.disks[index].disk_id)
            .unwrap();
        let parent = member
            .resource
            .root()
            .private_child(&member_directory(&member.disk, &pending.transition_hash))
            .unwrap();
        JointStore::open(
            member.resource.root(),
            &parent,
            &member.enrollment.identity,
            &pending.transition_hash,
            &pending.joint.as_ref().unwrap().status.proposal_hash,
            false,
        )
        .unwrap()
    }
    fn reopen(self, restore: bool) -> Self {
        let Self {
            temp,
            engine,
            original,
            source,
            archive,
            operation,
            version,
        } = self;
        drop(engine);
        let system = temp.path().join("system");
        let engine = if restore {
            fs::rename(&system, temp.path().join("old-system")).unwrap();
            Engine::restore_from_disks(
                &system,
                &original
                    .disks
                    .iter()
                    .map(|disk| disk.root.clone())
                    .collect::<Vec<_>>(),
            )
            .unwrap()
        } else {
            Engine::open(&system).unwrap()
        };
        Self {
            temp,
            engine,
            original,
            source,
            archive,
            operation,
            version,
        }
    }
    fn assert_body(&self) {
        let mut body = Vec::new();
        self.engine
            .open_content(&self.version)
            .unwrap()
            .file
            .read_to_end(&mut body)
            .unwrap();
        assert_eq!(body, b"retained exit body");
    }
}

#[test]
fn membership_joint_exit_union_roles_preserve_registration_body_and_permanent_results() {
    for (old, action) in [
        (3, MembershipExitAction::PlannedOffline),
        (4, MembershipExitAction::Deregister),
    ] {
        let mut fixture = ExitFixture::new(old, action);
        let before = fixture
            .original
            .disks
            .iter()
            .flat_map(|disk| files(&disk.root))
            .collect::<BTreeMap<_, _>>();
        let config = canonical_json(fixture.engine.config()).unwrap();
        let state = canonical_json(fixture.engine.state()).unwrap();
        let result = fixture.engine.resume_membership_joint_decision().unwrap();
        assert_eq!(
            (result.original_members, result.original_quorum),
            (old, old / 2 + 1)
        );
        assert_eq!(
            (result.target_members, result.target_quorum),
            (old - 1, (old - 1) / 2 + 1)
        );
        assert_eq!(result.union_members, old);
        assert_eq!(result.certificate_quorum_counts(), (old, old - 1));
        assert!(result.all_certificates_synchronized());
        let portable = fixture
            .store(0)
            .portable(
                result.certificate_hash.as_ref().unwrap(),
                &result.proposal_hash,
            )
            .unwrap();
        let archive = fixture
            .engine
            .pending_membership
            .as_ref()
            .unwrap()
            .source
            .generation_root();
        let proposed_authority: activation::PortableAuthority =
            json_codec::from_value(json_codec::json!({"joint": portable, "archive": archive}))
                .unwrap();
        assert_eq!(
            proposed_authority.intent().unwrap_err().code(),
            ErrorCode::RecoveryRequired
        );
        assert_eq!(
            result
                .votes
                .iter()
                .filter(|vote| vote.role == MembershipJointRole::OldOnly)
                .count(),
            1
        );
        assert_eq!(
            result
                .votes
                .iter()
                .filter(|vote| vote.role == MembershipJointRole::Retained)
                .count(),
            old - 1
        );
        let intent = &fixture.engine.membership_fence.as_ref().unwrap().intent;
        assert_eq!(intent.target.disks.len(), old);
        assert_eq!(intent.origin.config().disks.len(), old);
        assert!(!intent.target.disks[0].metadata_member);
        assert!(fixture.engine.config.disks[0].metadata_member);
        assert!(
            !fixture
                .engine
                .state
                .operations
                .contains_key(&fixture.operation)
        );
        for (path, bytes) in before {
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
        assert_eq!(canonical_json(fixture.engine.state()).unwrap(), state);
        assert_eq!(canonical_json(fixture.engine.config()).unwrap(), config);
        fixture.assert_body();
        let before = fixture
            .original
            .disks
            .iter()
            .flat_map(|disk| files(&disk.root))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            fixture
                .engine
                .activate_pending_membership()
                .unwrap_err()
                .code(),
            ErrorCode::RecoveryRequired
        );
        assert_eq!(
            fixture
                .original
                .disks
                .iter()
                .flat_map(|disk| files(&disk.root))
                .collect::<BTreeMap<_, _>>(),
            before
        );
        assert_blocked(&mut fixture.engine);
    }
}

fn subset_certificate(
    store: &JointStore,
    status: &MembershipJointDecisionStatus,
    votes: json_codec::Map<String, json_codec::Value>,
) -> String {
    let mut encoder = Encoder::new(MAX_METADATA);
    let certificate = Certificate {
        format: JointFormat::ExitControl,
        proposal_hash: status.proposal_hash.clone(),
        votes: encoder
            .map_root(votes, ProofMeasure::Plain)
            .unwrap()
            .reference()
            .clone(),
    };
    let hash = encoder.object(CERTIFICATE_KIND, &certificate).unwrap();
    store.install(&encoder.finish()).unwrap();
    hash
}

#[test]
fn membership_joint_exit_certificate_counts_fixed_quorums_without_requiring_old_only_vote() {
    for old in [3, 4] {
        let mut fixture = ExitFixture::new(old, MembershipExitAction::PlannedOffline);
        let status = fixture.engine.resume_membership_joint_decision().unwrap();
        let store = fixture.store(0);
        let certificate: Certificate = store
            .typed(CERTIFICATE_KIND, status.certificate_hash.as_ref().unwrap())
            .unwrap();
        let votes =
            crate::metadata_map::read_map(&certificate.votes, |kind, hash| store.load(kind, hash))
                .unwrap();
        // Every subset is made from actual durable votes, not synthetic receipts.
        for selected in 1usize..(1 << old) {
            let subset = fixture
                .original
                .disks
                .iter()
                .enumerate()
                .filter(|(index, _)| selected & (1 << index) != 0)
                .map(|(_, disk)| (disk.disk_id.clone(), votes[&disk.disk_id].clone()))
                .collect::<json_codec::Map<_, _>>();
            let old_votes = selected.count_ones() as usize;
            let target_votes = (selected & !1).count_ones() as usize;
            let expected = old_votes >= old / 2 + 1 && target_votes >= (old - 1) / 2 + 1;
            let hash = subset_certificate(&store, &status, subset);
            let portable = store.portable(&hash, &status.proposal_hash);
            assert_eq!(
                portable.is_ok(),
                expected,
                "old={old} selected={selected:b}"
            );
            if expected {
                portable.unwrap().verify().unwrap();
                if selected & 1 == 0 {
                    // A portable quorum does not invent a local vote for the omitted member.
                    assert!(
                        store
                            .verify_certificate(
                                &hash,
                                &status.proposal_hash,
                                &fixture.original.disks[0]
                            )
                            .is_err()
                    );
                }
            }
        }
        let mut relabelled = votes.clone();
        relabelled.insert(
            fixture.original.disks[1].disk_id.clone(),
            votes[&fixture.original.disks[0].disk_id].clone(),
        );
        let hash = subset_certificate(&store, &status, relabelled);
        assert!(store.portable(&hash, &status.proposal_hash).is_err());
        let mut foreign = votes;
        foreign.insert(
            "disk_ffffffffffffffffffffffffffffffff".into(),
            foreign[&fixture.original.disks[0].disk_id].clone(),
        );
        let hash = subset_certificate(&store, &status, foreign);
        assert!(store.portable(&hash, &status.proposal_hash).is_err());
    }
}

#[test]
fn membership_joint_exit_control_cold_resume_keeps_effect_and_fenced_source_generation() {
    for restore in [false, true] {
        for stop in [1, 2, 3] {
            let mut fixture = ExitFixture::new(3, MembershipExitAction::Deregister);
            fixture
                .engine
                .inject_fault_once(&format!("after_membership_joint_vote_{stop}"));
            assert!(fixture.engine.resume_membership_joint_decision().is_err());
            let proposal = fixture
                .engine
                .membership_joint_decision_status()
                .unwrap()
                .proposal_hash;
            let intent =
                canonical_json(&fixture.engine.membership_fence.as_ref().unwrap().intent).unwrap();
            let mut fixture = fixture.reopen(restore);
            assert_eq!(
                canonical_json(&fixture.engine.membership_fence.as_ref().unwrap().intent).unwrap(),
                intent
            );
            fixture.assert_body();
            assert_blocked(&mut fixture.engine);
            assert_eq!(
                fixture
                    .engine
                    .begin_membership_exit_preparation(
                        &fixture.original.disks[0].disk_id,
                        MembershipExitAction::PlannedOffline,
                        "admin",
                        fixture.operation
                    )
                    .unwrap_err()
                    .code(),
                ErrorCode::IdempotencyKeyReused
            );
            fixture
                .engine
                .begin_membership_exit_preparation(
                    &fixture.original.disks[0].disk_id,
                    MembershipExitAction::Deregister,
                    "admin",
                    fixture.operation,
                )
                .unwrap();
            fixture.enroll();
            let completed = fixture.engine.resume_membership_joint_decision().unwrap();
            assert_eq!(completed.proposal_hash, proposal);
            assert!(completed.all_certificates_synchronized());
            assert!(
                !fixture
                    .engine
                    .state
                    .operations
                    .contains_key(&fixture.operation)
            );
            assert_eq!(
                fixture
                    .engine
                    .activate_pending_membership()
                    .unwrap_err()
                    .code(),
                ErrorCode::RecoveryRequired
            );
        }
    }
}

#[test]
fn membership_joint_exit_target_preserves_exact_topology_and_requires_typed_effect() {
    let mut fixture = ExitFixture::new(3, MembershipExitAction::Deregister);
    fixture.engine.resume_membership_joint_decision().unwrap();
    let fence = fixture.engine.membership_fence.as_ref().unwrap();
    for mutation in ["remove", "root", "binding", "extra-voter"] {
        let mut target = (*fence.intent.target).clone();
        match mutation {
            "remove" => {
                target.disks.pop();
            }
            "root" => target.disks[0].root = fixture.temp.path().join("other"),
            "binding" => target.disks[0].binding_generation += 1,
            "extra-voter" => target.disks[1].metadata_member = false,
            _ => unreachable!(),
        }
        let target = ValidatedConfig::try_from(target).unwrap();
        assert!(
            fence
                .intent
                .effect
                .as_ref()
                .unwrap()
                .validate(fence.intent.origin.config(), &target)
                .is_err(),
            "{mutation}"
        );
    }
    let store = fixture.store(0);
    let status = fixture.engine.membership_joint_decision_status().unwrap();
    let mut proposal: Proposal = store.typed(PROPOSAL_KIND, &status.proposal_hash).unwrap();
    proposal.format = JointFormat::Joint;
    let object = PreparedMetadataObject::new(PROPOSAL_KIND, &proposal, MAX_METADATA).unwrap();
    store.install(std::slice::from_ref(&object)).unwrap();
    assert!(store.verify_proposal(&object.hash).is_err());
}

#[test]
fn membership_joint_exit_last_voter_gap_does_not_write_a_fence() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([temp.path().join("data")]).unwrap();
    let mut engine = Engine::initialize(&temp.path().join("system"), config.clone()).unwrap();
    let before = files(temp.path());
    let error = engine
        .begin_membership_exit_preparation(
            &config.disks[0].disk_id,
            MembershipExitAction::Deregister,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::RecoveryRequired);
    assert!(error.to_string().contains("terminal pool authority"));
    assert!(engine.membership_fence.is_none());
    assert!(!engine.is_read_only());
    assert_eq!(files(temp.path()), before);
}

#[test]
fn membership_joint_exit_partial_first_fence_cold_resume_preserves_exact_intent() {
    let temp = tempfile::tempdir().unwrap();
    let config =
        Config::development((0..3).map(|n| temp.path().join(format!("data-{n}")))).unwrap();
    let system = temp.path().join("system");
    let mut engine = Engine::initialize(&system, config.clone()).unwrap();
    let operation = OperationId::generate().unwrap();
    engine.inject_fault_once("after_membership_fence_1");
    assert!(
        engine
            .begin_membership_exit_preparation(
                &config.disks[0].disk_id,
                MembershipExitAction::PlannedOffline,
                "admin",
                operation
            )
            .is_err()
    );
    let intent = canonical_json(&engine.membership_fence.as_ref().unwrap().intent).unwrap();
    drop(engine);
    let mut engine = Engine::open(&system).unwrap();
    assert_eq!(
        canonical_json(&engine.membership_fence.as_ref().unwrap().intent).unwrap(),
        intent
    );
    assert_blocked(&mut engine);
    let fence = engine
        .begin_membership_exit_preparation(
            &config.disks[0].disk_id,
            MembershipExitAction::PlannedOffline,
            "admin",
            operation,
        )
        .unwrap();
    assert!(fence.all_members_fenced());
    let source = temp.path().join("archive");
    private_dir(&source).unwrap();
    let archive = engine.build_membership_control_archive(&source).unwrap();
    engine
        .enroll_pending_members(&source, &archive.root_hash, vec![])
        .unwrap();
    let status = engine.resume_membership_joint_decision().unwrap();
    assert_eq!(status.certificate_quorum_counts(), (3, 2));
    assert!(status.all_certificates_synchronized());
    assert!(!engine.state.operations.contains_key(&operation));
}

#[test]
fn membership_joint_exit_nonvoter_effect_keeps_votes_and_registration() {
    let temp = tempfile::tempdir().unwrap();
    let mut config =
        Config::development((0..3).map(|n| temp.path().join(format!("data-{n}")))).unwrap();
    config.disks[0].metadata_member = false;
    let mut engine = Engine::initialize(&temp.path().join("system"), config.clone()).unwrap();
    let operation = OperationId::generate().unwrap();
    engine
        .begin_membership_exit_preparation(
            &config.disks[0].disk_id,
            MembershipExitAction::PlannedOffline,
            "admin",
            operation,
        )
        .unwrap();
    let source = temp.path().join("archive");
    private_dir(&source).unwrap();
    let archive = engine.build_membership_control_archive(&source).unwrap();
    let pending = engine
        .enroll_pending_members(&source, &archive.root_hash, vec![])
        .unwrap();
    assert_eq!(
        (
            pending.control_participants,
            pending.target_metadata_members
        ),
        (2, 2)
    );
    let status = engine.resume_membership_joint_decision().unwrap();
    assert_eq!(status.certificate_quorum_counts(), (2, 2));
    assert!(
        status
            .votes
            .iter()
            .all(|vote| vote.role == MembershipJointRole::Retained)
    );
    assert_eq!(
        canonical_json(engine.config()).unwrap(),
        canonical_json(&config).unwrap()
    );
    assert_eq!(engine.disk_resources.handles().count(), 3);
    assert_eq!(
        engine.activate_pending_membership().unwrap_err().code(),
        ErrorCode::RecoveryRequired
    );
    assert!(!engine.state.operations.contains_key(&operation));
}

#[test]
fn membership_joint_exit_successor_fence_cannot_resume_predecessor_activation() {
    let mut fixture = Fixture::new(2, 1, false);
    fixture.run();
    fixture.engine.activate_pending_membership().unwrap();
    let predecessor = fixture.engine.config.generation_hash().unwrap().to_owned();
    let operation = OperationId::generate().unwrap();
    fixture
        .engine
        .begin_membership_exit_preparation(
            &fixture.target.disks[0].disk_id,
            MembershipExitAction::PlannedOffline,
            "admin",
            operation,
        )
        .unwrap();
    assert_eq!(
        fixture
            .engine
            .membership_fence
            .as_ref()
            .unwrap()
            .intent
            .origin
            .predecessor(),
        Some(predecessor.as_str())
    );
    let source = fixture.temp.path().join("exit-archive");
    private_dir(&source).unwrap();
    let archive = fixture
        .engine
        .build_membership_control_archive(&source)
        .unwrap();
    fixture
        .engine
        .enroll_pending_members(&source, &archive.root_hash, vec![])
        .unwrap();
    fixture.engine.resume_membership_joint_decision().unwrap();
    let Fixture {
        temp,
        engine,
        target,
        ..
    } = fixture;
    drop(engine);
    let before = target
        .disks
        .iter()
        .flat_map(|disk| files(&disk.root))
        .collect::<BTreeMap<_, _>>();
    let error = match Engine::resume_membership_activation(&temp.path().join("system")) {
        Err(error) => error,
        Ok(_) => panic!("exit fence resumed predecessor activation"),
    };
    assert_eq!(error.code(), ErrorCode::RecoveryRequired);
    assert!(
        error
            .to_string()
            .contains("current generation remains fenced")
    );
    assert_eq!(
        target
            .disks
            .iter()
            .flat_map(|disk| files(&disk.root))
            .collect::<BTreeMap<_, _>>(),
        before
    );
    let mut reopened = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(
        reopened.config.generation_hash(),
        Some(predecessor.as_str())
    );
    assert!(!reopened.state.operations.contains_key(&operation));
    assert_blocked(&mut reopened);
}
