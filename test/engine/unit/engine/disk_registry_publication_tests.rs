use super::*;
use crate::{
    CommitState, Config, CurrentHealth, OperationResult, ProtectionAtPublish, canonical_json,
    disk::private_dir,
    journal::{CandidateData, Finalize, JournalWriter, RecordType},
};
use json_codec::json;
use std::{
    fs,
    path::{Path, PathBuf},
};

type Objects = BTreeMap<(String, String), Vec<u8>>;
fn objects(prepared: &PreparedDiskRegistry) -> Objects {
    prepared
        .objects()
        .map(|(kind, hash, bytes)| ((kind.to_owned(), hash.to_owned()), bytes.to_vec()))
        .collect()
}
fn read(objects: &Objects, kind: &str, hash: &str) -> Result<Vec<u8>> {
    objects
        .get(&(kind.to_owned(), hash.to_owned()))
        .cloned()
        .ok_or_else(|| invalid("test registry object is missing"))
}
fn verify(prepared: &PreparedDiskRegistry) -> VerifiedDiskRegistry {
    let objects = objects(prepared);
    prepared
        .verify_with(|kind, hash| read(&objects, kind, hash))
        .unwrap()
}
fn fixture() -> (tempfile::TempDir, Engine, VerifiedDiskRegistry) {
    let temp = tempfile::tempdir().unwrap();
    let mut config =
        Config::development((0..3).map(|n| temp.path().join(format!("data-{n}")))).unwrap();
    config.pool_id = "pool_00000000000000000000000000000001".into();
    for (n, disk) in config.disks.iter_mut().enumerate() {
        disk.disk_id = format!("disk_{:032x}", n + 1);
    }
    config.disks[2].metadata_member = false;
    let engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
    let base = verify(&engine.prepare_disk_registry_base().unwrap());
    (temp, engine, base)
}
fn request(base: &VerifiedDiskRegistry, index: usize, detach: bool) -> RegistryExitRequest {
    let target = base
        .historical_topology()
        .nth(index)
        .unwrap()
        .identity
        .clone();
    let operation_id = "op_00000000000000000000000000000001"
        .parse::<OperationId>()
        .unwrap();
    let request_digest = object_hash(
        "registry-request-vector",
        &json!({"target":target,"detach":detach}),
    )
    .unwrap();
    let intent_hash = object_hash(
        "registry-intent-vector",
        &json!({"base":base.commitment(),"operation":operation_id,"request":request_digest}),
    )
    .unwrap();
    let effect = if detach {
        // A pre-publication reference vector, not a verified occurrence projection.
        RegistryExitEffect::Detach {
            occurrence_mask_hash: object_hash(
                "occurrence-mask-vector",
                &json!({"intent":intent_hash,"generation":"2"}),
            )
            .unwrap(),
        }
    } else {
        RegistryExitEffect::Disable
    };
    RegistryExitRequest {
        target,
        operation_id,
        principal: "admin".into(),
        request_digest,
        intent_hash,
        effect,
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

#[test]
fn membership_registry_disable_keeps_registration_and_topology_without_serving_side_effects() {
    let (temp, engine, base) = fixture();
    let before = files(temp.path());
    let state = canonical_json(engine.state()).unwrap();
    let config = canonical_json(engine.config()).unwrap();
    for index in [0, 2] {
        let request = request(&base, index, false);
        let proposed = base.prepare_exit(request.clone()).unwrap();
        let map = objects(&proposed);
        let next = base
            .verify_successor(proposed.commitment(), |kind, hash| read(&map, kind, hash))
            .unwrap();
        assert_eq!(
            next.registration(&request.target).unwrap(),
            Some(RegistrationMode::Disabled)
        );
        assert!(!next.is_voter(&request.target).unwrap());
        assert_eq!(next.voter_count(), if index == 0 { 1 } else { 2 });
        assert_eq!(
            next.historical_topology().cloned().collect::<Vec<_>>(),
            base.historical_topology().cloned().collect::<Vec<_>>()
        );
        assert_eq!(next.commitment().view_generation(), 2);
        assert_eq!(next.pending_exit(), Some(&request));
        assert!(next.prepare_exit(request).is_err());
    }
    assert_eq!(canonical_json(engine.state()).unwrap(), state);
    assert_eq!(canonical_json(engine.config()).unwrap(), config);
    assert_eq!(files(temp.path()), before);
    assert!(!engine.is_read_only());
}

#[test]
fn membership_registry_detach_uses_only_prepublication_dependencies_and_blocks_skipping_pending() {
    let (_temp, engine, base) = fixture();
    let request = request(&base, 2, true);
    let prepared = base.prepare_exit(request.clone()).unwrap();
    let next = verify(&prepared);
    assert_eq!(next.registration(&request.target).unwrap(), None);
    assert_eq!(next.voter_count(), base.voter_count());
    assert_eq!(next.historical_topology().count(), 3);
    let detached = &next.state.detached[&request.target.disk_id];
    assert_eq!(&detached.predecessor, base.commitment());
    assert_eq!(detached.request, request);
    let mut value = json_codec::to_value(detached).unwrap();
    assert!(value.get("publication_hash").is_none());
    assert!(value.get("committed_capsule_hash").is_none());
    for field in [
        "publication_hash",
        "prepared_capsule_hash",
        "committed_capsule_hash",
    ] {
        value[field] = json!("a".repeat(64));
        assert!(json_codec::from_value::<DetachedBinding>(value.clone()).is_err());
        value.as_object_mut().unwrap().remove(field);
    }
    let later = encode_registry({
        let mut state = next.state.clone();
        state.view_generation += 1;
        state.predecessor = Some(next.commitment.clone());
        state
    })
    .unwrap();
    let map = objects(&later);
    assert!(
        next.verify_successor(later.commitment(), |kind, hash| read(&map, kind, hash))
            .is_err()
    );
    assert!(!engine.state.operations.contains_key(&request.operation_id));
}

#[test]
fn membership_registry_detach_requires_only_occurrence_mask_and_rejects_recovery_fields() {
    let (_temp, _engine, base) = fixture();
    let request = request(&base, 2, true);
    let value = json_codec::to_value(&request).unwrap();
    let RegistryExitEffect::Detach {
        occurrence_mask_hash,
    } = &request.effect
    else {
        unreachable!();
    };
    assert_eq!(
        value["effect"],
        json!({"kind":"detach","occurrence_mask_hash":occurrence_mask_hash})
    );
    let decoded: RegistryExitRequest = decode_strict(&canonical_json(&value).unwrap()).unwrap();
    assert_eq!(decoded, request);
    base.prepare_exit(decoded).unwrap();
    for field in [
        "prepared_capsule_hash",
        "committed_capsule_hash",
        "recovery_capsule_hash",
        "recovery_backup_hash",
        "publication_hash",
    ] {
        for legacy_value in [json!("a".repeat(64)), json!(null)] {
            let mut legacy = value.clone();
            legacy["effect"][field] = legacy_value;
            assert!(
                decode_strict::<RegistryExitRequest>(&canonical_json(&legacy).unwrap()).is_err(),
                "{field}"
            );
        }
    }
    let mut missing = value.clone();
    missing["effect"]
        .as_object_mut()
        .unwrap()
        .remove("occurrence_mask_hash");
    assert!(decode_strict::<RegistryExitRequest>(&canonical_json(&missing).unwrap()).is_err());
    for hash in [
        String::new(),
        "ABCDEF".into(),
        "a".repeat(63),
        "A".repeat(64),
    ] {
        let mut invalid = request.clone();
        invalid.effect = RegistryExitEffect::Detach {
            occurrence_mask_hash: hash,
        };
        assert!(base.prepare_exit(invalid).is_err());
    }
}

#[test]
fn membership_registry_detach_preparation_needs_no_target_root_or_recovery_files() {
    let (temp, mut engine, base) = fixture();
    let before = files(temp.path());
    let roots = (0..3)
        .map(|n| temp.path().join(format!("unavailable-{n}")))
        .collect();
    engine.config = engine.config.clone().with_disk_roots(roots).unwrap();
    let relocated = verify(&engine.prepare_disk_registry_base().unwrap());
    assert_eq!(relocated.commitment(), base.commitment());
    drop(engine);
    // The preparation and reader have only registry bytes, with no target root
    // or live Engine. This is not an activated removal or a content projection.
    for index in [0, 2] {
        let request = request(&relocated, index, true);
        let prepared = relocated.prepare_exit(request.clone()).unwrap();
        let map = objects(&prepared);
        let next = relocated
            .verify_successor(prepared.commitment(), |kind, hash| read(&map, kind, hash))
            .unwrap();
        assert_eq!(next.registration(&request.target).unwrap(), None);
        assert_eq!(next.pending_exit(), Some(&request));
        assert_eq!(
            next.historical_topology().cloned().collect::<Vec<_>>(),
            base.historical_topology().cloned().collect::<Vec<_>>()
        );
    }
    assert_eq!(files(temp.path()), before);
}

#[test]
fn membership_registry_successor_rejects_rehashed_changes_outside_exact_effect() {
    let (_temp, _engine, base) = fixture();
    let prepared = base.prepare_exit(request(&base, 0, false)).unwrap();
    for mutation in [
        "predecessor",
        "generation",
        "source",
        "topology",
        "registration",
        "voters",
        "pending",
    ] {
        let mut state = prepared.expected.state.clone();
        match mutation {
            "predecessor" => state.predecessor = None,
            "generation" => state.view_generation += 1,
            "source" => state.source.pool_id = "pool_ffffffffffffffffffffffffffffffff".into(),
            "topology" => {
                state
                    .topology
                    .values_mut()
                    .next()
                    .unwrap()
                    .identity
                    .binding_generation += 1
            }
            "registration" => {
                state.registrations.pop_last();
            }
            "voters" => {
                state.voters = base.state.voters.clone();
            }
            "pending" => state.pending_exit = None,
            _ => unreachable!(),
        }
        let changed = encode_registry(state).unwrap();
        let map = objects(&changed);
        assert!(
            base.verify_successor(changed.commitment(), |kind, hash| read(&map, kind, hash))
                .is_err(),
            "{mutation}"
        );
    }
    let mut wrong = request(&base, 0, false);
    wrong.target.binding_generation += 1;
    assert!(base.prepare_exit(wrong).is_err());
    let mut exhausted = base.clone();
    exhausted.state.view_generation = u64::MAX;
    assert_eq!(
        exhausted
            .prepare_exit(request(&base, 0, false))
            .err()
            .unwrap()
            .code(),
        ErrorCode::ResourceLimit
    );
}

#[test]
fn membership_registry_paged_graph_requires_every_exact_dependency() {
    let (_temp, _engine, base) = fixture();
    let mut state = base.state.clone();
    for n in 4..300 {
        let mut disk = state.topology.values().next().unwrap().clone();
        disk.identity.disk_id = format!("disk_{n:032x}");
        disk.identity.slot_id = format!("P{n:03}");
        state
            .topology
            .insert(disk.identity.disk_id.clone(), disk.clone());
        state.registrations.insert(
            disk.identity.disk_id.clone(),
            Registration {
                identity: disk.identity,
                mode: RegistrationMode::Enabled,
            },
        );
    }
    // Layout-only vector; this does not represent 299 physically admitted roots.
    let prepared = encode_registry(state).unwrap();
    let map = objects(&prepared);
    assert!(map.keys().filter(|(kind, _)| kind == "state-page").count() > 4);
    assert!(map.values().all(|bytes| bytes.len() <= MAX_METADATA));
    prepared
        .verify_with(|kind, hash| read(&map, kind, hash))
        .unwrap();
    for key in map.keys() {
        let mut missing = map.clone();
        missing.remove(key);
        assert!(
            prepared
                .verify_with(|kind, hash| read(&missing, kind, hash))
                .is_err()
        );
        let mut damaged = map.clone();
        damaged.get_mut(key).unwrap().push(b'x');
        assert!(
            prepared
                .verify_with(|kind, hash| read(&damaged, kind, hash))
                .is_err()
        );
    }
}

fn make_candidate(
    registry: &VerifiedDiskRegistry,
    authority: &str,
    operation: OperationId,
) -> Candidate {
    let mut encoder = Encoder::new(MAX_METADATA);
    let history_roots = encoder
        .map_root(json_codec::Map::new(), ProofMeasure::Plain)
        .unwrap()
        .reference()
        .clone();
    let data = CandidateData {
        pool_id: registry.state.source.pool_id.clone(),
        sequence: registry.state.source.sequence + 1,
        transaction_id: "tx_00000000000000000000000000000001".into(),
        operation_id: operation,
        request_digest: object_hash("registry-stage-request-vector", &operation).unwrap(),
        previous_snapshot: registry.state.source.snapshot_hash.clone(),
        snapshot_hash: object_hash("registry-stage-snapshot-vector", &registry.commitment).unwrap(),
        versions: vec![],
        metadata_disk_ids: registry.state.voters.keys().cloned().collect(),
        history_roots,
    };
    json_codec::from_value(json!({"format":"disk-lifecycle-candidate", "authority_hash":authority,"registry":registry.commitment(),"decision":data})).unwrap()
}

#[test]
fn membership_registry_candidate_exact_binding_preserves_old_hashes_and_blocks_live_admission() {
    let (_temp, engine, base) = fixture();
    let next = verify(&base.prepare_exit(request(&base, 2, false)).unwrap());
    let authority = object_hash("registry-authority-vector", next.commitment()).unwrap();
    let candidate = make_candidate(&next, &authority, OperationId::generate().unwrap());
    let bytes = canonical_json(&candidate).unwrap();
    let witness = next.verify_candidate_binding(&bytes, &authority).unwrap();
    assert_eq!(witness.registry(), next.commitment());
    assert_eq!(
        witness.candidate_hash(),
        object_hash("candidate", &candidate).unwrap()
    );
    assert!(
        engine
            .config
            .verify_candidate_membership(&candidate)
            .is_err()
    );
    assert!(
        engine
            .config
            .verify_original_candidate_membership(&candidate)
            .is_err()
    );
    let original = Candidate::new((*candidate).clone(), None);
    assert_eq!(
        canonical_json(&original).unwrap(),
        canonical_json(&*candidate).unwrap()
    );
    let member = Candidate::new((*candidate).clone(), Some(&authority));
    let expected = json!({"format":"membership-generation-candidate","authority_hash":authority,"decision":&*candidate});
    assert_eq!(
        object_hash("candidate", &member).unwrap(),
        object_hash("candidate", &expected).unwrap()
    );
    assert!(
        next.verify_candidate_binding(&canonical_json(&member).unwrap(), &authority)
            .is_err()
    );
    for pointer in [
        "/registry/root_hash",
        "/authority_hash",
        "/decision/pool_id",
    ] {
        let mut changed = json_codec::to_value(&candidate).unwrap();
        *changed.pointer_mut(pointer).unwrap() = json!("f".repeat(64));
        assert!(
            next.verify_candidate_binding(&canonical_json(&changed).unwrap(), &authority)
                .is_err()
        );
    }
    for generation in ["0", "1", "3", "02"] {
        let mut changed = json_codec::to_value(&candidate).unwrap();
        changed["registry"]["view_generation"] = json!(generation);
        assert!(
            next.verify_candidate_binding(&canonical_json(&changed).unwrap(), &authority)
                .is_err()
        );
    }
    let mut changed = json_codec::to_value(&candidate).unwrap();
    changed["decision"]["metadata_disk_ids"] = json!([
        candidate.metadata_disk_ids[0],
        candidate.metadata_disk_ids[0]
    ]);
    assert!(
        next.verify_candidate_binding(&canonical_json(&changed).unwrap(), &authority)
            .is_err()
    );
    changed = json_codec::to_value(&candidate).unwrap();
    changed["registry"]["committed"] = json!(true);
    assert!(decode_strict::<Candidate>(&canonical_json(&changed).unwrap()).is_err());
}

fn publish_vector(
    root: &Path,
    config: &Config,
    candidate: &Candidate,
) -> (Publication, PublicationSeal) {
    let hash = object_hash("candidate", candidate).unwrap();
    let mut journals = Vec::new();
    let mut accepts = Vec::new();
    for (index, disk) in config
        .disks
        .iter()
        .filter(|disk| candidate.metadata_disk_ids.contains(&disk.disk_id))
        .enumerate()
    {
        let mut disk = disk.clone();
        disk.root = root.join(format!("receipt-{index}"));
        private_dir(&disk.root.join("metadata/application-log")).unwrap();
        let mut journal = JournalWriter::create(&disk).unwrap();
        accepts.push(
            journal
                .append(&disk, RecordType::Accept, candidate.sequence, &hash, &hash)
                .unwrap(),
        );
        journals.push((disk, journal));
    }
    let finalize = Finalize {
        candidate_hash: hash.clone(),
        accepts,
        recovery_id: None,
    };
    let finalize_hash = object_hash("finalize", &finalize).unwrap();
    let finalize_receipts = journals
        .iter_mut()
        .map(|(disk, journal)| {
            journal
                .append(
                    disk,
                    RecordType::Finalize,
                    candidate.sequence,
                    &hash,
                    &finalize_hash,
                )
                .unwrap()
        })
        .collect();
    let publication = Publication {
        candidate_hash: hash.clone(),
        finalize,
        finalize_receipts,
        payload_receipts: vec![],
        recovery: None,
        result: OperationResult {
            operation_id: candidate.operation_id,
            kind: "registry-stage-vector".into(),
            commit_id: None,
            history_tree_id: None,
            branch_id: None,
            commit_state: CommitState::Published,
            protection_at_publish: ProtectionAtPublish::NotApplicable,
            current_health: CurrentHealth::Healthy,
            request_digest: candidate.request_digest.clone(),
            sequence: candidate.sequence,
            result: json!({"registry":candidate.registry()}),
        },
    };
    let publication_hash = object_hash("publication", &publication).unwrap();
    let receipts = journals
        .iter_mut()
        .map(|(disk, journal)| {
            journal
                .append(
                    disk,
                    RecordType::Publication,
                    candidate.sequence,
                    &hash,
                    &publication_hash,
                )
                .unwrap()
        })
        .collect();
    (
        publication,
        PublicationSeal {
            candidate_hash: hash,
            publication_hash,
            receipts,
        },
    )
}

#[test]
fn membership_registry_publication_witness_is_independent_of_main_operation_success() {
    let (temp, engine, base) = fixture();
    let request = request(&base, 2, true);
    let next = verify(&base.prepare_exit(request.clone()).unwrap());
    let authority = object_hash("registry-authority-vector", next.commitment()).unwrap();
    let candidate = make_candidate(&next, &authority, OperationId::generate().unwrap());
    // Real durable journal frames verify the registry/receipt binding. The vector
    // deliberately does not claim prescribed snapshot/D3 or lifecycle completion.
    let (publication, seal) =
        publish_vector(&temp.path().join("vector"), engine.config(), &candidate);
    let encoded = canonical_json(&candidate).unwrap();
    let published = canonical_json(&publication).unwrap();
    let sealed = canonical_json(&seal).unwrap();
    let witness = next
        .verify_publication_binding(
            &encoded,
            &published,
            &sealed,
            &authority,
            &seal.candidate_hash,
        )
        .unwrap();
    assert_eq!(witness.publication_hash(), seal.publication_hash);
    assert!(!engine.state.operations.contains_key(&request.operation_id));
    let mut missing = seal.clone();
    missing.receipts.pop();
    assert!(
        next.verify_publication_binding(
            &encoded,
            &published,
            &canonical_json(&missing).unwrap(),
            &authority,
            &seal.candidate_hash
        )
        .is_err()
    );
    let mut wrong = candidate.clone();
    wrong.sequence += 1;
    assert!(
        next.verify_publication_binding(
            &canonical_json(&wrong).unwrap(),
            &published,
            &sealed,
            &authority,
            &seal.candidate_hash
        )
        .is_err()
    );
    let mut changed_slots = engine.config().clone();
    changed_slots.disks[0].slot_id = "D99".into();
    let (foreign, foreign_seal) = publish_vector(
        &temp.path().join("foreign-slot"),
        &changed_slots,
        &candidate,
    );
    verify_publication_seal(&candidate, &foreign, &foreign_seal).unwrap();
    assert!(
        next.verify_publication_binding(
            &encoded,
            &canonical_json(&foreign).unwrap(),
            &canonical_json(&foreign_seal).unwrap(),
            &authority,
            &seal.candidate_hash,
        )
        .is_err()
    );
    let main = make_candidate(&next, &authority, request.operation_id);
    let (publication, seal) = publish_vector(&temp.path().join("main-op"), engine.config(), &main);
    assert!(
        next.verify_publication_binding(
            &canonical_json(&main).unwrap(),
            &canonical_json(&publication).unwrap(),
            &canonical_json(&seal).unwrap(),
            &authority,
            &seal.candidate_hash
        )
        .is_err()
    );
}

#[test]
fn membership_registry_base_is_root_relocation_independent_and_directory_roles_are_optional() {
    let (_temp, mut engine, base) = fixture();
    let previous = base.commitment().clone();
    let roots = engine
        .config
        .disks
        .iter()
        .enumerate()
        .map(|(n, _)| PathBuf::from(format!("/relocated/{n}")))
        .collect();
    engine.config = engine.config.clone().with_disk_roots(roots).unwrap();
    assert_eq!(
        engine.prepare_disk_registry_base().unwrap().commitment(),
        &previous
    );
    assert_eq!(base.voter_count(), 2);
    assert_eq!(base.historical_topology().count(), 3);
    let prepared = base.prepare_exit(request(&base, 2, true)).unwrap();
    assert_eq!(verify(&prepared).voter_count(), 2);
}

#[test]
fn membership_registry_terminal_gap_and_unbound_commitments_do_not_mutate_sources() {
    let (temp, engine, base) = fixture();
    let before = files(temp.path());
    let mut terminal = base.clone();
    terminal
        .state
        .voters
        .retain(|disk, _| disk == &request(&base, 0, false).target.disk_id);
    assert!(terminal.prepare_exit(request(&base, 0, false)).is_err());
    let prepared = engine.prepare_disk_registry_base().unwrap();
    let map = objects(&prepared);
    let mut wrong = prepared.commitment().clone();
    wrong.root_hash = "f".repeat(64);
    assert!(load_registry(&wrong, |kind, hash| read(&map, kind, hash)).is_err());
    assert_eq!(files(temp.path()), before);
}
