//! Real published rename snapshots; mutations below are explicit fault fixtures.
use super::*;
use crate::{CommitRequest, Config, Engine, NamespaceOperation, NamespaceOperationKind};
use crate::history_package::path_audit::{self, PathAuditCarrier};
use storage_contract::OperationId;

fn fixture() -> (State, State, String, OperationId) {
    let root = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &root.path().join("state"),
        Config::development([
            root.path().join("p"),
            root.path().join("b"),
            root.path().join("c"),
        ]).unwrap(),
    ).unwrap();
    let repo = engine.create_disk_root("P01", "alice", OperationId::generate().unwrap()).unwrap();
    let workspace = engine.create_workspace(&repo.main_branch_id, "alice").unwrap();
    engine.stage_bytes(&workspace.workspace_id, "file.txt", b"immutable body").unwrap();
    engine.commit_workspace(CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: OperationId::generate().unwrap(),
        principal: "alice".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "save".into(),
        request_digest: None,
    }).unwrap();
    let before = engine.state().clone();
    let operation = OperationId::generate().unwrap();
    engine.file_namespace(&repo.repository_id, NamespaceOperation::parse(
        NamespaceOperationKind::Rename, "file.txt", Some("renamed.txt"),
    ).unwrap(), "alice", operation).unwrap();
    (before, engine.state().clone(), repo.history_tree_id, operation)
}

fn candidate(before: &State) -> State {
    let mut after = before.clone();
    after.begin_tracking();
    after
}

fn full_audit(state: &State, history: &str) -> json_codec::Map<String, Value> {
    path_audit::project(state, history).unwrap().into_iter()
        .map(|(id, carrier)| (id.to_string(), json_codec::to_value(carrier).unwrap())).collect()
}

#[test]
fn published_rename_full_indexed_and_delta_keep_original_receipt() {
    let (before, after, history, operation) = fixture();
    let index = DerivedOwners::from_state(&before);
    let delta = ProjectionDelta::between(&before, &after, &index).unwrap().unwrap();
    assert_eq!(delta.owners, DerivedOwners::from_state(&after));
    let rows = delta.owners.rows(&history).unwrap().values(Field::PathAudit, &after).unwrap();
    assert_eq!(rows, full_audit(&after, &history));
    assert!(rows.contains_key(&operation.to_string()));
    assert_eq!(delta.histories[&history].maps[&Field::PathAudit][&operation.to_string()],
        Some(rows[&operation.to_string()].clone()));
    let carrier = path_audit::carrier(&after, operation).unwrap().unwrap();
    let PathAuditCarrier::Complete { sources, .. } = carrier else {
        panic!("native rename must retain complete provenance");
    };
    assert_eq!(sources[&operation].operation, after.operations[&operation]);
    assert_eq!(sources[&operation].principal.as_str(), "alice");
}

#[test]
fn principal_only_change_is_dirty_and_fails_closed() {
    let (_, before, history, operation) = fixture();
    let index = DerivedOwners::from_state(&before);
    let mut after = candidate(&before);
    after.operation_principals.insert(operation, "mallory".into());
    assert!(Writes::between(&before, &after).unwrap().rows[&Field::PathAudit]
        .contains(&operation.to_string()));
    assert!(ProjectionDelta::between(&before, &after, &index).is_err());
    let owners = DerivedOwners::transition(&before, &after, &index, None).unwrap().unwrap();
    assert_eq!(owners, DerivedOwners::from_state(&after));
    assert!(owners.rows(&history).unwrap().values(Field::PathAudit, &after).is_err());
    assert!(path_audit::project(&after, &history).is_err());
}

#[test]
fn unrelated_sequence_preserves_audit_storage_without_full_rescan() {
    let (_, before, _, _) = fixture();
    let index = DerivedOwners::from_state(&before);
    let mut after = candidate(&before);
    after.sequence += 1;
    let writes = Writes::between(&before, &after).unwrap();
    assert!(writes.operations.is_empty());
    assert!(writes.rows[&Field::PathAudit].is_empty());
    let delta = ProjectionDelta::between(&before, &after, &index).unwrap().unwrap();
    assert!(delta.histories.is_empty());
    assert!(index.path_audit_owners.shares_storage_with(&delta.owners.path_audit_owners));
    assert!(index.history_rows.shares_storage_with(&delta.owners.history_rows));
    assert_eq!(delta.owners, DerivedOwners::from_state(&after));
}

#[test]
fn non_audit_replacement_removes_derived_row_with_exact_delta() {
    let (_, before, history, operation) = fixture();
    let index = DerivedOwners::from_state(&before);
    let mut after = candidate(&before);
    // Index-only fault fixture: this does not authorize rewriting a published
    // receipt. Exercise a carrier Some -> None change using an exact keyed write.
    let mut replacement = before.operations[&operation].clone();
    replacement.kind = "projection.fixture.non-audit".into();
    after.operations.insert(operation, replacement);
    let writes = Writes::between(&before, &after).unwrap();
    assert_eq!(writes.operations, BTreeSet::from([operation]));
    assert_eq!(writes.rows[&Field::PathAudit], BTreeSet::from([operation.to_string()]));
    let delta = ProjectionDelta::between(&before, &after, &index).unwrap().unwrap();
    assert_eq!(delta.owners, DerivedOwners::from_state(&after));
    assert!(!delta.owners.path_audit_owners.contains_key(&operation));
    assert_eq!(delta.histories[&history].maps[&Field::PathAudit][&operation.to_string()], None);
    let projected = delta.owners.rows(&history).unwrap()
        .values(Field::PathAudit, &after).unwrap();
    assert!(!projected.contains_key(&operation.to_string()));
    assert_eq!(projected, full_audit(&after, &history));
}

#[test]
fn foreign_receipt_removal_requires_full_rebuild_and_clears_audit_owners() {
    let (_, before, history, operation) = fixture();
    let index = DerivedOwners::from_state(&before);
    let mut after = candidate(&before);
    // Permanent operations have no tracked deletion API. This deliberate
    // foreign mutable escape must lose exact lineage, not claim a delta.
    after.operations.remove_discard(&operation);
    after.operation_principals.remove_discard(&operation);
    assert!(matches!(after.operations.changes_since(&before.operations), MapChanges::All));
    assert!(matches!(
        after.operation_principals.changes_since(&before.operation_principals),
        MapChanges::All,
    ));
    assert!(Writes::between(&before, &after).is_none());
    assert!(ProjectionDelta::between(&before, &after, &index).unwrap().is_none());
    let mut transition = ProjectionTransition::new(&before, &after, Some(&index)).unwrap();
    assert!(transition.take_history_edits().is_none());
    let owners = transition.next().owners();
    assert_eq!(owners, &DerivedOwners::from_state(&after));
    assert!(!owners.path_audit_owners.contains_key(&operation));
    assert_eq!(Field::PathAudit.value(&after, &operation.to_string()).unwrap(), None);
    let projected = owners.rows(&history).unwrap().values(Field::PathAudit, &after).unwrap();
    assert!(!projected.contains_key(&operation.to_string()));
    assert_eq!(projected, full_audit(&after, &history));
}

#[test]
fn malformed_carrier_tracks_new_histories_and_repair_removes_unscoped_membership() {
    let (_, valid, history, operation) = fixture();
    let mut before = valid.clone();
    before.operations.get_mut(&operation).unwrap().result = json_codec::json!({});
    let index = DerivedOwners::from_state(&before);
    assert!(index.unscoped_path_audit.contains_key(&operation));
    let mut after = candidate(&before);
    let other = "history_ffffffffffffffffffffffffffffffff";
    let mut added = after.histories[&history].clone();
    added.history_tree_id = other.into();
    after.histories.insert(other.into(), added);
    let owners = DerivedOwners::transition(&before, &after, &index, None).unwrap().unwrap();
    assert_eq!(owners, DerivedOwners::from_state(&after));
    assert!(owners.rows(other).unwrap().values(Field::PathAudit, &after).is_err());
    assert!(ProjectionDelta::between(&before, &after, &index).is_err());
    let mut removed = candidate(&after);
    removed.histories.remove(other);
    let removed_owners = DerivedOwners::transition(&after, &removed, &owners, None)
        .unwrap().unwrap();
    assert_eq!(removed_owners, DerivedOwners::from_state(&removed));
    assert!(removed_owners.rows(other).is_none());
    assert!(removed_owners.rows(&history).unwrap().values(Field::PathAudit, &removed).is_err());
    let mut repaired = candidate(&after);
    repaired.operations.insert(operation, valid.operations[&operation].clone());
    let delta = ProjectionDelta::between(&after, &repaired, &owners).unwrap().unwrap();
    assert_eq!(delta.owners, DerivedOwners::from_state(&repaired));
    assert!(!delta.owners.unscoped_path_audit.contains_key(&operation));
    assert_eq!(delta.histories[other].maps[&Field::PathAudit][&operation.to_string()], None);
}

#[test]
fn restore_carrier_indexes_imported_projection_without_reclassifying_source_as_native() {
    let (_, before, history, source_id) = fixture();
    let native = path_audit::carrier(&before, source_id).unwrap().unwrap();
    let PathAuditCarrier::Complete { mut sources, .. } = native else {
        panic!("native rename has complete evidence");
    };
    let target = "history_ffffffffffffffffffffffffffffffff";
    let copy_id = OperationId::generate().unwrap();
    let identity = crate::business::history_copy::identity;
    for source in sources.values_mut() {
        let mut derivations = BTreeMap::new();
        for (file, mut chain) in std::mem::take(&mut source.derivations) {
            chain.push(copy_id);
            let mapped = identity(copy_id, &file.to_string()).unwrap().parse().unwrap();
            derivations.insert(mapped, chain);
        }
        source.derivations = derivations;
        for row in &mut source.projections {
            row.file_id = identity(copy_id, &row.file_id.to_string()).unwrap().parse().unwrap();
            row.history_tree_id = target.parse().unwrap();
            row.branch_id = identity(copy_id, &row.branch_id.to_string()).unwrap().parse().unwrap();
            row.before_version_id = identity(copy_id, &row.before_version_id.to_string())
                .unwrap().parse().unwrap();
            row.after_version_id = identity(copy_id, &row.after_version_id.to_string())
                .unwrap().parse().unwrap();
        }
        source.projections.sort_by_key(|row| {
            (row.history_tree_id, row.file_id, row.source.event_id)
        });
    }
    path_audit::validate_sources(&sources).unwrap();
    // A receipt-reader fixture, not an admitted restore. Original source bytes,
    // principal and ID stay intact while the imported target projection differs.
    let restore_id = OperationId::generate().unwrap();
    let mut receipt = before.operations[&source_id].clone();
    receipt.operation_id = restore_id;
    receipt.kind = path_audit::RESTORE_KIND.into();
    receipt.sequence += 1;
    receipt.history_tree_id = Some(target.into());
    receipt.result = json_codec::json!({"restored_path_audit": &sources});
    let mut after = candidate(&before);
    after.operations.insert(restore_id, receipt);
    after.operation_principals.insert(restore_id, "alice".into());
    let index = DerivedOwners::from_state(&before);
    let delta = ProjectionDelta::between(&before, &after, &index).unwrap().unwrap();
    assert_eq!(delta.owners, DerivedOwners::from_state(&after));
    assert_eq!(delta.owners.rows(target).unwrap().values(Field::PathAudit, &after).unwrap(),
        full_audit(&after, target));
    assert!(!delta.histories.get(&history).and_then(|edits| edits.maps.get(&Field::PathAudit))
        .is_some_and(|rows| rows.contains_key(&restore_id.to_string())));
    assert_eq!(after.operations[&source_id], before.operations[&source_id]);
    let file = sources[&source_id].projections[0].file_id.to_string();
    let files = delta.owners.file_rows(&file).unwrap();
    assert!(files.path_operations.contains_key(&restore_id));
    assert!(!files.history_copies.contains_key(&restore_id));
}

#[test]
fn mutable_graph_owner_changes_do_not_retarget_immutable_audit_sources() {
    let (_, before, history, operation) = fixture();
    let index = DerivedOwners::from_state(&before);
    let mut after = candidate(&before);
    let fact = crate::persist::path_changes::read(&before.operations[&operation])
        .unwrap().remove(0);
    let other = "history_ffffffffffffffffffffffffffffffff";
    after.branches.get_mut(&fact.branch_id.to_string()).unwrap().history_tree_id = other.into();
    after.versions.get_mut(&fact.after_version_id.to_string()).unwrap().history_tree_id =
        other.into();
    let delta = ProjectionDelta::between(&before, &after, &index).unwrap().unwrap();
    assert_eq!(delta.owners, DerivedOwners::from_state(&after));
    assert!(index.path_audit_owners.shares_storage_with(&delta.owners.path_audit_owners));
    assert_eq!(delta.owners.rows(&history).unwrap().values(Field::PathAudit, &after).unwrap(),
        full_audit(&before, &history));
    assert!(!delta.histories.values().any(|edits| edits.maps.contains_key(&Field::PathAudit)));
}
