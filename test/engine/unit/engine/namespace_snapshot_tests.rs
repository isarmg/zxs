use super::*;
use crate::*;

#[test]
fn membership_registry_namespace_snapshot_is_thin_and_rechecks_the_current_head() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temp.path().join("system"),
        Config::development([temp.path().join("disk")]).unwrap(),
    )
    .unwrap();
    let repository = engine
        .create_repository(
            &RepositoryName::parse("snapshot").unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let captured = engine
        .namespace_snapshot(&repository.main_branch_id)
        .unwrap();
    assert!(captured.tree().entries().is_empty());
    let witness = captured.witness();
    let shared = match captured.into_entries() {
        NamespaceEntries::Commit(commit) => commit,
        NamespaceEntries::Projected(_) => panic!("initial namespace must share its commit"),
    };
    let owned = engine.state.commits.get_shared(&shared.commit_id).unwrap();
    assert!(Arc::ptr_eq(&shared, &owned));
    drop(shared);
    drop(owned);
    assert!(engine.namespace_snapshot_matches(&witness));
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "new", b"data")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "advance".into(),
            request_digest: None,
        })
        .unwrap();
    assert!(!engine.namespace_snapshot_matches(&witness));
}
