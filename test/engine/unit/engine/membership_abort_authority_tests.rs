use super::*;
use pending::activation::{AuthorityResolver, PortableAuthority};

pub(in crate::engine::membership_transition) struct ReadFixture {
    pub(in crate::engine::membership_transition) temp: tempfile::TempDir,
    pub(in crate::engine::membership_transition) authority: PortableAuthority,
    pub(in crate::engine::membership_transition) alternate: PortableAuthority,
    pub(in crate::engine::membership_transition) source: PathBuf,
    pub(in crate::engine::membership_transition) reference: MembershipControlArchive,
    pub(in crate::engine::membership_transition) base: State,
    pub(in crate::engine::membership_transition) original: Config,
}

pub(in crate::engine::membership_transition) fn read_fixture() -> ReadFixture {
    let fixture = Fixture::new(3);
    let context = fixture.context();
    let archive = PortableAbortArchive::read_for_test(
        DiskRoot::open(&fixture.source).unwrap(),
        &fixture.archive,
    )
    .unwrap();
    let make = |voters: &[usize]| {
        PortableAuthority::abort_for_test(
            context.core.clone(),
            AbortAuthorityEvidence {
                archive: archive.clone(),
                witness: context.certify(&fixture.votes(&context, voters)).unwrap(),
            },
            None,
        )
        .unwrap()
    };
    let authority = make(&[0, 1]);
    let alternate = make(&[1, 2]);
    ReadFixture {
        authority,
        alternate,
        base: (*fixture.engine.state).clone(),
        reference: fixture.archive,
        source: fixture.source,
        original: fixture.original,
        temp: fixture.temp,
    }
}

/// Independently build historical bytes. This is not an Abort publication API.
pub(in crate::engine::membership_transition) fn cancelled_candidate(
    authority: &PortableAuthority,
    base: &State,
) -> (Candidate, State, OperationResult) {
    let intent = authority.intent().unwrap();
    let hash = authority.hash().unwrap();
    let mut state = base.clone();
    state.sequence = intent.fence_sequence + 1;
    state.topology_generation += 1;
    let result = OperationResult {
        operation_id: intent.operation_id,
        kind: "membership-add-cancelled".into(),
        commit_id: None,
        history_tree_id: None,
        branch_id: None,
        commit_state: storage_contract::CommitState::Published,
        protection_at_publish: storage_contract::ProtectionAtPublish::NotApplicable,
        current_health: storage_contract::CurrentHealth::Healthy,
        request_digest: object_hash(INTENT_KIND, &intent).unwrap(),
        sequence: state.sequence,
        result: json_codec::json!({"generation": hash, "membership_epoch": authority.epoch().unwrap().to_string(), "metadata_members": intent.origin.config().metadata_members().len(), "topology_generation": state.topology_generation.to_string()}),
    };
    state.operations.insert(intent.operation_id, result.clone());
    state
        .operation_principals
        .insert(intent.operation_id, intent.principal);
    let snapshot = prepare_snapshot(&state, MAX_METADATA).unwrap();
    let (history, _) =
        crate::history_package::prepare_history_manifests(&state, MAX_METADATA).unwrap();
    let candidate = Candidate::new(
        CandidateData {
            pool_id: intent.origin.config().pool_id.clone(),
            sequence: state.sequence,
            transaction_id: format!("tx_{}", &hash[..32]),
            operation_id: intent.operation_id,
            request_digest: result.request_digest.clone(),
            previous_snapshot: intent.boundary.map(|boundary| boundary.snapshot_hash),
            snapshot_hash: snapshot.hash,
            versions: Vec::new(),
            metadata_disk_ids: intent
                .origin
                .config()
                .metadata_members()
                .map(|disk| disk.disk_id.clone())
                .collect(),
            history_roots: history.reference().clone(),
        },
        Some(&hash),
    );
    (candidate, state, result)
}

#[test]
fn membership_abort_authority_same_core_distinct_envelopes_are_equivalent() {
    let fixture = read_fixture();
    assert_eq!(
        fixture.authority.hash().unwrap(),
        fixture.alternate.hash().unwrap()
    );
    assert_ne!(
        canonical_json(&fixture.authority).unwrap(),
        canonical_json(&fixture.alternate).unwrap()
    );
    let left = fixture.authority.node_objects().unwrap();
    let right = fixture.alternate.node_objects().unwrap();
    assert_eq!(left[0].bytes(), right[0].bytes());
    assert_eq!(left[0].hash, fixture.authority.hash().unwrap());
    let value: json_codec::Value = decode_strict(left[0].bytes()).unwrap();
    assert_eq!(value["format"], "membership-abort-generation");
    assert!(value.get("evidence").is_none());
    for authority in [&fixture.authority, &fixture.alternate] {
        let decoded: PortableAuthority =
            decode_strict(&canonical_json(authority).unwrap()).unwrap();
        assert_eq!(decoded.hash().unwrap(), authority.hash().unwrap());
        assert_eq!(
            canonical_json(&decoded.effective_config().unwrap()).unwrap(),
            canonical_json(&fixture.original).unwrap()
        );
        let (candidate, _, _) = cancelled_candidate(authority, &fixture.base);
        pending::activation::verify_frozen_activation(
            authority,
            std::sync::Arc::new(fixture.base.clone()),
            &candidate,
        )
        .unwrap();
    }
}

#[test]
fn membership_abort_authority_envelope_rejects_missing_extra_rebound_and_ambiguous_proof() {
    let fixture = read_fixture();
    let value = json_codec::to_value(&fixture.authority).unwrap();
    let hash = fixture.authority.hash().unwrap();
    let foreign = read_fixture();
    let foreign_value = json_codec::to_value(&foreign.authority).unwrap();
    let foreign_hash = foreign.authority.hash().unwrap();
    let mut mutations = Vec::new();
    let mut missing = value.clone();
    missing.as_object_mut().unwrap().remove("evidence");
    mutations.push(missing);
    let mut empty = value.clone();
    empty["evidence"] = json_codec::json!({});
    mutations.push(empty);
    let mut extra = value.clone();
    extra["evidence"]["0".repeat(64)] = value["evidence"][&hash].clone();
    mutations.push(extra);
    let mut rebound = value.clone();
    rebound["evidence"][&hash] = foreign_value["evidence"][&foreign_hash].clone();
    mutations.push(rebound);
    let mut wrong_format = value.clone();
    wrong_format["format"] = "membership-generation-chain".into();
    mutations.push(wrong_format);
    let mut unknown_core = value.clone();
    unknown_core["head"]["format"] = "membership-abort-unknown".into();
    mutations.push(unknown_core);
    let mut ambiguous = value.clone();
    ambiguous["head"]["joint"] = json_codec::json!({});
    mutations.push(ambiguous);
    let mut missing_seal = value.clone();
    missing_seal["evidence"][&hash]["archive"]["seals"]
        .as_object_mut()
        .unwrap()
        .remove(&fixture.original.disks[0].disk_id);
    mutations.push(missing_seal);
    let mut damaged_seal = value.clone();
    damaged_seal["evidence"][&hash]["archive"]["seals"][&fixture.original.disks[0].disk_id] =
        json_codec::json!([0]);
    mutations.push(damaged_seal);
    for invalid in mutations {
        assert!(json_codec::from_value::<PortableAuthority>(invalid).is_err());
    }
    assert!(
        json_codec::from_value::<PortableAuthority>(value["head"].clone()).is_err(),
        "bare Abort core is not a portable authority"
    );
    let objects = value["evidence"][&hash]["archive"]["objects"]
        .as_object()
        .unwrap();
    for key in objects.keys() {
        let mut missing = value.clone();
        missing["evidence"][&hash]["archive"]["objects"]
            .as_object_mut()
            .unwrap()
            .remove(key);
        assert!(json_codec::from_value::<PortableAuthority>(missing).is_err());
    }
}

struct Resolver<'a> {
    authority: &'a PortableAuthority,
    evidence: Option<&'a PortableAuthority>,
}
impl AuthorityResolver for Resolver<'_> {
    fn core_object(&mut self, hash: &str) -> Result<AuthenticatedMetadataObject> {
        let object = self
            .authority
            .node_objects()?
            .into_iter()
            .find(|object| object.hash == hash)
            .unwrap();
        AuthenticatedMetadataObject::authenticate(
            AUTHORITY_KIND,
            hash,
            decode_strict(object.bytes())?,
        )
    }
    fn abort_evidence(&mut self, hash: &str) -> Result<AbortAuthorityEvidence> {
        self.evidence
            .and_then(|authority| authority.evidence_entries().get(hash))
            .cloned()
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::ObjectCorrupt,
                    "test resolver missing detached evidence",
                )
            })
    }
}

#[test]
fn membership_abort_core_loader_requires_separate_evidence_capability() {
    let fixture = read_fixture();
    let hash = fixture.authority.hash().unwrap();
    assert!(
        PortableAuthority::load(
            &hash,
            &mut Resolver {
                authority: &fixture.authority,
                evidence: None
            }
        )
        .is_err()
    );
    let loaded = PortableAuthority::load(
        &hash,
        &mut Resolver {
            authority: &fixture.authority,
            evidence: Some(&fixture.alternate),
        },
    )
    .unwrap();
    assert_eq!(loaded.hash().unwrap(), hash);
    assert_eq!(
        canonical_json(&loaded).unwrap(),
        canonical_json(&fixture.alternate).unwrap()
    );
}

#[test]
fn membership_abort_frozen_first_candidate_is_prescribed_and_uses_original_members() {
    let fixture = read_fixture();
    let (candidate, _, _) = cancelled_candidate(&fixture.authority, &fixture.base);
    let before = files(&fixture.source);
    let mut wrong_members = candidate.clone();
    wrong_members.metadata_disk_ids = fixture
        .authority
        .intent()
        .unwrap()
        .target
        .metadata_members()
        .map(|disk| disk.disk_id.clone())
        .collect();
    let mut wrong_snapshot = candidate.clone();
    wrong_snapshot.snapshot_hash = "f".repeat(64);
    let mut wrong_operation = candidate.clone();
    wrong_operation.operation_id = OperationId::generate().unwrap();
    let mut wrong_sequence = candidate.clone();
    wrong_sequence.sequence += 1;
    for invalid in [
        wrong_members,
        wrong_snapshot,
        wrong_operation,
        wrong_sequence,
    ] {
        assert!(
            pending::activation::verify_frozen_activation(
                &fixture.authority,
                std::sync::Arc::new(fixture.base.clone()),
                &invalid
            )
            .is_err()
        );
    }
    pending::activation::verify_frozen_activation(
        &fixture.authority,
        std::sync::Arc::new(fixture.base.clone()),
        &candidate,
    )
    .unwrap();
    assert_eq!(files(&fixture.source), before);
}

#[test]
fn membership_abort_current_head_refuses_add_discovery_before_rejected_target_probe() {
    let fixture = read_fixture();
    let intent = fixture.authority.intent().unwrap();
    let rejected = &intent.target.disks.last().unwrap().root;
    fs::rename(
        rejected,
        fixture.temp.path().join("preserved-rejected-target"),
    )
    .unwrap();
    // Following this path would fail UnsafePath, not the explicit Abort gate.
    std::os::unix::fs::symlink("/unreachable-abort-target", rejected).unwrap();
    for disk in &fixture.original.disks {
        fs::write(
            disk.root.join("metadata/pool-membership/generation.json"),
            canonical_json(&fixture.authority).unwrap(),
        )
        .unwrap();
    }
    let before = files(&fixture.original.disks[0].root);
    let error =
        pending::activation::discover(ValidatedConfig::try_from(fixture.original.clone()).unwrap())
            .unwrap_err();
    assert_eq!(error.code(), ErrorCode::RecoveryRequired);
    assert!(error.to_string().contains("Abort authority is read-only"));
    assert_preserved(&before);
    let error = Engine::resume_membership_activation(&fixture.temp.path().join("system"))
        .err()
        .unwrap();
    assert!(error.to_string().contains("Abort authority is read-only"));
    assert_preserved(&before);
}

#[test]
fn membership_abort_invalid_current_envelope_never_falls_back_to_fence_hint() {
    let fixture = read_fixture();
    let value = json_codec::to_value(&fixture.authority).unwrap();
    let hash = fixture.authority.hash().unwrap();
    let mut missing = value.clone();
    missing["evidence"].as_object_mut().unwrap().remove(&hash);
    let mut unknown = value.clone();
    unknown["format"] = "unknown-authority".into();
    for invalid in [json_codec::json!({}), missing, unknown] {
        for disk in &fixture.original.disks {
            fs::write(
                disk.root.join("metadata/pool-membership/generation.json"),
                canonical_json(&invalid).unwrap(),
            )
            .unwrap();
        }
        assert!(
            pending::activation::discover(
                ValidatedConfig::try_from(fixture.original.clone()).unwrap()
            )
            .is_err()
        );
    }
    // A missing marker paired with an active seal is not Genesis or proof that
    // the rejected target became a member. Keep all existing bytes intact.
    for disk in &fixture.original.disks {
        fs::remove_file(disk.root.join("metadata/pool-membership/generation.json")).unwrap();
        let mut seal = decode_seal(&fs::read(disk.root.join("disk.json")).unwrap()).unwrap();
        seal.format = DiskSealFormat::MembershipGeneration {
            authority_hash: hash.clone(),
        };
        fs::write(disk.root.join("disk.json"), canonical_json(&seal).unwrap()).unwrap();
    }
    let before = files(&fixture.original.disks[0].root);
    let error = pending::activation::discover(ValidatedConfig::try_from(fixture.original).unwrap())
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::RecoveryRequired);
    assert_preserved(&before);
}
