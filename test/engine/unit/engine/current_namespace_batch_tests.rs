//! Current batch consumers. Most cases use the parent's verified projection;
//! the final lifecycle case runs the actual D3/archive/retirement/reopen path.
//! No fixture here certifies physical power-loss or HTTP authorization behavior.
use super::*;
use std::{fs, os::unix::fs::PermissionsExt, path::Path};

fn current(fixture: &Fixture, path: &str) -> Entry {
    fixture
        .engine
        .namespace_tree(&fixture.repo.main_branch_id)
        .unwrap()
        .get(path)
        .unwrap()
        .clone()
}

fn initialize(fixture: &mut Fixture, path: &str) -> String {
    crate::directory_git_fixture::checkpoint(&mut fixture.engine, &fixture.repo, path)
        .directory_branch_id
        .unwrap()
}

fn selected(fixture: &Fixture, path: &str) -> String {
    let directory = if path.is_empty() {
        fixture.repo.repository_id.clone()
    } else {
        current(fixture, path).entry_id
    };
    fixture.engine.state.directory_selections[&directory].clone()
}

fn item(
    fixture: &Fixture,
    source_branch: &str,
    source_path: &str,
    target: Option<(&str, &str)>,
) -> DirectoryNamespaceItem {
    let entry = fixture
        .engine
        .current_directory_entries(source_branch)
        .unwrap()
        .iter()
        .find(|entry| entry.path() == source_path)
        .unwrap();
    DirectoryNamespaceItem {
        source: DirectoryNamespaceSource {
            directory_branch_id: source_branch.into(),
            expected_tree_id: fixture.engine.state.directory_branches[source_branch]
                .head_tree_id
                .clone(),
            path: source_path.into(),
            entry_id: entry.entry_id.clone(),
        },
        target: target.map(|(branch, path)| DirectoryNamespaceTarget {
            directory_branch_id: branch.into(),
            expected_tree_id: fixture.engine.state.directory_branches[branch]
                .head_tree_id
                .clone(),
            path: path.into(),
        }),
    }
}

fn apply(
    fixture: &mut Fixture,
    request: &DirectoryNamespaceBatch,
    operation: OperationId,
) -> Result<OperationResult> {
    fixture.engine.directory_namespace_batch(
        &fixture.repo.repository_id,
        request,
        "admin",
        operation,
    )
}

fn body(engine: &Engine, entry: &Entry) -> Vec<u8> {
    let mut bytes = Vec::new();
    engine
        .open_content(&entry.version_id)
        .unwrap()
        .file
        .read_to_end(&mut bytes)
        .unwrap();
    bytes
}

fn assert_clean(fixture: &Fixture) {
    let current = fixture
        .engine
        .namespace_tree(&fixture.repo.main_branch_id)
        .unwrap();
    let view = fixture.engine.current_namespaces.as_ref().unwrap();
    for scope in view.branches.values().chain(view.directories.values()) {
        assert!(scope.entries.iter().all(|entry| {
            entry.entry_id != fixture.removed.entry_id && entry.file_id != fixture.removed.file_id
        }));
    }
    let head = &fixture.engine.state.branches[&fixture.repo.main_branch_id].head_commit_id;
    assert_eq!(
        fixture.engine.state.commits[head].entries,
        current.entries()
    );
    for branch in fixture
        .engine
        .state
        .directory_branches
        .values()
        .filter(|branch| {
            branch.repository_id == fixture.repo.repository_id
                && fixture
                    .engine
                    .state
                    .directory_selections
                    .get(&branch.directory_id)
                    == Some(&branch.directory_branch_id)
                && fixture
                    .engine
                    .state
                    .directory_tree_selections
                    .get(&branch.directory_id)
                    == Some(&branch.head_tree_id)
                && (branch.directory_id == fixture.repo.repository_id
                    || current.entries().iter().any(|entry| {
                        entry.kind == NamespaceEntryKind::Directory
                            && entry.entry_id == branch.directory_id
                    }))
        })
    {
        assert_eq!(
            branch.entries,
            fixture
                .engine
                .current_directory_entries(&branch.directory_branch_id)
                .unwrap()
        );
        for path in branch.file_contexts.keys() {
            assert!(branch.entries.iter().any(|entry| {
                entry.kind == NamespaceEntryKind::File && entry.lookup_key() == path
            }));
        }
        for reference in branch.directory_refs.values() {
            let tree = &fixture.engine.state.directory_trees[&reference.tree_id];
            assert!(tree.entries.iter().all(|entry| {
                entry.entry_id != fixture.removed.entry_id
                    && entry.file_id != fixture.removed.file_id
            }));
        }
    }
}

#[test]
fn membership_registry_current_batch_rename_uses_hidden_name_without_reusing_hidden_identity() {
    let mut fixture = Fixture::new(true);
    let scope = initialize(&mut fixture, "dir");
    let context = fixture
        .engine
        .file_context(&fixture.repo.repository_id, "dir/b")
        .unwrap();
    let commits = fixture.engine.state.commits.clone();
    let trees = fixture.engine.state.directory_trees.clone();
    fixture.project(false);
    let request = DirectoryNamespaceBatch {
        kind: DirectoryBatchKind::Rename,
        items: vec![item(&fixture, &scope, "dir/b", Some((&scope, "dir/a")))],
    };
    let operation = op();
    let receipt = apply(&mut fixture, &request, operation).unwrap();
    let moved = current(&fixture, "dir/a");
    assert_eq!(moved.entry_id, fixture.retained.entry_id);
    assert_eq!(moved.file_id, fixture.retained.file_id);
    assert_eq!(moved.version_id, fixture.retained.version_id);
    let after = fixture
        .engine
        .file_context(&fixture.repo.repository_id, "dir/a")
        .unwrap();
    assert_eq!(after.branch_id, context.branch_id);
    assert_eq!(
        fixture
            .engine
            .namespace_tree(&after.branch_id)
            .unwrap()
            .get("dir/a"),
        Some(&moved)
    );
    assert_clean(&fixture);
    for (id, commit) in commits.iter() {
        assert_eq!(fixture.engine.state.commits.get(id), Some(commit));
    }
    for (id, tree) in trees.iter() {
        assert_eq!(fixture.engine.state.directory_trees.get(id), Some(tree));
    }
    save(
        &mut fixture.engine,
        &after.branch_id,
        "dir/a",
        b"independent save after batch",
    );
    let saved = current(&fixture, "dir/a");
    assert_eq!(saved.file_id, moved.file_id);
    assert_eq!(saved.entry_id, moved.entry_id);
    assert_ne!(saved.version_id, moved.version_id);
    assert_clean(&fixture);
    let before = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(apply(&mut fixture, &request, operation).unwrap(), receipt);
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    assert_eq!(
        fixture
            .engine
            .directory_namespace_batch(
                &fixture.repo.repository_id,
                &request,
                "another-principal",
                operation,
            )
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyKeyReused
    );
}

#[test]
fn membership_registry_current_batch_copy_allocates_new_bodies_and_refreshes_frozen_children() {
    let mut fixture = Fixture::new(true);
    save(
        &mut fixture.engine,
        &fixture.repo.main_branch_id,
        "dir/child/keep",
        b"child body",
    );
    let child = current(&fixture, "dir/child/keep");
    let root = initialize(&mut fixture, "");
    let old_trees = fixture.engine.state.directory_trees.clone();
    fixture.project(false);
    let request = DirectoryNamespaceBatch {
        kind: DirectoryBatchKind::Copy,
        items: vec![item(&fixture, &root, "dir", Some((&root, "copied")))],
    };
    let receipt = apply(&mut fixture, &request, op()).unwrap();
    assert_eq!(receipt.result["copies"].as_array().unwrap().len(), 2);
    let copied = current(&fixture, "copied/b");
    let copied_child = current(&fixture, "copied/child/keep");
    for (source, target) in [(&fixture.retained, &copied), (&child, &copied_child)] {
        assert_ne!(source.entry_id, target.entry_id);
        assert_ne!(source.file_id, target.file_id);
        assert_ne!(source.version_id, target.version_id);
        assert_eq!(body(&fixture.engine, source), body(&fixture.engine, target));
    }
    assert!(
        fixture
            .engine
            .namespace_tree(&fixture.repo.main_branch_id)
            .unwrap()
            .get("copied/a")
            .is_none()
    );
    assert_clean(&fixture);
    for (id, tree) in old_trees.iter() {
        assert_eq!(fixture.engine.state.directory_trees.get(id), Some(tree));
    }
    let scope = initialize(&mut fixture, "dir");
    let copy_to_hidden_name = DirectoryNamespaceBatch {
        kind: DirectoryBatchKind::Copy,
        items: vec![item(&fixture, &scope, "dir/b", Some((&scope, "dir/a")))],
    };
    apply(&mut fixture, &copy_to_hidden_name, op()).unwrap();
    let replacement = current(&fixture, "dir/a");
    assert_ne!(replacement.entry_id, fixture.removed.entry_id);
    assert_ne!(replacement.file_id, fixture.removed.file_id);
    assert_ne!(replacement.entry_id, fixture.retained.entry_id);
    assert_ne!(replacement.file_id, fixture.retained.file_id);
    save(
        &mut fixture.engine,
        &fixture.repo.main_branch_id,
        "dir/a",
        b"new copy saved",
    );
    assert_eq!(current(&fixture, "dir/a").file_id, replacement.file_id);
    assert_clean(&fixture);
}

#[test]
fn membership_registry_current_batch_moves_between_selected_scopes_and_renames_nested_directories()
{
    let mut fixture = Fixture::new(true);
    save(
        &mut fixture.engine,
        &fixture.repo.main_branch_id,
        "destination/keep",
        b"destination",
    );
    save(
        &mut fixture.engine,
        &fixture.repo.main_branch_id,
        "dir/child/keep",
        b"nested",
    );
    let source = initialize(&mut fixture, "dir");
    let destination = initialize(&mut fixture, "destination");
    let original_context = fixture
        .engine
        .file_context(&fixture.repo.repository_id, "dir/b")
        .unwrap();
    let trees = fixture.engine.state.directory_trees.clone();
    fixture.project(false);
    let request = DirectoryNamespaceBatch {
        kind: DirectoryBatchKind::Move,
        items: vec![item(
            &fixture,
            &source,
            "dir/b",
            Some((&destination, "destination/b")),
        )],
    };
    apply(&mut fixture, &request, op()).unwrap();
    let moved = current(&fixture, "destination/b");
    assert_eq!(moved.file_id, fixture.retained.file_id);
    assert_eq!(moved.version_id, fixture.retained.version_id);
    assert_eq!(
        fixture
            .engine
            .file_context(&fixture.repo.repository_id, "destination/b")
            .unwrap()
            .branch_id,
        original_context.branch_id
    );
    assert!(
        fixture
            .engine
            .namespace_tree(&fixture.repo.main_branch_id)
            .unwrap()
            .get("dir/b")
            .is_none()
    );
    assert_clean(&fixture);
    let request = DirectoryNamespaceBatch {
        kind: DirectoryBatchKind::Rename,
        items: vec![item(
            &fixture,
            &source,
            "dir/child",
            Some((&source, "dir/Moved")),
        )],
    };
    apply(&mut fixture, &request, op()).unwrap();
    let moved_directory = current(&fixture, "dir/Moved");
    let case_only = DirectoryNamespaceBatch {
        kind: DirectoryBatchKind::Rename,
        items: vec![item(
            &fixture,
            &source,
            "dir/Moved",
            Some((&source, "dir/MOVED")),
        )],
    };
    apply(&mut fixture, &case_only, op()).unwrap();
    assert_eq!(
        current(&fixture, "dir/MOVED").entry_id,
        moved_directory.entry_id
    );
    assert_eq!(
        body(&fixture.engine, &current(&fixture, "dir/MOVED/keep")),
        b"nested"
    );
    assert_clean(&fixture);
    for (id, tree) in trees.iter() {
        assert_eq!(fixture.engine.state.directory_trees.get(id), Some(tree));
    }
}

#[test]
fn membership_registry_current_file_delete_batch_keeps_each_shared_scope_precondition() {
    let mut fixture = Fixture::new(true);
    save(
        &mut fixture.engine,
        &fixture.repo.main_branch_id,
        "dir/c",
        b"second retained file",
    );
    let scope = initialize(&mut fixture, "dir");
    fixture.project(false);
    let first = item(&fixture, &scope, "dir/b", None);
    let second = item(&fixture, &scope, "dir/c", None);
    let files = [current(&fixture, "dir/b"), current(&fixture, "dir/c")];
    let commits = fixture.engine.state.commits.clone();
    let raw = fixture.engine.state.directory_branches[&scope]
        .entries
        .clone();
    let before = canonical_json(fixture.engine.state()).unwrap();
    let mut stale = second.clone();
    stale.source.expected_tree_id = new_id("dtree").unwrap();
    for (items, code) in [
        (
            vec![first.clone(), stale],
            ErrorCode::DirectoryBranchConflict,
        ),
        (
            vec![first.clone(), first.clone()],
            ErrorCode::BatchMappingConflict,
        ),
    ] {
        let request = DirectoryNamespaceBatch {
            kind: DirectoryBatchKind::Delete,
            items,
        };
        assert_eq!(
            apply(&mut fixture, &request, op()).unwrap_err().code(),
            code
        );
        assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    }
    let request = DirectoryNamespaceBatch {
        kind: DirectoryBatchKind::Delete,
        items: vec![first, second],
    };
    let operation = op();
    let receipt = apply(&mut fixture, &request, operation).unwrap();
    assert_eq!(receipt.kind, "file.delete.batch");
    assert_eq!(fixture.engine.state.file_deletions.len(), 2);
    assert!(
        !fixture
            .engine
            .state
            .file_deletions
            .contains_key(&fixture.removed.file_id)
    );
    for (file, bytes) in files
        .iter()
        .zip([b"retained".as_slice(), b"second retained file"])
    {
        assert_eq!(
            fixture.engine.state.file_deletions[&file.file_id].operation_id,
            operation
        );
        assert!(
            fixture
                .engine
                .namespace_tree(&fixture.repo.main_branch_id)
                .unwrap()
                .get(file.path())
                .is_none()
        );
        assert_eq!(body(&fixture.engine, file), bytes);
    }
    assert_eq!(fixture.engine.state.directory_branches[&scope].entries, raw);
    assert_eq!(fixture.engine.state.commits, commits);
    let after = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(apply(&mut fixture, &request, operation).unwrap(), receipt);
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), after);
}

#[test]
fn membership_registry_current_batch_delete_records_only_visible_occurrences() {
    let mut fixture = Fixture::new(true);
    let root = initialize(&mut fixture, "");
    let old_commits = fixture.engine.state.commits.clone();
    fixture.project(false);
    let request = DirectoryNamespaceBatch {
        kind: DirectoryBatchKind::Delete,
        items: vec![item(&fixture, &root, "dir", None)],
    };
    let result = apply(&mut fixture, &request, op()).unwrap();
    let head = result.result["commit_id"].as_str().unwrap();
    let deletions = &fixture.engine.state.commits[head]
        .namespace_deletions
        .as_ref()
        .unwrap()
        .paths;
    assert!(deletions.iter().any(|entry| entry.path == "dir/b"));
    assert!(deletions.iter().all(|entry| entry.path != "dir/a"));
    assert_clean(&fixture);
    for (id, commit) in old_commits.iter() {
        assert_eq!(fixture.engine.state.commits.get(id), Some(commit));
    }
    save(
        &mut fixture.engine,
        &fixture.repo.main_branch_id,
        "dir/a",
        b"fresh after deletion",
    );
    let created = current(&fixture, "dir/a");
    assert_ne!(created.entry_id, fixture.removed.entry_id);
    assert_ne!(created.file_id, fixture.removed.file_id);
    assert_clean(&fixture);
}

#[test]
fn membership_registry_current_batch_hidden_sources_and_stale_targets_fail_atomically() {
    let mut fixture = Fixture::new(true);
    let scope = initialize(&mut fixture, "dir");
    let hidden = item(&fixture, &scope, "dir/a", Some((&scope, "dir/hidden-copy")));
    fixture.project(false);
    let mut retained = item(&fixture, &scope, "dir/b", Some((&scope, "dir/new")));
    let before = canonical_json(fixture.engine.state()).unwrap();
    let hidden_request = DirectoryNamespaceBatch {
        kind: DirectoryBatchKind::Copy,
        items: vec![retained.clone(), hidden],
    };
    assert_eq!(
        apply(&mut fixture, &hidden_request, op())
            .unwrap_err()
            .code(),
        ErrorCode::EntryNotFound
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    retained.target.as_mut().unwrap().expected_tree_id = new_id("dtree").unwrap();
    let stale_request = DirectoryNamespaceBatch {
        kind: DirectoryBatchKind::Copy,
        items: vec![retained],
    };
    assert_eq!(
        apply(&mut fixture, &stale_request, op())
            .unwrap_err()
            .code(),
        ErrorCode::DirectoryBranchConflict
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
}

#[test]
fn membership_registry_current_batch_rejects_overlapping_active_scopes_before_any_publication() {
    let mut fixture = Fixture::new(true);
    let root = initialize(&mut fixture, "");
    let directory = initialize(&mut fixture, "dir");
    fixture.project(false);
    let request = DirectoryNamespaceBatch {
        kind: DirectoryBatchKind::Move,
        items: vec![item(
            &fixture,
            &root,
            "dir/b",
            Some((&directory, "dir/moved")),
        )],
    };
    let before = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(
        apply(&mut fixture, &request, op()).unwrap_err().code(),
        ErrorCode::DirectoryBranchScope
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
}

#[test]
fn membership_registry_current_batch_noop_does_not_materialize_raw_hidden_entries() {
    let mut fixture = Fixture::new(true);
    let scope = initialize(&mut fixture, "dir");
    fixture.project(false);
    let branches = fixture.engine.state.branches.clone();
    let directories = fixture.engine.state.directory_branches.clone();
    let contexts = fixture.engine.state.file_contexts.clone();
    let request = DirectoryNamespaceBatch {
        kind: DirectoryBatchKind::Rename,
        items: vec![item(&fixture, &scope, "dir/b", Some((&scope, "dir/b")))],
    };
    let result = apply(&mut fixture, &request, op()).unwrap();
    assert_eq!(result.result["no_change"], true);
    assert_eq!(fixture.engine.state.branches, branches);
    assert_eq!(fixture.engine.state.directory_branches, directories);
    assert_eq!(fixture.engine.state.file_contexts, contexts);
    assert!(
        fixture
            .engine
            .namespace_tree(&fixture.repo.main_branch_id)
            .unwrap()
            .get("dir/a")
            .is_none()
    );
}

#[test]
fn membership_registry_current_batch_inactive_and_historical_scopes_stay_fenced_but_receipts_survive()
 {
    let mut fixture = Fixture::new(true);
    initialize(&mut fixture, "dir");
    let inactive = fixture
        .engine
        .create_directory_branches(
            &fixture.repo.repository_id,
            Some(&name_policy::LookupPath::parse("dir").unwrap()),
            &BranchName::parse("frozen").unwrap(),
            "admin",
            op(),
        )
        .unwrap()
        .result["directory_branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let original = DirectoryNamespaceBatch {
        kind: DirectoryBatchKind::Rename,
        items: vec![item(
            &fixture,
            &inactive,
            "dir/b",
            Some((&inactive, "dir/previous")),
        )],
    };
    let operation = op();
    let receipt = apply(&mut fixture, &original, operation).unwrap();
    fixture.project(false);
    let rejected = DirectoryNamespaceBatch {
        kind: DirectoryBatchKind::Rename,
        items: vec![item(
            &fixture,
            &inactive,
            "dir/previous",
            Some((&inactive, "dir/new")),
        )],
    };
    let before = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(
        apply(&mut fixture, &rejected, op()).unwrap_err().code(),
        ErrorCode::RecoveryRequired
    );
    assert_eq!(apply(&mut fixture, &original, operation).unwrap(), receipt);
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);

    let active = selected(&fixture, "dir");
    let branch = fixture.engine.state.directory_branches[&active].clone();
    let historical = fixture.engine.state.directory_trees[&branch.head_tree_id]
        .parent_tree_id
        .clone()
        .unwrap();
    Arc::make_mut(&mut fixture.engine.state)
        .directory_tree_selections
        .insert(branch.directory_id, historical);
    let request = DirectoryNamespaceBatch {
        kind: DirectoryBatchKind::Delete,
        items: vec![item(&fixture, &active, "dir/b", None)],
    };
    let before = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(
        apply(&mut fixture, &request, op()).unwrap_err().code(),
        ErrorCode::RecoveryRequired
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
}

#[test]
fn membership_registry_current_batch_requires_the_exact_selected_occurrence_not_global_file_ids() {
    let mut fixture = Fixture::new(true);
    let scope = initialize(&mut fixture, "dir");
    fixture.project(false);
    let request = DirectoryNamespaceBatch {
        kind: DirectoryBatchKind::Rename,
        items: vec![item(&fixture, &scope, "dir/b", Some((&scope, "dir/new")))],
    };
    let view = Arc::make_mut(fixture.engine.current_namespaces.as_mut().unwrap());
    let directories = Arc::make_mut(&mut view.directories);
    let projected = Arc::make_mut(directories.get_mut(&scope).unwrap());
    Arc::make_mut(&mut projected.entries)
        .iter_mut()
        .find(|entry| entry.path() == "dir/b")
        .unwrap()
        .size += 1;
    let before = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(
        apply(&mut fixture, &request, op()).unwrap_err().code(),
        ErrorCode::WorkspaceChanged
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
}

#[test]
fn membership_registry_current_batch_preserves_preparation_exclusion_and_actual_payload_verification()
 {
    let mut fixture = Fixture::new(true);
    let scope = initialize(&mut fixture, "dir");
    fixture.project(false);
    let batch = DirectoryNamespaceBatch {
        kind: DirectoryBatchKind::Copy,
        items: vec![item(
            &fixture,
            &scope,
            "dir/b",
            Some((&scope, "dir/copied")),
        )],
    };
    let workspace = fixture
        .engine
        .create_workspace(&fixture.repo.main_branch_id, "admin")
        .unwrap();
    fixture
        .engine
        .stage_bytes(&workspace.workspace_id, "later", b"staged")
        .unwrap();
    let preparation = fixture
        .engine
        .prepare_workspace_commit(request(&workspace), None)
        .unwrap();
    let before = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(
        apply(&mut fixture, &batch, op()).unwrap_err().code(),
        ErrorCode::CommitPreparationBusy
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    drop(preparation);
    let version = &fixture.engine.state.versions[&fixture.retained.version_id];
    let disk = fixture.engine.disk("D04").unwrap();
    let directory = crate::disk::version_dir(disk, &version.history_tree_id, &version.version_id);
    let content = crate::container_storage::content_path(&directory, version).unwrap();
    // Published payloads are immutable. This fixture deliberately corrupts
    // only its temporary payload; ordinary writers must retain that protection.
    let permissions = fs::metadata(&content).unwrap().permissions();
    fs::set_permissions(
        &content,
        fs::Permissions::from_mode(permissions.mode() | 0o200),
    )
    .unwrap();
    fs::write(&content, b"corrupt retained payload").unwrap();
    fs::set_permissions(&content, permissions).unwrap();
    assert_eq!(
        apply(&mut fixture, &batch, op()).unwrap_err().code(),
        ErrorCode::ContentUnavailable
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    assert!(
        fixture
            .engine
            .workspace(&workspace.workspace_id)
            .unwrap()
            .files
            .values()
            .all(|file| fs::read(&file.source_path).unwrap() == b"staged")
    );
}

#[test]
fn membership_registry_current_batch_recovery_rejects_hidden_or_duplicate_occurrence_imports() {
    let mut fixture = Fixture::new(true);
    let scope = initialize(&mut fixture, "dir");
    fixture.project(false);
    let before = Arc::clone(&fixture.engine.state);
    let before_snapshot = fixture.engine.snapshot.clone();
    let source = fixture.engine.current_namespaces.clone();
    let request = DirectoryNamespaceBatch {
        kind: DirectoryBatchKind::Copy,
        items: vec![item(
            &fixture,
            &scope,
            "dir/b",
            Some((&scope, "dir/copied")),
        )],
    };
    apply(&mut fixture, &request, op()).unwrap();
    let copied = current(&fixture, "dir/copied");
    let published: State =
        json_codec::from_slice(&canonical_json(fixture.engine.state()).unwrap()).unwrap();
    let snapshot = fixture.engine.snapshot.clone().unwrap();
    fixture.engine.state = before;
    fixture.engine.snapshot = before_snapshot.clone();
    fixture.engine.current_namespaces = source;
    for original in [&fixture.removed, &fixture.retained] {
        let mut forged = published.clone();
        let head = forged.branches[&fixture.repo.main_branch_id]
            .head_commit_id
            .clone();
        let mut imported = original.clone();
        imported.relocate(name_policy::PortablePath::parse("dir/forged").unwrap());
        let commit = forged.commits.get_mut(&head).unwrap();
        commit.entries.push(imported);
        commit
            .entries
            .sort_by(|left, right| left.lookup_key().cmp(right.lookup_key()));
        assert_eq!(
            fixture
                .engine
                .install_recovered_namespace_state(forged, snapshot.clone())
                .unwrap_err()
                .code(),
            ErrorCode::WorkspaceChanged
        );
        assert_eq!(fixture.engine.snapshot, before_snapshot);
    }
    // A globally fresh copy ID cannot authorize an unselected helper snapshot.
    // Its occurrence must exactly match the owner admitted in this publication.
    let mut forged = published.clone();
    let helper = forged.file_contexts
        [&crate::business::context_key_from_key(&fixture.repo.repository_id, "dir/copied")]
        .branch_id
        .clone();
    let head = forged.branches[&helper].head_commit_id.clone();
    let commit = forged.commits.get_mut(&head).unwrap();
    commit
        .entries
        .iter_mut()
        .find(|entry| entry.kind == NamespaceEntryKind::File)
        .unwrap()
        .relocate(name_policy::PortablePath::parse("dir/forged").unwrap());
    commit
        .entries
        .sort_by(|left, right| left.lookup_key().cmp(right.lookup_key()));
    assert!(
        fixture
            .engine
            .install_recovered_namespace_state(forged, snapshot.clone())
            .is_err()
    );
    assert_eq!(fixture.engine.snapshot, before_snapshot);
    fixture
        .engine
        .install_recovered_namespace_state(published, snapshot)
        .unwrap();
    assert_eq!(current(&fixture, "dir/copied"), copied);
    assert_clean(&fixture);
}

fn files(root: &Path) -> BTreeMap<std::path::PathBuf, Vec<u8>> {
    let mut result = BTreeMap::new();
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            result.extend(files(&path));
        } else {
            let bytes = fs::read(&path).unwrap();
            result.insert(path, bytes);
        }
    }
    result
}

#[test]
fn disk_exit_current_batch_real_d3_restarts_without_reopening_or_rewriting_the_target() {
    for action in [
        MembershipExitAction::Deregister,
        MembershipExitAction::PlannedOffline,
    ] {
        let mut fixture = Fixture::new(true);
        let disk = fixture.engine.disk("P01").unwrap().clone();
        let held = Arc::clone(fixture.engine.disk_resources.get(&disk).unwrap());
        let scope = initialize(&mut fixture, "dir");
        let hidden = item(&fixture, &scope, "dir/a", Some((&scope, "dir/hidden-copy")));
        fixture
            .engine
            .begin_disk_exit(&disk.disk_id, action, "admin", op())
            .unwrap();
        let archive = fixture
            .engine
            .build_managed_membership_control_archive()
            .unwrap();
        fixture
            .engine
            .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
            .unwrap();
        fixture.engine.resume_membership_joint_decision().unwrap();
        let physical = files(&disk.root);
        let historical_commits = fixture.engine.state.commits.clone();
        let historical_trees = fixture.engine.state.directory_trees.clone();
        fixture.engine.activate_pending_disk_exit().unwrap();
        held.root.io_binding().unwrap().closed_proof().unwrap();
        assert_eq!(files(&disk.root), physical);
        let rejected = DirectoryNamespaceBatch {
            kind: DirectoryBatchKind::Copy,
            items: vec![hidden],
        };
        let before = canonical_json(fixture.engine.state()).unwrap();
        assert_eq!(
            apply(&mut fixture, &rejected, op()).unwrap_err().code(),
            if action == MembershipExitAction::Deregister {
                ErrorCode::EntryNotFound
            } else {
                ErrorCode::ContentUnavailable
            }
        );
        assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
        let request = DirectoryNamespaceBatch {
            kind: DirectoryBatchKind::Copy,
            items: vec![item(
                &fixture,
                &scope,
                "dir/b",
                Some((&scope, "dir/copied")),
            )],
        };
        let operation = op();
        let receipt = apply(&mut fixture, &request, operation).unwrap();
        let copied = current(&fixture, "dir/copied");
        assert_eq!(body(&fixture.engine, &copied), b"retained");
        assert_ne!(copied.file_id, fixture.retained.file_id);
        for (id, commit) in historical_commits.iter() {
            assert_eq!(fixture.engine.state.commits.get(id), Some(commit));
        }
        for (id, tree) in historical_trees.iter() {
            assert_eq!(fixture.engine.state.directory_trees.get(id), Some(tree));
        }
        assert_eq!(files(&disk.root), physical);
        drop(held);
        let system = fixture._temp.path().join("system");
        drop(fixture.engine);
        fixture.engine = Engine::open(&system).unwrap();
        assert_eq!(apply(&mut fixture, &request, operation).unwrap(), receipt);
        assert_eq!(
            body(&fixture.engine, &current(&fixture, "dir/copied")),
            b"retained"
        );
        if action == MembershipExitAction::Deregister {
            assert_clean(&fixture);
            assert!(fixture.engine.registered_disk(&disk.disk_id).is_none());
        } else {
            assert!(fixture.engine.registered_disk(&disk.disk_id).is_some());
            assert_eq!(current(&fixture, "dir/a"), fixture.removed);
        }
        assert!(fixture.engine.disk_resources.available(&disk).is_none());
        assert_eq!(files(&disk.root), physical);
    }
}

#[path = "current_file_deletion_batch_undo_tests.rs"]
mod file_deletion_batch_undo;
