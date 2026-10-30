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
            &crate::tests::history_read_control(),
        )
        .unwrap();
    let direct = crate::tests::directory_scopes_value(&direct);
    let branches = direct["directory_branches"].as_array().unwrap();
    assert!(!branches.is_empty());
    let branch = &branches[0];
    let request = || {
        crate::ReadDirectorySelection::parse(
            repo.repository_id.clone(),
            "",
            vec![crate::DirectorySelectionSourceInput {
                directory_branch_id: branch["directory_branch_id"].as_str().unwrap().to_owned(),
                expected_tree_id: branch["head_tree_id"].as_str().unwrap().to_owned(),
            }],
            vec![],
        )
        .unwrap()
    };
    let selected = backend
        .directory_selection(&identity, request(), &crate::tests::history_read_control())
        .unwrap();
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
                },
                &crate::tests::history_read_control()
            )
            .err()
            .unwrap()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
    assert_eq!(
        backend
            .directory_selection(&stranger, request(), &crate::tests::history_read_control())
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
