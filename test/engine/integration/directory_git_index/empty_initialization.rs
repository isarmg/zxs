//! Empty directories are real repositories, not a fixture seeded by a file save.
//! Exercise user-visible publication through the existing public Engine fixture.
use super::*;
use storage_contract::{NamespaceOperation, NamespaceOperationKind};

#[test]
fn empty_root_initialization_and_first_file_keep_work_index_and_head_distinct() {
    let (temp, mut engine, repo) = fixture();
    let ordinary = status(&engine, &repo, "");
    assert!(!ordinary.initialized);
    assert!(engine.list_tree(&repo.main_branch_id).unwrap().is_empty());
    let body = DirectoryGitInitializeBody {
        path: String::new(),
        expected_namespace_head: engine.state().branches[&repo.main_branch_id]
            .head_commit_id
            .clone(),
        operation_id: OperationId::generate().unwrap(),
    };
    let initialized = engine
        .initialize_directory_git(&repo.repository_id, &body, "admin")
        .unwrap();
    let unborn = status(&engine, &repo, "");
    assert!(unborn.initialized && unborn.staged && !unborn.unstaged);
    assert!(unborn.head_tree_id.is_none());
    assert_eq!((unborn.added, unborn.modified, unborn.deleted), (0, 0, 0));
    assert!(engine.state().versions.is_empty());
    let main = engine.state().directory_git_repositories[&repo.repository_id]
        .main_branch_id
        .clone();
    assert_eq!(unborn.directory_branch_id.as_deref(), Some(main.as_str()));
    assert!(
        engine
            .directory_git_main_workspace(&repo.repository_id, &repo.repository_id)
            .unwrap()
            .entries
            .is_empty()
    );

    // An unborn empty index must still produce a genuine first user commit.
    let operation = OperationId::generate().unwrap();
    let first = commit_reviewed(&mut engine, &repo, &unborn, operation).unwrap();
    assert_eq!(first.result["no_change"], false);
    let empty_head = first.result["head_tree_id"].as_str().unwrap().to_owned();
    let empty_tree = engine.state().directory_trees[&empty_head].clone();
    assert!(empty_tree.entries.is_empty() && empty_tree.directory_refs.is_empty());
    assert!(empty_tree.parent_tree_id.is_none());
    let marker = &engine.state().commits[&empty_tree.commit_id];
    assert_eq!(marker.kind, "directory_commit");
    assert_eq!(marker.provenance.as_deref(), Some(empty_head.as_str()));
    assert!(marker.parent_commit_ids.is_empty());
    assert!(!status(&engine, &repo, "").staged);

    save(&mut engine, &repo, "first.txt", b"first saved contents");
    let working = status(&engine, &repo, "");
    assert!(!working.staged && working.unstaged);
    assert_eq!(working.head_tree_id.as_deref(), Some(empty_head.as_str()));
    assert_eq!(bytes(&engine, &repo, "first.txt"), b"first saved contents");
    let no_change = commit_reviewed(
        &mut engine,
        &repo,
        &working,
        OperationId::generate().unwrap(),
    )
    .unwrap();
    assert_eq!(no_change.result["no_change"], true);
    assert_eq!(no_change.result["head_tree_id"], empty_head);
    assert!(status(&engine, &repo, "").unstaged);

    stage_owned(&mut engine, &repo, "");
    let reviewed = status(&engine, &repo, "");
    assert!(reviewed.staged && !reviewed.unstaged);
    assert_eq!(
        (reviewed.added, reviewed.modified, reviewed.deleted),
        (1, 0, 0)
    );
    let saved = commit_reviewed(
        &mut engine,
        &repo,
        &reviewed,
        OperationId::generate().unwrap(),
    )
    .unwrap();
    let saved_head = saved.result["head_tree_id"].as_str().unwrap();
    let saved_tree = &engine.state().directory_trees[saved_head];
    assert_eq!(
        saved_tree.parent_tree_id.as_deref(),
        Some(empty_head.as_str())
    );
    assert_eq!(saved_tree.entries.len(), 1);
    assert_eq!(saved_tree.entries[0].path(), "first.txt");
    assert_eq!(
        version_bytes(&engine, &saved_tree.entries[0].version_id),
        b"first saved contents"
    );
    assert_eq!(engine.state().directory_trees[&empty_head], empty_tree);
    let complete = status(&engine, &repo, "");
    assert!(!complete.staged && !complete.unstaged && !complete.needs_restage);

    drop(engine);
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(status(&engine, &repo, ""), complete);
    assert_eq!(engine.state().directory_trees[&empty_head], empty_tree);
    assert_eq!(
        engine.state().directory_git_repositories[&repo.repository_id].main_branch_id,
        main
    );
    let before = engine.state().clone();
    assert_eq!(
        engine
            .initialize_directory_git(&repo.repository_id, &body, "admin")
            .unwrap(),
        initialized
    );
    assert_eq!(
        commit_reviewed(&mut engine, &repo, &unborn, operation).unwrap(),
        first
    );
    assert_eq!(engine.state(), &before);
    assert_eq!(bytes(&engine, &repo, "first.txt"), b"first saved contents");
}

#[test]
fn empty_child_repository_is_pinned_only_after_its_own_explicit_commit() {
    let (temp, mut engine, repo) = fixture();
    engine
        .file_namespace(
            &repo.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::Mkdir, "child", None).unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let ordinary = status(&engine, &repo, "child");
    assert!(!ordinary.initialized);
    assert!(engine.state().directory_git_repositories.is_empty());
    assert!(engine.state().versions.is_empty());
    initialize(&mut engine, &repo, "");
    let parent_before = commit(&mut engine, &repo, "");
    assert!(!status(&engine, &repo, "child").initialized);

    initialize(&mut engine, &repo, "child");
    let child = status(&engine, &repo, "child");
    assert_eq!(child.directory_id, ordinary.directory_id);
    assert!(child.initialized && child.staged && child.head_tree_id.is_none());
    assert!(!child.unstaged && !child.needs_restage);
    assert!(
        engine
            .directory_git_main_workspace(&repo.repository_id, &child.directory_id)
            .unwrap()
            .entries
            .is_empty()
    );
    let boundary = status(&engine, &repo, "");
    assert!(boundary.needs_restage);
    assert_eq!(
        boundary.head_tree_id.as_deref(),
        Some(parent_before.as_str())
    );
    assert_eq!(boundary.children.len(), 1);
    assert_eq!(boundary.children[0].directory_id, child.directory_id);
    assert!(boundary.children[0].head_tree_id.is_none());
    assert!(boundary.children[0].staged_tree_id.is_none());
    assert!(boundary.children[0].committed_tree_id.is_none());

    let child_empty = commit(&mut engine, &repo, "child");
    let empty_tree = engine.state().directory_trees[&child_empty].clone();
    assert!(empty_tree.entries.is_empty());
    assert!(empty_tree.parent_tree_id.is_none());
    let parent = status(&engine, &repo, "");
    assert!(parent.children[0].reference_changed);
    assert!(parent.children[0].staged_tree_id.is_none());
    stage_owned(&mut engine, &repo, "");
    stage_child(&mut engine, &repo, "", "child");
    let pinned = commit(&mut engine, &repo, "");
    let parent_tree = engine.state().directory_trees[&pinned].clone();
    assert_eq!(
        parent_tree.directory_refs[&child.directory_id].tree_id,
        child_empty
    );
    assert_eq!(parent_tree.directory_refs.len(), 1);

    // Saving into the once-empty child does not create a child or parent commit.
    save(
        &mut engine,
        &repo,
        "child/first.txt",
        b"child saved contents",
    );
    let child_dirty = status(&engine, &repo, "child");
    assert!(!child_dirty.staged && child_dirty.unstaged);
    assert_eq!(
        child_dirty.head_tree_id.as_deref(),
        Some(child_empty.as_str())
    );
    let parent = status(&engine, &repo, "");
    assert!(!parent.staged && !parent.unstaged && !parent.needs_restage);
    assert!(parent.children[0].dirty);
    assert!(!parent.children[0].reference_changed);
    assert_eq!(parent.head_tree_id.as_deref(), Some(pinned.as_str()));
    assert_eq!(
        parent.children[0].committed_tree_id.as_deref(),
        Some(child_empty.as_str())
    );

    let child_full = commit(&mut engine, &repo, "child");
    let parent = status(&engine, &repo, "");
    assert!(!parent.staged && parent.children[0].reference_changed);
    assert!(!parent.children[0].dirty);
    assert_eq!(
        parent.children[0].staged_tree_id.as_deref(),
        Some(child_empty.as_str())
    );
    assert_eq!(engine.state().directory_trees[&pinned], parent_tree);
    stage_child(&mut engine, &repo, "", "child");
    let reviewed = status(&engine, &repo, "");
    let result = commit_reviewed(
        &mut engine,
        &repo,
        &reviewed,
        OperationId::generate().unwrap(),
    )
    .unwrap();
    let head = result.result["head_tree_id"].as_str().unwrap();
    assert_eq!(
        engine.state().directory_trees[head].directory_refs[&child.directory_id].tree_id,
        child_full
    );
    let parent_after = status(&engine, &repo, "");
    let child_after = status(&engine, &repo, "child");
    assert!(!parent_after.staged && !parent_after.unstaged && !parent_after.needs_restage);
    assert!(!parent_after.children[0].dirty && !parent_after.children[0].reference_changed);

    drop(engine);
    let engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(status(&engine, &repo, ""), parent_after);
    assert_eq!(status(&engine, &repo, "child"), child_after);
    assert_eq!(engine.state().directory_trees[&pinned], parent_tree);
    assert_eq!(engine.state().directory_trees[&child_empty], empty_tree);
    assert_eq!(
        bytes(&engine, &repo, "child/first.txt"),
        b"child saved contents"
    );
}
