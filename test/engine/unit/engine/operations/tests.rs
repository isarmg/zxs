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
    assert_eq!(engine.reads.lock().unwrap().get(&version), Some(&1));
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
    assert!(!engine.reads.lock().unwrap().contains_key(&version));
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
    let commit_id = operation.commit_id.clone().unwrap();
    let base_namespace = engine.state.commits[&commit_id].namespace.clone();
    // One immutable version may be referenced more than once. It must not
    // cause repeated full hashing or one lease per directory appearance.
    let mut built = crate::namespace::NamespaceObjects::new();
    let (entry, namespace) = engine
        .with_namespace_source(|base| {
            let entry = crate::namespace::NamespaceView::new(&base_namespace, base)
                .collect_entries()?
                .into_iter()
                .find(|entry| entry.kind == storage_contract::NamespaceEntryKind::File)
                .unwrap();
            let mut alias = entry.clone();
            alias.entry_id = new_id("entry");
            alias.path = name_policy::PortablePath::parse("alias-copy.txt").unwrap();
            let mut editor = crate::namespace::NamespaceEditor::open(&base_namespace, base)?;
            editor.upsert_file(&alias)?;
            Ok((entry, editor.seal(&mut built)?))
        })
        .unwrap();
    let members = engine.members();
    for object in built.take() {
        object.store(&members).unwrap();
    }
    std::sync::Arc::make_mut(&mut engine.state)
        .commits
        .get_mut(&commit_id)
        .unwrap()
        .namespace = namespace;
    for disk in &engine.config.disks {
        fs::remove_dir_all(disk.root.join("histories")).unwrap();
    }
    SNAPSHOT_LOAD_COUNT.with(|count| count.set(0));
    let plan =
        engine.prepare_operation_health(engine.operation_record(operation.operation_id).unwrap());
    assert_eq!(plan.versions.len(), 1);
    assert_eq!(
        engine.reads.lock().unwrap().get(&entry.version_id),
        Some(&1)
    );
    SNAPSHOT_LOAD_COUNT.with(|count| assert_eq!(count.get(), 0));
    drop(plan);
    assert!(engine.reads.lock().unwrap().is_empty());
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
    assert_eq!(
        engine
            .published_receipt(operation.operation_id)
            .unwrap()
            .unwrap(),
        operation
    );
    assert!(engine.reads.lock().unwrap().is_empty());
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
    assert!(engine.reads.lock().unwrap().is_empty());
    SNAPSHOT_LOAD_COUNT.with(|count| assert_eq!(count.get(), 0));
    // The receipt lives in the locator and log directories plus one segment
    // body; damaged receipt pages fail closed instead of guessing an owner.
    for field in ["operation_locators", "operation_log"] {
        for hash in crate::journal::map_pages(&members[0], &snapshot.maps[field]) {
            for disk in &members {
                fs::write(object_path(disk, "state-page", &hash), b"broken results").unwrap();
            }
        }
    }
    assert!(engine.operation_owner(operation).is_err());
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
