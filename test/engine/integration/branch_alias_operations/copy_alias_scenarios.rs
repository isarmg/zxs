use super::*;

#[test]
fn copied_file_branch_root_records_the_copy_actor_and_operation() {
    let mut pool = Pool::new();
    pool.save("source.txt", b"original");
    let source = pool.register("", "source");
    let target = pool.register("", "target");
    let untouched = pool.branch(&source);
    let operation = new_id("op");
    let request = DirectoryNamespaceBatch {
        kind: "copy".into(),
        items: vec![pool.mapping(&source, "source.txt", &target, "copy.txt")],
    };
    pool.engine
        .directory_namespace_batch(
            &pool.repo.repository_id,
            &request,
            "copy-author",
            &operation,
        )
        .unwrap();
    let copied = pool.entry(&target, "copy.txt");
    let branch = pool.branch(&target).file_contexts[&copied.lookup_key]
        .branch_id
        .clone();
    let check = |engine: &Engine| {
        let branch = &engine.state().branches[&branch];
        let commit = &engine.state().commits[&branch.head_commit_id];
        assert_eq!(commit.kind, "file_branch_root");
        assert_eq!(commit.operation_id, operation);
        assert_eq!(commit.author, "copy-author");
        assert_eq!(engine.state().directory_branches[&source], untouched);
    };
    check(&pool.engine);
    pool.reopen(check);
}

fn renamed_targets(pool: &mut Pool) -> (String, String) {
    pool.save("source/nested/report.txt", b"source body");
    pool.save("folder/keep.txt", b"untouched destination member");
    let before = pool.register("", "before rename");
    pool.engine
        .file_namespace(
            &pool.repo.repository_id,
            NamespaceOperation {
                kind: "rename".into(),
                path: "folder".into(),
                destination: Some("renamed".into()),
            },
            "admin",
            &new_id("op"),
        )
        .unwrap();
    let after = pool.register("", "after rename");
    assert_eq!(
        pool.entry(&before, "folder").entry_id,
        pool.entry(&after, "renamed").entry_id
    );
    (before, after)
}

#[test]
fn copied_file_aliases_keep_each_frozen_parent_and_destination_spelling() {
    for reverse in [false, true] {
        let mut pool = Pool::new();
        let (before, after) = renamed_targets(&mut pool);
        let source = pool.entry(&before, "source/nested/report.txt");
        let untouched = pool.register("", "untouched");
        let untouched_branch = pool.branch(&untouched);
        let current = pool.engine.list_tree(&pool.repo.main_branch_id).unwrap();
        let mut items = vec![
            pool.mapping(&before, &source.path, &before, "folder/Copied.txt"),
            pool.mapping(&before, &source.path, &after, "renamed/copied.txt"),
        ];
        if reverse {
            items.reverse();
        }
        let first_path = items[0].target.as_ref().unwrap().path.clone();
        let request = DirectoryNamespaceBatch {
            kind: "copy".into(),
            items,
        };
        let operation = new_id("op");
        let receipt = pool
            .engine
            .directory_namespace_batch(&pool.repo.repository_id, &request, "admin", &operation)
            .unwrap();
        let left = pool.entry(&before, "folder/Copied.txt");
        let right = pool.entry(&after, "renamed/copied.txt");
        assert_eq!(left.entry_id, right.entry_id);
        assert_eq!(left.file_id, right.file_id);
        assert_eq!(left.version_id, right.version_id);
        assert_ne!(left.file_id, source.file_id);
        assert_eq!(left.name, "Copied.txt");
        assert_eq!(right.name, "copied.txt");
        assert_eq!(left.lookup_key, "folder/copied.txt");
        assert_eq!(right.lookup_key, "renamed/copied.txt");
        assert_eq!(
            pool.branch(&before).file_contexts[&left.lookup_key],
            pool.branch(&after).file_contexts[&right.lookup_key]
        );
        assert_eq!(
            pool.engine.state().file_locations[&left.file_id],
            first_path
        );
        assert_eq!(
            pool.engine.state().file_names[&left.file_id],
            first_path.rsplit('/').next().unwrap()
        );
        assert_eq!(receipt.result["copies"].as_array().unwrap().len(), 1);
        assert_eq!(receipt.result["copies"][0]["path"], first_path);
        assert_eq!(pool.bytes(&left.version_id), b"source body");
        assert_eq!(pool.branch(&untouched), untouched_branch);
        assert_eq!(
            pool.engine.list_tree(&pool.repo.main_branch_id).unwrap(),
            current
        );
        let state = storage_engine::canonical_json(pool.engine.state()).unwrap();
        assert_eq!(
            pool.engine
                .directory_namespace_batch(&pool.repo.repository_id, &request, "admin", &operation)
                .unwrap()
                .result,
            receipt.result
        );
        assert_eq!(
            storage_engine::canonical_json(pool.engine.state()).unwrap(),
            state
        );
        pool.select("", "before rename");
        assert_eq!(pool.current_entry(&left.path), left);
        pool.select("", "after rename");
        assert_eq!(pool.current_entry(&right.path), right);
        pool.reopen(|engine| {
            assert_eq!(
                engine.state().directory_branches[&before].file_contexts[&left.lookup_key],
                engine.state().directory_branches[&after].file_contexts[&right.lookup_key]
            );
            assert_eq!(engine.state().file_locations[&left.file_id], first_path);
        });
    }
}

#[test]
fn copied_folder_aliases_share_ids_without_reusing_another_targets_paths() {
    let mut pool = Pool::new();
    let (before, after) = renamed_targets(&mut pool);
    let source = pool.entry(&before, "source/nested/report.txt");
    let receipt = pool.batch(
        "copy",
        vec![
            pool.mapping(&before, "source", &before, "folder/Copied"),
            pool.mapping(&before, "source", &after, "renamed/copied"),
        ],
    );
    for suffix in ["", "/nested", "/nested/report.txt"] {
        let left = pool.entry(&before, &format!("folder/Copied{suffix}"));
        let right = pool.entry(&after, &format!("renamed/copied{suffix}"));
        assert_eq!(left.entry_id, right.entry_id);
        assert_eq!(left.file_id, right.file_id);
        assert_eq!(left.version_id, right.version_id);
        assert_eq!(left.lookup_key, name_policy::path_key(&left.path));
        assert_eq!(right.lookup_key, name_policy::path_key(&right.path));
    }
    let copy = pool.entry(&after, "renamed/copied/nested/report.txt");
    assert_ne!(copy.file_id, source.file_id);
    assert_eq!(receipt.result["copies"].as_array().unwrap().len(), 1);
    assert_eq!(
        pool.engine.state().file_locations[&copy.file_id],
        "folder/Copied/nested/report.txt"
    );
    assert_eq!(pool.bytes(&copy.version_id), b"source body");
    pool.reopen(|engine| {
        assert!(
            engine.state().directory_branches[&before]
                .entries
                .iter()
                .any(|entry| entry.path == "folder/Copied/nested/report.txt"
                    && entry.file_id == copy.file_id)
        );
        assert!(
            engine.state().directory_branches[&after]
                .entries
                .iter()
                .any(|entry| entry.path == "renamed/copied/nested/report.txt"
                    && entry.file_id == copy.file_id)
        );
    });
}

#[test]
fn copied_file_case_only_targets_preserve_each_requested_alias_name() {
    let mut pool = Pool::new();
    let (before, after) = renamed_targets(&mut pool);
    pool.batch(
        "copy",
        vec![
            pool.mapping(&before, "source/nested/report.txt", &before, "Copy.TXT"),
            pool.mapping(&before, "source/nested/report.txt", &after, "copy.txt"),
        ],
    );
    let left = pool.entry(&before, "Copy.TXT");
    let right = pool.entry(&after, "copy.txt");
    assert_eq!(left.file_id, right.file_id);
    assert_eq!(left.version_id, right.version_id);
    assert_eq!(left.lookup_key, right.lookup_key);
    assert_eq!(
        pool.engine.state().file_locations[&left.file_id],
        "Copy.TXT"
    );
    assert_eq!(pool.engine.state().file_names[&left.file_id], "Copy.TXT");
}

#[test]
fn copied_alias_destination_collision_rejects_all_targets_without_mutation() {
    let mut pool = Pool::new();
    let (before, after) = renamed_targets(&mut pool);
    let request = DirectoryNamespaceBatch {
        kind: "copy".into(),
        items: vec![
            pool.mapping(&before, "source", &before, "folder/copied"),
            pool.mapping(&before, "source", &after, "renamed/copied"),
            pool.mapping(&before, "folder/keep.txt", &after, "renamed/keep.txt"),
        ],
    };
    let state = storage_engine::canonical_json(pool.engine.state()).unwrap();
    assert_eq!(
        pool.engine
            .directory_namespace_batch(&pool.repo.repository_id, &request, "admin", &new_id("op"))
            .unwrap_err()
            .code(),
        "NAME_COLLISION"
    );
    assert_eq!(
        storage_engine::canonical_json(pool.engine.state()).unwrap(),
        state
    );
}
