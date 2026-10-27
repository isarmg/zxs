use super::*;
fn fixture(point: &str) -> (tempfile::TempDir, Engine, OperationId, Candidate) {
    fixture_with_history(point, 0)
}
fn fixture_with_history(
    point: &str,
    retained: usize,
) -> (tempfile::TempDir, Engine, OperationId, Candidate) {
    fixture_with_members(point, retained, [true; 3])
}
fn fixture_with_members(
    point: &str,
    retained: usize,
    metadata_members: [bool; 3],
) -> (tempfile::TempDir, Engine, OperationId, Candidate) {
    let temp = tempfile::tempdir().unwrap();
    let mut config = Config::development([
        temp.path().join("p"),
        temp.path().join("b"),
        temp.path().join("c"),
    ]);
    for (disk, member) in config.disks.iter_mut().zip(metadata_members) {
        disk.metadata_member = member;
    }
    let mut engine = Engine::initialize(&temp.path().join("state"), config).unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("恢复测试").unwrap(),
            "P01",
            3,
            "alice",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    if retained > 0 {
        let mut next = engine.state().clone();
        next.sequence = engine.next_slot;
        let original = next.commits[&next.histories[&repo.history_tree_id].root_commit_id].clone();
        let mut parent = original.commit_id.clone();
        for _ in 0..retained {
            let mut commit = original.clone();
            commit.commit_id = new_id("commit");
            commit.parent_commit_ids = vec![parent];
            commit.sequence = next.sequence;
            parent = commit.commit_id.clone();
            next.commits.insert(commit.commit_id.clone(), commit);
        }
        let mut result = next.operations.values().next().unwrap().clone();
        result.operation_id = storage_contract::OperationId::generate();
        result.sequence = next.sequence;
        next.operations.insert(result.operation_id, result.clone());
        engine.persist(next, Vec::new(), result).unwrap();
    }
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "alice")
        .unwrap();
    engine
        .stage_bytes(
            &workspace.workspace_id,
            "lost.txt",
            b"permanently lost bytes",
        )
        .unwrap();
    engine
        .stage_bytes(
            &workspace.workspace_id,
            "healthy.txt",
            b"retained full bytes",
        )
        .unwrap();
    let operation = storage_contract::OperationId::generate();
    engine.inject_fault_once(point);
    assert!(
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: operation,
                principal: "alice".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "pending".into(),
                request_digest: None
            })
            .is_err()
    );
    let member = engine.members()[0].clone();
    let records = scan(&member).unwrap();
    let record = records.records.last().unwrap();
    let candidate = load_object(&member, "candidate", &record.0.candidate_hash).unwrap();
    (temp, engine, operation, candidate)
}

#[test]
fn recovery_preview_and_commit_read_required_seals_from_the_held_generation() {
    for metadata_members in [[true; 3], [true, false, true]] {
        for preview_first in [false, true] {
            for corrupt_bytes in [false, true] {
                let (temp, engine, operation, candidate) =
                    fixture_with_members("after_accept_2", 0, metadata_members);
                lose(&engine, &candidate, true);
                drop(engine);
                let mut engine = Engine::open_for_recovery(&temp.path().join("state")).unwrap();
                let plan = preview_first.then(|| {
                    engine
                        .prepare_degraded_recovery(operation, "alice", "all source bytes lost")
                        .unwrap()
                });
                let disk = engine
                    .config
                    .disks
                    .iter()
                    .find(|disk| !disk.metadata_member)
                    .unwrap_or(&engine.config.disks[0])
                    .clone();
                let journals = engine
                    .members()
                    .iter()
                    .map(|member| (log_path(member), fs::read(log_path(member)).unwrap()))
                    .collect::<Vec<_>>();
                let path = disk.root.join("disk.json");
                let replacement = path.with_extension("replacement");
                if corrupt_bytes {
                    fs::write(&replacement, b"broken seal").unwrap();
                } else {
                    let mut seal: DiskSeal = read_json(&path).unwrap();
                    seal.binding_generation += 1;
                    write_json(&replacement, &seal, false).unwrap();
                }
                fs::rename(&replacement, &path).unwrap();
                let result = match plan {
                    Some(plan) => engine
                        .commit_degraded_recovery(&plan.recovery_id, &plan.digest, "alice", true)
                        .map(|_| ()),
                    None => engine
                        .prepare_degraded_recovery(operation, "alice", "all source bytes lost")
                        .map(|_| ()),
                };
                let error = result.unwrap_err();
                if !corrupt_bytes {
                    assert_eq!(error.code(), ErrorCode::DiskIdentityMismatch);
                }
                assert!(!engine.state.operations.contains_key(&operation));
                for (path, bytes) in journals {
                    assert_eq!(fs::read(path).unwrap(), bytes, "no recovery decision write");
                }
            }
        }
    }
}

#[test]
fn recovery_commit_after_root_replacement_uses_original_seals_and_journals() {
    use std::os::unix::fs::PermissionsExt;
    let (temp, engine, operation, candidate) = fixture("after_accept_2");
    lose(&engine, &candidate, true);
    drop(engine);
    let mut engine = Engine::open_for_recovery(&temp.path().join("state")).unwrap();
    let plan = engine
        .prepare_degraded_recovery(operation, "alice", "all source bytes lost")
        .unwrap();
    let mut held_disks = Vec::new();
    for disk in &engine.config.disks {
        let mut held = disk.clone();
        held.root = disk.root.with_extension("held");
        fs::rename(&disk.root, &held.root).unwrap();
        fs::create_dir(&disk.root).unwrap();
        fs::set_permissions(&disk.root, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(disk.root.join("replacement-only"), b"not admitted").unwrap();
        held_disks.push(held);
    }
    let result = engine
        .commit_degraded_recovery(&plan.recovery_id, &plan.digest, "alice", true)
        .unwrap();
    assert_eq!(
        result.commit_state,
        storage_contract::CommitState::PublishedDegraded
    );
    for disk in held_disks {
        assert!(scan(&disk).unwrap().records.iter().any(|(record, _)| {
            record.sequence == candidate.sequence && record.record_type == RecordType::Publication
        }));
    }
    for disk in &engine.config.disks {
        assert_eq!(fs::read_dir(&disk.root).unwrap().count(), 1);
        assert_eq!(
            fs::read(disk.root.join("replacement-only")).unwrap(),
            b"not admitted"
        );
    }
}
fn lose(engine: &Engine, candidate: &Candidate, all_copies: bool) {
    let lost = candidate
        .versions
        .iter()
        .find(|v| v.version.creation_name == "lost.txt")
        .unwrap();
    for slot in lost
        .placement
        .route_slot_ids
        .iter()
        .take(if all_copies { usize::MAX } else { 1 })
    {
        let disk = engine.disk(slot).unwrap();
        fs::remove_file(
            crate::container_storage::content_path(
                &version_dir(
                    disk,
                    &lost.version.history_tree_id,
                    &lost.version.version_id,
                ),
                &lost.version,
            )
            .unwrap(),
        )
        .unwrap();
    }
    if all_copies {
        for workspace in engine.workspaces.values() {
            for source in workspace
                .files
                .values()
                .filter(|f| f.content_hash == lost.version.content_hash)
            {
                fs::remove_file(&source.source_path).unwrap();
            }
        }
    }
}
#[test]
fn explicit_two_accept_recovery_preserves_candidate_and_reads_remaining_files_after_restart() {
    let (temp, engine, operation, candidate) = fixture("after_accept_2");
    lose(&engine, &candidate, true);
    drop(engine);
    let mut engine = Engine::open_for_recovery(&temp.path().join("state")).unwrap();
    assert!(engine.is_read_only());
    let plan = engine
        .prepare_degraded_recovery(
            operation,
            "alice",
            "all registered copies and staged source were permanently lost",
        )
        .unwrap();
    assert_eq!(plan.original_accepts.len(), 2);
    assert_eq!(
        engine
            .commit_degraded_recovery(&plan.recovery_id, &plan.digest, "alice", false)
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::PermanentLossNotAcknowledged
    );
    let result = engine
        .commit_degraded_recovery(&plan.recovery_id, &plan.digest, "alice", true)
        .unwrap();
    assert_eq!(
        result.commit_state,
        storage_contract::CommitState::PublishedDegraded
    );
    assert_eq!(
        result.protection_at_publish,
        storage_contract::ProtectionAtPublish::BelowTarget
    );
    assert_eq!(result.current_health.as_str(), "UNAVAILABLE");
    let disk = engine.members()[0].clone();
    let scanned = scan(&disk).unwrap();
    assert_eq!(
        scanned
            .records
            .iter()
            .filter(|(r, _)| r.sequence == candidate.sequence)
            .map(|(r, _)| r.record_type)
            .collect::<Vec<_>>(),
        [
            RecordType::Prepare,
            RecordType::Accept,
            RecordType::Reprotect,
            RecordType::Finalize,
            RecordType::Publication
        ]
    );
    let (record, _) = scanned.records.last().unwrap();
    let publication: Publication = load_object(&disk, "publication", &record.object_hash).unwrap();
    let original = load_snapshot(&disk, &candidate.snapshot_hash).unwrap();
    let bound = crate::metadata_budget::preflight_publication(
        &engine,
        &candidate,
        &publication.result,
        &original,
        publication.recovery.clone(),
        MAX_METADATA,
    )
    .unwrap();
    let actual = candidate
        .versions
        .iter()
        .filter(|e| e.version.creation_name == "healthy.txt")
        .flat_map(|e| e.placement.route_slot_ids.iter().map(move |slot| (e, slot)))
        .map(|(e, slot)| {
            fs::read(
                version_dir(
                    engine.disk(slot).unwrap(),
                    &e.version.history_tree_id,
                    &e.version.version_id,
                )
                .join("publication-proof.json"),
            )
            .unwrap()
            .len()
        })
        .max()
        .unwrap();
    assert_eq!(bound, actual);
    assert_eq!(
        crate::metadata_budget::preflight_publication(
            &engine,
            &candidate,
            &publication.result,
            &original,
            publication.recovery.clone(),
            bound - 1
        )
        .unwrap_err()
        .code(),
        storage_contract::ErrorCode::ResourceLimit
    );

    assert_eq!(
        object_hash("candidate", &candidate).unwrap(),
        plan.candidate_hash
    );
    assert_eq!(
        engine
            .commit_degraded_recovery(&plan.recovery_id, &plan.digest, "alice", true)
            .unwrap()
            .sequence,
        result.sequence
    );
    let healthy = candidate
        .versions
        .iter()
        .find(|v| v.version.creation_name == "healthy.txt")
        .unwrap();
    assert!(engine.open_content(&healthy.version.version_id).is_ok());
    let lost = candidate
        .versions
        .iter()
        .find(|v| v.version.creation_name == "lost.txt")
        .unwrap();
    assert!(engine.open_content(&lost.version.version_id).is_err());
    drop(engine);
    let engine = Engine::open(&temp.path().join("state")).unwrap();
    let replay = engine.operation(operation).unwrap();
    assert_eq!(
        replay.commit_state,
        storage_contract::CommitState::PublishedDegraded
    );
    assert_eq!(
        replay.protection_at_publish,
        storage_contract::ProtectionAtPublish::BelowTarget
    );
    assert!(engine.open_content(&healthy.version.version_id).is_ok());
    for disk in engine.members() {
        let same: Candidate = load_object(&disk, "candidate", &plan.candidate_hash).unwrap();
        assert_eq!(
            object_hash("candidate", &same).unwrap(),
            plan.candidate_hash
        );
    }
    // The next clean restart may skip historical snapshot decoding, but it
    // must preserve the degraded-publication result overlay exactly.
    drop(engine);
    crate::journal::SNAPSHOT_LOAD_COUNT.with(|count| count.set(0));
    let engine = Engine::open(&temp.path().join("state")).unwrap();
    assert_eq!(
        crate::journal::SNAPSHOT_LOAD_COUNT.with(|count| count.get()),
        1
    );
    let replay = engine.operation(operation).unwrap();
    assert_eq!(
        replay.commit_state,
        storage_contract::CommitState::PublishedDegraded
    );
    assert_eq!(
        replay.protection_at_publish,
        storage_contract::ProtectionAtPublish::BelowTarget
    );
    assert!(engine.open_content(&healthy.version.version_id).is_ok());
    assert!(engine.open_content(&lost.version.version_id).is_err());
}
#[test]
fn degraded_recovery_uses_verified_snapshot_replica_and_repairs_missing_page() {
    for corrupt in [false, true] {
        let (temp, engine, operation, candidate) = fixture("after_accept_2");
        lose(&engine, &candidate, true);
        let members = engine.members();
        let snapshot: Snapshot =
            load_object(&members[0], "snapshot", &candidate.snapshot_hash).unwrap();
        let page = crate::journal::map_pages(&members[0], snapshot.maps.values().next().unwrap())
            [0]
        .clone();
        let path = object_path(&members[0], "state-page", &page);
        let original = fs::read(&path).unwrap();
        if corrupt {
            fs::write(&path, b"corrupt").unwrap();
        } else {
            fs::remove_file(&path).unwrap();
        }
        drop(engine);
        let root = temp.path().join("state");
        let mut engine = Engine::open_for_recovery(&root).unwrap();
        let plan = engine
            .prepare_degraded_recovery(operation, "alice", "permanent loss")
            .unwrap();
        let result = engine
            .commit_degraded_recovery(&plan.recovery_id, &plan.digest, "alice", true)
            .unwrap();
        assert_eq!(
            result.commit_state,
            storage_contract::CommitState::PublishedDegraded
        );
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(engine.snapshot.as_ref(), Some(&candidate.snapshot_hash));
        drop(engine);
        let engine = Engine::open(&root).unwrap();
        assert_eq!(
            engine.operation(operation).unwrap().commit_state,
            storage_contract::CommitState::PublishedDegraded
        );
    }
}

#[test]
fn degraded_recovery_verifies_and_repairs_exact_history_graph_before_reprotect() {
    for kind in ["state-page", "metadata-index", "history-manifest"] {
        for missing_all in [false, true] {
            let (_temp, mut engine, operation, candidate) =
                fixture_with_history("after_accept_2", 160);
            lose(&engine, &candidate, true);
            let members = engine.members();
            let objects =
                crate::history_package::bound_history_objects(&members, &candidate, None).unwrap();
            let object = objects.iter().find(|object| object.kind() == kind).unwrap();
            let hash = object.hash();
            let expected = object.bytes();
            let plan = engine
                .prepare_degraded_recovery(operation, "alice", "permanent loss")
                .unwrap();
            let logs = members
                .iter()
                .map(|member| fs::read(log_path(member)).unwrap())
                .collect::<Vec<_>>();
            for member in members.iter().take(if missing_all { 3 } else { 1 }) {
                fs::write(object_path(member, kind, hash), b"damaged").unwrap();
            }
            let result =
                engine.commit_degraded_recovery(&plan.recovery_id, &plan.digest, "alice", true);
            if missing_all {
                assert!(result.is_err());
                for (member, log) in members.iter().zip(logs) {
                    assert_eq!(fs::read(log_path(member)).unwrap(), log);
                }
            } else {
                assert_eq!(
                    result.unwrap().commit_state,
                    storage_contract::CommitState::PublishedDegraded
                );
                assert_eq!(
                    fs::read(object_path(&members[0], kind, hash)).unwrap(),
                    expected
                );
            }
        }
    }
}

#[test]
fn degraded_recovery_rejects_unverifiable_snapshot_before_reprotect() {
    let (_temp, mut engine, operation, candidate) = fixture("after_accept_2");
    lose(&engine, &candidate, true);
    let plan = engine
        .prepare_degraded_recovery(operation, "alice", "permanent loss")
        .unwrap();
    let members = engine.members();
    let snapshot: Snapshot =
        load_object(&members[0], "snapshot", &candidate.snapshot_hash).unwrap();
    let page =
        crate::journal::map_pages(&members[0], snapshot.maps.values().next().unwrap())[0].clone();
    let logs = members
        .iter()
        .map(|d| fs::read(log_path(d)).unwrap())
        .collect::<Vec<_>>();
    for member in &members {
        fs::remove_file(object_path(member, "state-page", &page)).unwrap();
    }
    assert!(
        engine
            .commit_degraded_recovery(&plan.recovery_id, &plan.digest, "alice", true)
            .is_err()
    );
    assert!(engine.is_read_only());
    assert!(!engine.state.operations.contains_key(&operation));
    for (member, log) in members.iter().zip(logs) {
        assert_eq!(fs::read(log_path(member)).unwrap(), log);
    }
}

#[test]
fn degraded_publication_stays_read_only_when_local_reconciliation_fails() {
    use std::error::Error as _;
    for fail_index in [true, false] {
        let (temp, engine, operation, candidate) = fixture("after_accept_2");
        lose(&engine, &candidate, true);
        drop(engine);
        let root = temp.path().join("state");
        let mut engine = Engine::open_for_recovery(&root).unwrap();
        let plan = engine
            .prepare_degraded_recovery(operation, "recovery-admin", "permanent loss")
            .unwrap();
        let obstruction = if fail_index {
            let path = root.join("index.sqlite");
            fs::remove_file(&path).unwrap();
            fs::create_dir(&path).unwrap();
            path
        } else {
            let path = root.join("workspaces").join("broken-workspace");
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("workspace.json"), b"{}").unwrap();
            path
        };
        let error = engine
            .commit_degraded_recovery(&plan.recovery_id, &plan.digest, "recovery-admin", true)
            .unwrap_err();
        assert_eq!(error.code(), storage_contract::ErrorCode::RecoveryRequired);
        assert!(!error.is_retryable());
        let Error::PublishedRecoveryReconciliation(context) = &error else {
            panic!("published recovery context was lost: {error}")
        };
        assert_eq!(context.recovery_id(), plan.recovery_id);
        assert_eq!(context.plan_digest(), plan.digest);
        assert_eq!(context.principal(), plan.principal);
        assert_eq!(
            engine.operation_owner(operation).unwrap().as_deref(),
            Some("alice")
        );
        assert_eq!(context.isolation_generation(), plan.isolation_generation);
        assert_eq!(
            context.stage(),
            if fail_index {
                RecoveryReconciliationStage::IndexRebuild
            } else {
                RecoveryReconciliationStage::WorkspaceReconciliation
            }
        );
        assert_eq!(error.published_result(), Some(context.result()));
        assert_eq!(context.result(), &engine.state().operations[&operation]);
        assert_eq!(
            context.result().protection_at_publish,
            ProtectionAtPublish::BelowTarget
        );
        assert_eq!(context.result().request_digest, candidate.request_digest);
        let source = context.source().unwrap().downcast_ref::<Error>().unwrap();
        assert_eq!(
            source.code(),
            if fail_index {
                ErrorCode::UnsafePath
            } else {
                ErrorCode::RecoveryRequired
            }
        );
        if !fail_index {
            assert_eq!(
                source
                    .source()
                    .unwrap()
                    .downcast_ref::<Error>()
                    .unwrap()
                    .code(),
                ErrorCode::InvalidJson
            );
        }
        // zxs-rescue's outer error formatter uses this native Display, not the
        // generic offline HTTP diagnostic that previously hid publication.
        assert!(error.to_string().contains("is already published"));
        assert!(error.to_string().contains(&operation.to_string()));
        assert!(error.to_string().contains(&source.to_string()));
        assert!(
            engine.is_read_only(),
            "failed reconciliation must retain the write fence"
        );
        assert_eq!(
            engine.operation(operation).unwrap().commit_state,
            storage_contract::CommitState::PublishedDegraded
        );
        assert_eq!(
            engine
                .create_repository(
                    &storage_contract::RepositoryName::parse("must-not-write").unwrap(),
                    "P01",
                    3,
                    "alice",
                    storage_contract::OperationId::generate()
                )
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::RecoveryRequired
        );
        // Idempotent result lookup must not reopen writes either.
        engine
            .commit_degraded_recovery(&plan.recovery_id, &plan.digest, "recovery-admin", true)
            .unwrap();
        assert!(engine.is_read_only());
        let denied = engine
            .commit_degraded_recovery(&plan.recovery_id, &plan.digest, "alice", true)
            .unwrap_err();
        assert_eq!(denied.code(), ErrorCode::PermissionDenied);
        assert!(denied.published_result().is_none());
        let expected = engine.state().clone();
        drop(engine);
        assert!(
            Engine::open(&root).is_err(),
            "published evidence does not relax strict startup"
        );
        assert!(
            Engine::open_for_recovery(&root).is_err(),
            "offline reopen still requires local reconciliation"
        );
        fs::remove_dir_all(&obstruction).unwrap();
        let engine = Engine::open(&root).unwrap();
        assert!(!engine.is_read_only());
        assert_eq!(*engine.state, expected);
        assert_eq!(
            engine.operation(operation).unwrap().commit_state,
            storage_contract::CommitState::PublishedDegraded
        );
    }
}

#[test]
fn insufficient_accepts_or_repairable_bytes_never_authorize_degraded_publication() {
    let (temp, engine, operation, candidate) = fixture("after_accept_1");
    lose(&engine, &candidate, true);
    drop(engine);
    let mut engine = Engine::open_for_recovery(&temp.path().join("state")).unwrap();
    assert_eq!(
        engine
            .prepare_degraded_recovery(operation, "alice", "loss")
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::RecoveryRequired
    );
    let (temp, engine, operation, candidate) = fixture("after_accept_2");
    lose(&engine, &candidate, false);
    drop(engine);
    let mut engine = Engine::open_for_recovery(&temp.path().join("state")).unwrap();
    assert_eq!(
        engine
            .prepare_degraded_recovery(operation, "alice", "one copy lost")
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::RecoveryRequired
    );
}
#[test]
fn original_normal_publication_is_preserved_when_content_was_lost_later() {
    let (temp, engine, operation, candidate) = fixture("after_publication_1");
    lose(&engine, &candidate, true);
    drop(engine);
    let mut engine = Engine::open_for_recovery(&temp.path().join("state")).unwrap();
    let result = engine.operation(operation).unwrap();
    assert_eq!(
        result.commit_state,
        storage_contract::CommitState::Published
    );
    assert_eq!(
        result.protection_at_publish,
        storage_contract::ProtectionAtPublish::TargetMet
    );
    assert_eq!(result.current_health.as_str(), "UNAVAILABLE");
    assert_eq!(
        engine
            .prepare_degraded_recovery(operation, "alice", "later loss")
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::RecoveryRequired
    );
}
#[test]
fn degraded_recovery_resumes_the_same_plan_after_each_durable_boundary() {
    for point in [
        "after_recovery_reprotect_1",
        "after_recovery_finalize",
        "after_recovery_publication_1",
    ] {
        let (temp, engine, operation, candidate) = fixture("after_accept_2");
        lose(&engine, &candidate, true);
        drop(engine);
        let root = temp.path().join("state");
        let mut engine = Engine::open_for_recovery(&root).unwrap();
        let plan = engine
            .prepare_degraded_recovery(operation, "alice", "permanent loss")
            .unwrap();
        engine.inject_fault_once(point);
        assert!(
            engine
                .commit_degraded_recovery(&plan.recovery_id, &plan.digest, "alice", true)
                .is_err(),
            "{point}"
        );
        drop(engine);
        if point == "after_recovery_reprotect_1" {
            fs::remove_file(plan_path(&root, &plan.recovery_id)).unwrap();
        }
        let mut engine = Engine::open_for_recovery(&root).unwrap();
        let result = engine
            .commit_degraded_recovery(&plan.recovery_id, &plan.digest, "alice", true)
            .unwrap();
        assert_eq!(result.result["recovery_id"], plan.recovery_id, "{point}");
        assert_eq!(
            result.commit_state,
            storage_contract::CommitState::PublishedDegraded,
            "{point}"
        );
    }
}
#[test]
fn incomplete_log_cannot_prove_absence_and_unknown_record_types_are_rejected() {
    use std::io::Write;
    let (temp, engine, operation, candidate) = fixture("after_accept_2");
    lose(&engine, &candidate, true);
    let disk = engine.members()[0].clone();
    drop(engine);
    let log = log_path(&disk);
    let mut file = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
    file.write_all(b"ZXSL").unwrap();
    file.sync_all().unwrap();
    drop(file);
    let length = fs::metadata(&log).unwrap().len();
    let mut engine = Engine::open_for_recovery(&temp.path().join("state")).unwrap();
    assert_eq!(
        engine
            .prepare_degraded_recovery(operation, "alice", "loss")
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::RecoveryRequired
    );
    assert_eq!(fs::metadata(log).unwrap().len(), length);
    drop(engine);
    let (temp, engine, _operation, _candidate) = fixture("after_accept_2");
    let disk = engine.members()[0].clone();
    let scanned = scan(&disk).unwrap();
    let (previous, previous_hash) = scanned.records.last().unwrap();
    let mut raw = json_codec::to_value(previous).unwrap();
    raw["record_type"] = json_codec::json!("UNRECOGNIZED");
    raw["previous_frame_hash"] = json_codec::json!(previous_hash);
    let payload = canonical_json(&raw).unwrap();
    let mut frame = b"ZXSLOG\0\0".to_vec();
    frame.extend((payload.len() as u32).to_le_bytes());
    frame.extend(payload);
    let digest = blake3::hash(&frame);
    frame.extend(digest.as_bytes());
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(log_path(&disk))
        .unwrap();
    file.write_all(&frame).unwrap();
    file.sync_all().unwrap();
    drop(file);
    let logs_before = engine
        .members()
        .iter()
        .map(|disk| (log_path(disk), fs::read(log_path(disk)).unwrap()))
        .collect::<Vec<_>>();
    drop(engine);
    let index_before = fs::read(temp.path().join("state/index.sqlite")).unwrap();
    assert_eq!(
        Engine::open_for_recovery(&temp.path().join("state"))
            .err()
            .unwrap()
            .code(),
        storage_contract::ErrorCode::InvalidJson
    );
    assert_eq!(
        fs::read(temp.path().join("state/index.sqlite")).unwrap(),
        index_before
    );
    for (path, bytes) in logs_before {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn prepare_only_failure_reserves_its_slot_and_is_not_an_indeterminate_operation() {
    let (temp, mut engine, operation, _candidate) = fixture("after_prepare_1");
    assert!(!engine.is_read_only());
    assert_eq!(
        engine.operation(operation).unwrap_err().code(),
        storage_contract::ErrorCode::OperationNotFound
    );
    assert!(engine.operation_owner(operation).unwrap().is_none());
    let branch = engine
        .state
        .repositories
        .values()
        .next()
        .unwrap()
        .main_branch_id
        .clone();
    let workspace = engine.create_workspace(&branch, "alice").unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "next.txt", b"next")
        .unwrap();
    let result = engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "alice".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "after preparation failure".into(),
            request_digest: None,
        })
        .unwrap();
    assert_eq!(result.sequence, 3);
    drop(engine);
    assert!(Engine::open(&temp.path().join("state")).is_ok());
}

#[test]
fn unreadable_local_publication_is_not_absent_evidence() {
    let (_temp, engine, _operation, candidate) = fixture("after_accept_2");
    let evidence = &candidate.versions[0];
    let directory = version_dir(
        engine.disk(&evidence.placement.route_slot_ids[0]).unwrap(),
        &evidence.version.history_tree_id,
        &evidence.version.version_id,
    );
    let path = directory.join("publication-proof.json");
    std::os::unix::fs::symlink("publication-proof.json", &path).unwrap();
    let hash = object_hash("candidate", &candidate).unwrap();
    let error = discover_publication(&engine, &candidate, &hash).unwrap_err();
    assert!(std::error::Error::source(&error).is_some());
}

#[test]
fn failed_pending_preflight_preserves_torn_evidence_after_published_history() {
    use std::io::Write;
    let (temp, engine, operation, candidate) = fixture("after_accept_2");
    lose(&engine, &candidate, true);
    let disks = engine.members();
    assert!(
        scan(&disks[0])
            .unwrap()
            .records
            .iter()
            .any(|(record, _)| record.record_type == RecordType::Publication
                && record.sequence < candidate.sequence)
    );
    drop(engine);
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(log_path(&disks[0]))
        .unwrap();
    file.write_all(b"ZXSL").unwrap();
    file.sync_all().unwrap();
    drop(file);
    let before: Vec<_> = disks
        .iter()
        .map(|disk| fs::read(log_path(disk)).unwrap())
        .collect();
    assert!(Engine::open(&temp.path().join("state")).is_err());
    for (disk, expected) in disks.iter().zip(before) {
        assert_eq!(fs::read(log_path(disk)).unwrap(), expected);
    }
    let mut evidence = Engine::open_for_recovery(&temp.path().join("state")).unwrap();
    assert_eq!(
        evidence
            .prepare_degraded_recovery(operation, "alice", "loss")
            .unwrap_err()
            .code(),
        ErrorCode::RecoveryRequired
    );
}

#[test]
fn recoverable_pending_torn_tail_completes_without_leaving_engine_blocked() {
    use std::io::Write;
    let (temp, engine, operation, _candidate) = fixture("after_accept_2");
    let disk = engine.members()[0].clone();
    drop(engine);
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(log_path(&disk))
        .unwrap();
    file.write_all(b"ZXSL").unwrap();
    file.sync_all().unwrap();
    drop(file);
    let engine = Engine::open(&temp.path().join("state")).unwrap();
    assert!(!engine.is_read_only());
    assert_eq!(
        engine.operation(operation).unwrap().commit_state,
        storage_contract::CommitState::Published
    );
    assert!(!scan(&disk).unwrap().torn_tail);
}

#[test]
fn incomplete_quorum_only_recovery_preserves_pending_torn_evidence() {
    use std::io::Write;
    let (temp, engine, operation, candidate) = fixture("after_accept_2");
    let disks = engine.members();
    drop(engine);
    let path = log_path(&disks[0]);
    let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    file.write_all(b"ZXSL").unwrap();
    file.sync_all().unwrap();
    drop(file);
    let torn = fs::read(&path).unwrap();
    let missing = disks[2].root.with_extension("offline");
    fs::rename(&disks[2].root, &missing).unwrap();
    let mut incomplete = Engine::open(&temp.path().join("state")).unwrap();
    assert!(incomplete.is_read_only());
    assert!(
        discover_publication(
            &incomplete,
            &candidate,
            &object_hash("candidate", &candidate).unwrap(),
        )
        .unwrap()
        .is_none()
    );
    assert_eq!(
        incomplete
            .prepare_degraded_recovery(operation, "alice", "not an absent-publication authority")
            .unwrap_err()
            .code(),
        ErrorCode::RecoveryRequired,
        "best-effort discovery does not lower required recovery admission"
    );
    assert_eq!(fs::read(&path).unwrap(), torn);
    drop(incomplete);
    fs::rename(missing, &disks[2].root).unwrap();
    let recovered = Engine::open(&temp.path().join("state")).unwrap();
    assert!(!recovered.is_read_only());
    assert!(!scan(&disks[0]).unwrap().torn_tail);
}

#[test]
fn scrub_classifies_acknowledged_permanent_loss_as_payload_unavailable_not_metadata_damage() {
    let (temp, engine, operation, candidate) = fixture("after_accept_2");
    lose(&engine, &candidate, true);
    drop(engine);
    let mut engine = Engine::open_for_recovery(&temp.path().join("state")).unwrap();
    let plan = engine
        .prepare_degraded_recovery(
            operation,
            "alice",
            "all acknowledged source bytes permanently lost",
        )
        .unwrap();
    engine
        .commit_degraded_recovery(&plan.recovery_id, &plan.digest, "alice", true)
        .unwrap();
    let report = engine.scrub(&IntegrityScrubCancellation::new()).unwrap();
    assert_eq!(report.metadata_failures, 0, "{report:?}");
    assert_eq!(report.payload_copies_required, 6);
    assert_eq!(report.payload_copies_verified, 3);
    assert_eq!(report.payload_copies_unavailable, 3);
    assert_eq!(report.payload_copies_unverified, 0);
    assert!(!report.is_healthy());

    // The exception belongs only to the explicitly unavailable version/routes.
    let healthy = candidate
        .versions
        .iter()
        .find(|evidence| evidence.version.creation_name == "healthy.txt")
        .unwrap();
    let directory = version_dir(
        &engine.members()[0],
        &healthy.version.history_tree_id,
        &healthy.version.version_id,
    );
    fs::remove_file(directory.join("publication-proof.json")).unwrap();
    let report = engine.scrub(&IntegrityScrubCancellation::new()).unwrap();
    assert_eq!(report.metadata_failures, 1, "{report:?}");
    assert_eq!(report.payload_copies_verified, 2);
    assert_eq!(report.payload_copies_unavailable, 3);
    assert_eq!(report.payload_copies_unverified, 1);
}
