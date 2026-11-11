use super::*;
use storage_contract::{BranchName, DirectoryTreeId};

fn reviewed(engine: &Engine, repo: &Repository, path: &str) -> DirectoryBranchCreateSource {
    let status = status(engine, repo, path);
    DirectoryBranchCreateSource {
        repository_id: repo.repository_id.clone(),
        directory_id: status.directory_id,
        directory_branch_id: status.directory_branch_id.unwrap(),
        working_tree_id: status.working_tree_id.unwrap(),
        head_tree_id: status.head_tree_id.unwrap(),
        index_generation: status.index_generation,
        namespace_head: engine.state().branches[&repo.main_branch_id].head_commit_id.clone(),
    }
}

fn source(
    engine: &Engine,
    repo: &Repository,
    path: &str,
    tree: &str,
) -> HistoricalDirectoryBranchSource {
    HistoricalDirectoryBranchSource::new(reviewed(engine, repo, path), tree.parse().unwrap())
        .unwrap()
}

fn create_history(
    engine: &mut Engine,
    source: &HistoricalDirectoryBranchSource,
    path: Option<&str>,
    name: &str,
    operation: OperationId,
) -> Result<OperationResult> {
    let path = path.map(|value| name_policy::LookupPath::parse(value).unwrap());
    engine.create_directory_branch_from_history(
        source,
        path.as_ref(),
        &BranchName::parse(name).unwrap(),
        &"admin".parse().unwrap(),
        operation,
    )
}

#[test]
fn historical_registration_uses_exact_commit_without_checkout_and_replays_after_restart() {
    let (temp, mut engine, repo) = fixture();
    save(&mut engine, &repo, "file", b"old committed bytes");
    initialize(&mut engine, &repo, "");
    let old = commit(&mut engine, &repo, "");
    let tree = engine.state().directory_trees[&old].clone();
    save(&mut engine, &repo, "file", b"current committed bytes");
    let current_head = commit(&mut engine, &repo, "");
    let source = source(&engine, &repo, "", &old);
    let namespace = engine.list_tree(&repo.main_branch_id).unwrap();
    let current = engine.state().directory_git_repositories[&repo.repository_id].clone();
    let branches = engine.state().branches.clone();
    let versions = engine.state().versions.clone();
    let selections = engine.state().directory_selections.clone();
    let operation = OperationId::generate().unwrap();
    let result = create_history(&mut engine, &source, None, "historical", operation).unwrap();
    let id = result.result["directory_branch_id"].as_str().unwrap().to_owned();
    assert_eq!(result.kind, "directory.git.branch-create-history");
    assert_eq!(result.result["head_tree_id"], old);
    assert_eq!(result.result["selected"], false);
    assert_eq!(result.result["materialized"], false);
    assert_eq!(result.result["copied_files"], "0");
    assert_eq!(result.result["placeholder_files"], "0");
    let branch = &engine.state().directory_branches[&id];
    assert_eq!(branch.entries, tree.entries);
    assert_eq!(branch.file_branches, tree.file_branches);
    assert_eq!(branch.directory_refs, tree.directory_refs);
    assert_eq!(branch.commit_id, tree.commit_id);
    assert_eq!(branch.parent_branch_id.as_ref(), Some(&tree.directory_branch_id));
    assert!(!branch.materialized);
    for (path, context) in &branch.file_contexts {
        let original = &tree.file_contexts[path];
        assert_eq!(context.branch_id, original.branch_id);
        assert_eq!(context.node_id, original.node_id);
        assert_eq!(context.selection_generation, 0);
        assert_eq!(context.inherited_from.as_ref(), Some(&id));
    }
    let git = &engine.state().directory_git_repositories[&repo.repository_id];
    assert_eq!(git.current_branch_id, current.current_branch_id);
    assert_eq!(git.main_branch_id, current.main_branch_id);
    assert_eq!(git.branches[&id].head_tree_id.as_ref(), Some(&old));
    assert_eq!(git.branches[&id].index_tree_id, old);
    assert_eq!(git.branches[&id].index_generation, 1);
    assert_eq!(
        git.branches[&current.current_branch_id],
        current.branches[&current.current_branch_id],
    );
    assert_eq!(engine.state().branches, branches);
    assert_eq!(engine.state().versions, versions);
    assert_eq!(engine.state().directory_selections, selections);
    assert_eq!(engine.list_tree(&repo.main_branch_id).unwrap(), namespace);
    assert_eq!(bytes(&engine, &repo, "file"), b"current committed bytes");
    let changed_source = HistoricalDirectoryBranchSource::new(
        source.current().clone(), current_head.parse().unwrap(),
    ).unwrap();
    assert_eq!(
        create_history(&mut engine, &changed_source, None, "historical", operation)
            .unwrap_err().code(),
        ErrorCode::IdempotencyKeyReused,
    );
    save(&mut engine, &repo, "file", b"later unsaved review");
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(
        create_history(&mut engine, &source, None, "historical", operation).unwrap(),
        result,
    );
    assert_eq!(bytes(&engine, &repo, "file"), b"later unsaved review");
    assert_eq!(engine.state().directory_branches[&id].head_tree_id, old);
}

#[test]
fn historical_registration_rejects_working_unknown_cross_directory_and_stale_sources() {
    let (_temp, mut engine, repo) = fixture();
    save(&mut engine, &repo, "dir/file", b"directory");
    save(&mut engine, &repo, "other/file", b"other directory");
    initialize(&mut engine, &repo, "dir");
    let working = status(&engine, &repo, "dir").working_tree_id.unwrap();
    let head = commit(&mut engine, &repo, "dir");
    initialize(&mut engine, &repo, "other");
    let other = commit(&mut engine, &repo, "other");
    let other_repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Other historical source").unwrap(),
            "P01", 1, "admin", OperationId::generate().unwrap(),
        )
        .unwrap();
    assert_ne!(other_repo.repository_id, repo.repository_id);
    save(&mut engine, &other_repo, "file", b"other repository");
    initialize(&mut engine, &other_repo, "");
    let foreign = commit(&mut engine, &other_repo, "");
    for tree in [working, other, foreign, DirectoryTreeId::generate().unwrap().to_string()] {
        let source = source(&engine, &repo, "dir", &tree);
        let before = engine.state().clone();
        let operation = OperationId::generate().unwrap();
        assert_eq!(
            create_history(&mut engine, &source, Some("dir"), "rejected", operation)
                .unwrap_err().code(),
            ErrorCode::DirectoryBranchConflict,
        );
        assert_eq!(engine.state(), &before);
        assert!(!engine.state().operations.contains_key(&operation));
    }
    let reviewed = reviewed(&engine, &repo, "dir");
    let mut unknown = reviewed.clone();
    unknown.repository_id = storage_contract::RepositoryId::generate().unwrap().to_string();
    let unknown = HistoricalDirectoryBranchSource::new(unknown, head.parse().unwrap()).unwrap();
    let before = engine.state().clone();
    assert_eq!(
        create_history(
            &mut engine, &unknown, Some("dir"), "unknown", OperationId::generate().unwrap(),
        ).unwrap_err().code(),
        ErrorCode::RepositoryNotFound,
    );
    assert_eq!(engine.state(), &before);
    for field in ["directory", "branch", "working", "head", "index", "namespace"] {
        let mut changed = reviewed.clone();
        match field {
            "directory" => {
                changed.directory_id = storage_contract::EntryId::generate().unwrap().to_string();
            }
            "branch" => {
                changed.directory_branch_id = storage_contract::DirectoryBranchId::generate()
                    .unwrap().to_string();
            }
            "working" => changed.working_tree_id = DirectoryTreeId::generate().unwrap().to_string(),
            "head" => changed.head_tree_id = DirectoryTreeId::generate().unwrap().to_string(),
            "index" => changed.index_generation += 1,
            "namespace" => {
                changed.namespace_head = storage_contract::CommitId::generate()
                    .unwrap()
                    .to_string();
            }
            _ => unreachable!(),
        }
        let source = HistoricalDirectoryBranchSource::new(changed, head.parse().unwrap()).unwrap();
        let before = engine.state().clone();
        assert_eq!(
            create_history(
                &mut engine, &source, Some("dir"), "stale", OperationId::generate().unwrap(),
            ).unwrap_err().code(),
            ErrorCode::DirectoryBranchConflict,
            "{field}",
        );
        assert_eq!(engine.state(), &before);
    }
}

#[test]
fn historical_registration_preserves_old_child_pin_and_dirty_archived_child_workspace() {
    let (temp, mut engine, repo) = fixture();
    save(&mut engine, &repo, "owned", b"old parent");
    save(&mut engine, &repo, "child/file", b"old child");
    initialize(&mut engine, &repo, "child");
    let old_child = commit(&mut engine, &repo, "child");
    initialize(&mut engine, &repo, "");
    stage_child(&mut engine, &repo, "", "child");
    let old_parent = commit(&mut engine, &repo, "");
    let historical = engine.state().directory_trees[&old_parent].clone();
    let entry = engine.namespace_tree(&repo.main_branch_id).unwrap()
        .get("child/file").unwrap().clone();
    let old_branch = engine.file_context(&repo.repository_id, "child/file").unwrap().branch_id;
    let alternate = engine.create_file_branch(
        &entry.file_id, &entry.version_id, &BranchName::parse("child current").unwrap(),
        "admin", OperationId::generate().unwrap(),
    ).unwrap().branch_id.unwrap();
    engine.select_file_branch_at(
        &repo.repository_id, &entry.path, &entry.file_id, &alternate,
        "admin", OperationId::generate().unwrap(),
    ).unwrap();
    save(&mut engine, &repo, "child/file", b"child advanced");
    let child_head = commit(&mut engine, &repo, "child");
    engine.archive_branch(&old_branch, "admin", OperationId::generate().unwrap()).unwrap();
    stage_child(&mut engine, &repo, "", "child");
    save(&mut engine, &repo, "owned", b"parent advanced");
    commit(&mut engine, &repo, "");
    save(&mut engine, &repo, "child/file", b"dirty independent child");
    let child_status = status(&engine, &repo, "child");
    assert!(child_status.unstaged);
    assert_ne!(child_head, old_child);
    let source = source(&engine, &repo, "", &old_parent);
    let operation = OperationId::generate().unwrap();
    let result = create_history(&mut engine, &source, None, "old parent", operation).unwrap();
    let id = result.result["directory_branch_id"].as_str().unwrap();
    let branch = &engine.state().directory_branches[id];
    assert_eq!(branch.entries, historical.entries);
    assert_eq!(branch.directory_refs, historical.directory_refs);
    let index = &engine.state().directory_git_repositories[&repo.repository_id].branches[id];
    assert_eq!(index.index_children.len(), 1);
    assert_eq!(index.index_children[&child_status.directory_id].tree_id.as_ref(), Some(&old_child));
    assert_eq!(index.index_boundaries[&child_status.directory_id], "child");
    assert_eq!(status(&engine, &repo, "child"), child_status);
    assert_eq!(bytes(&engine, &repo, "child/file"), b"dirty independent child");
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(
        create_history(&mut engine, &source, None, "old parent", operation).unwrap(),
        result,
    );
    assert_eq!(status(&engine, &repo, "child"), child_status);
}

#[test]
fn historical_registration_keeps_owned_dirty_and_ownership_restage_fences() {
    let (_temp, mut engine, repo) = fixture();
    save(&mut engine, &repo, "owned", b"old");
    save(&mut engine, &repo, "child/file", b"ordinary child");
    initialize(&mut engine, &repo, "");
    let old = commit(&mut engine, &repo, "");
    save(&mut engine, &repo, "owned", b"unstaged");
    for staged in [false, true] {
        if staged {
            stage_owned(&mut engine, &repo, "");
        }
        let source = source(&engine, &repo, "", &old);
        let before = engine.state().clone();
        assert_eq!(
            create_history(&mut engine, &source, None, "dirty", OperationId::generate().unwrap())
                .unwrap_err().code(),
            ErrorCode::DirectoryBranchConflict,
        );
        assert_eq!(engine.state(), &before);
    }
    commit(&mut engine, &repo, "");
    initialize(&mut engine, &repo, "child");
    assert!(status(&engine, &repo, "").needs_restage);
    let source = source(&engine, &repo, "", &old);
    let before = engine.state().clone();
    assert_eq!(
        create_history(&mut engine, &source, None, "restage", OperationId::generate().unwrap())
            .unwrap_err().code(),
        ErrorCode::DirectoryBranchConflict,
    );
    assert_eq!(engine.state(), &before);
}
