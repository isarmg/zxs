use super::*;

fn observed_version(id: &str, generation: u64, states: &[ObservedCopyState]) -> ObservedVersion {
    ObservedVersion {
        placement: VersionPlacementObservation {
            version_id: id.into(),
            primary_slot_id: "P03".into(),
            target_copies: 3,
            required_slots: vec!["P03".into(), "P01".into(), "P02".into()],
            topology_generation: generation,
        },
        slots: states
            .iter()
            .enumerate()
            .map(|(index, state)| ObservedSlot {
                identity: SlotIdentity {
                    slot_id: ["P03", "P01", "P02"][index].into(),
                    copy_level: index as u32 + 1,
                    disk_id: ["d3", "d1", "d2"][index].into(),
                    binding_generation: [0, 1 << 53, u64::MAX][index],
                },
                state: *state,
            })
            .collect(),
    }
}

#[test]
fn fresh_freeze_has_exact_existing_shape_order_and_hash_bytes() {
    let frozen = FrozenObservation::freeze(vec![observed_version(
        "v",
        u64::MAX,
        &[
            ObservedCopyState::CompleteVerified,
            ObservedCopyState::Missing,
            ObservedCopyState::Corrupt,
        ],
    )]);
    let (versions, losses) = frozen.wire();
    // Independent literal: numeric copy fields and decimal-string generations
    // must not change when the internal observation representation changes.
    let expected: Value = decode_strict(br#"[{"version_id":"v","primary_slot_id":"P03","target_copies":3,"required_slots":["P03","P01","P02"],"topology_generation":"18446744073709551615","slots":[{"slot_id":"P03","copy_level":1,"disk_id":"d3","binding_generation":"0","state":"COMPLETE_VERIFIED"},{"slot_id":"P01","copy_level":2,"disk_id":"d1","binding_generation":"9007199254740992","state":"UNAVAILABLE"},{"slot_id":"P02","copy_level":3,"disk_id":"d2","binding_generation":"18446744073709551615","state":"UNAVAILABLE"}]}]"#).unwrap();
    let expected_losses: Value = decode_strict(br#"[{"version_id":"v","slot_id":"P01","cause":"MISSING"},{"version_id":"v","slot_id":"P02","cause":"CORRUPT"}]"#).unwrap();
    assert_eq!(json!(versions), expected);
    assert_eq!(json!(losses), expected_losses);
    assert_eq!(
        encode_metadata(&versions, MAX_METADATA).unwrap(),
        canonical_json(&expected).unwrap()
    );
    assert_eq!(
        encode_metadata(&losses, MAX_METADATA).unwrap(),
        canonical_json(&expected_losses).unwrap()
    );
    for domain in ["recovery-plan", "recovery-plan-digest", "publication"] {
        let actual = json!({"version_protection":versions,"loss_details":losses});
        let expected = json!({"version_protection":expected,"loss_details":expected_losses});
        assert_eq!(
            object_hash(domain, &actual).unwrap(),
            object_hash(domain, &expected).unwrap()
        );
    }
    let verified = frozen.verified_payloads();
    assert!(verified.contains("v", "P03"));
    assert!(!verified.contains("v", "P01"));
    assert!(!verified.contains("v", "P02"));
    assert_eq!(
        frozen.current_health(),
        storage_contract::CurrentHealth::Degraded
    );
    assert!(!losses.is_empty());
}

#[test]
fn freeze_keeps_version_loss_order_and_first_matching_version_selection() {
    let frozen = FrozenObservation::freeze(vec![
        observed_version(
            "z",
            0,
            &[ObservedCopyState::Missing, ObservedCopyState::Corrupt],
        ),
        observed_version("a", 1 << 53, &[ObservedCopyState::Corrupt]),
        observed_version("z", 1, &[ObservedCopyState::CompleteVerified]),
    ]);
    let (versions, losses) = frozen.wire();
    assert_eq!(
        versions
            .iter()
            .map(|v| v["version_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["z", "a", "z"]
    );
    assert_eq!(
        json!(losses),
        json!([
            {"version_id":"z","slot_id":"P03","cause":"MISSING"},
            {"version_id":"z","slot_id":"P01","cause":"CORRUPT"},
            {"version_id":"a","slot_id":"P03","cause":"CORRUPT"}
        ])
    );
    assert_eq!(frozen.version_is_complete("z"), Some(false));
    assert!(!frozen.verified_payloads().contains("z", "P03"));
    assert_eq!(
        frozen.current_health(),
        storage_contract::CurrentHealth::Unavailable
    );
    assert!(frozen.verified_payloads().matches_receipts(&[]));
    let complete = FrozenObservation::freeze(vec![observed_version(
        "healthy",
        0,
        &[ObservedCopyState::CompleteVerified],
    )]);
    assert!(complete.wire().1.is_empty());
    assert_eq!(complete.version_is_complete("healthy"), Some(true));
}

fn published_fixture() -> (tempfile::TempDir, Engine, Candidate, Publication) {
    let (temp, engine, operation, candidate) = fixture("after_accept_2");
    lose(&engine, &candidate, true);
    drop(engine);
    let mut engine = Engine::open_for_recovery(&temp.path().join("state")).unwrap();
    let plan = engine
        .prepare_degraded_recovery(operation, "alice", "permanent loss")
        .unwrap();
    engine
        .commit_degraded_recovery(&plan.recovery_id, &plan.digest, "alice", true)
        .unwrap();
    let disk = engine.members()[0].clone();
    let scanned = scan(&disk).unwrap();
    let publication = load_object(
        &disk,
        "publication",
        &scanned.records.last().unwrap().0.object_hash,
    )
    .unwrap();
    (temp, engine, candidate, publication)
}

/// Rebind only the in-memory recovery proof for independent admission vectors.
/// This does not write forged evidence to the fixture's journals.
fn rebind_plan(publication: &mut Publication) {
    let proof = publication.recovery.as_mut().unwrap();
    proof.plan.digest = plan_digest(&proof.plan).unwrap();
    let hash = object_hash("recovery-plan", &proof.plan).unwrap();
    for receipt in &mut proof.reprotect {
        receipt.record.object_hash = hash.clone();
        let payload = canonical_json(&receipt.record).unwrap();
        let mut frame = b"ZXSLOG\0\0".to_vec();
        frame.extend((payload.len() as u32).to_le_bytes());
        frame.extend(payload);
        receipt.frame_hash = blake3::hash(&frame).to_hex().to_string();
    }
    publication.finalize.recovery_id = Some(proof.plan.recovery_id.clone());
    publication.result.result["recovery_id"] = json!(proof.plan.recovery_id);
    publication.result.result["recovery_plan_digest"] = json!(proof.plan.digest);
    publication.result.result["version_protection"] = json!(proof.plan.version_protection);
}

#[test]
fn persisted_proof_retains_unknown_absent_null_fields_and_exact_evidence_bytes() {
    let (_temp, _engine, candidate, mut publication) = published_fixture();
    let old_hash = object_hash("publication", &publication).unwrap();
    let plan = &mut publication.recovery.as_mut().unwrap().plan;
    for version in &mut plan.version_protection {
        version
            .as_object_mut()
            .unwrap()
            .remove("topology_generation");
        version["unknown"] = json!({"\u{e000}":null,"\u{10000}":[3,"3",false]});
        for slot in version["slots"].as_array_mut().unwrap() {
            slot.as_object_mut().unwrap().remove("copy_level");
            slot["disk_id"] = Value::Null;
            slot["binding_generation"] = json!({"opaque":true});
            slot["unknown"] = json!([null, 7]);
        }
    }
    plan.loss_details = vec![Value::Null, json!({"uninterpreted":"loss"})];
    rebind_plan(&mut publication);
    publication
        .recovery
        .as_ref()
        .unwrap()
        .verify(&candidate, &publication)
        .unwrap();
    let bytes = canonical_json(&publication).unwrap();
    let reread: Publication = decode_strict(&bytes).unwrap();
    assert_eq!(canonical_json(&reread).unwrap(), bytes);
    assert_eq!(
        object_hash("publication", &reread).unwrap(),
        object_hash("publication", &publication).unwrap()
    );
    assert_ne!(object_hash("publication", &publication).unwrap(), old_hash);
    let plan = &reread.recovery.as_ref().unwrap().plan;
    let mut raw = json_codec::to_value(plan).unwrap();
    raw["digest"] = json!("");
    assert_eq!(
        plan.digest,
        object_hash("recovery-plan-digest", &raw).unwrap()
    );
    assert!(
        plan.version_protection[0]
            .get("topology_generation")
            .is_none()
    );
    let mut explicit_null = reread.clone();
    explicit_null
        .recovery
        .as_mut()
        .unwrap()
        .plan
        .version_protection[0]["topology_generation"] = Value::Null;
    assert_ne!(
        plan_digest(plan).unwrap(),
        plan_digest(&explicit_null.recovery.as_ref().unwrap().plan).unwrap()
    );
    let mut number = reread.clone();
    number.recovery.as_mut().unwrap().plan.version_protection[0]["topology_generation"] = json!(7);
    rebind_plan(&mut number);
    number
        .recovery
        .as_ref()
        .unwrap()
        .verify(&candidate, &number)
        .unwrap();
    let mut decimal = number.clone();
    decimal.recovery.as_mut().unwrap().plan.version_protection[0]["topology_generation"] =
        json!("7");
    rebind_plan(&mut decimal);
    decimal
        .recovery
        .as_ref()
        .unwrap()
        .verify(&candidate, &decimal)
        .unwrap();
    assert_ne!(
        object_hash("publication", &number).unwrap(),
        object_hash("publication", &decimal).unwrap()
    );
    number.recovery.as_mut().unwrap().plan.version_protection[0]["topology_generation"] =
        json!(1_u64 << 53);
    assert_eq!(
        canonical_json(&number).unwrap_err().code(),
        ErrorCode::InvalidJson
    );
}

#[test]
fn proof_admission_preserves_candidate_state_receipt_and_error_precedence_checks() {
    let (_temp, engine, candidate, publication) = published_fixture();
    let proof = publication.recovery.as_ref().unwrap();
    proof.verify(&candidate, &publication).unwrap();
    let fresh = FrozenObservation::freeze(observe(&engine, &candidate).unwrap());
    let verified = fresh.verified_payloads();
    let expected_order = candidate
        .versions
        .iter()
        .flat_map(|v| {
            let verified = &verified;
            v.placement
                .route_slot_ids
                .iter()
                .filter(move |slot| verified.contains(&v.version.version_id, slot))
                .map(move |slot| (v.version.version_id.as_str(), slot.as_str()))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        publication
            .payload_receipts
            .iter()
            .map(|r| (r.version_id.as_str(), r.slot_id.as_str()))
            .collect::<Vec<_>>(),
        expected_order
    );
    assert!(verified.matches_receipts(&publication.payload_receipts));
    let mut duplicate = publication.payload_receipts.clone();
    duplicate.push(duplicate[0].clone());
    assert!(!verified.matches_receipts(&duplicate));
    assert!(!verified.matches_receipts(&publication.payload_receipts[1..]));
    let mut extra = publication.payload_receipts.clone();
    extra[0].slot_id = "not-in-route".into();
    assert!(!verified.matches_receipts(&extra));
    for receipts in [duplicate, publication.payload_receipts[1..].to_vec(), extra] {
        let mut invalid = publication.clone();
        invalid.payload_receipts = receipts;
        assert_eq!(
            proof.verify(&candidate, &invalid).unwrap_err().code(),
            ErrorCode::PublicationInvalid
        );
    }
    let mut other_candidate = candidate.clone();
    other_candidate.sequence += 1;
    assert_eq!(
        proof
            .verify(&other_candidate, &publication)
            .unwrap_err()
            .code(),
        ErrorCode::PublicationInvalid
    );

    for state in [
        Value::Null,
        json!(false),
        json!("MISSING"),
        json!({"UNAVAILABLE":null}),
    ] {
        let mut invalid = publication.clone();
        invalid.recovery.as_mut().unwrap().plan.version_protection[0]["slots"][0]["state"] = state;
        rebind_plan(&mut invalid);
        // Persisted envelopes still decode before semantic proof admission.
        let invalid: Publication = decode_strict(&canonical_json(&invalid).unwrap()).unwrap();
        assert_eq!(
            invalid
                .recovery
                .as_ref()
                .unwrap()
                .verify(&candidate, &invalid)
                .unwrap_err()
                .code(),
            ErrorCode::PublicationInvalid
        );
    }
    for (field, value) in [
        ("target_copies", json!(0)),
        ("primary_slot_id", json!("not-in-route")),
        ("required_slots", json!([])),
        ("version_id", json!("not-in-candidate")),
        ("slots", Value::Null),
    ] {
        let mut invalid = publication.clone();
        invalid.recovery.as_mut().unwrap().plan.version_protection[0][field] = value;
        rebind_plan(&mut invalid);
        assert_eq!(
            invalid
                .recovery
                .as_ref()
                .unwrap()
                .verify(&candidate, &invalid)
                .unwrap_err()
                .code(),
            ErrorCode::PublicationInvalid
        );
    }
    for duplicate_slot in [false, true] {
        let mut invalid = publication.clone();
        let slots = invalid.recovery.as_mut().unwrap().plan.version_protection[0]["slots"]
            .as_array_mut()
            .unwrap();
        if duplicate_slot {
            slots[1] = slots[0].clone();
        } else {
            slots.pop();
        }
        rebind_plan(&mut invalid);
        assert_eq!(
            invalid
                .recovery
                .as_ref()
                .unwrap()
                .verify(&candidate, &invalid)
                .unwrap_err()
                .code(),
            ErrorCode::PublicationInvalid
        );
    }
    let mut invalid = publication.clone();
    invalid.recovery.as_mut().unwrap().plan.recovery_id = "bad".into();
    invalid.recovery.as_mut().unwrap().plan.version_protection[0]["slots"][0]["state"] =
        Value::Null;
    // Digest/result mismatch still wins over both ID and nested observation admission.
    assert_eq!(
        invalid
            .recovery
            .as_ref()
            .unwrap()
            .verify(&candidate, &invalid)
            .unwrap_err()
            .code(),
        ErrorCode::PublicationInvalid
    );
    rebind_plan(&mut invalid);
    assert_eq!(
        invalid
            .recovery
            .as_ref()
            .unwrap()
            .verify(&candidate, &invalid)
            .unwrap_err()
            .code(),
        validate_id("bad", "recovery").unwrap_err().code()
    );
}

#[test]
fn plan_reuse_does_not_observe_and_commit_staleness_precedes_repairability() {
    let (temp, engine, operation, candidate) = fixture("after_accept_2");
    lose(&engine, &candidate, true);
    drop(engine);
    let mut engine = Engine::open_for_recovery(&temp.path().join("state")).unwrap();
    let plan = engine
        .prepare_degraded_recovery(operation, "alice", "permanent loss")
        .unwrap();
    let lost = candidate
        .versions
        .iter()
        .find(|v| v.version.creation_name == "lost.txt")
        .unwrap();
    let disk = engine.disk(&lost.placement.route_slot_ids[0]).unwrap();
    let content = version_dir(
        disk,
        &lost.version.history_tree_id,
        &lost.version.version_id,
    )
    .join("content")
    .join(&lost.version.creation_name);
    fs::create_dir_all(content.parent().unwrap()).unwrap();
    fs::create_dir(&content).unwrap();
    assert!(observe(&engine, &candidate).is_err());
    let reused = engine
        .prepare_degraded_recovery(
            operation,
            "alice",
            "same plan despite later payload failure",
        )
        .unwrap();
    assert_eq!(
        canonical_json(&reused).unwrap(),
        canonical_json(&plan).unwrap()
    );
    fs::remove_dir(&content).unwrap();
    fs::write(&content, b"permanently lost bytes").unwrap();
    assert!(
        engine
            .pending_payload_source(&lost.version, &lost.placement)
            .unwrap()
            .is_some()
    );
    assert_eq!(
        engine
            .commit_degraded_recovery(&plan.recovery_id, &plan.digest, "alice", true)
            .unwrap_err()
            .code(),
        ErrorCode::PlanStale
    );
    for disk in engine.members() {
        assert!(
            !scan(&disk)
                .unwrap()
                .records
                .iter()
                .any(|(record, _)| record.sequence == candidate.sequence
                    && record.record_type == RecordType::Reprotect)
        );
    }
}
