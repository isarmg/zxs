//! One normal lifecycle checks current deletion mounts without rewriting D.
use super::*;
use storage_contract::{
    NamespaceOperation, NamespaceOperationKind, OperationId, Repository,
    file_relocation::{NamespaceEntryId, ObjectMoveRequest},
    history_window::HistoryRepositoryId,
};

fn move_directory(
    engine: &mut Engine,
    source: &Repository,
    path: &str,
    target: &Repository,
    target_path: &str,
) -> storage_contract::OperationResult {
    let entry = engine
        .namespace_tree(&source.main_branch_id)
        .unwrap()
        .get(path)
        .unwrap()
        .clone();
    engine
        .move_object(
            &ObjectMoveRequest {
                source_repository_id: HistoryRepositoryId::parse(&source.repository_id).unwrap(),
                source_entry_id: NamespaceEntryId::parse(&entry.entry_id).unwrap(),
                source_path: name_policy::LookupPath::parse(path).unwrap(),
                target_slot_id: target.primary_slot_id.clone(),
                target_repository_id: Some(
                    HistoryRepositoryId::parse(&target.repository_id).unwrap(),
                ),
                target_path: name_policy::PortablePath::parse(target_path).unwrap(),
                replace: false,
                target_entry_id: None,
            },
            "owner",
            OperationId::generate().unwrap(),
        )
        .unwrap_or_else(|error| panic!("move {path} to {target_path}: {error}"))
}

fn ordinary_scope(engine: &Engine, repository: &Repository, path: &str) -> String {
    let entry = engine
        .list_tree_ref(&repository.main_branch_id)
        .unwrap()
        .iter()
        .find(|entry| entry.path() == path)
        .unwrap();
    assert!(
        !engine
            .state()
            .directory_git_repositories
            .contains_key(&entry.entry_id)
    );
    let branch_id = &engine.state().directory_selections[&entry.entry_id];
    let branch = &engine.state().directory_branches[branch_id];
    assert_eq!(branch.directory_id, entry.entry_id);
    assert_eq!(branch.repository_id, repository.repository_id);
    let tree_id = &engine.state().directory_tree_selections[&entry.entry_id];
    assert_eq!(
        engine.state().directory_trees[tree_id].directory_branch_id,
        *branch_id
    );
    branch_id.clone()
}

fn assert_directory_references(engine: &Engine, repository: &Repository) {
    let state = engine.state();
    let mut checked = 0;
    for tree in state.directory_trees.values().filter(|tree| {
        state.directory_branches[&tree.directory_branch_id].repository_id
            == repository.repository_id
    }) {
        for (directory, reference) in &tree.directory_refs {
            let child = &state.directory_branches[&reference.directory_branch_id];
            assert_eq!(child.repository_id, repository.repository_id);
            assert_eq!(child.directory_id, *directory);
            assert_eq!(
                state.directory_trees[&reference.tree_id].directory_branch_id,
                reference.directory_branch_id
            );
            checked += 1;
        }
    }
    assert!(checked > 0);
}

#[test]
fn deletion_list_tracks_current_mounts_after_parent_and_whole_owner_moves() {
    let (root, mut engine, _, _) = fixture();
    let source = engine
        .create_disk_root("P01", "owner", OperationId::generate().unwrap())
        .unwrap();
    for path in ["project", "project/container", "project/container/hidden"] {
        engine
            .file_namespace(
                &source.repository_id,
                NamespaceOperation::parse(NamespaceOperationKind::Mkdir, path, None).unwrap(),
                "owner",
                OperationId::generate().unwrap(),
            )
            .unwrap();
    }
    let owner = checkpoint(&mut engine, &source, "project", "owner").directory_id;
    let original_scopes = [
        ordinary_scope(&engine, &source, "project/container"),
        ordinary_scope(&engine, &source, "project/container/hidden"),
    ];
    let hidden = engine
        .namespace_tree(&source.main_branch_id)
        .unwrap()
        .get("project/container/hidden")
        .unwrap()
        .clone();
    let deletion = engine
        .prepare_current_directory_deletion(
            &source.repository_id,
            &name_policy::LookupPath::parse(hidden.path()).unwrap(),
            &hidden.entry_id,
        )
        .unwrap()
        .unwrap();
    let receipt = engine
        .directory_namespace_batch(
            &source.repository_id,
            &deletion,
            "owner",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    assert_eq!(receipt.kind, "directory.delete");
    let original = engine
        .directory_deletion(&source.repository_id, &owner, &hidden.entry_id)
        .unwrap()
        .clone();
    assert_eq!(original.path.as_str(), "project/container/hidden");
    assert!(original.files.is_empty() && engine.state().file_deletions.is_empty());
    let target = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Deletion destination").unwrap(),
            "P01",
            3,
            "owner",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let (backend, auth, _token, identity) = authenticated_backend(engine, root.path());
    let list = |repository: &str, owner: &str| {
        backend
            .directory_deletions(
                &identity,
                repository,
                owner,
                crate::ReadDirectoryDeletions::parse(BTreeMap::new()).unwrap(),
            )
            .unwrap()
    };
    let initial = list(&source.repository_id, &owner);
    assert_eq!(initial.entries.len(), 1);
    assert_eq!(initial.entries[0].path, original.path);

    {
        let mut engine = backend.engine.lock().unwrap();
        move_directory(
            &mut engine,
            &source,
            "project/container",
            &source,
            "project/relocated",
        );
    }
    let relocated = list(&source.repository_id, &owner);
    assert_eq!(relocated.entries.len(), 1);
    assert_eq!(relocated.entries[0].directory_id, hidden.entry_id);
    assert_eq!(relocated.entries[0].deletion_node_id, original.node_id);
    assert_eq!(relocated.entries[0].operation_id, original.operation_id);
    assert_eq!(
        relocated.entries[0].path.as_str(),
        "project/relocated/hidden"
    );
    assert!(relocated.next_after.is_none());
    let (target_owner, imported, movement) = {
        let mut engine = backend.engine.lock().unwrap();
        assert_eq!(
            engine.directory_deletion(&source.repository_id, &owner, &hidden.entry_id),
            Some(&original)
        );
        assert_eq!(engine.state().operations[&receipt.operation_id], receipt);
        assert!(
            engine
                .namespace_tree(&source.main_branch_id)
                .unwrap()
                .get("project/relocated/hidden")
                .is_none()
        );
        let source_refs = engine
            .state()
            .directory_trees
            .iter()
            .map(|(id, tree)| (id.clone(), tree.directory_refs.clone()))
            .collect::<BTreeMap<_, _>>();
        let movement = move_directory(&mut engine, &source, "project", &target, "arrived");
        for (id, references) in &source_refs {
            assert_eq!(
                &engine.state().directory_trees[id].directory_refs,
                references
            );
        }
        assert_directory_references(&engine, &target);
        for (path, original_scope) in ["arrived/relocated", "arrived/relocated/hidden"]
            .into_iter()
            .zip(&original_scopes)
        {
            assert_ne!(ordinary_scope(&engine, &target, path), *original_scope);
            assert_eq!(
                engine.state().directory_branches[original_scope].repository_id,
                source.repository_id
            );
        }
        let target_owner = engine
            .directory_git_status(&target.repository_id, "arrived")
            .unwrap()
            .directory_id;
        let mounted = engine
            .list_tree_ref(&target.main_branch_id)
            .unwrap()
            .iter()
            .find(|entry| entry.path() == "arrived/relocated/hidden")
            .unwrap();
        let imported = engine
            .directory_deletion(&target.repository_id, &target_owner, &mounted.entry_id)
            .unwrap()
            .clone();
        assert!(imported.files.is_empty() && engine.state().file_deletions.is_empty());
        assert_ne!(imported.node_id, original.node_id);
        (target_owner, imported, movement)
    };

    // Historical D remains readable, but its source owner is no longer mounted.
    let departed = list(&source.repository_id, &owner);
    assert!(departed.entries.is_empty() && departed.next_after.is_none());
    let arrived = list(&target.repository_id, &target_owner);
    assert_eq!(arrived.entries.len(), 1);
    assert_eq!(arrived.entries[0].directory_id, imported.directory_id);
    assert_eq!(arrived.entries[0].deletion_node_id, imported.node_id);
    assert_eq!(arrived.entries[0].operation_id, imported.operation_id);
    assert_eq!(arrived.entries[0].path.as_str(), "arrived/relocated/hidden");
    assert!(arrived.next_after.is_none());
    let engine = backend.engine.lock().unwrap();
    assert_eq!(
        engine.directory_deletion(&source.repository_id, &owner, &hidden.entry_id),
        Some(&original)
    );
    assert_eq!(engine.state().operations[&receipt.operation_id], receipt);
    assert_eq!(
        engine.directory_deletion(&target.repository_id, &target_owner, &imported.directory_id),
        Some(&imported)
    );
    assert!(
        engine
            .namespace_tree(&target.main_branch_id)
            .unwrap()
            .get("arrived/relocated/hidden")
            .is_none()
    );
    drop(engine);
    drop(auth);
    drop(backend);
    let recovered = Engine::open(&root.path().join("system")).unwrap();
    assert_directory_references(&recovered, &target);
    assert_eq!(
        recovered.operation(movement.operation_id).unwrap(),
        movement
    );
    assert_eq!(
        recovered.directory_deletion(&source.repository_id, &owner, &hidden.entry_id),
        Some(&original)
    );
    assert_eq!(
        recovered.directory_deletion(&target.repository_id, &target_owner, &imported.directory_id),
        Some(&imported)
    );
    for path in ["arrived/relocated", "arrived/relocated/hidden"] {
        ordinary_scope(&recovered, &target, path);
    }
    assert!(
        recovered
            .namespace_tree(&target.main_branch_id)
            .unwrap()
            .get("arrived/relocated/hidden")
            .is_none()
    );
}
