//! Source-only regressions for the disk-local directory inventory; not executed
//! during the current no-review/no-build/no-CI development slice.
use super::*;

fn identity() -> DiskRegistrationIdentity {
    DiskRegistrationIdentity {
        pool_id: format!("pool_{}", "1".repeat(32)),
        disk_id: format!("disk_{}", "2".repeat(32)),
        slot_id: "B01".into(),
        binding_generation: 1,
    }
}

fn directory(path: &str) -> DiskCatalogDirectory {
    DiskCatalogDirectory {
        repository_id: format!("repo_{}", "3".repeat(32)),
        history_tree_id: format!("history_{}", "4".repeat(32)),
        directory_id: format!("entry_{}", "5".repeat(32)),
        owner_path: name_policy::PortablePath::parse(path).unwrap(),
        operation_id: OperationId::generate().unwrap(),
    }
}

fn current(row: DiskCatalogDirectory, sequence: u64) -> DiskCatalogSnapshot {
    DiskCatalogSnapshot {
        identity: identity(),
        source_sequence: sequence,
        versions: Vec::new(),
        backup_directories: vec![row],
    }
}

fn record(sequence: u64, operation_id: OperationId, change: DiskHistoryChange) -> DiskHistoryRecord {
    DiskHistoryRecord {
        sequence,
        operation_id,
        candidate_hash: "a".repeat(64),
        snapshot_hash: "b".repeat(64),
        change,
    }
}

fn history(changes: Vec<DiskHistoryRecord>, through: u64) -> DiskHistorySnapshot {
    DiskHistorySnapshot {
        header: DiskHistoryHeader {
            identity: identity(),
            through_sequence: through,
            through_snapshot: Some("b".repeat(64)),
            publications: through as u32,
        },
        changes,
    }
}

#[test]
fn empty_directory_current_codec_keeps_identity_without_a_fake_version() {
    let snapshot = current(directory("backup/empty"), 1);
    let hash = storage_contract::object_hash("disk-current-catalog", &snapshot).unwrap();
    let bytes = database::encode(&snapshot, &hash).unwrap();
    assert_eq!(database::read(&bytes, &identity()).unwrap(), snapshot);
    let mut other = identity();
    other.binding_generation += 1;
    assert!(database::read(&bytes, &other).is_err());
    assert!(snapshot.versions.is_empty());
    assert_eq!(snapshot.backup_directories.len(), 1);
}

#[test]
fn offline_fold_keeps_old_empty_directory_path_after_a_rename() {
    let before = directory("backup/old");
    let mut after = before.clone();
    after.owner_path = name_policy::PortablePath::parse("backup/renamed").unwrap();
    after.operation_id = OperationId::generate().unwrap();
    let rename = after.operation_id;
    let material = history(vec![
        record(1, before.operation_id, DiskHistoryChange::BackupDirectoryEntered { after: before.clone() }),
        record(2, rename, DiskHistoryChange::BackupDirectoryDeparted { before: before.clone() }),
        record(2, rename, DiskHistoryChange::BackupDirectoryEntered { after: after.clone() }),
    ], 2);
    let recovered = history::fold_prepared(&material, Some(&current(before.clone(), 1))).unwrap();
    assert_eq!(recovered, current(after, 2));
    assert!(recovered.versions.is_empty());
    assert!(matches!(&material.changes[1].change,
        DiskHistoryChange::BackupDirectoryDeparted { before: retained } if retained == &before));
}

#[test]
fn offline_directory_fold_rejects_forged_predecessor_and_current_rows() {
    let before = directory("backup/empty");
    let mut after = before.clone();
    after.operation_id = OperationId::generate().unwrap();
    let missing = history(vec![record(1, after.operation_id,
        DiskHistoryChange::BackupDirectoryChanged { before: before.clone(), after: after.clone() })], 1);
    assert!(history::fold_prepared(&missing, None).is_err());
    let material = history(vec![record(1, before.operation_id,
        DiskHistoryChange::BackupDirectoryEntered { after: before })], 1);
    assert!(history::fold_prepared(&material, Some(&current(after, 1))).is_err());
}

#[test]
fn directory_projection_uses_exact_original_binding_and_each_states_policy() {
    let row = directory("backup/empty");
    let policy = ProtectionPolicy {
        policy_id: format!("policy_{}", "6".repeat(32)),
        repository_id: row.repository_id.clone(),
        path_prefix: "empty".into(),
        target_copies: 2,
        inheritable: true,
        scope_directory_id: Some(row.directory_id.clone()),
        scope_file_id: None,
        backup_prefixes: std::collections::BTreeMap::from([("B01".into(), "backup/empty".into())]),
        generation: 1,
    };
    let receipt = OperationResult {
        operation_id: row.operation_id,
        kind: "backup.configure-recursive-tree".into(),
        commit_id: None,
        history_tree_id: Some(row.history_tree_id.clone()),
        branch_id: None,
        commit_state: CommitState::Published,
        protection_at_publish: ProtectionAtPublish::NotApplicable,
        current_health: CurrentHealth::Healthy,
        request_digest: "a".repeat(64),
        sequence: 1,
        result: json_codec::json!({
            "policy": policy,
            "directory_namespace": {
                "format": "recursive-backup-directory-namespace",
                "replicas": {"B01": {"identity": identity(), "directories": [{
                    "entry_id": row.directory_id, "path": row.owner_path,
                }]}}
            }
        }),
    };
    let mut state = State::default();
    state.sequence = 1;
    state.protection_policies.insert(policy.policy_id.clone(), policy.clone());
    state.operations.insert(row.operation_id, receipt);
    assert_eq!(snapshot(&state, identity()).unwrap(), current(row, 1));
    let mut reused = identity();
    reused.binding_generation += 1;
    assert!(snapshot(&state, reused).unwrap().is_empty());
    let mut next = state.clone();
    next.begin_tracking();
    next.protection_policies.get_mut(&policy.policy_id).unwrap().generation += 1;
    assert!(crate::protection::directory_catalog_changed(&state, &next));
    assert!(snapshot(&next, identity()).unwrap().is_empty());
    assert_eq!(snapshot(&state, identity()).unwrap().backup_directories.len(), 1);
}
