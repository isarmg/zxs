use crate::*;
use std::{collections::BTreeMap, io::Read};
fn setup() -> (tempfile::TempDir, Engine, Repository) {
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
fn save(e: &mut Engine, r: &Repository, path: &str, data: &[u8]) {
    let w = e.create_workspace(&r.main_branch_id, "admin").unwrap();
    e.stage_bytes(&w.workspace_id, path, data).unwrap();
    e.commit_workspace(CommitRequest {
        workspace_id: w.workspace_id,
        operation_id: storage_contract::OperationId::generate(),
        principal: "admin".into(),
        expected_head: w.base_head,
        fencing_token: w.fencing_token,
        message: "path fixture".into(),
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
fn rename(e: &mut Engine, r: &Repository, from: &str, to: &str) -> OperationResult {
    e.file_namespace(
        &r.repository_id,
        NamespaceOperation::parse(NamespaceOperationKind::Rename, from, Some(to)).unwrap(),
        "admin",
        storage_contract::OperationId::generate(),
    )
    .unwrap()
}

#[test]
fn path_node_receipts_are_ordered_isolated_and_replayed_after_later_renames() {
    let (temporary, mut engine, repo) = setup();
    save(&mut engine, &repo, "folder/z.txt", b"Z");
    save(&mut engine, &repo, "folder/a.txt", b"A");
    rename(&mut engine, &repo, "folder", "first");
    let operation = OperationId::generate();
    let change =
        NamespaceOperation::parse(NamespaceOperationKind::Rename, "first", Some("second")).unwrap();
    let receipt = engine
        .file_namespace(&repo.repository_id, change.clone(), "admin", operation)
        .unwrap();
    let expected = engine
        .state()
        .file_path_nodes
        .values()
        .filter(|node| node.operation_id == operation)
        .map(|node| node.node_id.clone())
        .collect::<Vec<_>>();
    assert_eq!(expected.len(), 2);
    assert_eq!(receipt.result["file_node_ids"], json_codec::json!(expected));
    rename(&mut engine, &repo, "second", "third");
    assert_eq!(engine.state().file_path_nodes.len(), 6);
    for reopened in [false, true] {
        if reopened {
            drop(engine);
            engine = Engine::open(&temporary.path().join("system")).unwrap();
        }
        let before = canonical_json(engine.state()).unwrap();
        let replay = engine
            .file_namespace(&repo.repository_id, change.clone(), "admin", operation)
            .unwrap();
        assert_eq!(replay.result, receipt.result);
        assert_eq!(replay.request_digest, receipt.request_digest);
        assert_eq!(canonical_json(engine.state()).unwrap(), before);
    }
}

#[test]
fn workspace_path_node_receipt_is_exact_ordered_and_replayed_after_restart() {
    let (temporary, mut engine, repo) = setup();
    save(&mut engine, &repo, "folder/z.txt", b"Z");
    save(&mut engine, &repo, "folder/a.txt", b"A");
    rename(&mut engine, &repo, "folder", "first");
    let older_nodes = engine.state().file_path_nodes.clone();
    assert_eq!(older_nodes.len(), 2);
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .namespace_operation(
            &workspace.workspace_id,
            NamespaceOperation::parse(NamespaceOperationKind::Rename, "first", Some("second"))
                .unwrap(),
        )
        .unwrap();
    let operation = OperationId::generate();
    let request = CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: operation,
        principal: "admin".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "workspace rename receipt".into(),
        request_digest: None,
    };
    let receipt = engine.commit_workspace(request.clone()).unwrap();
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
    rename(&mut engine, &repo, "second", "third");
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
        let replay = engine.commit_workspace(request.clone()).unwrap();
        assert_eq!(replay.result, receipt.result);
        assert_eq!(replay.request_digest, receipt.request_digest);
        assert_eq!(replay.sequence, receipt.sequence);
        assert_eq!(canonical_json(engine.state()).unwrap(), before);
    }
}

#[test]
fn folder_and_file_rename_append_path_nodes_reusing_bodies_then_write_follows_the_path_node() {
    let (t, mut e, r) = setup();
    save(&mut e, &r, "folder/a.txt", b"A");
    save(&mut e, &r, "folder/nested/b.txt", b"B");
    e.file_namespace(
        &r.repository_id,
        NamespaceOperation::parse(NamespaceOperationKind::Mkdir, "folder/empty", None).unwrap(),
        "admin",
        storage_contract::OperationId::generate(),
    )
    .unwrap();
    let old = entries(&e, &r);
    let folderid = &old["folder"].entry_id;
    let selected = e.state().directory_selections[folderid].clone();
    let head = e.state().directory_branches[&selected].head_tree_id.clone();
    let untouched = e
        .create_directory_branches(
            &r.repository_id,
            Some(&name_policy::LookupPath::parse("folder").unwrap()),
            &storage_contract::BranchName::parse("untouched").unwrap(),
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap()
        .result["directory_branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let saved_untouched = e.state().directory_branches[&untouched]
        .head_tree_id
        .clone();
    let bodies = e.state().versions.len();
    rename(&mut e, &r, "folder", "renamed");
    let view = entries(&e, &r);
    assert_eq!(e.state().versions.len(), bodies);
    assert_eq!(e.state().file_path_nodes.len(), 2);
    let newhead = &e.state().directory_branches[&selected].head_tree_id;
    assert_ne!(newhead, &head);
    assert_eq!(
        e.state().directory_trees[newhead].parent_tree_id.as_deref(),
        Some(head.as_str())
    );
    assert_eq!(e.state().directory_branches[&selected].path, "renamed");
    assert_eq!(
        e.state().directory_branches[&untouched].head_tree_id,
        saved_untouched
    );
    for (from, to) in [
        ("folder/a.txt", "renamed/a.txt"),
        ("folder/nested/b.txt", "renamed/nested/b.txt"),
    ] {
        assert_eq!(view[to].version_id, old[from].version_id);
        let node = e
            .file_context(&r.repository_id, to)
            .unwrap()
            .node_id
            .unwrap();
        let n = &e.state().file_path_nodes[&node];
        assert_eq!(n.previous_path, from);
        assert_eq!(n.path, to);
        assert_eq!(n.version_id, old[from].version_id);
        assert_eq!(n.parent_node_ids, vec![old[from].version_id.clone()]);
    }
    let first = e
        .file_context(&r.repository_id, "renamed/a.txt")
        .unwrap()
        .node_id
        .unwrap();
    rename(&mut e, &r, "renamed/a.txt", "renamed/final.txt");
    let second = e
        .file_context(&r.repository_id, "renamed/final.txt")
        .unwrap()
        .node_id
        .unwrap();
    assert_eq!(
        e.state().file_path_nodes[&second].parent_node_ids,
        vec![first]
    );
    save(&mut e, &r, "renamed/final.txt", b"A changed");
    let finalentry = entries(&e, &r)["renamed/final.txt"].clone();
    assert_eq!(e.state().versions.len(), bodies + 1);
    assert_eq!(
        e.state().file_node_parents[&finalentry.version_id],
        vec![second]
    );
    assert_eq!(
        e.state().versions[&finalentry.version_id].source_version_ids,
        vec![old["folder/a.txt"].version_id.clone()]
    );
    assert!(
        e.file_context(&r.repository_id, "renamed/final.txt")
            .unwrap()
            .node_id
            .is_none()
    );
    drop(e);
    let e = Engine::open(&t.path().join("system")).unwrap();
    let mut data = Vec::new();
    std::fs::File::open(e.valid_replica(&finalentry.version_id).unwrap().0)
        .unwrap()
        .read_to_end(&mut data)
        .unwrap();
    assert_eq!(data, b"A changed");
    assert_eq!(e.state().file_path_nodes.len(), 3);
}
#[test]
fn path_node_note_and_new_branch_are_metadata_and_preserve_the_named_source_node() {
    let (_t, mut e, r) = setup();
    save(&mut e, &r, "a.txt", b"body");
    rename(&mut e, &r, "a.txt", "b.txt");
    let file = entries(&e, &r)["b.txt"].clone();
    let node = e
        .file_context(&r.repository_id, "b.txt")
        .unwrap()
        .node_id
        .unwrap();
    let trees = e.state().directory_trees.len();
    let versions = e.state().versions.len();
    e.set_file_node_note(
        &file.file_id,
        &node,
        &storage_contract::Note::parse("rename note".into()).unwrap(),
        "admin",
        storage_contract::OperationId::generate(),
    )
    .unwrap();
    assert_eq!(e.state().directory_trees.len(), trees);
    assert_eq!(e.state().versions.len(), versions);
    let branch = e
        .create_file_branch_from_node(
            &file.file_id,
            &file.version_id,
            &node,
            &storage_contract::BranchName::parse("from rename").unwrap(),
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap()
        .result["branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    e.select_file_branch_at(
        &r.repository_id,
        &name_policy::PortablePath::parse("b.txt").unwrap(),
        &file.file_id,
        &branch,
        "admin",
        storage_contract::OperationId::generate(),
    )
    .unwrap();
    assert_eq!(
        e.file_context(&r.repository_id, "b.txt")
            .unwrap()
            .node_id
            .as_deref(),
        Some(node.as_str())
    );
    save(&mut e, &r, "b.txt", b"new content");
    let new = entries(&e, &r)["b.txt"].version_id.clone();
    assert_eq!(e.state().file_node_parents[&new], vec![node]);
    assert!(e.state().branches[&branch].file_node_id.is_none());
}

#[test]
fn contextual_selection_rejects_unavailable_owned_content_without_publication() {
    let (_t, mut e, r) = setup();
    save(&mut e, &r, "a.txt", b"original body");
    let original = entries(&e, &r)["a.txt"].clone();
    let original_branch = e.file_context(&r.repository_id, "a.txt").unwrap().branch_id;
    let branch = e
        .create_file_branch(
            &original.file_id,
            &original.version_id,
            &storage_contract::BranchName::parse("other").unwrap(),
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap()
        .result["branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    e.select_file_branch_at(
        &r.repository_id,
        &name_policy::PortablePath::parse("a.txt").unwrap(),
        &original.file_id,
        &branch,
        "admin",
        storage_contract::OperationId::generate(),
    )
    .unwrap();
    save(&mut e, &r, "a.txt", b"available body");
    // Lose only the old body's physical replicas, leaving its active branch and
    // all immutable history metadata intact.
    while let Ok((path, _)) = e.valid_replica(&original.version_id) {
        std::fs::remove_file(path).unwrap();
    }
    let before = canonical_json(e.state()).unwrap();
    assert_eq!(
        e.select_file_branch(
            &original.file_id,
            &original_branch,
            "admin",
            storage_contract::OperationId::generate()
        )
        .unwrap_err()
        .code(),
        storage_contract::ErrorCode::ContentUnavailable
    );
    assert_eq!(
        e.select_file_branch_at(
            &r.repository_id,
            &name_policy::PortablePath::parse("a.txt").unwrap(),
            &original.file_id,
            &original_branch,
            "admin",
            storage_contract::OperationId::generate()
        )
        .unwrap_err()
        .code(),
        storage_contract::ErrorCode::ContentUnavailable
    );
    assert_eq!(canonical_json(e.state()).unwrap(), before);
    assert_eq!(
        e.file_context(&r.repository_id, "a.txt").unwrap().branch_id,
        branch
    );
}

#[test]
fn pending_selection_preserves_frozen_source_even_when_source_replicas_are_missing() {
    let (root, mut engine, repo) = setup();
    save(&mut engine, &repo, "file.txt", b"frozen source");
    let source = entries(&engine, &repo)["file.txt"].clone();
    let pending = engine
        .create_file_branch(
            &source.file_id,
            &source.version_id,
            &storage_contract::BranchName::parse("pending").unwrap(),
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap()
        .result["branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    save(&mut engine, &repo, "file.txt", b"current body");
    loop {
        match engine.valid_replica(&source.version_id) {
            Ok((path, _)) => std::fs::remove_file(path).unwrap(),
            Err(error) if error.code() == storage_contract::ErrorCode::ContentUnavailable => break,
            Err(error) => panic!("unexpected replica failure: {error}"),
        }
    }
    let path = name_policy::PortablePath::parse("file.txt").unwrap();
    let operation = storage_contract::OperationId::generate();
    let selected = engine
        .select_file_branch_at(
            &repo.repository_id,
            &path,
            &source.file_id,
            &pending,
            "admin",
            operation,
        )
        .unwrap();
    engine
        .select_file_branch(
            &source.file_id,
            &pending,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    assert_eq!(
        engine.state().branches[&pending].pending_file_versions[&source.file_id],
        source.version_id
    );
    assert_eq!(
        engine
            .file_context(&repo.repository_id, "file.txt")
            .unwrap()
            .branch_id,
        pending
    );
    drop(engine);
    let mut recovered = Engine::open(&root.path().join("system")).unwrap();
    let replay = recovered
        .select_file_branch_at(
            &repo.repository_id,
            &path,
            &source.file_id,
            &pending,
            "admin",
            operation,
        )
        .unwrap();
    assert_eq!(replay.result, selected.result);
    assert_eq!(replay.request_digest, selected.request_digest);
    assert_eq!(
        recovered.state().branches[&pending].pending_file_versions[&source.file_id],
        source.version_id
    );
}

#[test]
fn file_merge_requires_readable_chosen_body_but_receipt_replay_does_not() {
    for publish_first in [false, true] {
        let (_root, mut engine, repo) = setup();
        save(&mut engine, &repo, "file.txt", b"chosen source");
        let source = entries(&engine, &repo)["file.txt"].clone();
        let branches = ["left", "right"].map(|name| {
            engine
                .create_file_branch(
                    &source.file_id,
                    &source.version_id,
                    &storage_contract::BranchName::parse(name).unwrap(),
                    "admin",
                    storage_contract::OperationId::generate(),
                )
                .unwrap()
                .result["branch_id"]
                .as_str()
                .unwrap()
                .to_owned()
        });
        let inputs = branches
            .iter()
            .map(|branch| storage_contract::FileMergeInput {
                branch_id: branch.clone(),
                expected_node_id: source.version_id.clone(),
            })
            .collect::<Vec<_>>();
        let operation = storage_contract::OperationId::generate();
        let request = |operation_id| crate::FileBranchMergeRequest {
            file_id: &source.file_id,
            inputs: &inputs,
            chosen_node_id: &source.version_id,
            surviving_branch_id: &branches[0],
            location: None,
            principal: "admin",
            operation_id,
        };
        let receipt = if publish_first {
            Some(engine.merge_file_branches(request(operation)).unwrap())
        } else {
            None
        };
        loop {
            match engine.valid_replica(&source.version_id) {
                Ok((path, _)) => std::fs::remove_file(path).unwrap(),
                Err(error) if error.code() == storage_contract::ErrorCode::ContentUnavailable => {
                    break;
                }
                Err(error) => panic!("unexpected replica failure: {error}"),
            }
        }
        let before = crate::canonical_json(engine.state()).unwrap();
        assert_eq!(
            engine
                .merge_file_branches(request(storage_contract::OperationId::generate()))
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::ContentUnavailable
        );
        if let Some(receipt) = receipt {
            assert_eq!(
                engine
                    .merge_file_branches(request(operation))
                    .unwrap()
                    .result,
                receipt.result
            );
        }
        assert_eq!(crate::canonical_json(engine.state()).unwrap(), before);
    }
}
