use super::*;

#[test]
fn nonempty_path_audit_cannot_be_dropped_by_the_target_merge() {
    let mut fixture = crate::foreign_import::tests::fixture();
    let repository = fixture.engine.state().repositories.values().next().unwrap().clone();
    let receipt = fixture.engine.file_namespace(
        &repository.repository_id,
        NamespaceOperation::parse(
            NamespaceOperationKind::Rename, "document.txt", Some("renamed.txt"),
        ).unwrap(),
        "admin",
        OperationId::generate().unwrap(),
    ).unwrap();
    let manifest = HistoryManifest::from_state(
        fixture.engine.state(), &repository.history_tree_id,
    ).unwrap();
    manifest.validate().unwrap();
    assert!(!manifest.path_audit.is_empty());
    let source_before = canonical_json(fixture.engine.state()).unwrap();
    let audit_before = canonical_json(&manifest.path_audit).unwrap();
    let mut target = fixture.engine.state().clone();
    let target_before = canonical_json(&target).unwrap();
    let error = include(&mut target, manifest.clone()).unwrap_err();
    assert_eq!(error.code(), ErrorCode::UnsupportedOperation);
    assert!(matches!(error, Error::PathChange(crate::PathChangeError::ForeignSource)));
    assert_eq!(canonical_json(&target).unwrap(), target_before);
    assert_eq!(canonical_json(&manifest.path_audit).unwrap(), audit_before);
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), source_before);
    assert_eq!(fixture.engine.state().operations[&receipt.operation_id], receipt);
}
