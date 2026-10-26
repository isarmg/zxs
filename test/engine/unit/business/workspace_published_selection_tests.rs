//! Published workspaces retain the destination of files absent from their base.
use super::*;

fn fixture() -> (tempfile::TempDir, Engine, Repository) {
    let temporary = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temporary.path().join("system"),
        Config::development([temporary.path().join("primary")]),
    )
    .unwrap();
    let repo = engine
        .create_repository("Published selections", "P01", 1, "admin", &new_id("op"))
        .unwrap();
    (temporary, engine, repo)
}

fn save(engine: &mut Engine, branch: &str, bytes: &[u8]) -> Workspace {
    let workspace = engine.create_workspace(branch, "admin").unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "fresh.txt", bytes)
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id.clone(),
            operation_id: new_id("op"),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "published selection".into(),
            request_digest: None,
        })
        .unwrap();
    engine.workspace(&workspace.workspace_id).unwrap()
}

#[test]
fn published_new_file_context_fences_same_version_branch_switch() {
    for reopen in [false, true] {
        let (temporary, mut engine, repo) = fixture();
        let workspace = save(&mut engine, &repo.main_branch_id, b"old");
        let key = directory::context_key_from_key(&repo.repository_id, "fresh.txt");
        assert!(
            find_unordered(
                &engine.state().commits[&workspace.base_head].entries,
                "fresh.txt"
            )
            .is_none()
        );
        assert_eq!(workspace.state, WorkspaceState::Committed);
        assert_eq!(
            workspace.selected_file_contexts.get(&key),
            engine
                .file_context(&repo.repository_id, "fresh.txt")
                .as_ref()
        );
        assert!(workspace.selected_file_contexts.contains_key(&key));
        if reopen {
            drop(engine);
            engine = Engine::open(&temporary.path().join("system")).unwrap();
        }
        engine
            .validate_workspace_file_selection(&workspace.workspace_id, "fresh.txt")
            .unwrap();
        let source = engine.list_tree(&repo.main_branch_id).unwrap().remove(0);
        let draft = engine
            .create_file_branch(
                &source.file_id,
                &source.version_id,
                "same version draft",
                "admin",
                &new_id("op"),
            )
            .unwrap()
            .branch_id
            .unwrap();
        let directory_generations = engine.state().directory_selection_generations.clone();
        engine
            .select_file_branch(&source.file_id, &draft, "admin", &new_id("op"))
            .unwrap();
        let selected = engine.list_tree(&repo.main_branch_id).unwrap().remove(0);
        assert_eq!(
            (&selected.entry_id, &selected.version_id, &selected.path),
            (&source.entry_id, &source.version_id, &source.path)
        );
        assert_eq!(
            engine.state().directory_selection_generations,
            directory_generations
        );
        assert_eq!(
            engine.pending_file_source(&draft, &source.file_id),
            Some(source.version_id.as_str())
        );
        assert_ne!(
            workspace.selected_file_contexts.get(&key),
            engine
                .file_context(&repo.repository_id, "fresh.txt")
                .as_ref()
        );
        assert_eq!(
            engine
                .validate_workspace_file_selection(&workspace.workspace_id, "fresh.txt")
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::FileBranchConflict
        );
        assert_eq!(
            engine
                .workspace(&workspace.workspace_id)
                .unwrap()
                .selected_file_contexts,
            workspace.selected_file_contexts
        );
    }
}

#[test]
fn published_workspace_uses_post_save_context_and_fences_selection_round_trips() {
    let (_temporary, mut engine, repo) = fixture();
    let first = save(&mut engine, &repo.main_branch_id, b"old");
    engine
        .validate_workspace_file_selection(&first.workspace_id, "fresh.txt")
        .unwrap();
    let source = engine.list_tree(&repo.main_branch_id).unwrap().remove(0);
    let draft = engine
        .create_file_branch(
            &source.file_id,
            &source.version_id,
            "first write",
            "admin",
            &new_id("op"),
        )
        .unwrap()
        .branch_id
        .unwrap();
    engine
        .select_file_branch(&source.file_id, &draft, "admin", &new_id("op"))
        .unwrap();
    let mut completed = first;
    for bytes in [b"one".as_slice(), b"two".as_slice()] {
        completed = save(&mut engine, &repo.main_branch_id, bytes);
        assert_eq!(
            completed
                .selected_file_contexts
                .get(&directory::context_key_from_key(
                    &repo.repository_id,
                    "fresh.txt"
                )),
            engine
                .file_context(&repo.repository_id, "fresh.txt")
                .as_ref()
        );
        engine
            .validate_workspace_file_selection(&completed.workspace_id, "fresh.txt")
            .unwrap();
        let next = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        assert_eq!(
            next.selected_file_contexts,
            completed.selected_file_contexts
        );
        engine.abort_workspace(&next.workspace_id, "admin").unwrap();
    }
    let current = engine.list_tree(&repo.main_branch_id).unwrap().remove(0);
    let alternative = engine
        .create_file_branch(
            &current.file_id,
            &current.version_id,
            "round trip",
            "admin",
            &new_id("op"),
        )
        .unwrap()
        .branch_id
        .unwrap();
    for selected in [&alternative, &draft] {
        engine
            .select_file_branch(&current.file_id, selected, "admin", &new_id("op"))
            .unwrap();
    }
    assert_eq!(
        engine.list_tree(&repo.main_branch_id).unwrap()[0].version_id,
        current.version_id
    );
    assert_eq!(
        engine
            .file_context(&repo.repository_id, "fresh.txt")
            .unwrap()
            .branch_id,
        draft
    );
    assert_eq!(
        engine
            .validate_workspace_file_selection(&completed.workspace_id, "fresh.txt")
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::FileBranchConflict
    );
}

#[test]
fn recovery_refreshes_both_precompletion_mirrors_from_original_publication() {
    for wire in [false, true] {
        let (temporary, mut engine, repo) = fixture();
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, "fresh.txt", b"old")
            .unwrap();
        let body = wire.then(|| {
            let mut body = engine
                .seal_workspace(&workspace.workspace_id, "admin")
                .unwrap();
            body["request_digest"] =
                json!(crate::wire::commit_request_digest(&workspace.workspace_id, &body).unwrap());
            body
        });
        let request = CommitRequest {
            workspace_id: workspace.workspace_id.clone(),
            operation_id: body
                .as_ref()
                .map(|body| body["operation_id"].as_str().unwrap().to_owned())
                .unwrap_or_else(|| new_id("op")),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: String::new(),
            request_digest: None,
        };
        let before = engine.workspace(&workspace.workspace_id).unwrap();
        let manifests = [
            engine
                .root()
                .join("workspaces")
                .join(&workspace.workspace_id)
                .join("workspace.json"),
            engine.config().disks[0]
                .root
                .join("staging/workspaces")
                .join(&workspace.workspace_id)
                .join("workspace.json"),
        ];
        let original_manifests = manifests.clone().map(|path| std::fs::read(path).unwrap());
        let key = directory::context_key_from_key(&repo.repository_id, "fresh.txt");
        assert!(!before.selected_file_contexts.contains_key(&key));
        engine.inject_fault_once("after_local_publications");
        let result = if let Some(body) = body.clone() {
            engine.commit_workspace_wire(request.clone(), body)
        } else {
            engine.commit_workspace(request.clone())
        };
        assert_eq!(
            result.unwrap_err().code(),
            storage_contract::ErrorCode::InDoubt
        );
        assert_eq!(
            manifests.clone().map(|path| std::fs::read(path).unwrap()),
            original_manifests
        );
        drop(engine);

        let root = temporary.path().join("system");
        let mut engine = Engine::open(&root).unwrap();
        let completed = engine.workspace(&workspace.workspace_id).unwrap();
        let published_context = engine
            .file_context(&repo.repository_id, "fresh.txt")
            .unwrap();
        assert_eq!(completed.state, WorkspaceState::Committed);
        assert_eq!(completed.fencing_token, before.fencing_token + 1);
        assert_eq!(
            completed.selected_file_contexts.get(&key),
            Some(&published_context)
        );
        engine
            .validate_workspace_file_selection(&workspace.workspace_id, "fresh.txt")
            .unwrap();
        // Recovery does not rewrite either manifest. A later reopen must still
        // use the original publication, not a newer same-content selection.
        assert_eq!(
            manifests.clone().map(|path| std::fs::read(path).unwrap()),
            original_manifests
        );
        let source = engine.list_tree(&repo.main_branch_id).unwrap().remove(0);
        let draft = engine
            .create_file_branch(
                &source.file_id,
                &source.version_id,
                "later draft",
                "admin",
                &new_id("op"),
            )
            .unwrap()
            .branch_id
            .unwrap();
        engine
            .select_file_branch(&source.file_id, &draft, "admin", &new_id("op"))
            .unwrap();
        assert_ne!(
            engine
                .file_context(&repo.repository_id, "fresh.txt")
                .unwrap(),
            published_context
        );
        drop(engine);

        let mut engine = Engine::open(&root).unwrap();
        let recovered = engine.workspace(&workspace.workspace_id).unwrap();
        assert_eq!(
            recovered.selected_file_contexts,
            completed.selected_file_contexts
        );
        assert_eq!(
            engine
                .validate_workspace_file_selection(&workspace.workspace_id, "fresh.txt")
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::FileBranchConflict
        );
        let replayed = if let Some(body) = body {
            engine.commit_workspace_wire(request.clone(), body)
        } else {
            engine.commit_workspace(request.clone())
        }
        .unwrap();
        assert_eq!(replayed.operation_id, request.operation_id);
        assert_eq!(
            replayed.commit_state,
            storage_contract::CommitState::Published
        );
        assert_eq!(
            manifests.map(|path| std::fs::read(path).unwrap()),
            original_manifests
        );
    }
}
