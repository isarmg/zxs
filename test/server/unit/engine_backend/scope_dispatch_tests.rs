use super::*;
use storage_engine::Config;

#[test]
fn typed_scope_read_and_bounded_selection_do_not_change_state() {
    let root = crate::tests::private_tempdir();
    let mut engine = Engine::initialize(
        &root.path().join("system"),
        Config::development([root.path().join("disk")]),
    )
    .unwrap();
    let identity = install_test_administrator(&mut engine, "owner");
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Scopes").unwrap(),
            "P01",
            1,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    engine
        .initialize_directory_scope(
            &repo.repository_id,
            None,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let before = json_codec::to_value(engine.state()).unwrap();
    let backend = EngineBackend::new(engine);
    let direct = backend
        .directory_scopes(
            &identity,
            crate::ReadDirectoryScopes {
                repository_id: &repo.repository_id,
                filter: &crate::DirectoryBranchFilter::All,
                projection: crate::DirectoryBranchProjection::Full,
            },
        )
        .unwrap();
    let crate::DirectoryScopeRows::Full(branches) = direct.branches else {
        panic!("full request returned a summary")
    };
    assert!(!branches.is_empty());
    let branch = &branches[0].branch;
    let request = || {
        crate::ReadDirectorySelection::parse(
            repo.repository_id.clone(),
            "",
            vec![crate::DirectorySelectionSourceInput {
                directory_branch_id: branch.directory_branch_id.clone(),
                expected_tree_id: branch.head_tree_id.clone(),
            }],
            vec![],
        )
        .unwrap()
    };
    let selected = backend.directory_selection(&identity, request()).unwrap();
    assert_eq!(selected.branches.len(), 1);
    assert!(selected.branches[0].entries.is_empty());
    let stranger = Identity {
        principal: "other".into(),
        ..identity
    };
    assert_eq!(
        backend
            .directory_scopes(
                &stranger,
                crate::ReadDirectoryScopes {
                    repository_id: &repo.repository_id,
                    filter: &crate::DirectoryBranchFilter::All,
                    projection: crate::DirectoryBranchProjection::Full
                }
            )
            .err()
            .unwrap()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
    assert_eq!(
        backend
            .directory_selection(&stranger, request())
            .err()
            .unwrap()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
    assert_eq!(
        json_codec::to_value(backend.engine.lock().unwrap().state()).unwrap(),
        before
    );
}
