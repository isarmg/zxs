use super::*;
use crate::history_package::path_audit::{self as audit, PathAuditCarrier};
use storage_contract::{OperationId, Principal};

fn source(root: &Path) -> (Engine, crate::Repository, crate::OperationResult) {
    let mut engine = initialize(root);
    let repo = engine.create_disk_root("P01", "alice", OperationId::generate().unwrap()).unwrap();
    save(&mut engine, &repo.main_branch_id, b"source proof");
    let receipt = engine.file_namespace(
        &repo.repository_id,
        NamespaceOperation::parse(
            NamespaceOperationKind::Rename, "file.txt", Some("current.txt"),
        ).unwrap(),
        "alice", OperationId::generate().unwrap(),
    ).unwrap();
    (engine, repo, receipt)
}

#[test]
fn path_audit_rejects_changed_principal_endpoint_source_fact_and_same_id_different_proof() {
    let temporary = tempfile::tempdir().unwrap();
    let (engine, repo, receipt) = source(&temporary.path().join("source"));
    let manifest = HistoryManifest::from_state(engine.state(), &repo.history_tree_id).unwrap();
    let original = canonical_json(&manifest).unwrap();
    for corruption in 0..4 {
        let mut changed = manifest.clone();
        let PathAuditCarrier::Complete { sources, .. } =
            changed.path_audit.get_mut(&receipt.operation_id).unwrap()
        else {
            panic!("native rename must have complete provenance");
        };
        let evidence = sources.get_mut(&receipt.operation_id).unwrap();
        match corruption {
            0 => evidence.principal = Principal::new("other-actor".into()).unwrap(),
            1 => evidence.projections[0].before_version_id =
                storage_contract::VersionId::generate().unwrap(),
            2 => evidence.operation.result["path_changes"][0]["after"]["path"] =
                json!("forged.txt"),
            3 => evidence.projections.clear(),
            _ => unreachable!(),
        }
        assert!(changed.validate().is_err());
    }
    let mut conflict = manifest.clone();
    let mut duplicate = conflict.path_audit[&receipt.operation_id].clone();
    let PathAuditCarrier::Complete { sources, .. } = &mut duplicate else {
        panic!("expected complete carrier");
    };
    sources.get_mut(&receipt.operation_id).unwrap().operation.result["extra"] = json!(true);
    conflict.path_audit.insert(OperationId::generate().unwrap(), duplicate);
    assert!(audit::portable(&conflict, engine.state.sequence).is_err());
    assert_eq!(canonical_json(&manifest).unwrap(), original);
}

#[test]
fn restore_rejects_same_operation_id_with_different_native_proof_without_writes() {
    let temporary = tempfile::tempdir().unwrap();
    let (engine, repo, receipt) = source(&temporary.path().join("source"));
    let package = temporary.path().join("package");
    engine.export_history_package(&repo.history_tree_id, &package).unwrap();
    let mut target = initialize(&temporary.path().join("target"));
    let other = target.create_disk_root("P01", "bob", OperationId::generate().unwrap()).unwrap();
    save(&mut target, &other.main_branch_id, b"different native body");
    target.file_namespace(
        &other.repository_id,
        NamespaceOperation::parse(
            NamespaceOperationKind::Rename, "file.txt", Some("different.txt"),
        ).unwrap(),
        "bob", receipt.operation_id,
    ).unwrap();
    let state = canonical_json(target.state()).unwrap();
    let files = super::export_audit_guard::disk_image(temporary.path());
    let error = target.restore_history_package(
        &package, "P01", 3, "bob", OperationId::generate().unwrap(),
    ).unwrap_err();
    assert!(matches!(error, Error::PathChange(crate::PathChangeError::Publication)));
    assert_eq!(canonical_json(target.state()).unwrap(), state);
    assert_eq!(super::export_audit_guard::disk_image(temporary.path()), files);
}

#[test]
fn copied_restored_provenance_survives_cold_open_without_inventing_source_operations() {
    let temporary = tempfile::tempdir().unwrap();
    let (source, repo, native) = source(&temporary.path().join("source"));
    let package = temporary.path().join("package");
    source.export_history_package(&repo.history_tree_id, &package).unwrap();
    let root = temporary.path().join("target");
    let mut config = Config::development(
        (0..4).map(|index| root.join(format!("disk-{index}"))),
    ).unwrap();
    config.disks[3].level = 1;
    config.disks[3].upstream_slot_id = None;
    config.disks[3].metadata_member = false;
    let mut target = Engine::initialize(&root.join("state"), config).unwrap();
    let restored = target.restore_history_package(
        &package, "P01", 3, "restore-manager", OperationId::generate().unwrap(),
    ).unwrap();
    assert!(restored.sequence > source.state.sequence);
    assert!(!target.state.operations.contains_key(&native.operation_id));
    let before_repeat = canonical_json(target.state()).unwrap();
    let files_before_repeat = super::export_audit_guard::disk_image(temporary.path());
    assert_eq!(target.restore_history_package(
        &package, "P01", 3, "restore-manager", OperationId::generate().unwrap(),
    ).unwrap_err().code(), ErrorCode::ObjectCollision);
    assert_eq!(canonical_json(target.state()).unwrap(), before_repeat);
    assert_eq!(
        super::export_audit_guard::disk_image(temporary.path()), files_before_repeat
    );
    drop(target);
    let mut target = Engine::open(&root.join("state")).unwrap();
    let destination = target.create_disk_root(
        "D04", "copy-manager", OperationId::generate().unwrap(),
    ).unwrap();
    let request = crate::FileHistoryCopyRequest {
        source_repository_id: repo.repository_id.clone(),
        source_path: "current.txt".into(),
        expected_source_head: target.state.branches[&repo.main_branch_id].head_commit_id.clone(),
        target_repository_id: destination.repository_id.clone(),
        target_name: "copied.txt".into(),
        expected_target_head: target.state.branches[&destination.main_branch_id]
            .head_commit_id.clone(),
    };
    let copied = target.copy_file_history(
        &request, "copy-manager", OperationId::generate().unwrap(),
    ).unwrap();
    let file = copied.result["file_id"].as_str().unwrap().to_owned();
    let prefix = crate::business::history_copy::retained::read(&copied, &file).unwrap();
    assert_eq!(prefix.path_audit.len(), 1);
    let proof = &prefix.path_audit[&native.operation_id];
    assert_eq!(proof.derivations[&prefix.path_changes[0].file_id], vec![copied.operation_id]);
    let mut forged = prefix.clone();
    forged.path_audit.get_mut(&native.operation_id).unwrap().derivations
        .get_mut(&prefix.path_changes[0].file_id).unwrap().clear();
    assert!(audit::validate_copy(&forged).is_err());
    assert_eq!(prefix.path_audit[&native.operation_id].operation, native);
    assert_eq!(prefix.path_audit[&native.operation_id].principal.as_str(), "alice");
    assert_eq!(prefix.path_changes.len(), 1);
    let mut old_receipt = copied.clone();
    old_receipt.result["retained_lifecycle"].as_object_mut().unwrap()
        .remove("path_audit").unwrap();
    let original_bytes = canonical_json(&old_receipt).unwrap();
    assert!(crate::business::history_copy::retained::read(&old_receipt, &file).is_err());
    assert_eq!(canonical_json(&old_receipt).unwrap(), original_bytes);
    assert_ne!(prefix.path_changes[0].file_id, prefix.path_changes[0].source.file_id);
    drop(target);
    let target = Engine::open(&root.join("state")).unwrap();
    let projection = crate::state_projection::ProjectionSnapshot::new(target.state());
    let facts = crate::business::history_copy::retained::path_changes(
        projection.view(), &storage_contract::history_window::HistoryFileId::parse(&file).unwrap(),
    ).unwrap();
    assert_eq!(facts, prefix.path_changes);
    assert!(!target.state.operations.contains_key(&native.operation_id));
    target.export_history_package(
        &destination.history_tree_id, &temporary.path().join("recopied-package"),
    ).unwrap();
}

#[test]
fn required_path_audit_schema_rejects_old_graph_without_rewrite() {
    let temporary = tempfile::tempdir().unwrap();
    let (engine, repo, _) = source(&temporary.path().join("source"));
    let package = temporary.path().join("package");
    engine.export_history_package(&repo.history_tree_id, &package).unwrap();
    let path = package.join("manifest.json");
    let mut old: json_codec::Value = read_json(&path).unwrap();
    old["maps"].as_object_mut().unwrap().remove("path_audit").unwrap();
    write_json(&path, &old, true).unwrap();
    let bytes = fs::read(&path).unwrap();
    let files = super::export_audit_guard::disk_image(temporary.path());
    assert!(offline_check_history_package(&package).is_err());
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(super::export_audit_guard::disk_image(temporary.path()), files);
    let mut logical = json_codec::to_value(
        HistoryManifest::from_state(engine.state(), &repo.history_tree_id).unwrap(),
    ).unwrap();
    logical.as_object_mut().unwrap().remove("path_audit").unwrap();
    let old_bytes = canonical_json_value(&logical).unwrap();
    assert!(json_codec::from_value::<HistoryManifest>(logical).is_err());
    assert!(!old_bytes.is_empty());
}
