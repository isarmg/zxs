//! Acceptance of the documented composition and write-intent rules.
use crate::*;
use std::{collections::BTreeMap, io::Read};
fn fixture() -> (tempfile::TempDir, Engine, Repository) {
    let t = tempfile::tempdir().unwrap();
    let mut e = Engine::initialize(
        &t.path().join("system"),
        Config::development([t.path().join("p"), t.path().join("b"), t.path().join("c")]),
    )
    .unwrap();
    let r = e
        .create_disk_root("P01", "admin", storage_contract::OperationId::generate())
        .unwrap();
    (t, e, r)
}
fn save(e: &mut Engine, r: &Repository, path: &str, bytes: &[u8]) {
    let w = e.create_workspace(&r.main_branch_id, "admin").unwrap();
    e.stage_bytes(&w.workspace_id, path, bytes).unwrap();
    e.commit_workspace(CommitRequest {
        workspace_id: w.workspace_id,
        operation_id: storage_contract::OperationId::generate(),
        principal: "admin".into(),
        expected_head: w.base_head,
        fencing_token: w.fencing_token,
        message: "fixture save".into(),
        request_digest: None,
    })
    .unwrap();
}
fn entries(e: &Engine, r: &Repository) -> BTreeMap<String, Entry> {
    e.list_tree(&r.main_branch_id)
        .unwrap()
        .into_iter()
        .map(|x| (x.path().to_owned(), x))
        .collect()
}
fn body(e: &Engine, id: &str) -> Vec<u8> {
    let mut o = Vec::new();
    e.valid_replica(id)
        .unwrap()
        .0
        .open()
        .unwrap()
        .read_to_end(&mut o)
        .unwrap();
    o
}
fn create(e: &mut Engine, r: &Repository, path: &str, name: &str) -> String {
    e.create_directory_branches(
        &r.repository_id,
        (if (path).is_empty() {
            None
        } else {
            Some(name_policy::LookupPath::parse(path).unwrap())
        })
        .as_ref(),
        &storage_contract::BranchName::parse(name).unwrap(),
        "admin",
        storage_contract::OperationId::generate(),
    )
    .unwrap()
    .result["directory_branch_id"]
        .as_str()
        .unwrap()
        .into()
}
fn select(e: &mut Engine, r: &Repository, path: &str, name: &str) {
    e.select_directory_branches(
        &r.repository_id,
        (if (path).is_empty() {
            None
        } else {
            Some(name_policy::LookupPath::parse(path).unwrap())
        })
        .as_ref(),
        &storage_contract::BranchName::parse(name).unwrap(),
        "admin",
        storage_contract::OperationId::generate(),
    )
    .unwrap();
}
#[test]
fn directory_registration_reuses_nodes_and_lazy_first_write_forks_one_public_file() {
    let (_t, mut e, r) = fixture();
    save(&mut e, &r, "a/one.txt", b"one");
    save(&mut e, &r, "a/two.txt", b"two");
    let before = entries(&e, &r);
    let branches = e.state().branches.len();
    let versions = e.state().versions.len();
    let draft = create(&mut e, &r, "a", "草稿");
    assert_eq!(e.state().branches.len(), branches);
    assert_eq!(e.state().versions.len(), versions);
    let parent = e.state().directory_branches[&draft].head_tree_id.clone();
    select(&mut e, &r, "a", "草稿");
    assert_eq!(e.state().directory_branches[&draft].head_tree_id, parent);
    assert_eq!(body(&e, &entries(&e, &r)["a/one.txt"].version_id), b"one");
    let main = e.state().file_selections[&before["a/one.txt"].file_id].clone();
    let parent_head = e.state().branches[&main].head_commit_id.clone();
    // Identical first save is still a genuine first branch-owned node.
    save(&mut e, &r, "a/one.txt", b"one");
    let after = entries(&e, &r);
    let fork = e.state().file_selections[&after["a/one.txt"].file_id].clone();
    assert_ne!(fork, main);
    assert_eq!(e.state().branches[&fork].name, "草稿");
    assert_eq!(e.state().branches[&main].head_commit_id, parent_head);
    assert_ne!(
        after["a/one.txt"].version_id,
        before["a/one.txt"].version_id
    );
    assert_eq!(
        after["a/two.txt"].version_id,
        before["a/two.txt"].version_id
    );
    assert_eq!(e.state().versions.len(), versions + 1);
    assert_eq!(
        e.state().directory_branches[&draft].directory_branch_id,
        draft
    );
    assert!(e.state().directory_branches[&draft].materialized);
    let head = e.state().directory_branches[&draft].head_tree_id.clone();
    assert_eq!(
        e.state().directory_trees[&head].parent_tree_id.as_deref(),
        Some(parent.as_str())
    );
    save(&mut e, &r, "a/one.txt", b"next");
    assert_eq!(e.state().file_selections[&after["a/one.txt"].file_id], fork);
}
#[test]
fn selecting_old_directory_restores_names_membership_and_hierarchy() {
    let (_t, mut e, r) = fixture();
    save(&mut e, &r, "folder/old.txt", b"old");
    let draft = create(&mut e, &r, "folder", "snapshot");
    let old = e.state().directory_branches[&draft].head_tree_id.clone();
    e.file_namespace(
        &r.repository_id,
        NamespaceOperation::parse(
            NamespaceOperationKind::Rename,
            "folder/old.txt",
            Some("folder/new.txt"),
        )
        .unwrap(),
        "admin",
        storage_contract::OperationId::generate(),
    )
    .unwrap();
    save(&mut e, &r, "folder/nested/added.txt", b"added");
    select(&mut e, &r, "folder", "snapshot");
    let view = entries(&e, &r);
    assert!(view.contains_key("folder/old.txt"));
    assert!(!view.contains_key("folder/new.txt"));
    assert!(!view.contains_key("folder/nested"));
    assert_eq!(e.state().directory_branches[&draft].head_tree_id, old);
}
#[test]
fn explicit_same_node_selection_changes_intent_once_and_fences_old_workspaces() {
    let (_t, mut e, r) = fixture();
    save(&mut e, &r, "folder/a.txt", b"A");
    let file = entries(&e, &r)["folder/a.txt"].clone();
    let main = e.state().file_selections[&file.file_id].clone();
    create(&mut e, &r, "folder", "draft");
    select(&mut e, &r, "folder", "draft");
    let old = e.create_workspace(&r.main_branch_id, "admin").unwrap();
    let count = e.state().directory_trees.len();
    e.select_file_branch(
        &file.file_id,
        &main,
        "admin",
        storage_contract::OperationId::generate(),
    )
    .unwrap();
    assert!(e.state().directory_trees.len() > count);
    assert!(!e.state().file_write_origins.contains_key(&file.file_id));
    assert_eq!(
        e.stage_bytes(&old.workspace_id, "folder/a.txt", b"stale")
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::FileBranchConflict
    );
    let count = e.state().directory_trees.len();
    e.select_file_branch(
        &file.file_id,
        &main,
        "admin",
        storage_contract::OperationId::generate(),
    )
    .unwrap();
    assert_eq!(e.state().directory_trees.len(), count);
    save(&mut e, &r, "folder/a.txt", b"B");
    assert_eq!(e.state().file_selections[&file.file_id], main);
    assert!(
        !e.state()
            .branches
            .values()
            .any(|b| b.file_id.as_deref() == Some(&file.file_id) && b.name == "draft")
    );
}
#[test]
fn selecting_child_directory_uses_its_frozen_namespace_without_same_name_file_branches() {
    let (_t, mut e, r) = fixture();
    save(&mut e, &r, "folder/child/a.txt", b"old");
    create(&mut e, &r, "folder/child", "child-draft");
    let outer = create(&mut e, &r, "folder", "outer");
    select(&mut e, &r, "folder", "outer");
    let old = e.state().directory_branches[&outer].head_tree_id.clone();
    select(&mut e, &r, "folder/child", "child-draft");
    let head = e.state().directory_branches[&outer].head_tree_id.clone();
    assert_ne!(head, old);
    assert_eq!(
        e.state().directory_trees[&head].parent_tree_id.as_deref(),
        Some(old.as_str())
    );
    assert!(e.state().directory_branches[&outer].materialized);
}
fn item(e: &Engine, src: &str, path: &str, dst: Option<(&str, &str)>) -> DirectoryNamespaceItem {
    let b = &e.state().directory_branches[src];
    DirectoryNamespaceItem {
        source: DirectoryNamespaceSource {
            directory_branch_id: src.into(),
            expected_tree_id: b.head_tree_id.clone(),
            path: path.into(),
            entry_id: b
                .entries
                .iter()
                .find(|x| x.path == path)
                .unwrap()
                .entry_id
                .clone(),
        },
        target: dst.map(|(id, path)| DirectoryNamespaceTarget {
            directory_branch_id: id.into(),
            expected_tree_id: e.state().directory_branches[id].head_tree_id.clone(),
            path: path.into(),
        }),
    }
}

#[test]
fn batch_move_path_node_receipt_is_exact_ordered_and_replayed_after_restart() {
    let (temporary, mut engine, repo) = fixture();
    save(&mut engine, &repo, "a/z.txt", b"Z");
    save(&mut engine, &repo, "a/a.txt", b"A");
    save(&mut engine, &repo, "b/keep.txt", b"keep");
    engine
        .file_namespace(
            &repo.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::Rename, "a", Some("source")).unwrap(),
            "admin",
            OperationId::generate(),
        )
        .unwrap();
    let older_nodes = engine.state().file_path_nodes.clone();
    assert_eq!(older_nodes.len(), 2);
    let source = create(&mut engine, &repo, "source", "source branch");
    let target = create(&mut engine, &repo, "b", "target branch");
    select(&mut engine, &repo, "source", "source branch");
    select(&mut engine, &repo, "b", "target branch");
    let request = DirectoryNamespaceBatch {
        kind: DirectoryBatchKind::Move,
        items: vec![
            item(&engine, &source, "source/z.txt", Some((&target, "b/z.txt"))),
            item(&engine, &source, "source/a.txt", Some((&target, "b/a.txt"))),
        ],
    };
    let operation = OperationId::generate();
    let receipt = engine
        .directory_namespace_batch(&repo.repository_id, &request, "admin", operation)
        .unwrap();
    let expected = engine
        .state()
        .file_path_nodes
        .values()
        .filter(|node| node.operation_id == operation && node.sequence == receipt.sequence)
        .map(|node| node.node_id.clone())
        .collect::<Vec<_>>();
    assert_eq!(expected.len(), 2);
    assert!(expected.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(receipt.result["file_node_ids"], json_codec::json!(expected));
    assert!(older_nodes.values().all(|node| {
        node.operation_id != operation
            && node.sequence < receipt.sequence
            && !expected.contains(&node.node_id)
    }));
    engine
        .file_namespace(
            &repo.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::Rename, "b", Some("later")).unwrap(),
            "admin",
            OperationId::generate(),
        )
        .unwrap();
    assert!(
        engine
            .state()
            .file_path_nodes
            .values()
            .any(|node| { node.operation_id != operation && node.sequence > receipt.sequence })
    );
    for reopened in [false, true] {
        if reopened {
            drop(engine);
            engine = Engine::open(&temporary.path().join("system")).unwrap();
        }
        let before = canonical_json(engine.state()).unwrap();
        let replay = engine
            .directory_namespace_batch(&repo.repository_id, &request, "admin", operation)
            .unwrap();
        assert_eq!(replay.result, receipt.result);
        assert_eq!(replay.request_digest, receipt.request_digest);
        assert_eq!(replay.sequence, receipt.sequence);
        assert_eq!(canonical_json(engine.state()).unwrap(), before);
    }
}

#[test]
fn move_transfers_following_but_retains_unselected_frozen_alias_and_identity() {
    let (t, mut e, r) = fixture();
    save(&mut e, &r, "a/f.txt", b"F1");
    save(&mut e, &r, "b/keep.txt", b"keep");
    let f = entries(&e, &r)["a/f.txt"].clone();
    let m = create(&mut e, &r, "a", "m");
    let n = create(&mut e, &r, "a", "n");
    let l = create(&mut e, &r, "b", "l");
    let p = create(&mut e, &r, "b", "p");
    select(&mut e, &r, "a", "m");
    select(&mut e, &r, "b", "l");
    let n_head = e.state().directory_branches[&n].head_tree_id.clone();
    let p_head = e.state().directory_branches[&p].head_tree_id.clone();
    let versions = e.state().versions.len();
    let request = DirectoryNamespaceBatch {
        kind: storage_contract::DirectoryBatchKind::Move,
        items: vec![item(&e, &m, "a/f.txt", Some((&l, "b/f.txt")))],
    };
    let op = storage_contract::OperationId::generate();
    let receipt = e
        .directory_namespace_batch(&r.repository_id, &request, "admin", op)
        .unwrap();
    assert_eq!(
        e.directory_namespace_batch(&r.repository_id, &request, "admin", op)
            .unwrap()
            .result,
        receipt.result
    );
    let view = entries(&e, &r);
    assert!(!view.contains_key("a/f.txt"));
    assert_eq!(view["b/f.txt"].file_id, f.file_id);
    assert_eq!(view["b/f.txt"].version_id, f.version_id);
    assert_eq!(e.state().versions.len(), versions);
    assert_eq!(e.state().file_locations[&f.file_id], "b/f.txt");
    let m_head = e.state().directory_branches[&m].head_tree_id.clone();
    save(&mut e, &r, "b/f.txt", b"F2");
    assert_eq!(e.state().directory_branches[&m].head_tree_id, m_head);
    assert_eq!(e.state().directory_branches[&n].head_tree_id, n_head);
    assert_eq!(e.state().directory_branches[&p].head_tree_id, p_head);
    assert_eq!(body(&e, &f.version_id), b"F1");
    assert_eq!(body(&e, &entries(&e, &r)["b/f.txt"].version_id), b"F2");
    drop(e);
    let e = Engine::open(&t.path().join("system")).unwrap();
    assert_eq!(entries(&e, &r)["b/f.txt"].file_id, f.file_id);
    assert_eq!(body(&e, &f.version_id), b"F1");
}
#[test]
fn batch_move_recovery_keeps_source_and_target_in_one_decision() {
    for (fault, published) in [
        ("after_prepare_1", false),
        ("after_accept_2", true),
        ("after_container_names", true),
    ] {
        let (t, mut e, r) = fixture();
        save(&mut e, &r, "a/f.txt", b"source");
        save(&mut e, &r, "b/keep.txt", b"keep");
        let f = entries(&e, &r)["a/f.txt"].clone();
        let m = create(&mut e, &r, "a", "m");
        let l = create(&mut e, &r, "b", "l");
        select(&mut e, &r, "a", "m");
        select(&mut e, &r, "b", "l");
        let request = DirectoryNamespaceBatch {
            kind: storage_contract::DirectoryBatchKind::Move,
            items: vec![item(&e, &m, "a/f.txt", Some((&l, "b/f.txt")))],
        };
        let op = storage_contract::OperationId::generate();
        e.inject_fault_once(fault);
        let error = e
            .directory_namespace_batch(&r.repository_id, &request, "admin", op)
            .unwrap_err();
        assert_eq!(
            error.code(),
            if published {
                storage_contract::ErrorCode::InDoubt
            } else {
                storage_contract::ErrorCode::InjectedFailure
            }
        );
        drop(e);
        let mut e = Engine::open(&t.path().join("system")).unwrap();
        let view = entries(&e, &r);
        assert_eq!(view.contains_key("b/f.txt"), published);
        assert_eq!(view.contains_key("a/f.txt"), !published);
        assert_eq!(body(&e, &f.version_id), b"source");
        let result = e
            .directory_namespace_batch(&r.repository_id, &request, "admin", op)
            .unwrap();
        assert_eq!(
            result.commit_state,
            storage_contract::CommitState::Published
        );
        assert_eq!(entries(&e, &r)["b/f.txt"].file_id, f.file_id);
    }
}
#[test]
fn batch_rejects_ambiguous_move_and_late_collision_without_any_branch_change() {
    let (_t, mut e, r) = fixture();
    save(&mut e, &r, "a/f.txt", b"source");
    save(&mut e, &r, "b/exists.txt", b"exists");
    save(&mut e, &r, "c/keep.txt", b"keep");
    let m = create(&mut e, &r, "a", "m");
    let n = create(&mut e, &r, "a", "n");
    let l = create(&mut e, &r, "b", "l");
    let q = create(&mut e, &r, "c", "q");
    let before = canonical_json(e.state()).unwrap();
    for (expected, items) in [
        (
            storage_contract::ErrorCode::BatchMappingConflict,
            vec![
                item(&e, &m, "a/f.txt", Some((&l, "b/f.txt"))),
                item(&e, &n, "a/f.txt", Some((&q, "c/f.txt"))),
            ],
        ),
        (
            storage_contract::ErrorCode::NameCollision,
            vec![
                item(&e, &m, "a/f.txt", Some((&l, "b/f.txt"))),
                item(&e, &n, "a/f.txt", Some((&l, "b/exists.txt"))),
            ],
        ),
    ] {
        let error = e
            .directory_namespace_batch(
                &r.repository_id,
                &DirectoryNamespaceBatch {
                    kind: storage_contract::DirectoryBatchKind::Move,
                    items,
                },
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap_err();
        assert_eq!(error.code(), expected);
        assert_eq!(canonical_json(e.state()).unwrap(), before);
    }
}
#[test]
fn inherited_automatic_file_branch_name_is_utf8_safe_and_collision_free() {
    let (_t, mut e, r) = fixture();
    save(&mut e, &r, "a/f.txt", b"parent");
    let name = "汉".repeat(42);
    let first = create(&mut e, &r, "a", &name);
    select(&mut e, &r, "a", &name);
    save(&mut e, &r, "a/f.txt", b"first");
    let original = entries(&e, &r)["a/f.txt"].clone();
    let existing = e.state().file_selections[&original.file_id].clone();
    e.rename_directory_branch(
        &r.repository_id,
        &first,
        &storage_contract::BranchName::parse("renamed").unwrap(),
        "admin",
        storage_contract::OperationId::generate(),
    )
    .unwrap();
    create(&mut e, &r, "a", &name);
    select(&mut e, &r, "a", &name);
    save(&mut e, &r, "a/f.txt", b"second");
    let branch = &e.state().branches[&e.state().file_selections[&original.file_id]];
    assert_ne!(branch.branch_id, existing);
    assert!(branch.name.len() <= 128);
    assert!(branch.name.ends_with("-1"));
    assert!(std::str::from_utf8(branch.name.as_bytes()).is_ok());
}
#[test]
fn first_lazy_save_receipt_allows_the_same_writer_to_continue_but_fences_later_selection() {
    let (_t, mut e, r) = fixture();
    save(&mut e, &r, "a/f.txt", b"parent");
    create(&mut e, &r, "a", "draft");
    select(&mut e, &r, "a", "draft");
    let w = e.create_workspace(&r.main_branch_id, "admin").unwrap();
    e.stage_bytes(&w.workspace_id, "a/f.txt", b"own1").unwrap();
    e.commit_workspace(CommitRequest {
        workspace_id: w.workspace_id.clone(),
        operation_id: storage_contract::OperationId::generate(),
        principal: "admin".into(),
        expected_head: w.base_head,
        fencing_token: w.fencing_token,
        message: "first own save".into(),
        request_digest: None,
    })
    .unwrap();
    e.validate_workspace_file_selection(
        &w.workspace_id,
        &name_policy::PortablePath::parse("a/f.txt").unwrap(),
    )
    .unwrap();
    save(&mut e, &r, "a/f.txt", b"own2");
    e.validate_workspace_file_selection(
        &w.workspace_id,
        &name_policy::PortablePath::parse("a/f.txt").unwrap(),
    )
    .unwrap();
    select(&mut e, &r, "a", "main");
    assert_eq!(
        e.validate_workspace_file_selection(
            &w.workspace_id,
            &name_policy::PortablePath::parse("a/f.txt").unwrap()
        )
        .unwrap_err()
        .code(),
        storage_contract::ErrorCode::DirectoryBranchConflict
    );
}
#[test]
fn a_directory_created_after_workspace_open_still_fences_new_file_staging() {
    let (_t, mut e, r) = fixture();
    let w = e.create_workspace(&r.main_branch_id, "admin").unwrap();
    e.file_namespace(
        &r.repository_id,
        NamespaceOperation::parse(NamespaceOperationKind::Mkdir, "new", None).unwrap(),
        "admin",
        storage_contract::OperationId::generate(),
    )
    .unwrap();
    create(&mut e, &r, "new", "draft");
    select(&mut e, &r, "new", "draft");
    assert_eq!(
        e.stage_bytes(&w.workspace_id, "new/f.txt", b"stale")
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::DirectoryBranchConflict
    );
}

#[test]
fn content_save_with_equivalent_spelling_preserves_the_existing_display_path() {
    for (original_path, lookup_path) in [
        ("Report.TXT", "report.txt"),
        ("Folder/Straße.txt", "FOLDER/STRASSE.txt"),
    ] {
        let (temporary, mut e, r) = fixture();
        save(&mut e, &r, original_path, b"before");
        let original = entries(&e, &r)[original_path].clone();
        save(&mut e, &r, lookup_path, b"after");
        let current = entries(&e, &r)[original_path].clone();
        assert_eq!(current.file_id, original.file_id);
        assert_eq!(current.entry_id, original.entry_id);
        assert_ne!(current.version_id, original.version_id);
        assert_eq!(body(&e, &current.version_id), b"after");
        assert_eq!(e.state().file_locations[&current.file_id], original_path);
        assert_eq!(e.state().file_names[&current.file_id], original.name());
        assert_eq!(
            e.state().versions[&current.version_id].creation_name,
            original.name()
        );
        assert!(e.state().file_path_nodes.is_empty());
        let selected = &e.state().file_selections[&current.file_id];
        assert!(e.list_tree(selected).unwrap().iter().any(|entry| {
            entry.file_id == current.file_id && entry.version_id == current.version_id
        }));
        let versions = e.state().versions.len();
        save(&mut e, &r, lookup_path, b"after");
        assert_eq!(e.state().versions.len(), versions);
        assert_eq!(entries(&e, &r)[original_path], current);
        drop(e);
        let mut e = Engine::open(&temporary.path().join("system")).unwrap();
        assert_eq!(entries(&e, &r)[original_path], current);
        // Explicit equivalent-key renames remain real metadata changes.
        e.file_namespace(
            &r.repository_id,
            NamespaceOperation::parse(
                NamespaceOperationKind::Rename,
                original_path,
                Some(lookup_path),
            )
            .unwrap(),
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
        // Display paths are reconstructed from component names: a file rename
        // respells its own leaf, while ancestor spelling stays owned by the
        // directory entry (the flat model allowed incoherent per-entry paths).
        let renamed_display = match (original_path.rsplit_once('/'), lookup_path.rsplit_once('/')) {
            (Some((parent, _)), Some((_, leaf))) => format!("{parent}/{leaf}"),
            _ => lookup_path.to_owned(),
        };
        assert_eq!(
            entries(&e, &r)[renamed_display.as_str()].version_id,
            current.version_id
        );
        assert!(e.state().file_path_nodes.values().any(|node| {
            node.file_id == current.file_id
                && node.path == renamed_display
                && node.version_id == current.version_id
        }));
    }
}

#[test]
fn namespace_bookkeeping_uses_paths_and_keeps_the_first_match_in_unsorted_entries() {
    let mut state = State::default();
    let mut before = Vec::new();
    for (path, file) in [
        ("Z.txt", "shared"),
        ("A.txt", "shared"),
        ("Gone.txt", "removed"),
    ] {
        let key = name_policy::path_key(path);
        before.push(Entry {
            entry_id: path.into(),
            path: name_policy::PortablePath::parse(path).unwrap(),
            kind: NamespaceEntryKind::File,
            file_id: file.into(),
            version_id: "version".into(),
            size: 1,
        });
        state.file_contexts.insert(
            format!("repo/{key}"),
            FileContext {
                node_id: Some(format!("node-{path}")),
                selection_generation: 7,
                branch_id: format!("branch-{path}"),
                inherited_from: Some("inherited".into()),
            },
        );
        state.file_selections.insert(file.into(), "branch".into());
        state
            .file_write_origins
            .insert(file.into(), "inherited".into());
    }
    let mut after = vec![before[0].clone(), before[1].clone()];
    after[0].version_id = "updated".into();
    // The paged wire cannot carry duplicate sibling keys at all, so the old
    // shadowing hazard is structurally impossible; bookkeeping still resolves
    // by normalized path key against both views.
    let before_namespace = crate::namespace::fixture_store::namespace(&before);
    let after_namespace = crate::namespace::fixture_store::namespace(&after);
    let expected = state.file_contexts.clone();
    let mut fresh = crate::namespace::NamespaceObjects::new();
    super::directory::namespace_locations(
        &mut state,
        "repo",
        &before_namespace,
        &after_namespace,
        &[NamespaceOperation::parse(NamespaceOperationKind::Delete, "Gone.txt", None).unwrap()],
        OperationContext {
            principal: "admin",
            operation_id: OperationId::generate(),
        },
        NamespaceMutation {
            base: &crate::namespace::fixture_store::FixtureSource,
            fresh: &mut fresh,
        },
    )
    .unwrap();
    assert_eq!(state.file_contexts.len(), 2);
    for key in ["repo/z.txt", "repo/a.txt"] {
        assert_eq!(state.file_contexts.get(key), expected.get(key));
    }
    assert!(state.file_selections.contains_key("shared"));
    assert!(state.file_write_origins.contains_key("shared"));
    assert!(!state.file_selections.contains_key("removed"));
    assert!(!state.file_write_origins.contains_key("removed"));
    assert!(state.file_path_nodes.is_empty());
}

#[test]
fn system_clock_seconds_preserve_epoch_and_reject_before_epoch() {
    use std::{
        error::Error as _,
        time::{Duration, UNIX_EPOCH},
    };
    assert_eq!(super::unix_seconds(UNIX_EPOCH).unwrap(), 0);
    assert_eq!(
        super::unix_seconds(UNIX_EPOCH + Duration::new(42, 999_999_999)).unwrap(),
        42
    );
    let error = super::unix_seconds(UNIX_EPOCH - Duration::from_nanos(1)).unwrap_err();
    assert_eq!(error.code(), storage_contract::ErrorCode::IoError);
    assert!(matches!(error, Error::Clock(_)));
    assert_eq!(
        error
            .source()
            .unwrap()
            .downcast_ref::<std::time::SystemTimeError>()
            .unwrap()
            .duration(),
        Duration::from_nanos(1)
    );
    let large = 253_402_300_800;
    assert_eq!(
        super::unix_seconds(UNIX_EPOCH + Duration::from_secs(large)).unwrap(),
        large
    );
}

#[test]
fn system_clock_receipt_replay_bypasses_new_mutation_planning() {
    let (_temp, engine, _) = fixture();
    let receipt = engine
        .state
        .operations
        .tail()
        .first()
        .unwrap()
        .result
        .clone();
    let sequence = engine.state.sequence;
    let plan = engine
        .plan_mutation_digest(
            &receipt.kind,
            "admin",
            receipt.operation_id,
            json_codec::Value::Null,
            Some(receipt.request_digest.clone()),
            |_, _, _| panic!("receipt replay must not enter new mutation planning"),
        )
        .unwrap();
    let crate::engine::MutationPlan::Existing(replayed) = plan else {
        panic!("receipt replay must not prepare a new mutation");
    };
    assert_eq!(replayed.operation_id, receipt.operation_id);
    assert_eq!(replayed.result["timestamp"], receipt.result["timestamp"]);
    assert_eq!(engine.state.sequence, sequence);
}
