use super::*;

fn setup() -> (tempfile::TempDir, Engine, Repository) {
    let root = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &root.path().join("system"),
        Config::development([
            root.path().join("p"),
            root.path().join("b"),
            root.path().join("c"),
        ]),
    )
    .unwrap();
    let repo = engine
        .create_disk_root("P01", "admin", &new_id("op"))
        .unwrap();
    (root, engine, repo)
}
fn save(engine: &mut Engine, branch: &str, path: &str, bytes: &[u8]) -> Entry {
    let w = engine.create_workspace(branch, "admin").unwrap();
    engine.stage_bytes(&w.workspace_id, path, bytes).unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: w.workspace_id,
            operation_id: new_id("op"),
            principal: "admin".into(),
            expected_head: w.base_head,
            fencing_token: w.fencing_token,
            message: "labels fixture".into(),
            request_digest: None,
        })
        .unwrap();
    engine
        .list_tree(branch)
        .unwrap()
        .into_iter()
        .find(|e| e.path == path)
        .unwrap()
}
fn target(engine: &Engine, branch: &str, node: &str) -> FileTagTarget {
    FileTagTarget {
        branch_id: branch.into(),
        source_node_id: node.into(),
        expected_node_id: engine.file_branch_head(branch).unwrap(),
    }
}
fn edit(
    engine: &mut Engine,
    file: &str,
    branch: &str,
    node: &str,
    add: &[&str],
    remove: &[&str],
) -> String {
    let t = target(engine, branch, node);
    engine
        .update_file_node_tags(crate::FileNodeTagsUpdateRequest {
            file_id: file,
            targets: &[t],
            add: &add.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            remove: &remove.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            location: None,
            principal: "admin",
            operation_id: &new_id("op"),
        })
        .unwrap()
        .result["nodes"][0]["node_id"]
        .as_str()
        .unwrap()
        .into()
}

#[test]
fn labels_inherit_across_content_path_events_and_recovery_without_new_bodies() {
    let (root, mut e, r) = setup();
    let first = save(&mut e, &r.main_branch_id, "folder/a.txt", b"first");
    let context = e.file_context(&r.repository_id, "folder/a.txt").unwrap();
    let branch = context.branch_id;
    let count = e.state().versions.len();
    let tagged = edit(
        &mut e,
        &first.file_id,
        &branch,
        &first.version_id,
        &["草稿", "review"],
        &[],
    );
    assert_eq!(e.state().versions.len(), count);
    assert_eq!(
        e.file_node_tags(&first.file_id, &first.version_id).unwrap(),
        BTreeSet::new()
    );
    assert_eq!(
        e.file_context(&r.repository_id, "folder/a.txt")
            .unwrap()
            .node_id
            .as_deref(),
        Some(tagged.as_str())
    );
    let second = save(&mut e, &r.main_branch_id, "folder/a.txt", b"second");
    assert_eq!(
        e.file_node_parents(&first.file_id, &second.version_id)
            .unwrap(),
        vec![tagged]
    );
    assert_eq!(
        e.file_node_tags(&first.file_id, &second.version_id)
            .unwrap(),
        BTreeSet::from(["草稿".into(), "review".into()])
    );
    e.file_namespace(
        &r.repository_id,
        NamespaceOperation {
            kind: NamespaceOperationKind::Rename,
            path: "folder/a.txt".into(),
            destination: Some("folder/b.txt".into()),
        },
        "admin",
        &new_id("op"),
    )
    .unwrap();
    let path = e
        .file_context(&r.repository_id, "folder/b.txt")
        .unwrap()
        .node_id
        .unwrap();
    assert_eq!(e.file_node_tags(&first.file_id, &path).unwrap().len(), 2);
    let final_node = edit(&mut e, &first.file_id, &branch, &path, &["发布"], &["草稿"]);
    assert_eq!(
        e.file_node_tags(&first.file_id, &final_node).unwrap(),
        BTreeSet::from(["review".into(), "发布".into()])
    );
    assert_eq!(e.state().versions.len(), count + 1);
    drop(e);
    let e = Engine::open(&root.path().join("system")).unwrap();
    assert_eq!(
        e.file_node_tags(&first.file_id, &final_node).unwrap().len(),
        2
    );
}

#[test]
fn historical_labels_add_a_child_without_rewinding_newer_content() {
    let (_root, mut e, r) = setup();
    let first = save(&mut e, &r.main_branch_id, "a.txt", b"old");
    let second = save(&mut e, &r.main_branch_id, "a.txt", b"new");
    let branch = e.file_context(&r.repository_id, "a.txt").unwrap().branch_id;
    let head = e.file_branch_head(&branch).unwrap();
    let child = edit(
        &mut e,
        &first.file_id,
        &branch,
        &first.version_id,
        &["historical"],
        &[],
    );
    assert_eq!(e.file_branch_head(&branch).unwrap(), head);
    assert_eq!(
        e.list_tree(&r.main_branch_id)
            .unwrap()
            .into_iter()
            .find(|e| e.path == "a.txt")
            .unwrap()
            .version_id,
        second.version_id
    );
    assert_eq!(
        e.state().file_metadata_nodes[&child].parent_node_ids,
        vec![first.version_id]
    );
}

#[test]
fn stale_and_over_capacity_batches_are_atomic_and_retry_is_idempotent() {
    let (_root, mut e, r) = setup();
    let first = save(&mut e, &r.main_branch_id, "a.txt", b"A");
    let branch = e.file_context(&r.repository_id, "a.txt").unwrap().branch_id;
    let mut t = target(&e, &branch, &first.version_id);
    let op = new_id("op");
    let add = vec!["one".into()];
    let receipt = e
        .update_file_node_tags(crate::FileNodeTagsUpdateRequest {
            file_id: &first.file_id,
            targets: &[t.clone()],
            add: &add,
            remove: &[],
            location: None,
            principal: "admin",
            operation_id: &op,
        })
        .unwrap();
    let state = canonical_json(e.state()).unwrap();
    let retry = e
        .update_file_node_tags(crate::FileNodeTagsUpdateRequest {
            file_id: &first.file_id,
            targets: &[t.clone()],
            add: &add,
            remove: &[],
            location: None,
            principal: "admin",
            operation_id: &op,
        })
        .unwrap();
    assert_eq!(receipt.result, retry.result);
    assert_eq!(canonical_json(e.state()).unwrap(), state);
    let location = crate::FileLocation {
        repository_id: r.repository_id.clone(),
        path: "a.txt".into(),
    };
    for (principal, location) in [("other-actor", None), ("admin", Some(&location))] {
        assert_eq!(
            e.update_file_node_tags(crate::FileNodeTagsUpdateRequest {
                file_id: &first.file_id,
                targets: &[t.clone()],
                add: &add,
                remove: &[],
                location,
                principal,
                operation_id: &op,
            })
            .unwrap_err()
            .code(),
            storage_contract::ErrorCode::IdempotencyKeyReused
        );
        assert_eq!(canonical_json(e.state()).unwrap(), state);
    }
    assert_eq!(
        e.update_file_node_tags(crate::FileNodeTagsUpdateRequest {
            file_id: &first.file_id,
            targets: &[t.clone()],
            add: &["two".into()],
            remove: &[],
            location: None,
            principal: "admin",
            operation_id: &new_id("op")
        })
        .unwrap_err()
        .code(),
        storage_contract::ErrorCode::FileBranchConflict
    );
    t.source_node_id = receipt.result["nodes"][0]["node_id"]
        .as_str()
        .unwrap()
        .into();
    t.expected_node_id = e.file_branch_head(&branch).unwrap();
    let additions = (0..16).map(|i| format!("tag{i}")).collect::<Vec<_>>();
    assert_eq!(
        e.update_file_node_tags(crate::FileNodeTagsUpdateRequest {
            file_id: &first.file_id,
            targets: &[t],
            add: &additions,
            remove: &[],
            location: None,
            principal: "admin",
            operation_id: &new_id("op")
        })
        .unwrap_err()
        .code(),
        storage_contract::ErrorCode::TagLimitExceeded
    );
    assert_eq!(canonical_json(e.state()).unwrap(), state);
    assert!(validate_tag(&"汉".repeat(43)).is_err());
    assert!(validate_tag(" hidden ").is_err());
}

#[test]
fn pending_file_labels_preserve_first_write_and_batches_keep_branch_snapshots_independent() {
    let (_root, mut e, r) = setup();
    let file = save(&mut e, &r.main_branch_id, "a.txt", b"source");
    let main = e.file_context(&r.repository_id, "a.txt").unwrap().branch_id;
    let pending = e
        .create_file_branch(
            &file.file_id,
            &file.version_id,
            "review",
            "admin",
            &new_id("op"),
        )
        .unwrap()
        .result["branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    e.select_file_branch(&file.file_id, &pending, "admin", &new_id("op"))
        .unwrap();
    let tagged = edit(
        &mut e,
        &file.file_id,
        &pending,
        &file.version_id,
        &["review"],
        &[],
    );
    assert!(
        e.state().branches[&pending]
            .pending_file_versions
            .contains_key(&file.file_id)
    );
    assert!(
        e.file_node_tags(&file.file_id, &e.file_branch_head(&main).unwrap())
            .unwrap()
            .is_empty()
    );
    let count = e.state().versions.len();
    let written = save(&mut e, &r.main_branch_id, "a.txt", b"source");
    assert_eq!(
        e.state().versions.len(),
        count + 1,
        "tag metadata must not complete the pending first body write"
    );
    assert_ne!(written.version_id, file.version_id);
    assert_eq!(
        e.file_node_parents(&file.file_id, &written.version_id)
            .unwrap(),
        vec![tagged]
    );
    assert_eq!(
        e.file_node_tags(&file.file_id, &written.version_id)
            .unwrap(),
        BTreeSet::from(["review".into()])
    );
    let targets = vec![
        target(&e, &main, &file.version_id),
        target(&e, &pending, &written.version_id),
    ];
    let receipt = e
        .update_file_node_tags(crate::FileNodeTagsUpdateRequest {
            file_id: &file.file_id,
            targets: &targets,
            add: &["shared".into()],
            remove: &[],
            location: None,
            principal: "admin",
            operation_id: &new_id("op"),
        })
        .unwrap();
    assert_eq!(receipt.result["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(
        e.file_node_tags(&file.file_id, &e.file_branch_head(&main).unwrap())
            .unwrap(),
        BTreeSet::from(["shared".into()])
    );
    assert_eq!(
        e.file_node_tags(&file.file_id, &e.file_branch_head(&pending).unwrap())
            .unwrap(),
        BTreeSet::from(["review".into(), "shared".into()])
    );
}
