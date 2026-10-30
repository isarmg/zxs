use super::*;
const LEGACY_DESCRIPTOR_BUDGET: usize = 128;

struct TestBudget {
    budget: Arc<PayloadProofBudget>,
    previous: Option<Arc<PayloadProofBudget>>,
}
impl TestBudget {
    fn held(&self) -> usize {
        self.budget.held.load(Ordering::Relaxed)
    }
}
impl Drop for TestBudget {
    fn drop(&mut self) {
        TEST_PROOF_BUDGET.with(|budget| *budget.borrow_mut() = self.previous.take());
    }
}
fn test_budget(capacity: usize) -> TestBudget {
    use_budget(Arc::new(PayloadProofBudget {
        capacity,
        held: AtomicUsize::new(0),
    }))
}
fn use_budget(budget: Arc<PayloadProofBudget>) -> TestBudget {
    let previous = TEST_PROOF_BUDGET.with(|current| current.replace(Some(Arc::clone(&budget))));
    TestBudget { budget, previous }
}

fn fixture() -> (tempfile::TempDir, Engine, Candidate) {
    fixture_with_files(1)
}
fn fixture_with_files(count: usize) -> (tempfile::TempDir, Engine, Candidate) {
    fixture_with_payload(count, b"original")
}
fn fixture_with_payload(count: usize, payload: &[u8]) -> (tempfile::TempDir, Engine, Candidate) {
    fixture_with_payload_and_members(count, payload, [true; 3])
}
fn fixture_with_payload_and_members(
    count: usize,
    payload: &[u8],
    metadata_members: [bool; 3],
) -> (tempfile::TempDir, Engine, Candidate) {
    let temp = tempfile::tempdir().unwrap();
    let mut config = Config::development([
        temp.path().join("p"),
        temp.path().join("b"),
        temp.path().join("t"),
    ]);
    for (disk, member) in config.disks.iter_mut().zip(metadata_members) {
        disk.metadata_member = member;
    }
    let mut engine = Engine::initialize(&temp.path().join("state"), config).unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("proof").unwrap(),
            "P01",
            3,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    for index in 0..count {
        engine
            .stage_bytes(
                &workspace.workspace_id,
                &format!("body-{index}.txt"),
                payload,
            )
            .unwrap();
    }
    let result = engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "proof".into(),
            request_digest: None,
        })
        .unwrap();
    let candidate = Candidate {
        pool_id: engine.config.pool_id.clone(),
        sequence: engine.state.sequence,
        transaction_id: new_id("tx"),
        operation_id: result.operation_id,
        request_digest: result.request_digest,
        previous_snapshot: None,
        snapshot_hash: engine.snapshot.clone().unwrap(),
        versions: engine
            .state
            .versions
            .values()
            .map(|version| VersionEvidence {
                version: version.clone(),
                placement: engine.state.placements[&version.version_id].clone(),
            })
            .collect(),
        metadata_disk_ids: engine
            .members()
            .iter()
            .map(|disk| disk.disk_id.clone())
            .collect(),
        history_roots: crate::journal::stored_map(&[], json_codec::Map::new()),
    };
    (temp, engine, candidate)
}
#[test]
fn large_candidate_uses_bounded_full_revalidation_and_rejects_changes() {
    let _budget = test_budget(LEGACY_DESCRIPTOR_BUDGET);
    let (_temp, mut engine, candidate) = fixture_with_files(11);
    let proof = VerifiedPayloadSet::prepare(&engine, &candidate).unwrap();
    assert!(proof.full_revalidation);
    assert!(proof.entries.is_empty());
    assert_eq!(proof.disks.len(), 3);
    proof.validate(&engine.config, &candidate).unwrap();
    let mut next = engine.state().clone();
    next.sequence = engine.next_slot;
    let prepared = candidate
        .versions
        .iter()
        .map(|evidence| {
            let disk = engine.disk(&evidence.placement.route_slot_ids[0]).unwrap();
            let directory = version_dir(
                disk,
                &evidence.version.history_tree_id,
                &evidence.version.version_id,
            );
            crate::engine::Prepared {
                version: evidence.version.clone(),
                placement: evidence.placement.clone(),
                source: crate::engine::ReadSource::on_disk(
                    engine.disk_resources.get(disk).unwrap(),
                    crate::container_storage::content_path(&directory, &evidence.version).unwrap(),
                ),
            }
        })
        .collect();
    let result = engine.state.operations[&candidate.operation_id].clone();
    let detached = engine
        .prepare_publication(next, prepared, result, MAX_METADATA)
        .unwrap();
    let evidence = &candidate.versions[0];
    let slot = &evidence.placement.route_slot_ids[0];
    let disk = engine.disk(slot).unwrap();
    let directory = version_dir(
        disk,
        &evidence.version.history_tree_id,
        &evidence.version.version_id,
    );
    let body = crate::container_storage::content_path(&directory, &evidence.version).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&body, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::write(&body, b"tampered").unwrap();
    assert!(proof.validate(&engine.config, &candidate).is_err());
    assert!(
        proof
            .cursor()
            .validate_payload(&engine.config, &evidence.version, slot)
            .is_err()
    );
    let sequence = engine.state.sequence;
    let next_slot = engine.next_slot;
    assert_eq!(
        engine.decide_publication(detached).unwrap_err().code(),
        storage_contract::ErrorCode::PreparationStale
    );
    assert_eq!(engine.state.sequence, sequence);
    assert_eq!(engine.next_slot, next_slot);
    assert!(!engine.blocked);
}
#[test]
fn held_proof_rejects_in_place_content_changes() {
    let _budget = test_budget(LEGACY_DESCRIPTOR_BUDGET);
    let (_temp, engine, candidate) = fixture();
    let epoch = engine.metadata_integrity.capture();
    let proof = VerifiedPayloadSet::prepare(&engine, &candidate).unwrap();
    proof.validate(&engine.config, &candidate).unwrap();
    let evidence = &candidate.versions[0];
    let disk = engine.disk(&evidence.placement.route_slot_ids[0]).unwrap();
    let dir = version_dir(
        disk,
        &evidence.version.history_tree_id,
        &evidence.version.version_id,
    );
    let body = crate::container_storage::content_path(&dir, &evidence.version).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&body, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::write(body, b"tampered").unwrap();
    assert_eq!(
        proof
            .validate(&engine.config, &candidate)
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::PreparationStale
    );
    assert!(epoch.matches(&engine.metadata_integrity));
}
#[test]
fn held_proof_rejects_mode_changes_without_content_changes() {
    let _budget = test_budget(LEGACY_DESCRIPTOR_BUDGET);
    let (_temp, engine, candidate) = fixture();
    let proof = VerifiedPayloadSet::prepare(&engine, &candidate).unwrap();
    let body = &proof.entries[0].body.file;
    use std::os::unix::fs::PermissionsExt;
    let mode = body.metadata().unwrap().permissions().mode();
    body.set_permissions(std::fs::Permissions::from_mode(mode ^ 0o100))
        .unwrap();
    assert_eq!(
        proof
            .validate(&engine.config, &candidate)
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::PreparationStale
    );
}
#[test]
fn held_proof_rejects_replaced_checksum_inode() {
    let _budget = test_budget(LEGACY_DESCRIPTOR_BUDGET);
    let (_temp, engine, candidate) = fixture();
    let epoch = engine.metadata_integrity.capture();
    let proof = VerifiedPayloadSet::prepare(&engine, &candidate).unwrap();
    let entry = &proof.entries[0];
    let path = entry.directory.join("checksums.bin");
    let bytes = std::fs::read(&path).unwrap();
    std::fs::rename(&path, path.with_extension("old")).unwrap();
    std::fs::write(&path, bytes).unwrap();
    assert_eq!(
        proof
            .validate(&engine.config, &candidate)
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::PreparationStale
    );
    assert!(epoch.matches(&engine.metadata_integrity));
}
#[test]
fn held_proof_rejects_disk_seal_replacement_and_candidate_changes() {
    let _budget = test_budget(LEGACY_DESCRIPTOR_BUDGET);
    let (_temp, engine, candidate) = fixture();
    let epoch = engine.metadata_integrity.capture();
    let proof = VerifiedPayloadSet::prepare(&engine, &candidate).unwrap();
    let mut changed = candidate.clone();
    changed.versions[0].version.content_hash = "0".repeat(64).parse().unwrap();
    assert_eq!(
        proof.validate(&engine.config, &changed).unwrap_err().code(),
        storage_contract::ErrorCode::PreparationStale
    );
    assert!(epoch.matches(&engine.metadata_integrity));
    let path = proof.disks[0].path.clone();
    let bytes = std::fs::read(&path).unwrap();
    std::fs::rename(&path, path.with_extension("old")).unwrap();
    std::fs::write(&path, bytes).unwrap();
    assert_eq!(
        proof
            .validate(&engine.config, &candidate)
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::PreparationStale
    );
    assert!(!epoch.matches(&engine.metadata_integrity));
}

#[test]
fn old_payload_proof_identity_fault_revokes_the_current_writer_after_a_later_publication() {
    let _budget = test_budget(LEGACY_DESCRIPTOR_BUDGET);
    let (_temp, mut engine, candidate) = fixture();
    let proof = VerifiedPayloadSet::prepare(&engine, &candidate).unwrap();
    engine
        .persist_auth_state(
            json_codec::json!({"auth": "later publication"}),
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let current_epoch = engine.metadata_integrity.capture();
    let path = proof
        .disks
        .iter()
        .find(|disk| disk.metadata_member)
        .unwrap()
        .path
        .clone();
    let bytes = std::fs::read(&path).unwrap();
    std::fs::rename(&path, path.with_extension("old")).unwrap();
    std::fs::write(path, bytes).unwrap();
    assert_eq!(
        proof
            .validate(&engine.config, &candidate)
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::PreparationStale
    );
    assert!(!current_epoch.matches(&engine.metadata_integrity));
    assert!(!engine.blocked);
}

#[test]
fn non_metadata_payload_disk_identity_fault_does_not_revoke_shared_graph_provenance() {
    let _budget = test_budget(LEGACY_DESCRIPTOR_BUDGET);
    let (_temp, engine, candidate) =
        fixture_with_payload_and_members(1, b"original", [true, false, true]);
    let proof = VerifiedPayloadSet::prepare(&engine, &candidate).unwrap();
    let epoch = engine.metadata_integrity.capture();
    let path = proof
        .disks
        .iter()
        .find(|disk| !disk.metadata_member)
        .unwrap()
        .path
        .clone();
    let bytes = std::fs::read(&path).unwrap();
    std::fs::rename(&path, path.with_extension("old")).unwrap();
    std::fs::write(path, bytes).unwrap();
    assert_eq!(
        proof
            .validate(&engine.config, &candidate)
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::PreparationStale
    );
    assert!(epoch.matches(&engine.metadata_integrity));
}

#[test]
fn payload_preparation_reports_a_missing_metadata_seal_to_its_writer() {
    let _budget = test_budget(LEGACY_DESCRIPTOR_BUDGET);
    let (_temp, engine, candidate) = fixture();
    let epoch = engine.metadata_integrity.capture();
    let member = engine.config.metadata_members().next().unwrap();
    std::fs::remove_file(member.root.join("disk.json")).unwrap();
    assert!(
        PayloadPreparation::new(
            &engine.config,
            &engine.disk_resources,
            &candidate,
            Arc::clone(&engine.metadata_integrity)
        )
        .is_err()
    );
    assert!(!epoch.matches(&engine.metadata_integrity));
}
#[test]
fn detached_decision_rejects_changed_topology_before_journaling() {
    let _budget = test_budget(LEGACY_DESCRIPTOR_BUDGET);
    let (_temp, mut engine, candidate) = fixture();
    let mut next = engine.state().clone();
    next.sequence = engine.next_slot;
    let result = engine.state.operations[&candidate.operation_id].clone();
    let detached = engine
        .prepare_publication(next, Vec::new(), result, MAX_METADATA)
        .unwrap();
    let sequence = engine.state.sequence;
    let slot = engine.next_slot;
    let mut config = engine.config().clone();
    config.metadata_reserve_bytes += 1;
    engine.config = ValidatedConfig::try_from(config).unwrap();
    assert_eq!(
        engine.decide_publication(detached).unwrap_err().code(),
        storage_contract::ErrorCode::PreparationStale
    );
    assert_eq!(engine.state.sequence, sequence);
    assert_eq!(engine.next_slot, slot);
    assert!(!engine.blocked);
}
#[test]
fn preparation_pins_disk_identity_before_payload_work() {
    let _budget = test_budget(LEGACY_DESCRIPTOR_BUDGET);
    let (_temp, engine, candidate) = fixture();
    let epoch = engine.metadata_integrity.capture();
    let preparation = PayloadPreparation::new(
        &engine.config,
        &engine.disk_resources,
        &candidate,
        Arc::clone(&engine.metadata_integrity),
    )
    .unwrap();
    let path = preparation.disks[0].path.clone();
    let bytes = std::fs::read(&path).unwrap();
    std::fs::rename(&path, path.with_extension("old")).unwrap();
    std::fs::write(&path, bytes).unwrap();
    match preparation.verify(&engine.config, &candidate) {
        Err(error) => assert_eq!(error.code(), storage_contract::ErrorCode::PreparationStale),
        Ok(_) => panic!("replaced disk seal was accepted"),
    }
    assert!(!epoch.matches(&engine.metadata_integrity));
}

// Pre-optimization reference: retain the discarded first pass explicitly.
fn reference_prepare(engine: &Engine, candidate: &Candidate) -> Result<VerifiedPayloadSet> {
    let config = &engine.config;
    let preparation = PayloadPreparation::new(
        config,
        &engine.disk_resources,
        candidate,
        Arc::clone(&engine.metadata_integrity),
    )?;
    let replicas: usize = candidate
        .versions
        .iter()
        .map(|e| e.placement.route_slot_ids.len())
        .sum();
    let reservation = payload_proof_budget().reserve(replicas * 4);
    let full_revalidation = reservation.is_none();
    let mut entries = Vec::new();
    for evidence in &candidate.versions {
        for slot in &evidence.placement.route_slot_ids {
            let entry = verify_replica(config, &engine.disk_resources, &evidence.version, slot)?;
            if !full_revalidation {
                entries.push(entry);
            }
        }
    }
    let proof = VerifiedPayloadSet {
        metadata_integrity: preparation.metadata_integrity,
        entries,
        _reservation: reservation,
        disks: preparation.disks,
        resources: preparation.resources,
        full_revalidation,
        replicas,
    };
    proof.validate(config, candidate)?;
    Ok(proof)
}
fn verification_cost<T>(run: impl FnOnce() -> T) -> (T, (u64, u64)) {
    VERIFICATION_COST.with(|cost| cost.set((0, 0)));
    let result = run();
    (result, VERIFICATION_COST.with(|cost| cost.get()))
}
#[test]
fn preparation_hash_cost_matches_reference_at_proof_budget_boundary() {
    let budget = test_budget(LEGACY_DESCRIPTOR_BUDGET);
    let (_temp, engine, candidate) = fixture_with_files(11);
    for replicas in [32, 33] {
        let mut candidate = candidate.clone();
        let last = candidate.versions.last_mut().unwrap();
        last.placement.route_slot_ids.truncate(replicas - 30);
        let (reference, old_cost) = verification_cost(|| reference_prepare(&engine, &candidate));
        let reference = reference.unwrap();
        let expected_mode = reference.full_revalidation;
        let expected_entries = reference.entries.len();
        drop(reference);
        assert_eq!(budget.held(), 0);
        let (optimized, new_cost) =
            verification_cost(|| VerifiedPayloadSet::prepare(&engine, &candidate));
        let optimized = optimized.unwrap();
        assert_eq!(expected_mode, optimized.full_revalidation);
        assert_eq!(expected_entries, optimized.entries.len());
        let replicas = replicas as u64;
        assert_eq!(new_cost, (replicas, replicas * 8));
        let passes = if optimized.full_revalidation { 2 } else { 1 };
        assert_eq!(old_cost, (replicas * passes, replicas * 8 * passes));
        optimized.validate(&engine.config, &candidate).unwrap();
    }
}
#[test]
fn large_preparation_rejects_corruption_like_reference() {
    let budget = test_budget(LEGACY_DESCRIPTOR_BUDGET);
    let (_temp, engine, candidate) = fixture_with_files(11);
    // Corrupt the final replica to prove the optimized pass visits all replicas.
    let evidence = candidate.versions.last().unwrap();
    let slot = evidence.placement.route_slot_ids.last().unwrap();
    let disk = engine.disk(slot).unwrap();
    let directory = version_dir(
        disk,
        &evidence.version.history_tree_id,
        &evidence.version.version_id,
    );
    let body = crate::container_storage::content_path(&directory, &evidence.version).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&body, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::write(body, b"tampered").unwrap();
    let old_error = reference_prepare(&engine, &candidate).err().unwrap();
    let new_error = VerifiedPayloadSet::prepare(&engine, &candidate)
        .err()
        .unwrap();
    assert_eq!(
        new_error.code(),
        storage_contract::ErrorCode::ChecksumMismatch
    );
    assert_eq!(old_error.code(), new_error.code());
    assert_eq!(budget.held(), 0);
}
#[test]
#[ignore = "manual preparation-only benchmark; not a physical-storage benchmark"]
fn benchmark_large_preparation_hash_passes() {
    let _budget = test_budget(LEGACY_DESCRIPTOR_BUDGET);
    let payload = vec![0x5a; 1024 * 1024];
    let (_temp, engine, candidate) = fixture_with_payload(11, &payload);
    for round in 0..4 {
        for reference in if round % 2 == 0 {
            [true, false]
        } else {
            [false, true]
        } {
            let start = std::time::Instant::now();
            let (proof, (replicas, bytes)) = verification_cost(|| {
                if reference {
                    reference_prepare(&engine, &candidate)
                } else {
                    VerifiedPayloadSet::prepare(&engine, &candidate)
                }
            });
            let proof = proof.unwrap();
            assert!(proof.full_revalidation);
            assert!(proof.entries.is_empty());
            eprintln!(
                "preparation round={round} reference={reference} elapsed_us={} replica_hashes={replicas} payload_bytes={bytes}",
                start.elapsed().as_micros()
            );
        }
    }
}

#[test]
fn large_preparation_rechecks_disk_identity_after_last_replica() {
    let budget = test_budget(LEGACY_DESCRIPTOR_BUDGET);
    let (_temp, engine, candidate) = fixture_with_files(11);
    let epoch = engine.metadata_integrity.capture();
    let preparation = PayloadPreparation::new(
        &engine.config,
        &engine.disk_resources,
        &candidate,
        Arc::clone(&engine.metadata_integrity),
    )
    .unwrap();
    let path = preparation.disks[0].path.clone();
    let bytes = std::fs::read(&path).unwrap();
    // Replace with identical contents after all hashes succeed, while the
    // original disk seal is still held. Content equality is not live evidence.
    AFTER_REPLICA_VALIDATION.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            std::fs::rename(&path, path.with_extension("old")).unwrap();
            std::fs::write(&path, bytes).unwrap();
        }));
    });
    let error = preparation
        .verify(&engine.config, &candidate)
        .err()
        .unwrap();
    assert_eq!(error.code(), storage_contract::ErrorCode::PreparationStale);
    assert!(!epoch.matches(&engine.metadata_integrity));
    assert_eq!(budget.held(), 0);
}
#[test]
fn large_preparation_rejects_disk_change_before_hashing() {
    let budget = test_budget(LEGACY_DESCRIPTOR_BUDGET);
    let (_temp, engine, candidate) = fixture_with_files(11);
    let epoch = engine.metadata_integrity.capture();
    let preparation = PayloadPreparation::new(
        &engine.config,
        &engine.disk_resources,
        &candidate,
        Arc::clone(&engine.metadata_integrity),
    )
    .unwrap();
    let path = preparation.disks[0].path.clone();
    let bytes = std::fs::read(&path).unwrap();
    std::fs::rename(&path, path.with_extension("old")).unwrap();
    std::fs::write(&path, bytes).unwrap();
    let (result, cost) = verification_cost(|| preparation.verify(&engine.config, &candidate));
    assert_eq!(
        result.err().unwrap().code(),
        storage_contract::ErrorCode::PreparationStale
    );
    assert_eq!(cost, (0, 0));
    assert!(!epoch.matches(&engine.metadata_integrity));
    assert_eq!(budget.held(), 0);
}

fn existing_prepared(engine: &Engine, candidate: &Candidate) -> Vec<crate::engine::Prepared> {
    candidate
        .versions
        .iter()
        .map(|evidence| {
            let disk = engine.disk(&evidence.placement.route_slot_ids[0]).unwrap();
            let directory = version_dir(
                disk,
                &evidence.version.history_tree_id,
                &evidence.version.version_id,
            );
            crate::engine::Prepared {
                version: evidence.version.clone(),
                placement: evidence.placement.clone(),
                source: crate::engine::ReadSource::on_disk(
                    engine.disk_resources.get(disk).unwrap(),
                    crate::container_storage::content_path(&directory, &evidence.version).unwrap(),
                ),
            }
        })
        .collect()
}

#[test]
fn existing_admission_preserves_capacity_and_corruption_causes() {
    let budget = test_budget(LEGACY_DESCRIPTOR_BUDGET);
    for damage in ["none", "body", "manifest", "unsafe", "missing"] {
        let (_temp, mut engine, candidate) = fixture();
        let prepared = existing_prepared(&engine, &candidate);
        let evidence = &candidate.versions[0];
        let disk = engine.disk(&evidence.placement.route_slot_ids[0]).unwrap();
        let directory = version_dir(
            disk,
            &evidence.version.history_tree_id,
            &evidence.version.version_id,
        );
        let body = crate::container_storage::content_path(&directory, &evidence.version).unwrap();
        match damage {
            "body" => {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&body, std::fs::Permissions::from_mode(0o600)).unwrap();
                std::fs::write(&body, b"tampered").unwrap();
            }
            "manifest" => std::fs::write(directory.join("checksums.bin"), b"corrupt").unwrap(),
            "unsafe" => {
                let path = directory.join("checksums.bin");
                std::fs::remove_file(&path).unwrap();
                std::os::unix::fs::symlink("missing", path).unwrap();
            }
            "missing" => std::fs::remove_file(&body).unwrap(),
            "none" => (),
            _ => unreachable!(),
        }
        let mut config = engine.config().clone();
        for disk in &mut config.disks {
            disk.capacity_bytes = Some(0);
        }
        engine.config = ValidatedConfig::try_from(config).unwrap();
        let mut next = engine.state().clone();
        next.sequence = engine.next_slot;
        let result = engine.state.operations[&candidate.operation_id].clone();
        let outcome = engine.prepare_publication(next, prepared, result, MAX_METADATA);
        match damage {
            "none" => drop(outcome.unwrap()),
            "body" | "manifest" => assert_eq!(
                outcome.err().unwrap().code(),
                storage_contract::ErrorCode::ChecksumMismatch
            ),
            "missing" => assert_eq!(
                outcome.err().unwrap().code(),
                storage_contract::ErrorCode::NoSpace
            ),
            "unsafe" => assert_ne!(
                outcome.err().unwrap().code(),
                storage_contract::ErrorCode::NoSpace
            ),
            _ => unreachable!(),
        }
        assert!(!engine.blocked);
        assert_eq!(budget.held(), 0);
    }
}

#[test]
fn existing_install_token_does_not_replace_decision_byte_verification() {
    let budget = test_budget(LEGACY_DESCRIPTOR_BUDGET);
    let (_temp, engine, candidate) = fixture();
    let evidence = &candidate.versions[0];
    let disk = engine.disk(&evidence.placement.route_slot_ids[0]).unwrap();
    let directory = version_dir(
        disk,
        &evidence.version.history_tree_id,
        &evidence.version.version_id,
    );
    let proof = VerifiedExisting::read(&directory, &evidence.version).unwrap();
    let body = crate::container_storage::content_path(&directory, &evidence.version).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&body, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::write(&body, b"tampered").unwrap();
    // Same inode: this token guarantees the synchronization target, not a cache
    // invalidation protocol. The actual next publication phase must read bytes.
    crate::container_storage::install_version_content(
        &directory,
        crate::container_storage::PreparedContent::Verified(proof),
    )
    .unwrap();
    assert_eq!(
        PayloadPreparation::new(
            &engine.config,
            &engine.disk_resources,
            &candidate,
            Arc::clone(&engine.metadata_integrity)
        )
        .unwrap()
        .verify(&engine.config, &candidate)
        .err()
        .unwrap()
        .code(),
        storage_contract::ErrorCode::ChecksumMismatch
    );
    assert!(!engine.blocked);
    assert_eq!(budget.held(), 0);
}

#[test]
fn existing_admission_above_descriptor_budget_uses_uncached_preparation() {
    let budget = test_budget(LEGACY_DESCRIPTOR_BUDGET);
    let (_temp, engine, candidate) = fixture_with_files(43);
    let prepared = existing_prepared(&engine, &candidate);
    assert!(
        prepared
            .iter()
            .map(|p| p.placement.route_slot_ids.len())
            .sum::<usize>()
            > LEGACY_DESCRIPTOR_BUDGET
    );
    let mut next = engine.state().clone();
    next.sequence = engine.next_slot;
    let result = engine.state.operations[&candidate.operation_id].clone();
    let detached = engine
        .prepare_publication(next, prepared, result, MAX_METADATA)
        .unwrap();
    assert!(detached.verified.full_revalidation);
    assert!(detached.verified.entries.is_empty());
    assert_eq!(budget.held(), 0);
}

#[test]
fn name_projection_uses_the_owner_but_payload_admission_reports_a_corrupt_seal() {
    let _budget = test_budget(LEGACY_DESCRIPTOR_BUDGET);
    let (_temp, engine, candidate) = fixture();
    let version = &candidate.versions[0].version;
    let mut next = engine.state().clone();
    next.file_names
        .insert(version.file_id.clone(), "renamed.txt".into());
    let disk = engine
        .disk(&candidate.versions[0].placement.route_slot_ids[0])
        .unwrap();
    assert!(disk.metadata_member);
    let epoch = engine.metadata_integrity.capture();
    std::fs::remove_file(disk.root.join("disk.json")).unwrap();
    crate::container_storage::prepare_names(
        &engine.config,
        &engine.disk_resources,
        &crate::state_projection::ProjectionTransition::new(engine.state(), &next, None).unwrap(),
        MAX_METADATA,
    )
    .unwrap();
    assert!(epoch.matches(&engine.metadata_integrity));
    assert!(
        PayloadPreparation::new(
            &engine.config,
            &engine.disk_resources,
            &candidate,
            Arc::clone(&engine.metadata_integrity)
        )
        .is_err()
    );
    assert!(!epoch.matches(&engine.metadata_integrity));
}

#[test]
fn held_proof_does_not_rebind_when_the_configured_names_are_replaced() {
    let _budget = test_budget(LEGACY_DESCRIPTOR_BUDGET);
    let (_temp, engine, candidate) = fixture();
    let proof = VerifiedPayloadSet::prepare(&engine, &candidate).unwrap();
    for disk in &engine.config.disks {
        std::fs::rename(&disk.root, disk.root.with_extension("held")).unwrap();
        std::fs::create_dir(&disk.root).unwrap();
    }
    proof.validate(&engine.config, &candidate).unwrap();
    assert!(
        engine
            .config
            .disks
            .iter()
            .all(|disk| std::fs::read_dir(&disk.root).unwrap().next().is_none())
    );
}

#[test]
fn detached_decision_rejects_a_different_disk_resource_generation() {
    let _budget = test_budget(LEGACY_DESCRIPTOR_BUDGET);
    let (_temp, mut engine, candidate) = fixture();
    let mut next = engine.state().clone();
    next.sequence = engine.next_slot;
    let result = engine.state.operations[&candidate.operation_id].clone();
    let detached = engine
        .prepare_publication(next, Vec::new(), result, MAX_METADATA)
        .unwrap();
    let sequence = engine.state.sequence;
    let slot = engine.next_slot;
    engine.disk_resources = Arc::new(DiskResources::open(&engine.config, true).unwrap());
    assert_eq!(
        engine.decide_publication(detached).unwrap_err().code(),
        ErrorCode::PreparationStale
    );
    assert_eq!(engine.state.sequence, sequence);
    assert_eq!(engine.next_slot, slot);
    assert!(!engine.blocked);
}

#[test]
fn descriptor_capacity_is_a_bounded_fraction_of_the_initial_soft_limit() {
    for (limit, capacity) in [
        (Some(0), 0),
        (Some(3), 0),
        (Some(4), 1),
        (Some(512), 128),
        (Some(1024), 256),
        (Some(32767), 8191),
        (Some(32768), 8192),
        (Some(u64::MAX), 8192),
        (None, 8192),
    ] {
        let budget = PayloadProofBudget::from_soft_limit(limit);
        assert_eq!(budget.capacity, capacity);
        assert_eq!(budget.held.load(Ordering::Relaxed), 0);
    }
}

#[test]
fn default_payload_proof_budget_is_process_shared() {
    let budget = payload_proof_budget();
    let other = std::thread::spawn(payload_proof_budget).join().unwrap();
    assert!(Arc::ptr_eq(&budget, &other));
    assert!(budget.capacity <= 8192);
}

#[test]
fn concurrent_reservations_never_exceed_capacity_and_are_reusable() {
    let budget = test_budget(8);
    let acquired = Arc::new(std::sync::Barrier::new(9));
    let release = Arc::new(std::sync::Barrier::new(9));
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let budget = Arc::clone(&budget.budget);
                let acquired = Arc::clone(&acquired);
                let release = Arc::clone(&release);
                scope.spawn(move || {
                    let reservation = budget.reserve(2);
                    let retained = reservation.is_some();
                    acquired.wait();
                    release.wait();
                    drop(reservation);
                    retained
                })
            })
            .collect();
        acquired.wait();
        let held = budget.held();
        let unavailable = budget.budget.reserve(1).is_none();
        let oversized = budget.budget.reserve(usize::MAX).is_none();
        release.wait();
        let winners = workers
            .into_iter()
            .map(|worker| usize::from(worker.join().unwrap()))
            .sum::<usize>();
        assert_eq!(held, 8);
        assert!(unavailable);
        assert!(oversized);
        assert_eq!(winners, 4);
    });
    assert_eq!(budget.held(), 0);
    drop(budget.budget.reserve(8).unwrap());
    assert_eq!(budget.held(), 0);
}

fn new_publication_state(engine: &Engine, candidate: &Candidate) -> (State, OperationResult) {
    let mut next = engine.state().clone();
    next.sequence = engine.next_slot;
    let mut result = next.operations[&candidate.operation_id].clone();
    result.operation_id = storage_contract::OperationId::generate();
    result.sequence = next.sequence;
    next.operations.insert(result.operation_id, result.clone());
    (next, result)
}

#[test]
fn thirty_three_replica_decisions_have_exact_hash_and_linear_lookup_costs() {
    for capacity in [128, 132] {
        let budget = test_budget(capacity);
        let (_temp, mut engine, candidate) = fixture_with_files(11);
        let prepared = existing_prepared(&engine, &candidate);
        let (next, result) = new_publication_state(&engine, &candidate);
        PROOF_LOOKUPS.with(|lookups| lookups.set(0));
        let (detached, cost) = verification_cost(|| {
            engine.prepare_publication(next, prepared, result.clone(), MAX_METADATA)
        });
        let detached = detached.unwrap();
        let retained = capacity == 132;
        assert_eq!(detached.verified.full_revalidation, !retained);
        assert_eq!(
            detached.verified.entries.len(),
            if retained { 33 } else { 0 }
        );
        assert_eq!(budget.held(), if retained { 132 } else { 0 });
        assert_eq!(cost, (33, 33 * 8));
        assert_eq!(
            PROOF_LOOKUPS.with(|lookups| lookups.get()),
            if retained { 33 } else { 0 }
        );
        PROOF_LOOKUPS.with(|lookups| lookups.set(0));
        let (decided, cost) = verification_cost(|| engine.decide_publication(detached));
        assert_eq!(decided.unwrap().operation_id, result.operation_id);
        assert_eq!(cost, if retained { (0, 0) } else { (66, 66 * 8) });
        assert_eq!(
            PROOF_LOOKUPS.with(|lookups| lookups.get()),
            if retained { 66 } else { 0 }
        );
        assert_eq!(budget.held(), 0);
    }
}

#[test]
fn held_preparations_on_distinct_engines_share_the_same_reservation_pool() {
    let budget = test_budget(24);
    let (_temp, engine, candidate) = fixture();
    let (_other_temp, other_engine, other_candidate) = fixture();
    let first = VerifiedPayloadSet::prepare(&engine, &candidate).unwrap();
    assert!(!first.full_revalidation);
    assert_eq!(budget.held(), 12);
    let (ready, prepared) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let shared = Arc::clone(&budget.budget);
    let worker = std::thread::spawn(move || {
        let _budget = use_budget(shared);
        let proof = VerifiedPayloadSet::prepare(&other_engine, &other_candidate).unwrap();
        assert!(!proof.full_revalidation);
        ready.send(()).unwrap();
        released.recv().unwrap();
        drop(proof);
    });
    prepared.recv().unwrap();
    assert_eq!(budget.held(), 24);
    let (fallback, cost) = verification_cost(|| VerifiedPayloadSet::prepare(&engine, &candidate));
    let fallback = fallback.unwrap();
    assert!(fallback.full_revalidation);
    assert!(fallback.entries.is_empty());
    assert_eq!(cost, (3, 24));
    assert_eq!(budget.held(), 24);
    drop(first);
    assert_eq!(budget.held(), 12);
    release.send(()).unwrap();
    worker.join().unwrap();
    assert_eq!(budget.held(), 0);
    let reused = VerifiedPayloadSet::prepare(&engine, &candidate).unwrap();
    assert!(!reused.full_revalidation);
    assert_eq!(budget.held(), 12);
    drop(reused);
    drop(fallback);
    assert_eq!(budget.held(), 0);
}

#[test]
fn partial_verification_failure_releases_reservations_and_preserves_open_errors() {
    for capacity in [0, 128] {
        let budget = test_budget(capacity);
        let (_temp, engine, candidate) = fixture_with_files(2);
        let evidence = candidate.versions.last().unwrap();
        let disk = engine
            .disk(evidence.placement.route_slot_ids.last().unwrap())
            .unwrap();
        let directory = version_dir(
            disk,
            &evidence.version.history_tree_id,
            &evidence.version.version_id,
        );
        let body = crate::container_storage::content_path(&directory, &evidence.version).unwrap();
        std::fs::remove_file(body).unwrap();
        let error = VerifiedPayloadSet::prepare(&engine, &candidate)
            .err()
            .unwrap();
        match error {
            Error::Filesystem(platform_io::fs_safety::Error::Io(error))
            | Error::Contract(storage_contract::Error::Io { source: error, .. }) => {
                assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
            }
            error => panic!("open error was replaced: {error}"),
        }
        assert_eq!(budget.held(), 0);
    }
}

#[test]
fn detached_reservations_release_on_drop_and_stale_decision() {
    for stale in [false, true] {
        let budget = test_budget(12);
        let (_temp, mut engine, candidate) = fixture();
        let prepared = existing_prepared(&engine, &candidate);
        let (next, result) = new_publication_state(&engine, &candidate);
        let detached = engine
            .prepare_publication(next, prepared, result, MAX_METADATA)
            .unwrap();
        // Existing-copy admission held three reservations. All must have closed
        // and returned before the twelve-descriptor decision reservation fits.
        assert!(!detached.verified.full_revalidation);
        assert_eq!(budget.held(), 12);
        if stale {
            let mut config = engine.config().clone();
            config.metadata_reserve_bytes += 1;
            engine.config = ValidatedConfig::try_from(config).unwrap();
            let sequence = engine.state.sequence;
            let slot = engine.next_slot;
            assert_eq!(
                engine.decide_publication(detached).unwrap_err().code(),
                ErrorCode::PreparationStale
            );
            assert_eq!(engine.state.sequence, sequence);
            assert_eq!(engine.next_slot, slot);
        } else {
            drop(detached);
        }
        assert_eq!(budget.held(), 0);
        assert!(!engine.blocked);
    }
}

#[cfg(feature = "fault-injection")]
#[test]
fn decision_error_releases_retained_proofs() {
    let budget = test_budget(12);
    let (_temp, mut engine, candidate) = fixture();
    let (next, result) = new_publication_state(&engine, &candidate);
    let detached = engine
        .prepare_publication(
            next,
            existing_prepared(&engine, &candidate),
            result,
            MAX_METADATA,
        )
        .unwrap();
    assert_eq!(budget.held(), 12);
    engine.inject_fault_once("after_payloads");
    assert!(engine.decide_publication(detached).is_err());
    assert_eq!(budget.held(), 0);
}

#[test]
fn existing_copy_reservations_follow_installation_success_and_error() {
    for replace_body in [false, true] {
        let budget = test_budget(3);
        let (_temp, engine, candidate) = fixture();
        let prepared = existing_prepared(&engine, &candidate);
        let p = &prepared[0];
        let disk = engine.disk(&p.placement.route_slot_ids[0]).unwrap();
        let handle = engine.disk_resources.get(disk).unwrap();
        let directory = version_dir(disk, &p.version.history_tree_id, &p.version.version_id);
        let reservation = budget.budget.reserve(1).unwrap();
        let proof = VerifiedExisting::read_at(&handle.root, &directory, &p.version).unwrap();
        if replace_body {
            let path = proof.path();
            let bytes = std::fs::read(path).unwrap();
            std::fs::rename(path, path.with_extension("old")).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        let copies = vec![
            Some(super::super::RetainedExisting { proof, reservation }),
            None,
            None,
        ];
        assert_eq!(budget.held(), 1);
        let result = engine
            .planning_context()
            .prepare_payload_with_existing(p, Some(copies));
        if replace_body {
            assert_eq!(result.unwrap_err().code(), ErrorCode::PreparationStale);
        } else {
            result.unwrap();
        }
        assert_eq!(budget.held(), 0);
    }
}

#[test]
fn ordered_proofs_reject_version_slot_hash_and_count_mismatches() {
    let _budget = test_budget(LEGACY_DESCRIPTOR_BUDGET);
    let (_temp, engine, candidate) = fixture_with_files(2);
    let proof = VerifiedPayloadSet::prepare(&engine, &candidate).unwrap();
    for change in [
        "version_order",
        "slot_order",
        "version_id",
        "version_hash",
        "missing",
        "extra",
    ] {
        let mut changed = candidate.clone();
        match change {
            "version_order" => changed.versions.swap(0, 1),
            "slot_order" => changed.versions[0].placement.route_slot_ids.swap(0, 1),
            "version_id" => changed.versions[0].version.version_id = new_id("ver"),
            "version_hash" => {
                changed.versions[0].version.content_hash = "0".repeat(64).parse().unwrap();
            }
            "missing" => {
                changed.versions.pop();
            }
            "extra" => changed.versions.push(changed.versions[0].clone()),
            _ => unreachable!(),
        }
        assert_eq!(
            proof.validate(&engine.config, &changed).unwrap_err().code(),
            ErrorCode::PreparationStale,
            "{change}"
        );
    }
    let mut cursor = proof.cursor();
    let first = &candidate.versions[0];
    cursor
        .validate_payload(
            &engine.config,
            &first.version,
            &first.placement.route_slot_ids[0],
        )
        .unwrap();
    assert_eq!(
        cursor.finish().unwrap_err().code(),
        ErrorCode::PreparationStale
    );
    proof.validate(&engine.config, &candidate).unwrap();
    let _fallback_budget = test_budget(0);
    let fallback = VerifiedPayloadSet::prepare(&engine, &candidate).unwrap();
    assert!(fallback.full_revalidation);
    for extra in [false, true] {
        let mut changed = candidate.clone();
        if extra {
            changed.versions.push(changed.versions[0].clone());
        } else {
            changed.versions.pop();
        }
        assert_eq!(
            fallback
                .validate(&engine.config, &changed)
                .unwrap_err()
                .code(),
            ErrorCode::PreparationStale
        );
    }
}

#[test]
#[should_panic(expected = "invariant: each routed replica has an admission result")]
fn incomplete_private_admission_is_not_an_uncached_request() {
    let _budget = test_budget(LEGACY_DESCRIPTOR_BUDGET);
    let (_temp, engine, candidate) = fixture();
    let prepared = existing_prepared(&engine, &candidate);
    engine
        .planning_context()
        .prepare_payload_with_existing(&prepared[0], Some(Vec::new()))
        .unwrap();
}

#[test]
#[should_panic(expected = "invariant: each prepared version has its admission results")]
fn incomplete_private_version_admission_cannot_be_truncated_by_zip() {
    let _budget = test_budget(LEGACY_DESCRIPTOR_BUDGET);
    let (_temp, engine, candidate) = fixture();
    let prepared = existing_prepared(&engine, &candidate);
    engine
        .planning_context()
        .prepare_payloads(&prepared, Vec::new())
        .unwrap();
}
