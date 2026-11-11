use crate::history_package::HistoryManifest;
use crate::{Config, Engine};
use std::error::Error as _;
use storage_contract::{
    DeletedPath, ErrorCode, NamespaceDeletions, NamespaceEntryKind, OperationId, RepositoryName,
    encoding::canonical_json,
};

#[test]
fn namespace_timestamp_admission_preserves_causes_order_and_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let mut engine = Engine::initialize(
        &root.join("state"),
        Config::development([root.join("p"), root.join("b"), root.join("c")]).unwrap(),
    )
    .unwrap();
    let repo = engine
        .create_repository(
            &RepositoryName::parse("timestamp admission").unwrap(),
            "P01",
            1,
            "alice",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let state_before = canonical_json(engine.state()).unwrap();
    let mut manifest = HistoryManifest::from_state(engine.state(), &repo.history_tree_id).unwrap();
    let commit = manifest.history.root_commit_id.clone();
    manifest.commits.get_mut(&commit).unwrap().namespace_deletions = Some(NamespaceDeletions {
        timestamp: 0,
        paths: vec![DeletedPath {
            path: "parent/intermediate.txt".into(),
            kind: NamespaceEntryKind::File,
        }],
    });

    for timestamp in [0, 1_709_251_199, 253_402_300_799] {
        let mut valid = manifest.clone();
        valid
            .commits
            .get_mut(&commit)
            .unwrap()
            .namespace_deletions
            .as_mut()
            .unwrap()
            .timestamp = timestamp;
        let before = canonical_json(&valid).unwrap();
        valid.validate().unwrap();
        assert_eq!(canonical_json(&valid).unwrap(), before);
        let decoded: HistoryManifest = json_codec::from_slice(&before).unwrap();
        decoded.validate().unwrap();
        assert_eq!(canonical_json(&decoded).unwrap(), before);
    }

    for timestamp in [u64::MAX, i64::MAX as u64, 253_402_300_800] {
        let mut invalid = manifest.clone();
        let deletions = invalid
            .commits
            .get_mut(&commit)
            .unwrap()
            .namespace_deletions
            .as_mut()
            .unwrap();
        deletions.timestamp = timestamp;
        // Timestamp admission precedes path admission, including when both fail.
        deletions.paths[0].path = "../escape".into();
        let before = canonical_json(&invalid).unwrap();
        let error = invalid.validate().unwrap_err();
        assert_eq!(error.code(), ErrorCode::HistoryPackageInvalid);
        assert!(!error.is_retryable());
        assert!(error.published_result().is_none());
        let cause = error.source().unwrap().source().unwrap();
        if timestamp == u64::MAX {
            assert!(cause.is::<std::num::TryFromIntError>());
        } else {
            assert!(cause.is::<date_time::error::ComponentRange>());
        }
        assert_eq!(canonical_json(&invalid).unwrap(), before);
    }
    assert_eq!(canonical_json(engine.state()).unwrap(), state_before);
}
