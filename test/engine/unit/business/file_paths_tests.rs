use crate::*;
use std::{collections::BTreeMap, io::Read};
fn setup() -> (tempfile::TempDir, Engine, Repository) {
    let t = tempfile::tempdir().unwrap();
    let mut e = Engine::initialize(
        &t.path().join("system"),
        Config::development([t.path().join("p"), t.path().join("b"), t.path().join("c")]).unwrap(),
    )
    .unwrap();
    let r = e
        .create_disk_root(
            "P01",
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    (t, e, r)
}
fn save(e: &mut Engine, r: &Repository, path: &str, data: &[u8]) {
    let w = e.create_workspace(&r.main_branch_id, "admin").unwrap();
    e.stage_bytes(&w.workspace_id, path, data).unwrap();
    e.commit_workspace(CommitRequest {
        workspace_id: w.workspace_id,
        operation_id: storage_contract::OperationId::generate().unwrap(),
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
        storage_contract::OperationId::generate().unwrap(),
    )
    .unwrap()
}

#[test]
fn path_fact_receipts_are_ordered_isolated_and_replayed_after_later_renames() {
    let (temporary, mut engine, repo) = setup();
    save(&mut engine, &repo, "folder/z.txt", b"Z");
    save(&mut engine, &repo, "folder/a.txt", b"A");
    rename(&mut engine, &repo, "folder", "first");
    let operation = OperationId::generate().unwrap();
    let change =
        NamespaceOperation::parse(NamespaceOperationKind::Rename, "first", Some("second")).unwrap();
    let receipt = engine
        .file_namespace(&repo.repository_id, change.clone(), "admin", operation)
        .unwrap();
    let expected = crate::persist::path_changes::read(&receipt).unwrap();
    assert_eq!(expected.len(), 2);
    assert!(expected.windows(2).all(|pair| pair[0].event_id < pair[1].event_id));
    assert!(expected.iter().all(|fact| {
        fact.before.path.as_str().starts_with("first/")
            && fact.after.path.as_str().starts_with("second/")
            && fact.before_version_id == fact.after_version_id
    }));
    let branches = engine.state().branches.values()
        .filter(|branch| branch.file_id.is_some()).cloned().collect::<Vec<_>>();
    let versions = engine.state().versions.clone();
    let parents = engine.state().file_node_parents.clone();
    rename(&mut engine, &repo, "second", "third");
    assert_eq!(engine.state().branches.values()
        .filter(|branch| branch.file_id.is_some()).cloned().collect::<Vec<_>>(), branches);
    assert_eq!(engine.state().versions, versions);
    assert_eq!(engine.state().file_node_parents, parents);
    assert!(engine.state().file_path_nodes.is_empty());
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
fn workspace_path_fact_receipt_is_exact_ordered_and_replayed_after_restart() {
    let (temporary, mut engine, repo) = setup();
    save(&mut engine, &repo, "folder/z.txt", b"Z");
    save(&mut engine, &repo, "folder/a.txt", b"A");
    rename(&mut engine, &repo, "folder", "first");
    let older_nodes = engine.state().file_path_nodes.clone();
    assert!(older_nodes.is_empty());
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
    let operation = OperationId::generate().unwrap();
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
    let expected = crate::persist::path_changes::read(&receipt).unwrap();
    assert_eq!(expected.len(), 2);
    assert!(expected.windows(2).all(|pair| pair[0].event_id < pair[1].event_id));
    assert!(expected.iter().all(|fact| {
        fact.operation_id == operation && fact.sequence.get() == receipt.sequence
            && fact.before.path.as_str().starts_with("first/")
            && fact.after.path.as_str().starts_with("second/")
    }));
    assert_eq!(engine.state().file_path_nodes, older_nodes);
    let later = rename(&mut engine, &repo, "second", "third");
    let later_facts = crate::persist::path_changes::read(&later).unwrap();
    assert_eq!(later_facts.len(), 2);
    assert!(later_facts.iter().all(|fact| {
        fact.operation_id != operation && fact.sequence.get() > receipt.sequence
            && expected.iter().all(|old| old.event_id != fact.event_id)
    }));
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
fn folder_and_file_rename_preserve_content_heads_then_write_follows_real_version() {
    let (t, mut e, r) = setup();
    save(&mut e, &r, "folder/a.txt", b"A");
    save(&mut e, &r, "folder/nested/b.txt", b"B");
    e.file_namespace(
        &r.repository_id,
        NamespaceOperation::parse(NamespaceOperationKind::Mkdir, "folder/empty", None).unwrap(),
        "admin",
        storage_contract::OperationId::generate().unwrap(),
    )
    .unwrap();
    crate::directory_git_fixture::checkpoint(&mut e, &r, "folder");
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
            storage_contract::OperationId::generate().unwrap(),
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
    let renamed = rename(&mut e, &r, "folder", "renamed");
    let view = entries(&e, &r);
    assert_eq!(e.state().versions.len(), bodies);
    assert!(e.state().file_path_nodes.is_empty());
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
        let facts = crate::persist::path_changes::read(&renamed).unwrap();
        let fact = facts.iter().find(|fact| fact.before.path.as_str() == from).unwrap();
        assert_eq!(fact.after.path.as_str(), to);
        assert_eq!(fact.before_version_id.to_string(), old[from].version_id);
        assert_eq!(fact.before_version_id, fact.after_version_id);
        assert!(e.file_context(&r.repository_id, to).unwrap().node_id.is_none());
    }
    let context = e.file_context(&r.repository_id, "renamed/a.txt").unwrap();
    let original_head = e.state().branches[&context.branch_id].clone();
    let renamed_again = rename(&mut e, &r, "renamed/a.txt", "renamed/final.txt");
    assert_eq!(e.state().branches[&context.branch_id], original_head);
    let second = crate::persist::path_changes::read(&renamed_again).unwrap();
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].before.path.as_str(), "renamed/a.txt");
    assert_eq!(second[0].after.path.as_str(), "renamed/final.txt");
    save(&mut e, &r, "renamed/final.txt", b"A changed");
    let finalentry = entries(&e, &r)["renamed/final.txt"].clone();
    assert_eq!(e.state().versions.len(), bodies + 1);
    assert_eq!(
        crate::business::file_nodes::base_parents(e.state(), &finalentry.version_id),
        vec![old["folder/a.txt"].version_id.clone()]
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
    e.valid_replica(&finalentry.version_id)
        .unwrap()
        .0
        .open()
        .unwrap()
        .read_to_end(&mut data)
        .unwrap();
    assert_eq!(data, b"A changed");
    assert!(e.state().file_path_nodes.is_empty());
}
#[test]
fn renamed_file_note_and_new_branch_preserve_the_real_content_source_node() {
    let (_t, mut e, r) = setup();
    save(&mut e, &r, "a.txt", b"body");
    let moved = rename(&mut e, &r, "a.txt", "b.txt");
    let file = entries(&e, &r)["b.txt"].clone();
    let node = file.version_id.clone();
    assert!(e.file_context(&r.repository_id, "b.txt").unwrap().node_id.is_none());
    let trees = e.state().directory_trees.len();
    let versions = e.state().versions.len();
    let fact = crate::persist::path_changes::read(&moved).unwrap().remove(0);
    let before = canonical_json(e.state()).unwrap();
    let rejected = e.set_file_node_note(
        &file.file_id,
        &fact.event_id.to_string(),
        &storage_contract::Note::parse("not a content node".into()).unwrap(),
        "admin",
        OperationId::generate().unwrap(),
    ).unwrap_err();
    assert_eq!(rejected.code(), storage_contract::ErrorCode::FileNodeNotFound);
    assert_eq!(canonical_json(e.state()).unwrap(), before);
    e.set_file_note(
        &file.file_id,
        Some(&node),
        &storage_contract::Note::parse("rename note".into()).unwrap(),
        "admin",
        storage_contract::OperationId::generate().unwrap(),
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
            storage_contract::OperationId::generate().unwrap(),
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
        storage_contract::OperationId::generate().unwrap(),
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
            storage_contract::OperationId::generate().unwrap(),
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
        storage_contract::OperationId::generate().unwrap(),
    )
    .unwrap();
    save(&mut e, &r, "a.txt", b"available body");
    // Lose only the old body's physical replicas, leaving its active branch and
    // all immutable history metadata intact.
    while let Ok((path, _)) = e.valid_replica(&original.version_id) {
        std::fs::remove_file(path.path()).unwrap();
    }
    let before = canonical_json(e.state()).unwrap();
    assert_eq!(
        e.select_file_branch(
            &original.file_id,
            &original_branch,
            "admin",
            storage_contract::OperationId::generate().unwrap()
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
            storage_contract::OperationId::generate().unwrap()
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
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap()
        .result["branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    save(&mut engine, &repo, "file.txt", b"current body");
    loop {
        match engine.valid_replica(&source.version_id) {
            Ok((path, _)) => std::fs::remove_file(path.path()).unwrap(),
            Err(error) if error.code() == storage_contract::ErrorCode::ContentUnavailable => break,
            Err(error) => panic!("unexpected replica failure: {error}"),
        }
    }
    let path = name_policy::PortablePath::parse("file.txt").unwrap();
    let operation = storage_contract::OperationId::generate().unwrap();
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
            storage_contract::OperationId::generate().unwrap(),
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
                    storage_contract::OperationId::generate().unwrap(),
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
        let operation = storage_contract::OperationId::generate().unwrap();
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
                Ok((path, _)) => std::fs::remove_file(path.path()).unwrap(),
                Err(error) if error.code() == storage_contract::ErrorCode::ContentUnavailable => {
                    break;
                }
                Err(error) => panic!("unexpected replica failure: {error}"),
            }
        }
        let before = crate::canonical_json(engine.state()).unwrap();
        assert_eq!(
            engine
                .merge_file_branches(request(storage_contract::OperationId::generate().unwrap()))
                .unwrap_err()
                .code(),
            if publish_first {
                // The successful merge consumed these branch heads. A new
                // operation must reject stale inputs before reading a body.
                storage_contract::ErrorCode::FileBranchConflict
            } else {
                storage_contract::ErrorCode::ContentUnavailable
            }
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

#[test]
fn save_and_rename_records_path_fact_but_advances_only_the_real_version() {
    let (temporary, mut engine, repository) = setup();
    save(&mut engine, &repository, "before.txt", b"before");
    let original = entries(&engine, &repository)["before.txt"].clone();
    let count = engine.state().versions.len();
    let workspace = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
    engine.stage_bytes(&workspace.workspace_id, "before.txt", b"after").unwrap();
    engine.namespace_operation(
        &workspace.workspace_id,
        NamespaceOperation::parse(
            NamespaceOperationKind::Rename, "before.txt", Some("after.txt"),
        ).unwrap(),
    ).unwrap();
    let request = CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: OperationId::generate().unwrap(),
        principal: "admin".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "content and namespace in one publication".into(),
        request_digest: None,
    };
    let receipt = engine.commit_workspace(request.clone()).unwrap();
    let entry = entries(&engine, &repository)["after.txt"].clone();
    let facts = crate::persist::path_changes::read(&receipt).unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].file_id.to_string(), original.file_id);
    assert_eq!(facts[0].entry_id.to_string(), original.entry_id);
    assert_eq!(facts[0].before_version_id.to_string(), original.version_id);
    assert_eq!(facts[0].after_version_id.to_string(), entry.version_id);
    assert_eq!(facts[0].before.path.as_str(), "before.txt");
    assert_eq!(facts[0].after.path.as_str(), "after.txt");
    assert_ne!(entry.version_id, original.version_id);
    assert_eq!(engine.state().versions.len(), count + 1);
    assert!(engine.state().file_path_nodes.is_empty());
    let context = engine.file_context(&repository.repository_id, "after.txt").unwrap();
    assert!(context.node_id.is_none());
    assert_eq!(engine.file_branch_head(&context.branch_id).unwrap(), entry.version_id);
    assert_eq!(
        crate::business::file_nodes::base_parents(engine.state(), &entry.version_id),
        vec![original.version_id],
    );
    drop(engine);
    let mut engine = Engine::open(&temporary.path().join("system")).unwrap();
    assert_eq!(engine.file_branch_head(&context.branch_id).unwrap(), entry.version_id);
    let before_retry = canonical_json(engine.state()).unwrap();
    assert_eq!(engine.commit_workspace(request).unwrap().result, receipt.result);
    assert_eq!(canonical_json(engine.state()).unwrap(), before_retry);
}

#[test]
fn new_path_helper_requires_exact_final_occurrence_and_every_parent_before_writing() {
    let (_temporary, mut engine, repository) = setup();
    save(&mut engine, &repository, "folder/a.txt", b"body");
    let visible = entries(&engine, &repository);
    let original = visible["folder/a.txt"].clone();
    let mut moved = original.clone();
    moved.relocate(name_policy::PortablePath::parse("folder/b.txt").unwrap());
    let mut state = engine.state().clone();
    // Exercise the helper creator on a candidate with no independent file
    // branch. The namespace/version source is from the real preceding save.
    let helpers = state.branches.values()
        .filter(|branch| branch.file_id.as_deref() == Some(original.file_id.as_str()))
        .map(|branch| branch.branch_id.clone()).collect::<Vec<_>>();
    for helper in helpers {
        state.branches.remove(&helper);
    }
    state.file_selections.remove(&original.file_id);
    state.file_contexts.remove(&format!("{}/folder/a.txt", repository.repository_id));
    state.sequence += 1;
    let complete = vec![visible["folder"].clone(), moved.clone()];
    for incomplete in [vec![visible["folder"].clone()], vec![moved.clone()]] {
        let before = canonical_json(&state).unwrap();
        let result = super::file_paths::record_change(
            &mut state,
            &repository.repository_id,
            super::file_paths::PathChange {
                old: &original,
                new: &moved,
                after_entries: &incomplete,
                context: None,
            },
            "admin",
            OperationId::generate().unwrap(),
        );
        assert!(result.is_err());
        assert_eq!(canonical_json(&state).unwrap(), before);
    }
    let operation = OperationId::generate().unwrap();
    let changed = super::file_paths::record_change(
        &mut state,
        &repository.repository_id,
        super::file_paths::PathChange {
            old: &original,
            new: &moved,
            after_entries: &complete,
            context: None,
        },
        "admin",
        operation,
    ).unwrap();
    let branch = &state.branches[&changed.context.branch_id];
    let head = &state.commits[&branch.head_commit_id];
    assert_eq!(branch.generation, 1);
    assert!(branch.file_node_id.is_none());
    assert_eq!(head.kind, "file_branch_root");
    assert_eq!(head.entries, complete);
    assert_eq!(head.operation_id, operation);
    assert_eq!(head.parent_commit_ids, vec![branch.fork_commit_id.clone()]);
    assert_eq!(changed.fact.before_version_id, changed.fact.after_version_id);
    assert!(state.file_path_nodes.is_empty());
}

#[test]
fn renamed_file_workspace_binds_explicit_path_and_keeps_unchanged_content_head() {
    let (temporary, mut engine, repository) = setup();
    save(&mut engine, &repository, "a.txt", b"body");
    rename(&mut engine, &repository, "a.txt", "b.txt");
    let context = engine.file_context(&repository.repository_id, "b.txt").unwrap();
    let old_head = engine.state().branches[&context.branch_id].clone();
    let stale = engine.create_workspace(&context.branch_id, "admin").unwrap();
    rename(&mut engine, &repository, "b.txt", "c.txt");
    let before = canonical_json(engine.state()).unwrap();
    assert_eq!(
        engine.stage_bytes(&stale.workspace_id, "c.txt", b"wrong target").unwrap_err().code(),
        storage_contract::ErrorCode::FileBranchConflict,
    );
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    assert!(engine.workspace(&stale.workspace_id).unwrap().files.is_empty());
    engine.abort_workspace(&stale.workspace_id, "admin").unwrap();
    let workspace = engine.create_workspace(&context.branch_id, "admin").unwrap();
    engine.stage_bytes(&workspace.workspace_id, "c.txt", b"body").unwrap();
    let unchanged = engine.commit_workspace(CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: OperationId::generate().unwrap(),
        principal: "admin".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "unchanged at renamed occurrence".into(),
        request_digest: None,
    }).unwrap();
    assert_eq!(unchanged.result["no_change"], true);
    assert_eq!(engine.state().branches[&context.branch_id], old_head);
    let workspace = engine.create_workspace(&context.branch_id, "admin").unwrap();
    engine.stage_range(
        &workspace.workspace_id, &new_id("entry").unwrap(), "c.txt", 0, b"B", None,
    ).unwrap();
    let saved_workspace_id = workspace.workspace_id.clone();
    let saved = engine.commit_workspace(CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: OperationId::generate().unwrap(),
        principal: "admin".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "real content at renamed occurrence".into(),
        request_digest: None,
    }).unwrap();
    assert!(crate::persist::path_changes::read(&saved).unwrap().is_empty());
    let saved_path = name_policy::PortablePath::parse("c.txt").unwrap();
    engine.validate_workspace_file_selection(
        &engine.workspace(&saved_workspace_id).unwrap(), &saved_path,
    ).unwrap();
    let entry = entries(&engine, &repository)["c.txt"].clone();
    assert_eq!(engine.file_branch_head(&context.branch_id).unwrap(), entry.version_id);
    assert!(engine.state().file_path_nodes.is_empty());
    drop(engine);
    let engine = Engine::open(&temporary.path().join("system")).unwrap();
    engine.validate_workspace_file_selection(
        &engine.workspace(&saved_workspace_id).unwrap(), &saved_path,
    ).unwrap();
    let mut bytes = Vec::new();
    engine.open_content(&entry.version_id).unwrap().file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"Body");
    assert_eq!(entries(&engine, &repository)["c.txt"], entry);
}
