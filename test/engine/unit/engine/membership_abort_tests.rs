use super::*;
use crate::engine::membership_transition::decision::DecisionKind;
use std::{
    fs,
    path::{Path, PathBuf},
};

#[path = "membership_abort_authority_tests.rs"]
pub(in crate::engine::membership_transition) mod authority_reads;

struct Fixture {
    temp: tempfile::TempDir,
    original: Config,
    target: Config,
    engine: Engine,
    source: PathBuf,
    archive: MembershipControlArchive,
}

impl Fixture {
    fn new(old: usize) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let original =
            Config::development((0..old).map(|n| temp.path().join(format!("old-{n}")))).unwrap();
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
            Config::development((0..=old).map(|n| temp.path().join(format!("new-{n}")))).unwrap();
        let disk = additional.disks.into_iter().last().unwrap();
        private_dir(&disk.root).unwrap();
        target.disks.push(disk);
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

    fn context(&self) -> AbortContext {
        AbortContext::from_archive(
            VerifiedAbortArchive::read(DiskRoot::open(&self.source).unwrap(), &self.archive)
                .unwrap(),
        )
        .unwrap()
    }

    fn root(&self, index: usize) -> &DiskRoot {
        &self
            .engine
            .disk_resources
            .get(&self.original.disks[index])
            .unwrap()
            .root
    }

    fn id(&self, index: usize) -> &str {
        &self.original.disks[index].disk_id
    }

    fn store(&self, context: &AbortContext, index: usize) -> DecisionStore {
        DecisionStore::open(
            self.root(index),
            &context.acceptance(self.id(index)).unwrap().identity,
            &context.core.transition_hash,
            true,
        )
        .unwrap()
    }

    fn receipt(&self, context: &AbortContext, index: usize) -> DecisionReceipt {
        let acceptance = context.acceptance(self.id(index)).unwrap();
        self.store(context, index)
            .receipt(
                DecisionKind::Abort,
                &context.generation.hash,
                &object_hash(ACCEPTANCE_KIND, acceptance).unwrap(),
                true,
                #[cfg(feature = "fault-injection")]
                None,
            )
            .unwrap()
    }

    fn vote(&self, context: &AbortContext, index: usize) -> ObservedAbortVote {
        context
            .observe_vote(
                self.id(index),
                self.root(index),
                self.receipt(context, index),
            )
            .unwrap()
    }

    fn votes(&self, context: &AbortContext, indices: &[usize]) -> Vec<ObservedAbortVote> {
        indices
            .iter()
            .map(|&index| self.vote(context, index))
            .collect()
    }

    fn joint(&self, context: &AbortContext, index: usize) -> DecisionReceipt {
        self.store(context, index)
            .receipt(
                DecisionKind::Joint,
                &"a".repeat(64),
                &"b".repeat(64),
                true,
                #[cfg(feature = "fault-injection")]
                None,
            )
            .unwrap()
    }

    fn decision_path(&self, index: usize) -> PathBuf {
        self.original.disks[index]
            .root
            .join("metadata/membership-decisions")
            .join(&self.archive.transition_hash)
            .join("decision.json")
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

fn assert_preserved(before: &BTreeMap<PathBuf, Vec<u8>>) {
    for (path, bytes) in before {
        assert_eq!(&fs::read(path).unwrap(), bytes, "{}", path.display());
    }
}

// Rebuild valid content-addressed containers around deliberately invalid vote
// values. A rejection must be semantic, not merely a stale outer hash.
fn evidence_with(
    context: &AbortContext,
    entries: Vec<(String, AbortVote, AbortAcceptance)>,
) -> PortableAbortEvidence {
    let mut encoder = Encoder::new(MAX_METADATA);
    let mut votes = json_codec::Map::new();
    for (key, vote, acceptance) in entries {
        encoder.object(ACCEPTANCE_KIND, &acceptance).unwrap();
        assert!(
            votes
                .insert(key, json_codec::to_value(vote).unwrap())
                .is_none()
        );
    }
    let votes = encoder
        .map_root(votes, ProofMeasure::Plain)
        .unwrap()
        .reference()
        .clone();
    let certificate_hash = encoder
        .object(
            CERTIFICATE_KIND,
            &AbortCertificate {
                format: CertificateFormat::AbortCertificate,
                core_hash: context.generation.hash.clone(),
                votes,
            },
        )
        .unwrap();
    let objects = encoder
        .finish()
        .into_iter()
        .map(|object| {
            (
                format!("{}/{}", object.kind(), object.hash),
                decode_strict(object.bytes()).unwrap(),
            )
        })
        .collect();
    PortableAbortEvidence {
        core_hash: context.generation.hash.clone(),
        certificate_hash,
        objects,
    }
}

fn entries(
    context: &AbortContext,
    votes: &[ObservedAbortVote],
) -> Vec<(String, AbortVote, AbortAcceptance)> {
    votes
        .iter()
        .map(|vote| {
            (
                vote.disk_id.clone(),
                vote.vote.clone(),
                context.acceptance(&vote.disk_id).unwrap().clone(),
            )
        })
        .collect()
}

#[test]
fn membership_abort_distinct_majorities_share_core_and_prescribed_outcome() {
    let fixture = Fixture::new(3);
    let context = fixture.context();
    let first = context.certify(&fixture.votes(&context, &[0, 1])).unwrap();
    let second = context.certify(&fixture.votes(&context, &[1, 2])).unwrap();
    let all = context
        .certify(&fixture.votes(&context, &[2, 0, 1]))
        .unwrap();
    assert_ne!(first.certificate_hash, second.certificate_hash);
    assert_ne!(first.certificate_hash, all.certificate_hash);
    for evidence in [&first, &second, &all] {
        let verified = evidence.verify(&context).unwrap();
        assert_eq!(verified.core_hash, context.generation.hash);
        assert_eq!(verified.certificate_hash, evidence.certificate_hash);
        assert_eq!(evidence.core_hash, context.generation.hash);
    }
    let reordered = context.certify(&fixture.votes(&context, &[1, 0])).unwrap();
    assert_eq!(reordered.certificate_hash, first.certificate_hash);
    assert_eq!(
        canonical_json(&reordered).unwrap(),
        canonical_json(&first).unwrap()
    );
    let reloaded = fixture.context();
    assert_eq!(reloaded.generation.bytes(), context.generation.bytes());
    assert_eq!(
        reloaded
            .prescribed_outcome(fixture.engine.state.topology_generation)
            .unwrap(),
        context
            .prescribed_outcome(fixture.engine.state.topology_generation)
            .unwrap()
    );
    let intent = &fixture.engine.membership_fence.as_ref().unwrap().intent;
    assert_eq!(
        context
            .prescribed_outcome(fixture.engine.state.topology_generation)
            .unwrap()
            .operation_id,
        intent.operation_id
    );
    assert_eq!(
        context
            .prescribed_outcome(fixture.engine.state.topology_generation)
            .unwrap()
            .principal,
        intent.principal
    );
    assert_eq!(
        context
            .prescribed_outcome(fixture.engine.state.topology_generation)
            .unwrap()
            .request_digest,
        fixture.archive.transition_hash
    );
    assert_eq!(
        context
            .prescribed_outcome(fixture.engine.state.topology_generation)
            .unwrap()
            .sequence,
        intent.fence_sequence + 1
    );
    assert_eq!(
        context
            .prescribed_outcome(fixture.engine.state.topology_generation)
            .unwrap()
            .kind(),
        "membership-add-cancelled"
    );
    assert_eq!(
        context
            .prescribed_outcome(fixture.engine.state.topology_generation)
            .unwrap()
            .topology_generation,
        fixture.engine.state.topology_generation + 1
    );
    assert_eq!(
        canonical_json(context.effective_config()).unwrap(),
        canonical_json(&fixture.original).unwrap()
    );
    context.verify_core(context.generation.bytes()).unwrap();
}

#[test]
fn membership_abort_reforms_witness_without_first_voter_or_failed_target() {
    let fixture = Fixture::new(3);
    let context = fixture.context();
    let first = context.certify(&fixture.votes(&context, &[0, 1])).unwrap();
    fs::rename(
        &fixture.original.disks[0].root,
        fixture.temp.path().join("lost-old"),
    )
    .unwrap();
    fs::rename(
        &fixture.target.disks[3].root,
        fixture.temp.path().join("lost-target"),
    )
    .unwrap();
    // Full F supplies the authenticated previous view without touching any
    // target path or requiring the first certificate's exact voter subset.
    let recovered = fixture.context();
    let replacement = recovered
        .certify(&fixture.votes(&recovered, &[1, 2]))
        .unwrap();
    assert_eq!(first.core_hash, replacement.core_hash);
    assert_ne!(first.certificate_hash, replacement.certificate_hash);
    assert_eq!(replacement.verify(&recovered).unwrap().voters.len(), 2);
    assert!(fixture.engine.is_read_only());
}

#[test]
fn membership_abort_minor_joint_can_remain_but_no_joint_majority_can_double_vote() {
    for (old, joint, can_abort) in [(5, 2, true), (4, 2, false), (3, 2, false)] {
        let fixture = Fixture::new(old);
        let context = fixture.context();
        let mut decisions = Vec::new();
        for index in 0..joint {
            decisions.push(fixture.joint(&context, index));
            assert!(
                fixture
                    .store(&context, index)
                    .receipt(
                        DecisionKind::Abort,
                        &context.generation.hash,
                        &object_hash(
                            ACCEPTANCE_KIND,
                            context.acceptance(fixture.id(index)).unwrap()
                        )
                        .unwrap(),
                        true,
                        #[cfg(feature = "fault-injection")]
                        None,
                    )
                    .is_err()
            );
        }
        let unchanged = (0..joint)
            .map(|i| {
                (
                    fixture.decision_path(i),
                    fs::read(fixture.decision_path(i)).unwrap(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let voters = (joint..old).collect::<Vec<_>>();
        let observed = fixture.votes(&context, &voters);
        let result = context.certify(&observed);
        assert_eq!(result.is_ok(), can_abort, "N={old}, Joint={joint}");
        assert_eq!(context.quorum(), old / 2 + 1);
        if let Ok(evidence) = result {
            evidence.verify(&context).unwrap();
        }
        for (index, decision) in decisions.into_iter().enumerate() {
            fixture
                .store(&context, index)
                .verify_receipt(&decision, true)
                .unwrap();
            assert!(
                context
                    .observe_vote(fixture.id(index), fixture.root(index), decision)
                    .is_err()
            );
        }
        assert_preserved(&unchanged);
    }
}

#[test]
fn membership_abort_partial_abort_blocks_joint_and_keeps_original_bytes() {
    let mut fixture = Fixture::new(3);
    let context = fixture.context();
    let original = fixture
        .original
        .disks
        .iter()
        .flat_map(|disk| files(&disk.root))
        .collect::<BTreeMap<_, _>>();
    let archived = files(&fixture.source);
    let reserved = files(&fixture.target.disks[3].root);
    let state = canonical_json(&*fixture.engine.state).unwrap();
    let votes = fixture.votes(&context, &[0]);
    assert!(context.certify(&votes).is_err());
    let store = fixture.store(&context, 0);
    assert!(store.require_joint_allowed().is_err());
    assert!(
        store
            .receipt(
                DecisionKind::Joint,
                &"a".repeat(64),
                &"b".repeat(64),
                true,
                #[cfg(feature = "fault-injection")]
                None,
            )
            .is_err()
    );
    let complete = context.certify(&fixture.votes(&context, &[0, 1])).unwrap();
    complete.verify(&context).unwrap();
    assert!(
        fixture
            .engine
            .create_repository(
                &RepositoryName::parse("still-blocked").unwrap(),
                "P01",
                1,
                "admin",
                OperationId::generate().unwrap()
            )
            .is_err()
    );
    assert!(fixture.engine.membership_fence.is_some());
    assert_eq!(canonical_json(&*fixture.engine.state).unwrap(), state);
    assert_preserved(&original);
    assert_eq!(files(&fixture.source), archived);
    assert_eq!(files(&fixture.target.disks[3].root), reserved);
}

#[test]
fn membership_abort_rejects_empty_incomplete_duplicate_and_mixed_core_observations() {
    let fixture = Fixture::new(3);
    let context = fixture.context();
    assert!(context.certify(&[]).is_err());
    assert!(context.certify(&fixture.votes(&context, &[0])).is_err());
    assert!(context.certify(&fixture.votes(&context, &[0, 0])).is_err());
    let other = Fixture::new(3);
    let foreign = other.context();
    assert!(
        context
            .certify(&[fixture.vote(&context, 0), other.vote(&foreign, 1)])
            .is_err()
    );
    let first = fixture.vote(&context, 0);
    let mut changed = fixture.vote(&context, 1);
    changed.vote.decision = other.receipt(&foreign, 1);
    assert!(context.certify(&[first, changed]).is_err());
    assert!(
        context
            .acceptance(&fixture.target.disks[3].disk_id)
            .is_err()
    );
    assert!(context.acceptance("invented-member").is_err());
}

#[test]
fn membership_abort_certificate_rejects_relabelled_foreign_incomplete_and_mixed_votes() {
    let fixture = Fixture::new(3);
    let context = fixture.context();
    let observed = fixture.votes(&context, &[0, 1]);
    let valid = entries(&context, &observed);
    assert!(
        evidence_with(&context, valid[..1].to_vec())
            .verify(&context)
            .is_err()
    );
    for key in [
        fixture.id(1),
        &fixture.target.disks[3].disk_id,
        "invented-member",
    ] {
        let mut invalid = valid.clone();
        invalid[1] = invalid[0].clone();
        invalid[1].0 = key.to_owned();
        assert!(
            evidence_with(&context, invalid).verify(&context).is_err(),
            "{key}"
        );
    }
    let other = Fixture::new(3);
    let foreign = other.context();
    for field in [
        "identity",
        "original_fence",
        "original_seal_hash",
        "core_hash",
    ] {
        let mut invalid = valid.clone();
        let replacement = foreign.acceptance(other.id(1)).unwrap();
        let mut value = json_codec::to_value(&invalid[1].2).unwrap();
        value[field] = json_codec::to_value(replacement).unwrap()[field].clone();
        invalid[1].2 = json_codec::from_value(value).unwrap();
        invalid[1].1.acceptance_hash = object_hash(ACCEPTANCE_KIND, &invalid[1].2).unwrap();
        assert!(
            evidence_with(&context, invalid).verify(&context).is_err(),
            "{field}"
        );
    }
    let mut joint = valid.clone();
    joint[1].1.decision = fixture.joint(&context, 2);
    assert!(evidence_with(&context, joint).verify(&context).is_err());
    let mut mixed = valid;
    mixed[1].1.decision = other.receipt(&foreign, 1);
    assert!(evidence_with(&context, mixed).verify(&context).is_err());
}

#[test]
fn membership_abort_decision_full_identity_intent_choice_and_digest_are_bound() {
    let fixture = Fixture::new(3);
    let context = fixture.context();
    let decision = fixture.receipt(&context, 0);
    let baseline = json_codec::to_value(&decision).unwrap();
    for (field, value) in [
        ("pool_id", "pool_other"),
        ("disk_id", "disk_other"),
        ("slot_id", "other"),
        ("binding_generation", "2"),
        ("device", "0"),
        ("inode", "0"),
    ] {
        let mut forged = baseline.clone();
        forged["record"]["identity"][field] = value.into();
        forged["record_hash"] = object_hash("membership-decision", &forged["record"])
            .unwrap()
            .into();
        let receipt = json_codec::from_value(forged).unwrap();
        assert!(
            context
                .observe_vote(fixture.id(0), fixture.root(0), receipt)
                .is_err(),
            "{field}"
        );
    }
    for path in [
        "transition_hash",
        "core_hash",
        "acceptance_hash",
        "record_hash",
        "simulation",
    ] {
        let mut forged = baseline.clone();
        match path {
            "record_hash" => forged[path] = "f".repeat(64).into(),
            "simulation" => {
                forged["record"]["identity"][path] = false.into();
                forged["record_hash"] = object_hash("membership-decision", &forged["record"])
                    .unwrap()
                    .into();
            }
            "transition_hash" => {
                forged["record"][path] = "f".repeat(64).into();
                forged["record_hash"] = object_hash("membership-decision", &forged["record"])
                    .unwrap()
                    .into();
            }
            _ => {
                forged["record"]["choice"][path] = "f".repeat(64).into();
                forged["record_hash"] = object_hash("membership-decision", &forged["record"])
                    .unwrap()
                    .into();
            }
        }
        let receipt = json_codec::from_value(forged).unwrap();
        assert!(
            context
                .observe_vote(fixture.id(0), fixture.root(0), receipt)
                .is_err(),
            "{path}"
        );
    }
    assert!(
        context
            .observe_vote(fixture.id(1), fixture.root(1), decision)
            .is_err()
    );
}

#[test]
fn membership_abort_detached_closure_rejects_missing_corrupt_extra_and_unknown_formats() {
    let fixture = Fixture::new(3);
    let context = fixture.context();
    let valid = context.certify(&fixture.votes(&context, &[0, 1])).unwrap();
    for key in valid.objects.keys() {
        let mut missing = valid.clone();
        missing.objects.remove(key);
        assert!(missing.verify(&context).is_err(), "missing {key}");
        let mut damaged = valid.clone();
        damaged.objects.insert(key.clone(), json_codec::Value::Null);
        assert!(damaged.verify(&context).is_err(), "damaged {key}");
    }
    let mut extra = valid.clone();
    extra
        .objects
        .insert("unrelated/object".into(), json_codec::Value::Null);
    assert!(extra.verify(&context).is_err());
    let mut mixed = valid.clone();
    mixed.core_hash = "0".repeat(64);
    assert!(mixed.verify(&context).is_err());
    let mut wrong_certificate = valid;
    let key = format!("{CERTIFICATE_KIND}/{}", wrong_certificate.certificate_hash);
    let mut certificate = wrong_certificate.objects.remove(&key).unwrap();
    certificate["core_hash"] = "0".repeat(64).into();
    wrong_certificate.certificate_hash = object_hash(CERTIFICATE_KIND, &certificate).unwrap();
    wrong_certificate.objects.insert(
        format!("{CERTIFICATE_KIND}/{}", wrong_certificate.certificate_hash),
        certificate,
    );
    assert!(wrong_certificate.verify(&context).is_err());
    let mut core = json_codec::to_value(&context.core).unwrap();
    core["format"] = "legacy-abort".into();
    assert!(
        context
            .verify_core(&canonical_json(&core).unwrap())
            .is_err()
    );
    core = json_codec::to_value(&context.core).unwrap();
    core["witness"] = "not-part-of-the-core".into();
    assert!(
        context
            .verify_core(&canonical_json(&core).unwrap())
            .is_err()
    );
    core = json_codec::to_value(&context.core).unwrap();
    core["transition_hash"] = "0".repeat(64).into();
    assert!(
        context
            .verify_core(&canonical_json(&core).unwrap())
            .is_err()
    );
}

#[test]
fn membership_abort_cached_receipt_never_substitutes_for_actual_named_slot() {
    for corrupt in [b"{".as_slice(), b"".as_slice()] {
        let fixture = Fixture::new(3);
        let context = fixture.context();
        let votes = fixture.votes(&context, &[0, 1]);
        fs::write(fixture.decision_path(1), corrupt).unwrap();
        assert!(context.certify(&votes).is_err());
        assert_eq!(fs::read(fixture.decision_path(1)).unwrap(), corrupt);
    }
    let fixture = Fixture::new(3);
    let context = fixture.context();
    let votes = fixture.votes(&context, &[0, 1]);
    fs::rename(
        fixture.decision_path(1),
        fixture.temp.path().join("detached-receipt"),
    )
    .unwrap();
    assert!(context.certify(&votes).is_err());
    assert!(!fixture.decision_path(1).exists());
}

#[test]
fn membership_abort_archive_requires_exact_complete_frozen_evidence() {
    let fixture = Fixture::new(3);
    let before = files(&fixture.source);
    for changed in [true, false] {
        let mut expected = fixture.archive.clone();
        if changed {
            expected.root_hash = "0".repeat(64);
        } else {
            expected.transition_hash = "0".repeat(64);
        }
        assert!(
            VerifiedAbortArchive::read(DiskRoot::open(&fixture.source).unwrap(), &expected)
                .is_err()
        );
    }
    assert_eq!(files(&fixture.source), before);
    let journal = before
        .keys()
        .find(|path| path.file_name().unwrap() == "journal.bin")
        .unwrap();
    fs::write(journal, b"{").unwrap();
    assert!(
        VerifiedAbortArchive::read(DiskRoot::open(&fixture.source).unwrap(), &fixture.archive)
            .is_err()
    );
    assert_eq!(fs::read(journal).unwrap(), b"{");
}

#[test]
fn membership_abort_fabricated_valid_hash_and_staging_do_not_create_a_vote() {
    let fixture = Fixture::new(3);
    let context = fixture.context();
    let _neutral = fixture.store(&context, 0);
    let acceptance = context.acceptance(fixture.id(0)).unwrap();
    let record = json_codec::json!({
        "format": "fenced-membership-decision",
        "transition_hash": context.core.transition_hash,
        "identity": acceptance.identity,
        "choice": {
            "kind": "Abort",
            "core_hash": context.generation.hash,
            "acceptance_hash": object_hash(ACCEPTANCE_KIND, acceptance).unwrap()
        }
    });
    let forged: DecisionReceipt = json_codec::from_value(json_codec::json!({
        "record_hash": object_hash("membership-decision", &record).unwrap(),
        "record": record
    }))
    .unwrap();
    let staging = fixture
        .decision_path(0)
        .parent()
        .unwrap()
        .join(format!(".state-atomic-{}.tmp", "d".repeat(32)));
    fs::write(&staging, canonical_json(&record).unwrap()).unwrap();
    let bytes = fs::read(&staging).unwrap();
    assert!(
        context
            .observe_vote(fixture.id(0), fixture.root(0), forged)
            .is_err()
    );
    assert!(!fixture.decision_path(0).exists());
    assert_eq!(fs::read(staging).unwrap(), bytes);
}

#[test]
fn membership_abort_current_seal_must_still_match_the_frozen_identity() {
    let fixture = Fixture::new(3);
    let context = fixture.context();
    let votes = fixture.votes(&context, &[0, 1]);
    let path = fixture.original.disks[1].root.join("disk.json");
    let mut seal: DiskSeal = decode_seal(&fs::read(&path).unwrap()).unwrap();
    seal.binding_generation += 1;
    let corrupt = canonical_json(&seal).unwrap();
    fs::write(&path, &corrupt).unwrap();
    assert!(context.certify(&votes).is_err());
    assert_eq!(fs::read(path).unwrap(), corrupt);
}

#[test]
fn membership_abort_rejects_membership_removal_and_exit_effect_archives() {
    for effect in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let original =
            Config::development((0..3).map(|n| temp.path().join(format!("old-{n}")))).unwrap();
        let mut engine = Engine::initialize(&temp.path().join("system"), original.clone()).unwrap();
        let operation = OperationId::generate().unwrap();
        if effect {
            engine
                .begin_membership_exit_preparation(
                    &original.disks[2].disk_id,
                    MembershipExitAction::PlannedOffline,
                    "admin",
                    operation,
                )
                .unwrap();
        } else {
            let mut target = original.clone();
            target.disks[2].metadata_member = false;
            engine
                .begin_membership_handoff_with_effect(
                    ValidatedConfig::try_from(target).unwrap(),
                    None,
                    "admin",
                    operation,
                )
                .unwrap();
        }
        let source = temp.path().join("archive");
        private_dir(&source).unwrap();
        let expected = engine.build_membership_control_archive(&source).unwrap();
        let verified =
            VerifiedAbortArchive::read(DiskRoot::open(&source).unwrap(), &expected).unwrap();
        let before = files(&source);
        assert!(AbortContext::from_archive(verified).is_err());
        assert_eq!(files(&source), before);
    }
}

#[test]
fn membership_abort_successor_binds_authenticated_predecessor_and_original_config() {
    let mut fixture = Fixture::new(3);
    let first_core = fixture.context().generation.hash;
    fixture
        .engine
        .enroll_pending_members(
            &fixture.source,
            &fixture.archive.root_hash,
            vec![PendingMemberDirectory {
                disk_id: fixture.target.disks[3].disk_id.clone(),
                directory: fs::File::open(&fixture.target.disks[3].root).unwrap(),
            }],
        )
        .unwrap();
    fixture.engine.resume_membership_joint_decision().unwrap();
    fixture.engine.activate_pending_membership().unwrap();
    let predecessor = fixture.engine.config.generation_hash().unwrap().to_owned();
    fixture.original = fixture.target.clone();
    let extra = Config::development((0..5).map(|n| fixture.temp.path().join(format!("later-{n}"))))
        .unwrap();
    let added = extra.disks.into_iter().last().unwrap();
    private_dir(&added.root).unwrap();
    fixture.target.disks.push(added);
    fixture
        .engine
        .begin_membership_handoff(
            fixture.target.clone(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    fixture.source = fixture.temp.path().join("later-archive");
    private_dir(&fixture.source).unwrap();
    fixture.archive = fixture
        .engine
        .build_membership_control_archive(&fixture.source)
        .unwrap();
    let before = files(&fixture.source);
    let context = fixture.context();
    assert_eq!(
        context.intent.origin.predecessor(),
        Some(predecessor.as_str())
    );
    assert_eq!(context.effective_config().metadata_members().len(), 4);
    assert_eq!(context.quorum(), 3);
    assert_ne!(context.generation.hash, first_core);
    let one = context
        .certify(&fixture.votes(&context, &[0, 1, 2]))
        .unwrap();
    let two = context
        .certify(&fixture.votes(&context, &[1, 2, 3]))
        .unwrap();
    assert_eq!(
        one.verify(&context).unwrap().core_hash,
        two.verify(&context).unwrap().core_hash
    );
    assert_ne!(one.certificate_hash, two.certificate_hash);
    let previous = &fixture.engine.config.generation().unwrap().portable;
    let previous_bytes = canonical_json(previous).unwrap();
    let previous_hash = previous.hash().unwrap();
    // The first Add is still exactly its original core, with no enum tag or
    // detached wrapper introduced by an Abort successor.
    assert_eq!(previous.node_objects().unwrap()[0].bytes(), previous_bytes);
    let portable = pending::activation::PortableAuthority::abort_for_test(
        context.core.clone(),
        AbortAuthorityEvidence {
            archive: PortableAbortArchive::read_for_test(
                DiskRoot::open(&fixture.source).unwrap(),
                &fixture.archive,
            )
            .unwrap(),
            witness: one,
        },
        Some(previous),
    )
    .unwrap();
    let ancestor = portable
        .node_objects()
        .unwrap()
        .into_iter()
        .find(|object| object.hash == previous_hash)
        .unwrap();
    assert_eq!(ancestor.bytes(), previous_bytes);
    let reloaded: pending::activation::PortableAuthority =
        decode_strict(&canonical_json(&portable).unwrap()).unwrap();
    assert_eq!(reloaded.hash().unwrap(), context.generation.hash);
    assert_eq!(
        reloaded
            .effective_config()
            .unwrap()
            .metadata_members()
            .len(),
        4
    );
    assert_eq!(
        context
            .prescribed_outcome(fixture.engine.state.topology_generation)
            .unwrap()
            .sequence,
        fixture
            .engine
            .membership_fence
            .as_ref()
            .unwrap()
            .intent
            .fence_sequence
            + 1
    );
    assert_eq!(files(&fixture.source), before);
}

#[path = "membership_abort_runtime_tests.rs"]
mod runtime_tests;
