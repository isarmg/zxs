//! Whole-directory user flows over the existing public Engine test facilities.
//! Requires the agreed ObjectMoveRequest and Engine::move_object implementation.
//! No fault/replay matrix here; the shared executor and SMB suites own those.
use super::*;
use std::collections::BTreeMap;
use storage_contract::{
    Entry, FileTagTarget, NamespaceEntryKind, NamespaceOperation, NamespaceOperationKind,
    directory_history::{DirectoryHistoryCollection, DirectoryHistoryRequest},
    file_relocation::{NamespaceEntryId, OBJECT_MOVE_KIND, ObjectMoveFact, ObjectMoveRequest},
    history_window::{HistoryRepositoryId, HistoryWindowCancellation},
};

#[derive(Clone, Copy, Debug)]
enum Route {
    SameDisk,
    ExistingDisk,
    EmptyDisk,
}

struct Destination {
    slot: &'static str,
    repository: Option<Repository>,
}

fn fixture_for(route: Route) -> (tempfile::TempDir, Engine, Repository, Destination) {
    let temp = tempfile::tempdir().unwrap();
    let mut config =
        Config::development([temp.path().join("source"), temp.path().join("target")]).unwrap();
    config.disks[1].level = 1;
    config.disks[1].upstream_slot_id = None;
    let mut engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
    let source = engine
        .create_disk_root("P01", "admin", OperationId::generate().unwrap())
        .unwrap();
    let destination = match route {
        Route::SameDisk => Destination {
            slot: "P01",
            repository: Some(source.clone()),
        },
        Route::ExistingDisk => Destination {
            slot: "B01",
            repository: Some(
                engine
                    .create_disk_root("B01", "admin", OperationId::generate().unwrap())
                    .unwrap(),
            ),
        },
        Route::EmptyDisk => Destination {
            slot: "B01",
            repository: None,
        },
    };
    (temp, engine, source, destination)
}

fn entry(engine: &Engine, repository: &Repository, path: &str) -> Entry {
    engine
        .namespace_tree(&repository.main_branch_id)
        .unwrap()
        .get(path)
        .unwrap()
        .clone()
}

fn move_to(
    engine: &mut Engine,
    source: &Repository,
    destination: &Destination,
    path: &str,
    target_path: &str,
) -> Repository {
    let request = ObjectMoveRequest {
        source_repository_id: HistoryRepositoryId::parse(&source.repository_id).unwrap(),
        source_entry_id: NamespaceEntryId::parse(&entry(engine, source, path).entry_id).unwrap(),
        source_path: name_policy::LookupPath::parse(path).unwrap(),
        target_slot_id: destination.slot.into(),
        target_repository_id: destination
            .repository
            .as_ref()
            .map(|repository| HistoryRepositoryId::parse(&repository.repository_id).unwrap()),
        target_path: name_policy::PortablePath::parse(target_path).unwrap(),
        replace: false,
        target_entry_id: None,
    };
    let operation = OperationId::generate().unwrap();
    let operations = engine.state().operations.len();
    let repositories = engine.state().repositories.len();
    let receipt = engine.move_object(&request, "admin", operation).unwrap();
    assert_eq!(receipt.operation_id, operation);
    assert_eq!(receipt.kind, OBJECT_MOVE_KIND);
    assert_eq!(receipt.request_digest, request.digest("admin").unwrap());
    // Empty target creation is part of this operation, never a separate receipt.
    assert_eq!(engine.state().operations.len(), operations + 1);
    assert_eq!(
        engine.state().repositories.len(),
        repositories + usize::from(destination.repository.is_none())
    );
    let fact: ObjectMoveFact = json_codec::from_value(receipt.result["movement"].clone()).unwrap();
    assert_eq!(fact.request, request);
    assert_eq!(fact.principal, "admin");
    assert!(fact.source_file_id.is_none() && fact.displaced_target_file_id.is_none());
    let target = engine.state().repositories[fact.target_repository_id.as_str()].clone();
    assert_eq!(target.primary_slot_id, destination.slot);
    assert_eq!(
        entry(engine, &target, target_path).entry_id,
        fact.target_entry_id.as_str()
    );
    assert!(
        engine
            .namespace_tree(&source.main_branch_id)
            .unwrap()
            .get(path)
            .is_none()
    );
    target
}

fn snapshot_files(engine: &Engine, head: &str) -> BTreeMap<String, Vec<u8>> {
    let tree = &engine.state().directory_trees[head];
    let prefix = format!("{}/", tree.path);
    tree.entries
        .iter()
        .filter(|entry| entry.kind == NamespaceEntryKind::File)
        .map(|entry| {
            let path = if tree.path.is_empty() {
                entry.path()
            } else {
                entry.path().strip_prefix(&prefix).unwrap()
            };
            (path.to_owned(), version_bytes(engine, &entry.version_id))
        })
        .collect()
}

#[test]
fn empty_directory_moves_to_same_existing_and_empty_disks_as_one_usable_object() {
    for route in [Route::SameDisk, Route::ExistingDisk, Route::EmptyDisk] {
        let (_temp, mut engine, source, destination) = fixture_for(route);
        engine
            .file_namespace(
                &source.repository_id,
                NamespaceOperation::parse(NamespaceOperationKind::Mkdir, "empty", None).unwrap(),
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        let original = entry(&engine, &source, "empty");
        let target = move_to(&mut engine, &source, &destination, "empty", "moved");
        let moved = entry(&engine, &target, "moved");
        assert_eq!(moved.kind, NamespaceEntryKind::Directory, "{route:?}");
        if matches!(route, Route::SameDisk) {
            assert_eq!(moved.entry_id, original.entry_id);
        }
        assert!(engine.state().versions.is_empty(), "{route:?}");
        assert!(!status(&engine, &target, "moved").initialized, "{route:?}");
        initialize(&mut engine, &target, "moved");
        let head = commit(&mut engine, &target, "moved");
        assert!(
            engine.state().directory_trees[&head].entries.is_empty(),
            "{route:?}"
        );
        assert!(!status(&engine, &target, "moved").staged, "{route:?}");
    }
}

#[test]
fn repository_move_keeps_saved_work_separate_from_its_branches_and_child_pins() {
    for route in [Route::SameDisk, Route::ExistingDisk, Route::EmptyDisk] {
        let (_temp, mut engine, source, destination) = fixture_for(route);
        save(&mut engine, &source, "project/owned", b"committed parent");
        save(&mut engine, &source, "project/child/file", b"committed child");
        initialize(&mut engine, &source, "project/child");
        commit(&mut engine, &source, "project/child");
        initialize(&mut engine, &source, "project");
        stage_child(&mut engine, &source, "project", "project/child");
        let parent_head = commit(&mut engine, &source, "project");
        engine
            .create_directory_branches(
                &source.repository_id,
                Some(&name_policy::LookupPath::parse("project").unwrap()),
                &storage_contract::BranchName::parse("topic").unwrap(),
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        // These are real saves after the user commit, not staged fixture edits.
        save(&mut engine, &source, "project/owned", b"saved parent");
        save(&mut engine, &source, "project/child/file", b"saved child");
        save(&mut engine, &source, "project/added", b"never user committed");
        let added = entry(&engine, &source, "project/added");
        let context = engine
            .file_context(&source.repository_id, "project/added")
            .unwrap();
        let node = engine.file_branch_head(&context.branch_id).unwrap();
        engine
            .update_file_node_tags(storage_engine::FileNodeTagsUpdateRequest {
                file_id: &added.file_id,
                targets: &[FileTagTarget {
                    branch_id: context.branch_id,
                    source_node_id: node.clone(),
                    expected_node_id: node,
                }],
                add: &["saved-work".into()],
                remove: &[],
                location: None,
                principal: "admin",
                operation_id: OperationId::generate().unwrap(),
            })
            .unwrap();
        let before = status(&engine, &source, "project");
        let child_before = status(&engine, &source, "project/child");
        let committed = snapshot_files(&engine, &parent_head);
        assert!(!committed.contains_key("added"));
        assert!(!before.staged && before.unstaged && before.children[0].dirty);
        assert!(!child_before.staged && child_before.unstaged);

        let target = move_to(&mut engine, &source, &destination, "project", "moved");
        assert_eq!(
            bytes(&engine, &target, "moved/owned"),
            b"saved parent",
            "{route:?}"
        );
        assert_eq!(
            bytes(&engine, &target, "moved/child/file"),
            b"saved child",
            "{route:?}"
        );
        assert_eq!(
            bytes(&engine, &target, "moved/added"),
            b"never user committed",
            "{route:?}"
        );
        let after = status(&engine, &target, "moved");
        let child_after = status(&engine, &target, "moved/child");
        assert!(!after.staged && after.unstaged, "{route:?}");
        assert!(!child_after.staged && child_after.unstaged, "{route:?}");
        assert_eq!(after.children.len(), 1);
        assert_eq!(after.children[0].directory_id, child_after.directory_id);
        assert!(after.children[0].dirty && !after.children[0].reference_changed);
        assert_eq!(after.children[0].committed_tree_id, child_after.head_tree_id);
        assert_eq!(
            snapshot_files(&engine, after.head_tree_id.as_deref().unwrap()),
            committed
        );
        // The first explicit commits are roots. Moving them must not turn
        // those real history nodes into uncommitted working projections.
        for moved in [&after, &child_after] {
            let request = DirectoryHistoryRequest::parse(
                &target.repository_id,
                &moved.directory_id,
                DirectoryHistoryCollection::Nodes,
                BTreeMap::new(),
            )
            .unwrap();
            let read = engine
                .capture_directory_history(request.clone(), HistoryWindowCancellation::new())
                .unwrap();
            let page = read.page().unwrap();
            engine.recheck_directory_history(&read).unwrap();
            page.validate(&request).unwrap();
            assert_eq!(page.current.committed_tree_id, moved.head_tree_id);
            assert!(
                page.nodes.iter().any(|node| {
                    Some(node.tree_id.as_str()) == moved.head_tree_id.as_deref()
                }),
                "{route:?}"
            );
        }
        let git = &engine.state().directory_git_repositories[&after.directory_id];
        let main = git.main_branch_id.clone();
        let names = git
            .branches
            .keys()
            .map(|id| engine.state().directory_branches[id].name.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(names, ["main", "topic"].into());
        let moved_added = entry(&engine, &target, "moved/added");
        let context = engine
            .file_context(&target.repository_id, "moved/added")
            .unwrap();
        assert!(
            engine
                .file_node_tags(&moved_added.file_id, context.node_id.as_ref().unwrap())
                .unwrap()
                .contains("saved-work"),
            "{route:?}"
        );
        if matches!(route, Route::SameDisk) {
            assert_eq!(after.directory_id, before.directory_id);
            assert_eq!(after.directory_branch_id, before.directory_branch_id);
            assert_eq!(child_after.directory_id, child_before.directory_id);
            assert_eq!(moved_added.file_id, added.file_id);
        }
        // A later explicit parent commit includes saved owned files but still
        // pins the child's original commit, not the child's dirty working body.
        let pinned = after.children[0].committed_tree_id.clone().unwrap();
        // Moving the root may require a new review of absolute child locations.
        // Only this explicit staging step may advance that reviewed boundary.
        stage_owned(&mut engine, &target, "moved");
        let reviewed = status(&engine, &target, "moved");
        assert!(reviewed.staged && !reviewed.unstaged && !reviewed.needs_restage);
        assert_eq!(
            reviewed.children[0].committed_tree_id.as_deref(),
            Some(pinned.as_str())
        );
        let receipt = commit_reviewed(
            &mut engine,
            &target,
            &reviewed,
            OperationId::generate().unwrap(),
        )
        .unwrap();
        let head = receipt.result["head_tree_id"].as_str().unwrap().to_owned();
        let files = snapshot_files(&engine, &head);
        assert_eq!(files["owned"], b"saved parent");
        assert_eq!(files["added"], b"never user committed");
        assert_eq!(files["child/file"], b"committed child");
        assert_eq!(
            engine.state().directory_trees[&head].directory_refs[&child_after.directory_id].tree_id,
            pinned
        );
        assert!(status(&engine, &target, "moved/child").unstaged);

        // Retaining a branch name is insufficient: its old owned contents must
        // remain selectable without replacing the independent child's work.
        engine
            .select_directory_branches(
                &target.repository_id,
                Some(&name_policy::LookupPath::parse("moved").unwrap()),
                &storage_contract::BranchName::parse("topic").unwrap(),
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        assert_eq!(bytes(&engine, &target, "moved/owned"), b"committed parent");
        assert_eq!(bytes(&engine, &target, "moved/child/file"), b"saved child");
        assert!(
            engine
                .namespace_tree(&target.main_branch_id)
                .unwrap()
                .get("moved/added")
                .is_none()
        );
        assert!(status(&engine, &target, "moved/child").unstaged);
        let saved_main = engine
            .directory_git_main_workspace(&target.repository_id, &after.directory_id)
            .unwrap();
        assert_eq!(saved_main.directory_branch_id, main);
        let owned = saved_main
            .entries
            .iter()
            .find(|entry| entry.path() == "moved/owned")
            .unwrap();
        assert_eq!(version_bytes(&engine, &owned.version_id), b"saved parent");
        assert!(
            saved_main
                .entries
                .iter()
                .any(|entry| entry.file_id == moved_added.file_id)
        );
    }
}

#[test]
fn same_disk_move_refreshes_both_parent_workspaces_without_committing_either() {
    let (_temp, mut engine, source, destination) = fixture_for(Route::SameDisk);
    for path in ["a", "a/empty", "b"] {
        engine
            .file_namespace(
                &source.repository_id,
                NamespaceOperation::parse(NamespaceOperationKind::Mkdir, path, None).unwrap(),
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
    }
    initialize(&mut engine, &source, "a");
    initialize(&mut engine, &source, "b");
    let source_head = commit(&mut engine, &source, "a");
    let target_head = commit(&mut engine, &source, "b");
    let parents = ["a", "b"].map(|path| {
        let status = status(&engine, &source, path);
        let git = &engine.state().directory_git_repositories[&status.directory_id];
        (path, status.directory_id, git.branches[&git.main_branch_id].clone())
    });

    move_to(&mut engine, &source, &destination, "a/empty", "b/moved");
    for (path, directory, original_index) in &parents {
        let moved = status(&engine, &source, path);
        assert!(!moved.staged && moved.unstaged && !moved.needs_restage, "{path}");
        let git = &engine.state().directory_git_repositories[directory];
        assert_eq!(&git.branches[&git.main_branch_id], original_index);
    }
    assert!(
        engine
            .directory_git_main_workspace(&source.repository_id, &parents[0].1)
            .unwrap()
            .entries
            .is_empty()
    );
    let target = engine
        .directory_git_main_workspace(&source.repository_id, &parents[1].1)
        .unwrap();
    assert_eq!(target.entries.len(), 1);
    assert_eq!(target.entries[0].path(), "b/moved");
    assert_eq!(
        engine.state().directory_trees[&source_head].entries[0].path(),
        "a/empty"
    );
    assert!(engine.state().directory_trees[&target_head].entries.is_empty());

    let removed = commit(&mut engine, &source, "a");
    assert!(engine.state().directory_trees[&removed].entries.is_empty());
    assert_eq!(
        status(&engine, &source, "b").head_tree_id.as_deref(),
        Some(target_head.as_str())
    );
    assert!(status(&engine, &source, "b").unstaged);
    let added = commit(&mut engine, &source, "b");
    assert_eq!(
        engine.state().directory_trees[&added].entries[0].path(),
        "b/moved"
    );
}
