use super::*;
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
    let (_temp, mut engine, candidate) =
        fixture_with_files(DEFAULT_MAX_HELD_PAYLOAD_FILES / 12 + 1);
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
    let result = engine
        .published_receipt(candidate.operation_id)
        .unwrap()
        .unwrap();
    let detached = engine
        .prepare_publication(next, prepared, Vec::new(), result, MAX_METADATA)
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
    let (_temp, mut engine, candidate) = fixture();
    let mut next = engine.state().clone();
    next.sequence = engine.next_slot;
    let result = engine
        .published_receipt(candidate.operation_id)
        .unwrap()
        .unwrap();
    let detached = engine
        .prepare_publication(next, Vec::new(), Vec::new(), result, MAX_METADATA)
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
    let full_revalidation = replicas > config.max_held_payload_files().get() / 4;
    let mut entries = Vec::new();
    for evidence in &candidate.versions {
        for slot in &evidence.placement.route_slot_ids {
            let entry = verify_replica(config, &engine.disk_resources, &evidence.version, slot)?;
            if !full_revalidation {
                entries.push(entry);
            }
        }
    }
    entries.sort_by(|a, b| a.identity().cmp(&b.identity()));
    let proof = VerifiedPayloadSet {
        metadata_integrity: preparation.metadata_integrity,
        entries,
        disks: preparation.disks,
        resources: preparation.resources,
        full_revalidation,
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
    let (_temp, engine, candidate) = fixture_with_files(DEFAULT_MAX_HELD_PAYLOAD_FILES / 12 + 1);
    for replicas in [
        DEFAULT_MAX_HELD_PAYLOAD_FILES / 4,
        DEFAULT_MAX_HELD_PAYLOAD_FILES / 4 + 1,
    ] {
        let mut candidate = candidate.clone();
        let last = candidate.versions.last_mut().unwrap();
        last.placement.route_slot_ids.truncate(replicas - 30);
        let (reference, old_cost) = verification_cost(|| reference_prepare(&engine, &candidate));
        let (optimized, new_cost) =
            verification_cost(|| VerifiedPayloadSet::prepare(&engine, &candidate));
        let reference = reference.unwrap();
        let optimized = optimized.unwrap();
        assert_eq!(reference.full_revalidation, optimized.full_revalidation);
        assert_eq!(reference.entries.len(), optimized.entries.len());
        let replicas = replicas as u64;
        assert_eq!(new_cost, (replicas, replicas * 8));
        let passes = if optimized.full_revalidation { 2 } else { 1 };
        assert_eq!(old_cost, (replicas * passes, replicas * 8 * passes));
        optimized.validate(&engine.config, &candidate).unwrap();
    }
}
#[test]
fn large_preparation_rejects_corruption_like_reference() {
    let (_temp, engine, candidate) = fixture_with_files(DEFAULT_MAX_HELD_PAYLOAD_FILES / 12 + 1);
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
}
#[test]
#[ignore = "manual preparation-only benchmark; not a physical-storage benchmark"]
fn benchmark_large_preparation_hash_passes() {
    let payload = vec![0x5a; 1024 * 1024];
    let (_temp, engine, candidate) =
        fixture_with_payload(DEFAULT_MAX_HELD_PAYLOAD_FILES / 12 + 1, &payload);
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
    let (_temp, engine, candidate) = fixture_with_files(DEFAULT_MAX_HELD_PAYLOAD_FILES / 12 + 1);
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
}
#[test]
fn large_preparation_rejects_disk_change_before_hashing() {
    let (_temp, engine, candidate) = fixture_with_files(DEFAULT_MAX_HELD_PAYLOAD_FILES / 12 + 1);
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
        let result = engine
            .published_receipt(candidate.operation_id)
            .unwrap()
            .unwrap();
        let outcome = engine.prepare_publication(next, prepared, Vec::new(), result, MAX_METADATA);
        match damage {
            "none" => assert!(outcome.is_ok()),
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
    }
}

#[test]
fn existing_install_token_does_not_replace_decision_byte_verification() {
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
}

#[test]
fn existing_admission_above_descriptor_budget_uses_uncached_preparation() {
    let (_temp, engine, candidate) = fixture_with_files(DEFAULT_MAX_HELD_PAYLOAD_FILES / 3 + 1);
    let prepared = existing_prepared(&engine, &candidate);
    assert!(
        prepared
            .iter()
            .map(|p| p.placement.route_slot_ids.len())
            .sum::<usize>()
            > DEFAULT_MAX_HELD_PAYLOAD_FILES
    );
    let mut next = engine.state().clone();
    next.sequence = engine.next_slot;
    let result = engine
        .published_receipt(candidate.operation_id)
        .unwrap()
        .unwrap();
    let detached = engine
        .prepare_publication(next, prepared, Vec::new(), result, MAX_METADATA)
        .unwrap();
    assert!(detached.verified.full_revalidation);
    assert!(detached.verified.entries.is_empty());
}

#[test]
fn name_projection_uses_the_owner_but_payload_admission_reports_a_corrupt_seal() {
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
        &engine
            .with_namespace_source(|source| {
                crate::state_projection::ProjectionTransition::new(
                    engine.state(),
                    &next,
                    None,
                    source,
                )
            })
            .unwrap(),
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
    let (_temp, mut engine, candidate) = fixture();
    let mut next = engine.state().clone();
    next.sequence = engine.next_slot;
    let result = engine
        .published_receipt(candidate.operation_id)
        .unwrap()
        .unwrap();
    let detached = engine
        .prepare_publication(next, Vec::new(), Vec::new(), result, MAX_METADATA)
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
fn held_proof_lookup_preserves_arbitrary_candidate_and_slot_order() {
    let (_temp, engine, mut candidate) = fixture_with_files(3);
    candidate.versions.reverse();
    for evidence in &mut candidate.versions {
        evidence.placement.route_slot_ids.reverse();
    }
    let proof = VerifiedPayloadSet::prepare(&engine, &candidate).unwrap();
    assert!(!proof.full_revalidation);
    assert!(
        proof
            .entries
            .windows(2)
            .all(|pair| pair[0].identity() < pair[1].identity())
    );
    proof.validate(&engine.config, &candidate).unwrap();
    candidate.versions.reverse();
    for evidence in &mut candidate.versions {
        evidence.placement.route_slot_ids.reverse();
        for slot in &evidence.placement.route_slot_ids {
            proof
                .validate_payload(&engine.config, &evidence.version, slot)
                .unwrap();
        }
    }
    proof.validate(&engine.config, &candidate).unwrap();
}

#[test]
fn held_proof_lookup_rejects_unknown_version_and_slot_pairs() {
    let (_temp, engine, candidate) = fixture_with_files(2);
    let proof = VerifiedPayloadSet::prepare(&engine, &candidate).unwrap();
    let evidence = &candidate.versions[0];
    let slot = &evidence.placement.route_slot_ids[0];
    let mut unknown = evidence.version.clone();
    unknown.version_id = "version_missing".into();
    for (version, slot) in [
        (&unknown, slot.as_str()),
        (&evidence.version, "slot_missing"),
    ] {
        assert_eq!(
            proof
                .validate_payload(&engine.config, version, slot)
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::PreparationStale
        );
    }
    proof.validate(&engine.config, &candidate).unwrap();
}

#[test]
fn held_proof_lookup_keeps_the_first_matching_prepared_entry() {
    let (_temp, engine, mut candidate) = fixture();
    candidate.versions.push(candidate.versions[0].clone());
    let mut proof = VerifiedPayloadSet::prepare(&engine, &candidate).unwrap();
    let evidence = &candidate.versions[0];
    let slot = &evidence.placement.route_slot_ids[0];
    let key = (evidence.version.version_id.as_str(), slot.as_str());
    proof
        .entries
        .iter_mut()
        .filter(|entry| entry.identity() == key)
        .nth(1)
        .unwrap()
        .body
        .stamp
        .size += 1;
    proof
        .validate_payload(&engine.config, &evidence.version, slot)
        .unwrap();
    proof.validate(&engine.config, &candidate).unwrap();
}

#[test]
fn configured_budget_covers_existing_copy_and_final_proof_thresholds() {
    let (_temp, mut engine, candidate) = fixture_with_files(11);
    assert_eq!(engine.max_held_payload_files().get(), 128);
    // 33 existing-body proofs, then 132 final-proof FDs. The two phases
    // consume the same runtime cap at their different per-replica costs.
    for limit in [1, 3, 32, 33, 128, 131, 132, 256] {
        engine.set_max_held_payload_files(std::num::NonZeroUsize::new(limit).unwrap());
        let prepared = existing_prepared(&engine, &candidate);
        let mut next = engine.state().clone();
        next.sequence = engine.next_slot;
        let result = engine
            .published_receipt(candidate.operation_id)
            .unwrap()
            .unwrap();
        let detached = engine
            .prepare_publication(next, prepared, Vec::new(), result, MAX_METADATA)
            .unwrap();
        let proof = &detached.verified;
        assert_eq!(proof.full_revalidation, limit < 132, "limit {limit}");
        assert_eq!(proof.entries.len(), if limit < 132 { 0 } else { 33 });
        assert_eq!(proof.disks.len(), 3);
        proof.validate(&engine.config, &candidate).unwrap();
    }
}
