use crate::history_package::HistoryManifest;
use crate::{CommitRequest, Config, Engine};
use storage_contract::{OperationId, RepositoryName, encoding::canonical_json};

#[test]
fn manifest_storage_identity_admission_rejects_invalid_wire_without_rewriting_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let mut engine = Engine::initialize(
        &root.join("state"),
        Config::development([root.join("p"), root.join("b"), root.join("c")]).unwrap(),
    )
    .unwrap();
    let repo = engine
        .create_repository(
            &RepositoryName::parse("integer admission").unwrap(),
            "P01",
            1,
            "alice",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "alice")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "source.txt", b"integer boundary")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "alice".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "integer admission".into(),
            request_digest: None,
        })
        .unwrap();
    let state_before = canonical_json(engine.state()).unwrap();
    let manifest = HistoryManifest::from_state(engine.state(), &repo.history_tree_id).unwrap();
    manifest.validate().unwrap();
    let original = canonical_json(&manifest).unwrap();
    let version_id = manifest.versions.keys().next().unwrap().clone();
    for malformed in ["", "invalid.txt", "00000000000000", "0000000000000000", "ABC000000000000"] {
        let mut wire = json_codec::to_value(&manifest).unwrap();
        wire["versions"][&version_id]["storage_id"] = json_codec::json!(malformed);
        let before = canonical_json(&wire).unwrap();
        assert!(json_codec::from_slice::<HistoryManifest>(&before).is_err());
        assert_eq!(canonical_json(&wire).unwrap(), before);
    }
    // All admitted identity characters survive the package wire round-trip.
    let mut maximum = manifest.clone();
    maximum.versions.get_mut(&version_id).unwrap().storage_id =
        "zzzzzzzzzzzzzzz".parse().unwrap();
    let before = canonical_json(&maximum).unwrap();
    maximum.validate().unwrap();
    let decoded: HistoryManifest = json_codec::from_slice(&before).unwrap();
    decoded.validate().unwrap();
    assert_eq!(canonical_json(&maximum).unwrap(), before);
    assert_eq!(canonical_json(&decoded).unwrap(), before);
    assert_eq!(canonical_json(&manifest).unwrap(), original);
    assert_eq!(canonical_json(engine.state()).unwrap(), state_before);
}
