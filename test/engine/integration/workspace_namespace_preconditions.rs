//! A workspace acts on the stable entries it opened, not later namesakes.
use storage_engine::{CommitRequest, Config, Engine, NamespaceOperation, new_id};

#[test]
fn stale_namespace_operations_preserve_recreated_same_name_directories() {
    for kind in ["delete", "delete_tree", "rename"] {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("system");
        let config = Config::development([
            temporary.path().join("primary"),
            temporary.path().join("backup"),
            temporary.path().join("tertiary"),
        ]);
        let mut engine = Engine::initialize(&root, config).unwrap();
        let repo = engine
            .create_repository("Namespace preconditions", "P01", 3, "admin", &new_id("op"))
            .unwrap();
        let change = |kind: &str| {
            NamespaceOperation::parse(
                kind.parse().unwrap(),
                "Documents",
                (kind == "rename").then_some("Moved"),
            )
            .unwrap()
        };
        engine
            .file_namespace(&repo.repository_id, change("mkdir"), "admin", &new_id("op"))
            .unwrap();
        let original = engine.list_tree(&repo.main_branch_id).unwrap()[0].clone();
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        engine
            .namespace_operation(&workspace.workspace_id, change(kind))
            .unwrap();
        engine
            .file_namespace(
                &repo.repository_id,
                change("delete"),
                "admin",
                &new_id("op"),
            )
            .unwrap();
        engine
            .file_namespace(&repo.repository_id, change("mkdir"), "admin", &new_id("op"))
            .unwrap();
        let replacement = engine.list_tree(&repo.main_branch_id).unwrap()[0].clone();
        assert_ne!(original.entry_id, replacement.entry_id);
        // Preconditions must survive service restarts alongside the pending work.
        drop(engine);
        let mut engine = Engine::open(&root).unwrap();
        let error = engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id.clone(),
                operation_id: new_id("op"),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "Apply pending namespace change".into(),
                request_digest: None,
            })
            .unwrap_err();
        assert_eq!(
            error.code(),
            storage_contract::ErrorCode::NamespaceConflict,
            "{kind}"
        );
        let current = engine.list_tree(&repo.main_branch_id).unwrap();
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].entry_id, replacement.entry_id);
        assert_eq!(current[0].path, "Documents");
        assert_eq!(
            engine
                .workspace(&workspace.workspace_id)
                .unwrap()
                .state
                .as_str(),
            "open"
        );
    }
}

#[test]
fn canonical_equivalent_directory_sources_keep_stored_parent_spelling() {
    let temporary = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temporary.path().join("system"),
        Config::development([
            temporary.path().join("primary"),
            temporary.path().join("backup"),
            temporary.path().join("tertiary"),
        ]),
    )
    .unwrap();
    let repo = engine
        .create_repository("Canonical queries", "P01", 1, "admin", &new_id("op"))
        .unwrap();
    let change = |kind: &str, path: &str, destination: Option<&str>| {
        NamespaceOperation::parse(
            kind.parse().unwrap(),
            path,
            (destination.map(str::to_owned)).as_deref(),
        )
        .unwrap()
    };
    engine
        .file_namespace(
            &repo.repository_id,
            change("mkdir", "Café/Å", None),
            "admin",
            &new_id("op"),
        )
        .unwrap();
    let original = engine.list_tree(&repo.main_branch_id).unwrap();
    for operation in [
        NamespaceOperation::parse("mkdir".parse().unwrap(), "Cafe\u{301}/new", None),
        NamespaceOperation::parse("rename".parse().unwrap(), "Café/Å", Some("Café/e\u{301}")),
    ] {
        assert_eq!(
            operation.unwrap_err().code(),
            storage_contract::ErrorCode::NameNotNormalized
        );
        assert_eq!(
            storage_engine::canonical_json(&engine.list_tree(&repo.main_branch_id).unwrap())
                .unwrap(),
            storage_engine::canonical_json(&original).unwrap()
        );
    }
    engine
        .file_namespace(
            &repo.repository_id,
            change("rename", "Cafe\u{301}/A\u{30a}", Some("Café/가")),
            "admin",
            &new_id("op"),
        )
        .unwrap();
    let moved = engine.list_tree(&repo.main_branch_id).unwrap();
    assert!(moved.iter().any(|entry| entry.path == "Café"));
    let before = original
        .iter()
        .find(|entry| entry.path == "Café/Å")
        .unwrap();
    let after = moved.iter().find(|entry| entry.path == "Café/가").unwrap();
    assert_eq!(before.entry_id, after.entry_id);
    engine
        .file_namespace(
            &repo.repository_id,
            change("delete", "Cafe\u{301}/\u{1100}\u{1161}", None),
            "admin",
            &new_id("op"),
        )
        .unwrap();
    let remaining = engine.list_tree(&repo.main_branch_id).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].path, "Café");
}

#[test]
fn canonical_equivalent_workspace_source_survives_reopen_before_commit() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("system");
    let mut engine = Engine::initialize(
        &root,
        Config::development([
            temporary.path().join("primary"),
            temporary.path().join("backup"),
            temporary.path().join("tertiary"),
        ]),
    )
    .unwrap();
    let repo = engine
        .create_repository("Persisted query", "P01", 1, "admin", &new_id("op"))
        .unwrap();
    let initial = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&initial.workspace_id, "Café/Å.txt", b"unchanged body")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: initial.workspace_id,
            operation_id: new_id("op"),
            principal: "admin".into(),
            expected_head: initial.base_head,
            fencing_token: initial.fencing_token,
            message: "initial".into(),
            request_digest: None,
        })
        .unwrap();
    let original = engine
        .list_tree(&repo.main_branch_id)
        .unwrap()
        .into_iter()
        .find(|entry| entry.path == "Café/Å.txt")
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .namespace_operation(
            &workspace.workspace_id,
            NamespaceOperation::parse(
                storage_contract::NamespaceOperationKind::Rename,
                "Cafe\u{301}/A\u{30a}.txt",
                Some("Café/가.txt"),
            )
            .unwrap(),
        )
        .unwrap();
    drop(engine);
    let mut engine = Engine::open(&root).unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: new_id("op"),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "rename persisted source query".into(),
            request_digest: None,
        })
        .unwrap();
    let entries = engine.list_tree(&repo.main_branch_id).unwrap();
    assert!(entries.iter().any(|entry| entry.path == "Café"));
    let renamed = entries
        .iter()
        .find(|entry| entry.path == "Café/가.txt")
        .unwrap();
    assert_eq!(renamed.name(), "가.txt");
    assert_eq!(renamed.entry_id, original.entry_id);
    assert_eq!(renamed.file_id, original.file_id);
    assert_eq!(renamed.version_id, original.version_id);
}
