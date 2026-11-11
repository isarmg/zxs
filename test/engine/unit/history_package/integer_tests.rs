use crate::history_package::HistoryManifest;
use crate::{CommitRequest, Config, Engine, Error};
use std::error::Error as _;
use storage_contract::{ErrorCode, OperationId, RepositoryName, encoding::canonical_json};

#[test]
fn manifest_storage_integer_admission_preserves_cause_order_and_bytes() {
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
    for corrupt_calendar in [false, true] {
        let mut invalid = manifest.clone();
        let version = invalid.versions.get_mut(&version_id).unwrap();
        let (_, suffix) = version.storage_name.split_once('_').unwrap();
        version.storage_name = format!("v18446744073709551616_{suffix}");
        if corrupt_calendar {
            version.created_at = "2023-02-29T00:00:00Z".into();
        }
        let before = canonical_json(&invalid).unwrap();
        let error = invalid.validate().unwrap_err();
        let mut current = &error;
        for code in [
            ErrorCode::HistoryPackageInvalid,
            ErrorCode::ContainerLayoutInvalid,
        ] {
            assert_eq!(current.code(), code);
            assert!(!current.is_retryable());
            assert!(current.published_result().is_none());
            let Error::Context { source, .. } = current else {
                panic!("manifest admission must retain both context layers: {current:?}");
            };
            current = source;
        }
        assert!(matches!(current, Error::Integer(_)));
        assert_eq!(current.code(), ErrorCode::ContainerLayoutInvalid);
        assert!(!current.is_retryable());
        assert!(current.published_result().is_none());
        let cause = current
            .source()
            .unwrap()
            .downcast_ref::<std::num::ParseIntError>()
            .unwrap();
        assert_eq!(cause.kind(), &std::num::IntErrorKind::PosOverflow);
        assert!(cause.source().is_none());
        assert_eq!(canonical_json(&invalid).unwrap(), before);
    }
    // A retained maximum storage sequence is valid until a new one is requested.
    let mut maximum = manifest.clone();
    let version = maximum.versions.get_mut(&version_id).unwrap();
    let (_, suffix) = version.storage_name.split_once('_').unwrap();
    version.storage_name = format!("v{}_{suffix}", u64::MAX);
    let before = canonical_json(&maximum).unwrap();
    maximum.validate().unwrap();
    let decoded: HistoryManifest = json_codec::from_slice(&before).unwrap();
    decoded.validate().unwrap();
    assert_eq!(canonical_json(&maximum).unwrap(), before);
    assert_eq!(canonical_json(&decoded).unwrap(), before);
    assert_eq!(canonical_json(&manifest).unwrap(), original);
    assert_eq!(canonical_json(engine.state()).unwrap(), state_before);
}
