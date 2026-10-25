use crate::*;
use std::{collections::BTreeMap, io::Read};
fn setup() -> (tempfile::TempDir, Engine, Repository) {
    let t = tempfile::tempdir().unwrap();
    let mut e = Engine::initialize(
        &t.path().join("system"),
        Config::development([t.path().join("p"), t.path().join("b"), t.path().join("c")]),
    )
    .unwrap();
    let r = e.create_disk_root("P01", "admin", &new_id("op")).unwrap();
    (t, e, r)
}
fn save(e: &mut Engine, r: &Repository, path: &str, data: &[u8]) {
    let w = e.create_workspace(&r.main_branch_id, "admin").unwrap();
    e.stage_bytes(&w.workspace_id, path, data).unwrap();
    e.commit_workspace(CommitRequest {
        workspace_id: w.workspace_id,
        operation_id: new_id("op"),
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
        .map(|x| (x.path.clone(), x))
        .collect()
}
fn rename(e: &mut Engine, r: &Repository, from: &str, to: &str) -> OperationResult {
    e.file_namespace(
        &r.repository_id,
        NamespaceOperation {
            kind: "rename".into(),
            path: from.into(),
            destination: Some(to.into()),
        },
        "admin",
        &new_id("op"),
    )
    .unwrap()
}
#[test]
fn folder_and_file_rename_append_path_nodes_reusing_bodies_then_write_follows_the_path_node() {
    let (t, mut e, r) = setup();
    save(&mut e, &r, "folder/a.txt", b"A");
    save(&mut e, &r, "folder/nested/b.txt", b"B");
    e.file_namespace(
        &r.repository_id,
        NamespaceOperation {
            kind: "mkdir".into(),
            path: "folder/empty".into(),
            destination: None,
        },
        "admin",
        &new_id("op"),
    )
    .unwrap();
    let old = entries(&e, &r);
    let folderid = &old["folder"].entry_id;
    let selected = e.state().directory_selections[folderid].clone();
    let head = e.state().directory_branches[&selected].head_tree_id.clone();
    let untouched = e
        .create_directory_branches(
            &r.repository_id,
            "folder",
            "untouched",
            "admin",
            &new_id("op"),
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
    e.set_file_path_note(&file.file_id, &node, "rename note", "admin", &new_id("op"))
        .unwrap();
    assert_eq!(e.state().directory_trees.len(), trees);
    assert_eq!(e.state().versions.len(), versions);
    assert_eq!(
        e.set_file_path_note(
            &file.file_id,
            &node,
            &"x".repeat(129),
            "admin",
            &new_id("op")
        )
        .unwrap_err()
        .code(),
        storage_contract::ErrorCode::InvalidRequest
    );
    let branch = e
        .create_file_branch_from_node(
            &file.file_id,
            &file.version_id,
            &node,
            "from rename",
            "admin",
            &new_id("op"),
        )
        .unwrap()
        .result["branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    e.select_file_branch_at(
        &r.repository_id,
        "b.txt",
        &file.file_id,
        &branch,
        "admin",
        &new_id("op"),
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
            "other",
            "admin",
            &new_id("op"),
        )
        .unwrap()
        .result["branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    e.select_file_branch_at(
        &r.repository_id,
        "a.txt",
        &original.file_id,
        &branch,
        "admin",
        &new_id("op"),
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
        e.select_file_branch(&original.file_id, &original_branch, "admin", &new_id("op"))
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::ContentUnavailable
    );
    assert_eq!(
        e.select_file_branch_at(
            &r.repository_id,
            "a.txt",
            &original.file_id,
            &original_branch,
            "admin",
            &new_id("op")
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
