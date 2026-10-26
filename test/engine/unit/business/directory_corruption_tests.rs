//! Corrupt persisted references must fail before publishing any mutation.
use super::*;

fn fixture() -> (tempfile::TempDir, Engine, Repository, String) {
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
        .create_disk_root("P01", "admin", &new_id("op"))
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "folder/file.txt", b"data")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: new_id("op"),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "fixture".into(),
            request_digest: None,
        })
        .unwrap();
    let branch = engine
        .create_directory_branches(
            &repo.repository_id,
            "folder",
            "draft",
            "admin",
            &new_id("op"),
        )
        .unwrap()
        .result["directory_branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    (temporary, engine, repo, branch)
}

#[test]
fn missing_directory_selection_references_return_errors_without_publication() {
    for corruption in [
        "branch",
        "commit",
        "tree",
        "file",
        "descendant_branch",
        "descendant_tree",
    ] {
        let (_temporary, mut engine, repo, selected) = fixture();
        match corruption {
            "branch" => {
                std::sync::Arc::make_mut(&mut engine.state)
                    .branches
                    .remove(&repo.main_branch_id);
            }
            "commit" => {
                let head = engine.state.branches[&repo.main_branch_id]
                    .head_commit_id
                    .clone();
                std::sync::Arc::make_mut(&mut engine.state)
                    .commits
                    .remove(&head);
            }
            "tree" => {
                let tree = engine.state.directory_branches[&selected]
                    .head_tree_id
                    .clone();
                std::sync::Arc::make_mut(&mut engine.state)
                    .directory_trees
                    .remove(&tree);
            }
            "file" => {
                std::sync::Arc::make_mut(&mut engine.state)
                    .directory_branches
                    .get_mut(&selected)
                    .unwrap()
                    .file_branches
                    .clear();
            }
            "descendant_branch" | "descendant_tree" => {
                let target = std::sync::Arc::make_mut(&mut engine.state)
                    .directory_branches
                    .get_mut(&selected)
                    .unwrap();
                target.directory_refs.insert(
                    "corrupt".into(),
                    DirectoryReference {
                        directory_branch_id: if corruption == "descendant_branch" {
                            "missing".into()
                        } else {
                            selected.clone()
                        },
                        tree_id: if corruption == "descendant_tree" {
                            "missing".into()
                        } else {
                            target.head_tree_id.clone()
                        },
                    },
                );
            }
            _ => unreachable!(),
        }
        let before = canonical_json(engine.state()).unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            engine.select_directory_branches(
                &repo.repository_id,
                "folder",
                "draft",
                "admin",
                &new_id("op"),
            )
        }));
        if matches!(corruption, "branch" | "commit") {
            assert!(result.is_err(), "{corruption}: admitted root must exist");
            assert_eq!(canonical_json(engine.state()).unwrap(), before);
            continue;
        }
        let error = result.unwrap().unwrap_err();
        assert_eq!(
            error.code(),
            storage_contract::ErrorCode::StateInvalid,
            "{corruption}"
        );
        assert_eq!(
            canonical_json(engine.state()).unwrap(),
            before,
            "{corruption}"
        );
    }
}

#[test]
fn missing_baseline_branch_or_tree_cannot_initialize_or_fork() {
    for missing in ["branch", "tree"] {
        let (_temporary, mut engine, repo, _) = fixture();
        let (directory, _, _) = scope(&engine.state, &repo.repository_id, "folder").unwrap();
        if missing == "branch" {
            let branch = engine.state.directory_selections[&directory].clone();
            std::sync::Arc::make_mut(&mut engine.state)
                .directory_branches
                .remove(&branch);
        } else {
            let tree = engine.state.directory_tree_selections[&directory].clone();
            std::sync::Arc::make_mut(&mut engine.state)
                .directory_trees
                .remove(&tree);
        }
        let before = canonical_json(engine.state()).unwrap();
        let error = engine
            .initialize_directory_scope(&repo.repository_id, "folder", "admin", &new_id("op"))
            .unwrap_err();
        assert_eq!(error.code(), storage_contract::ErrorCode::StateInvalid);
        let error = engine
            .create_directory_branches(
                &repo.repository_id,
                "folder",
                "another",
                "admin",
                &new_id("op"),
            )
            .unwrap_err();
        assert_eq!(error.code(), storage_contract::ErrorCode::StateInvalid);
        assert_eq!(canonical_json(engine.state()).unwrap(), before);
    }
}

#[test]
fn unchanged_selection_still_rejects_unavailable_file_branch() {
    let (_temporary, mut engine, repo, selected) = fixture();
    engine
        .select_directory_branches(
            &repo.repository_id,
            "folder",
            "draft",
            "admin",
            &new_id("op"),
        )
        .unwrap();
    let file_branch = engine.state.directory_branches[&selected]
        .file_branches
        .values()
        .next()
        .unwrap()
        .clone();
    std::sync::Arc::make_mut(&mut engine.state)
        .branches
        .remove(&file_branch);
    let before = canonical_json(engine.state()).unwrap();
    let error = engine
        .select_directory_branches(
            &repo.repository_id,
            "folder",
            "draft",
            "admin",
            &new_id("op"),
        )
        .unwrap_err();
    assert_eq!(
        error.code(),
        storage_contract::ErrorCode::FileBranchNotFound
    );
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
}
