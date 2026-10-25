use super::*;
use crate::BackendAction;
use storage_engine::{Config, new_id};

#[test]
fn scope_reads_dispatch_by_action_without_changing_state() {
    let root = crate::tests::private_tempdir();
    let mut engine = Engine::initialize(
        &root.path().join("system"),
        Config::development([root.path().join("disk")]),
    )
    .unwrap();
    let identity = install_test_administrator(&mut engine, "owner");
    let repo = engine
        .create_repository("Scopes", "P01", 1, "owner", &new_id("op"))
        .unwrap();
    engine
        .initialize_directory_scope(&repo.repository_id, "", "owner", &new_id("op"))
        .unwrap();
    let before = json_codec::to_value(engine.state()).unwrap();
    let backend = EngineBackend::new(engine);
    let direct = backend
        .call(
            BackendAction::DirectoryBranchScopes,
            &identity,
            json!({"repository_id":repo.repository_id}),
        )
        .unwrap();
    let via_get = backend
        .call(
            BackendAction::DirectoryBranchScopes,
            &identity,
            json!({"repository_id":repo.repository_id,"query":{}}),
        )
        .unwrap();
    assert_eq!(direct, via_get);
    let branches = direct["directory_branches"].as_array().unwrap();
    assert!(!branches.is_empty());
    let branch = &branches[0];
    let selected = backend
        .call(
            BackendAction::DirectoryBranchSelection,
            &identity,
            json!({"repository_id":repo.repository_id,"path":"","sources":[{
                "directory_branch_id":branch["directory_branch_id"],
                "expected_tree_id":branch["head_tree_id"]
            }],"entries":[]}),
        )
        .unwrap();
    assert_eq!(selected["directory_branches"].as_array().unwrap().len(), 1);
    assert_eq!(selected["directory_branches"][0]["entries"], json!([]));
    // A missing selection payload must never silently become a full-scope read.
    assert!(
        backend
            .call(
                BackendAction::DirectoryBranchSelection,
                &identity,
                json!({"repository_id":repo.repository_id}),
            )
            .is_err()
    );
    let stranger = Identity {
        principal: "other".into(),
        ..identity
    };
    for action in [
        BackendAction::DirectoryBranchScopes,
        BackendAction::DirectoryBranchSelection,
    ] {
        assert_eq!(
            backend.call(action, &stranger, json!({})).unwrap_err().code,
            "UNAUTHORIZED"
        );
    }
    assert_eq!(
        json_codec::to_value(backend.engine.lock().unwrap().state()).unwrap(),
        before
    );
}
