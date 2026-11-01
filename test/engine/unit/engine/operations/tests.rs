use super::*;
use crate::disk::version_dir;
use std::fs;

fn fixture() -> (tempfile::TempDir, Engine, Repository) {
    let root = tempfile::tempdir().unwrap();
    let config = Config::development([
        root.path().join("d1"),
        root.path().join("d2"),
        root.path().join("d3"),
    ]);
    let mut engine = Engine::initialize(&root.path().join("system"), config).unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Health").unwrap(),
            "P01",
            3,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    (root, engine, repo)
}

fn save(engine: &mut Engine, branch: &str, bytes: &[u8]) -> OperationResult {
    let workspace = engine.create_workspace(branch, "owner").unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "data.txt", bytes)
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "owner".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: String::new(),
            request_digest: None,
        })
        .unwrap()
}

fn assert_frozen(observed: &OperationResult, published: &OperationResult) {
    let mut normalized = observed.clone();
    normalized.current_health = published.current_health;
    assert_eq!(&normalized, published);
}

#[test]
fn current_observation_is_separate_from_the_local_result_projection() {
    let (_root, mut engine, repo) = fixture();
    let published = save(&mut engine, &repo.main_branch_id, b"healthy content");
    let record_with_frozen_health = || {
        let mut record = engine.operation_record(published.operation_id).unwrap();
        record.result.current_health = storage_contract::CurrentHealth::Unknown;
        record
    };
    let observed = engine
        .prepare_operation_health(record_with_frozen_health())
        .observe();
    assert_eq!(
        observed.health(),
        Some(storage_contract::CurrentHealth::Healthy)
    );
    assert_eq!(
        observed.into_record().result().current_health,
        storage_contract::CurrentHealth::Unknown
    );
    let local = engine
        .prepare_operation_health(record_with_frozen_health())
        .check();
    assert_eq!(
        local.result().current_health,
        storage_contract::CurrentHealth::Unknown
    );
}

#[test]
fn operation_read_plan_blocks_purge_until_observation_finishes() {
    let (_root, mut engine, repo) = fixture();
    let original = save(&mut engine, &repo.main_branch_id, b"old payload");
    let version = engine.list_tree(&repo.main_branch_id).unwrap()[0]
        .version_id
        .clone();
    save(&mut engine, &repo.main_branch_id, b"new payload");
    let purge = engine
        .create_purge_plan(&repo.history_tree_id, vec![version.clone()], "owner")
        .unwrap();
    let plan =
        engine.prepare_operation_health(engine.operation_record(original.operation_id).unwrap());
    assert_eq!(
        engine.reads.lock().unwrap().counts().unwrap().get(&version),
        Some(&1)
    );
    assert_eq!(
        engine
            .commit_plan(
                &purge.plan_id,
                "owner",
                storage_contract::OperationId::generate()
            )
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::ContentPinned
    );
    let observed = plan.check().into_result();
    assert_eq!(observed.current_health.as_str(), "HEALTHY");
    assert_frozen(&observed, &original);
    assert!(
        !engine
            .reads
            .lock()
            .unwrap()
            .counts()
            .unwrap()
            .contains_key(&version)
    );
    engine
        .commit_plan(
            &purge.plan_id,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let observed = engine.operation(original.operation_id).unwrap();
    assert_eq!(observed.current_health.as_str(), "UNAVAILABLE");
    assert_frozen(&observed, &original);
}

#[test]
fn operation_capture_is_metadata_only_deduplicated_and_cancel_safe() {
    let (_root, mut engine, repo) = fixture();
    let operation = save(&mut engine, &repo.main_branch_id, b"content");
    let commit = std::sync::Arc::make_mut(&mut engine.state)
        .commits
        .get_mut(operation.commit_id.as_ref().unwrap())
        .unwrap();
    let entry = commit.entries[0].clone();
    // One immutable version may be referenced more than once. It must not
    // cause repeated full hashing or one lease per directory appearance.
    commit.entries.push(entry.clone());
    for disk in &engine.config.disks {
        fs::remove_dir_all(disk.root.join("histories")).unwrap();
    }
    SNAPSHOT_LOAD_COUNT.with(|count| count.set(0));
    let plan =
        engine.prepare_operation_health(engine.operation_record(operation.operation_id).unwrap());
    assert_eq!(plan.versions.len(), 1);
    assert_eq!(
        engine
            .reads
            .lock()
            .unwrap()
            .counts()
            .unwrap()
            .get(&entry.version_id),
        Some(&1)
    );
    SNAPSHOT_LOAD_COUNT.with(|count| assert_eq!(count.get(), 0));
    drop(plan);
    assert!(engine.reads.lock().unwrap().counts().unwrap().is_empty());
    let observed = engine.operation(operation.operation_id).unwrap();
    assert_eq!(observed.current_health.as_str(), "UNAVAILABLE");
    assert_frozen(&observed, &operation);
}

#[test]
fn health_reverifies_each_replica_and_never_rewrites_publication_facts() {
    let (_root, mut engine, repo) = fixture();
    let operation = save(&mut engine, &repo.main_branch_id, b"content");
    let version_id = engine.list_tree(&repo.main_branch_id).unwrap()[0]
        .version_id
        .clone();
    let version = &engine.state.versions[&version_id];
    let directories: Vec<_> = engine
        .config
        .disks
        .iter()
        .map(|disk| version_dir(disk, &version.history_tree_id, &version_id))
        .collect();
    let plan =
        engine.prepare_operation_health(engine.operation_record(operation.operation_id).unwrap());
    // Corrupt the complete payload after capture, keeping its length. The
    // observation must actually hash bytes, not trust a prior good result.
    let path = crate::container_storage::content_path(&directories[0], version).unwrap();
    fs::remove_file(&path).unwrap();
    fs::write(&path, b"changed").unwrap();
    let observed = plan.check().into_result();
    assert_eq!(observed.current_health.as_str(), "DEGRADED");
    assert_frozen(&observed, &operation);
    // A checksum header without its bound hash and an unrelated publication
    // proof are not sufficient health evidence, even if a body is present.
    fs::write(directories[1].join("checksums.bin"), b"untrusted").unwrap();
    fs::write(directories[2].join("publication-proof.json"), b"{}").unwrap();
    let observed = engine.operation(operation.operation_id).unwrap();
    assert_eq!(observed.current_health.as_str(), "UNAVAILABLE");
    assert_frozen(&observed, &operation);
    assert_eq!(engine.state.operations[&operation.operation_id], operation);
    assert!(engine.reads.lock().unwrap().counts().unwrap().is_empty());
}

#[test]
fn pending_receipt_reads_only_exact_operation_pages_and_owner_first() {
    let (_root, mut engine, _repo) = fixture();
    let operation = storage_contract::OperationId::generate();
    engine.inject_fault_once("after_accept_1");
    assert!(
        engine
            .create_repository(
                &storage_contract::RepositoryName::parse("pending").unwrap(),
                "P01",
                3,
                "owner",
                operation
            )
            .is_err()
    );
    let (members, snapshot, _) = engine.pending_operation_snapshot(operation).unwrap();
    for field in ["repositories", "commits"] {
        for hash in crate::journal::map_pages(&members[0], &snapshot.maps[field]) {
            for disk in &members {
                fs::write(
                    object_path(disk, "state-page", &hash),
                    b"unrelated broken page",
                )
                .unwrap();
            }
        }
    }
    SNAPSHOT_LOAD_COUNT.with(|count| count.set(0));
    let record = engine.operation_record(operation).unwrap();
    assert_eq!(record.owner(), Some("owner"));
    assert_eq!(
        record.result().commit_state,
        storage_contract::CommitState::InDoubt
    );
    assert_eq!(
        record.result().protection_at_publish,
        storage_contract::ProtectionAtPublish::Unconfirmed
    );
    assert_eq!(record.result().current_health.as_str(), "RECOVERING");
    let plan = engine.prepare_operation_health(record);
    assert!(plan.versions.is_empty());
    assert!(engine.reads.lock().unwrap().counts().unwrap().is_empty());
    SNAPSHOT_LOAD_COUNT.with(|count| assert_eq!(count.get(), 0));
    // Ownership lookup does not load potentially large operation results.
    for hash in crate::journal::map_pages(&members[0], &snapshot.maps["operations"]) {
        for disk in &members {
            fs::write(object_path(disk, "state-page", &hash), b"broken results").unwrap();
        }
    }
    assert_eq!(
        engine.operation_owner(operation).unwrap().as_deref(),
        Some("owner")
    );
    assert!(engine.operation_record(operation).is_err());
}

#[test]
fn pending_receipt_does_not_turn_corrupt_journals_into_not_found() {
    let (_root, mut engine, _repo) = fixture();
    let operation = storage_contract::OperationId::generate();
    engine.inject_fault_once("after_accept_1");
    assert!(
        engine
            .create_repository(
                &storage_contract::RepositoryName::parse("pending").unwrap(),
                "P01",
                3,
                "owner",
                operation
            )
            .is_err()
    );
    for disk in engine.members() {
        fs::write(log_path(&disk), b"invalid journal frame").unwrap();
    }
    let error = engine.operation_record(operation).err().unwrap();
    assert_eq!(error.code(), storage_contract::ErrorCode::LogCorrupt);
    let owner_error = engine.operation_owner(operation).unwrap_err();
    assert_eq!(owner_error.code(), storage_contract::ErrorCode::LogCorrupt);
}

#[test]
fn batch_health_owns_configuration_after_engine_drop_and_rechecks_member_identity() {
    for replace_members in [false, true] {
        let (root, mut engine, repo) = fixture();
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "owner")
            .unwrap();
        for path in ["first", "second"] {
            engine
                .stage_bytes(&workspace.workspace_id, path, path.as_bytes())
                .unwrap();
        }
        let published = engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: storage_contract::OperationId::generate(),
                principal: "owner".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: String::new(),
                request_digest: None,
            })
            .unwrap();
        let plan = engine
            .prepare_operation_health(engine.operation_record(published.operation_id).unwrap());
        assert_eq!(plan.versions.len(), 2);
        let roots: Vec<_> = engine
            .config
            .disks
            .iter()
            .map(|disk| disk.root.clone())
            .collect();
        drop(engine);
        if replace_members {
            for (index, member) in roots.iter().enumerate() {
                let removed = root.path().join(format!("removed-{index}"));
                fs::rename(member, &removed).unwrap();
                fs::create_dir(member).unwrap();
                fs::copy(removed.join("disk.json"), member.join("disk.json")).unwrap();
            }
        }
        let observed = plan.check().into_result();
        assert_eq!(
            observed.current_health.as_str(),
            if replace_members {
                "UNAVAILABLE"
            } else {
                "HEALTHY"
            }
        );
        assert_frozen(&observed, &published);
    }
}

#[test]
fn pending_receipt_reports_checksum_valid_malformed_record_as_log_corruption() {
    use std::error::Error as _;
    let (_root, mut engine, repo) = fixture();
    let published = save(&mut engine, &repo.main_branch_id, b"published content");
    let operation = storage_contract::OperationId::generate();
    engine.inject_fault_once("after_accept_1");
    assert!(
        engine
            .create_repository(
                &storage_contract::RepositoryName::parse("pending malformed record").unwrap(),
                "P01",
                3,
                "owner",
                operation,
            )
            .is_err()
    );
    let expected = engine.operation_record(operation).unwrap().result().clone();
    let state = canonical_json(engine.state.as_ref()).unwrap();
    let originals: Vec<_> = engine
        .members()
        .iter()
        .map(|disk| {
            let path = log_path(disk);
            let bytes = fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect();
    // A checksum proves frame bytes, not the record schema they encode.
    for payload in [
        b"{truncated".as_slice(),
        br#"{"record_type":"UNKNOWN"}"#.as_slice(),
    ] {
        let mut frame = originals[0].1[..8].to_vec();
        frame.extend((payload.len() as u32).to_le_bytes());
        frame.extend(payload);
        let digest = blake3::hash(&frame);
        frame.extend(digest.as_bytes());
        for (path, _) in &originals {
            fs::write(path, &frame).unwrap();
        }
        for error in [
            engine.operation_record(operation).err().unwrap(),
            engine.operation_owner(operation).unwrap_err(),
        ] {
            assert_eq!(error.code(), ErrorCode::LogCorrupt);
            assert_eq!(error.message(), "log record JSON is invalid");
            assert!(!error.is_retryable());
            assert!(error.published_result().is_none());
            let mut source = error.source();
            let mut found = false;
            while let Some(cause) = source {
                found |= cause.is::<json_codec::Error>();
                source = cause.source();
            }
            assert!(found, "malformed stored record retains its JSON cause");
        }
        // Published metadata does not consult unrelated damaged pending evidence.
        assert_eq!(
            engine
                .operation_record(published.operation_id)
                .unwrap()
                .result(),
            &published
        );
        assert_eq!(
            engine
                .operation_owner(published.operation_id)
                .unwrap()
                .as_deref(),
            Some("owner")
        );
        assert_eq!(canonical_json(engine.state.as_ref()).unwrap(), state);
        assert!(engine.blocked);
        for (path, _) in &originals {
            assert_eq!(fs::read(path).unwrap(), frame);
        }
    }
    for (path, bytes) in originals {
        fs::write(path, bytes).unwrap();
    }
    assert_eq!(
        engine.operation_record(operation).unwrap().result(),
        &expected
    );
    assert_eq!(
        engine.operation_owner(operation).unwrap().as_deref(),
        Some("owner")
    );
}

fn pending_fixture(accepts: usize) -> (tempfile::TempDir, Engine, OperationId) {
    let (root, mut engine, _repo) = fixture();
    let operation = OperationId::generate();
    engine.inject_fault_once(&format!("after_accept_{accepts}"));
    assert!(
        engine
            .create_repository(
                &storage_contract::RepositoryName::parse("pending seal").unwrap(),
                "P01",
                3,
                "owner",
                operation,
            )
            .is_err()
    );
    assert!(engine.blocked);
    (root, engine, operation)
}

fn accepted_members(engine: &Engine) -> Vec<DiskConfig> {
    engine
        .members()
        .into_iter()
        .filter(|disk| {
            scan_at(&engine.disk_resources.get(disk).unwrap().root, disk)
                .unwrap()
                .records
                .iter()
                .any(|(record, _)| {
                    record.record_type == RecordType::Accept
                        && record.sequence > engine.state.sequence
                })
        })
        .collect()
}

#[test]
fn pending_receipt_preserves_rejected_seal_causes_when_evidence_cannot_answer() {
    use std::error::Error as _;
    for all_members in [false, true] {
        for corrupt in [false, true] {
            let (_root, engine, operation) = pending_fixture(1);
            let rejected = if all_members {
                engine.members()
            } else {
                let accepted = accepted_members(&engine);
                assert_eq!(accepted.len(), 1);
                accepted
            };
            for disk in &rejected {
                let path = disk.root.join("disk.json");
                if corrupt {
                    fs::write(path, b"{broken seal").unwrap();
                } else {
                    fs::remove_file(path).unwrap();
                }
            }
            for error in [
                engine.operation_record(operation).err().unwrap(),
                engine.operation_owner(operation).unwrap_err(),
            ] {
                assert_eq!(
                    error.code(),
                    if corrupt {
                        ErrorCode::DiskIdentityMismatch
                    } else {
                        ErrorCode::IoError
                    }
                );
                assert!(error.message().contains(&rejected[0].disk_id));
                assert!(error.message().contains(&rejected[0].slot_id));
                let mut source = error.source();
                let mut found = false;
                while let Some(cause) = source {
                    found |= if corrupt {
                        cause.is::<json_codec::Error>()
                    } else {
                        cause.downcast_ref::<std::io::Error>().is_some_and(|io| {
                            io.kind() == std::io::ErrorKind::NotFound
                                && io.raw_os_error() == Some(2)
                        })
                    };
                    source = cause.source();
                }
                assert!(found, "seal rejection must retain its concrete cause");
            }
        }
    }
}

#[test]
fn pending_receipt_uses_sufficient_healthy_evidence_despite_rejected_member() {
    let (_root, engine, operation) = pending_fixture(2);
    let expected = engine.operation_record(operation).unwrap().result().clone();
    let accepted = accepted_members(&engine);
    assert_eq!(accepted.len(), 2);
    fs::write(accepted[0].root.join("disk.json"), b"{broken seal").unwrap();
    assert_eq!(
        engine.operation_record(operation).unwrap().result(),
        &expected
    );
    assert_eq!(
        engine.operation_owner(operation).unwrap().as_deref(),
        Some("owner")
    );
    assert!(engine.blocked);
}

#[test]
fn pending_receipt_distinguishes_trusted_absence_from_unreadable_evidence() {
    let (_root, engine, _operation) = pending_fixture(1);
    let absent = OperationId::generate();
    assert_eq!(
        engine.operation_record(absent).err().unwrap().code(),
        ErrorCode::OperationNotFound
    );
    assert_eq!(engine.operation_owner(absent).unwrap(), None);
    fs::write(engine.members()[0].root.join("disk.json"), b"{broken seal").unwrap();
    assert_eq!(
        engine.operation_record(absent).err().unwrap().code(),
        ErrorCode::DiskIdentityMismatch
    );
    assert_eq!(
        engine.operation_owner(absent).unwrap_err().code(),
        ErrorCode::DiskIdentityMismatch
    );
}

#[test]
fn history_export_preserves_seal_cause_only_when_healthy_evidence_cannot_answer() {
    let (root, engine, repo) = fixture();
    let members = engine.members();
    fs::write(members[0].root.join("disk.json"), b"{broken seal").unwrap();
    engine
        .export_history_package(
            &repo.history_tree_id,
            &root.path().join("exports/healthy-export"),
        )
        .unwrap();
    for disk in &members[1..] {
        fs::write(disk.root.join("disk.json"), b"{broken seal").unwrap();
    }
    let destination = root.path().join("exports/unavailable-export");
    let error = engine
        .export_history_package(&repo.history_tree_id, &destination)
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::RecoveryRequired);
    assert_seal_json_cause(&error);
    assert!(!destination.exists());
}

#[test]
fn purge_evidence_keeps_recovery_classification_and_does_not_delete_without_proof() {
    let (_root, mut engine, repo) = fixture();
    save(&mut engine, &repo.main_branch_id, b"old payload");
    let version = engine.list_tree(&repo.main_branch_id).unwrap()[0]
        .version_id
        .clone();
    save(&mut engine, &repo.main_branch_id, b"current payload");
    let purge = engine
        .create_purge_plan(&repo.history_tree_id, vec![version.clone()], "owner")
        .unwrap();
    engine.inject_fault_once("after_purge_evidence");
    assert_eq!(
        engine
            .commit_plan(&purge.plan_id, "owner", OperationId::generate())
            .unwrap_err()
            .code(),
        ErrorCode::InjectedFailure
    );
    let body = &engine.state.versions[&version];
    let paths: Vec<_> = engine
        .members()
        .iter()
        .map(|disk| {
            let directory = version_dir(disk, &body.history_tree_id, &version);
            crate::container_storage::content_path(&directory, body).unwrap()
        })
        .collect();
    for disk in engine.members() {
        fs::write(disk.root.join("disk.json"), b"{broken seal").unwrap();
    }
    let error = engine.cleanup_purged().unwrap_err();
    assert_eq!(error.code(), ErrorCode::RecoveryRequired);
    assert_seal_json_cause(&error);
    for path in paths {
        assert_eq!(fs::read(path).unwrap(), b"old payload");
    }
}

fn assert_seal_json_cause(error: &Error) {
    use std::error::Error as _;
    let Error::Context { source, .. } = error else {
        panic!("unavailable evidence must retain its seal cause");
    };
    assert_eq!(source.code(), ErrorCode::DiskIdentityMismatch);
    let mut source = error.source();
    while let Some(cause) = source {
        if cause.is::<json_codec::Error>() {
            return;
        }
        source = cause.source();
    }
    panic!("seal rejection lost its concrete JSON source");
}
