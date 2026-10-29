use super::*;

fn initialize(pool: &mut Pool, path: &str) -> String {
    pool.engine
        .initialize_directory_scope(
            &pool.repo.repository_id,
            (if path.is_empty() {
                None
            } else {
                Some(name_policy::LookupPath::parse(path).unwrap())
            })
            .as_ref(),
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap()
        .result["directory_branch_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn frozen_batch_chain_is_order_independent_and_survives_reopen() {
    for (kind, reverse) in [
        (storage_contract::DirectoryBatchKind::Rename, false),
        (storage_contract::DirectoryBatchKind::Rename, true),
        (storage_contract::DirectoryBatchKind::Move, false),
        (storage_contract::DirectoryBatchKind::Move, true),
    ] {
        let mut pool = Pool::new();
        pool.save("a.txt", b"A");
        pool.save("b.txt", b"B");
        let branch = initialize(&mut pool, "");
        let a = pool.entry(&branch, "a.txt");
        let b = pool.entry(&branch, "b.txt");
        let mut mappings = vec![
            pool.mapping(&branch, "a.txt", &branch, "b.txt"),
            pool.mapping(&branch, "b.txt", &branch, "c.txt"),
        ];
        if reverse {
            mappings.reverse();
        }
        pool.batch(kind, mappings);
        assert_eq!(pool.current_entry("b.txt").file_id, a.file_id);
        assert_eq!(pool.current_entry("c.txt").file_id, b.file_id);
        assert_eq!(pool.engine.state().file_locations[&a.file_id], "b.txt");
        assert_eq!(pool.engine.state().file_locations[&b.file_id], "c.txt");
        let main = pool.repo.main_branch_id.clone();
        pool.reopen(|engine| {
            let entries = engine.list_tree(&main).unwrap();
            assert_eq!(entries.len(), 2);
            assert_eq!(
                entries.iter().find(|e| e.path == "b.txt").unwrap().file_id,
                a.file_id
            );
            assert_eq!(
                entries.iter().find(|e| e.path == "c.txt").unwrap().file_id,
                b.file_id
            );
        });
    }
}

#[test]
fn frozen_batch_swap_preserves_identities_contexts_and_retry_receipt() {
    let mut pool = Pool::new();
    pool.save("a.txt", b"A");
    pool.save("b.txt", b"B");
    let branch = initialize(&mut pool, "");
    let a = pool.entry(&branch, "a.txt");
    let b = pool.entry(&branch, "b.txt");
    let a_context = pool
        .engine
        .file_context(&pool.repo.repository_id, "a.txt")
        .unwrap()
        .unwrap();
    let b_context = pool
        .engine
        .file_context(&pool.repo.repository_id, "b.txt")
        .unwrap()
        .unwrap();
    let request = DirectoryNamespaceBatch {
        kind: storage_contract::DirectoryBatchKind::Rename,
        items: vec![
            pool.mapping(&branch, "a.txt", &branch, "b.txt"),
            pool.mapping(&branch, "b.txt", &branch, "a.txt"),
        ],
    };
    let operation = storage_contract::OperationId::generate();
    let result = pool
        .engine
        .directory_namespace_batch(&pool.repo.repository_id, &request, "admin", operation)
        .unwrap();
    let after = storage_engine::canonical_json(pool.engine.state()).unwrap();
    assert_eq!(
        pool.engine
            .directory_namespace_batch(&pool.repo.repository_id, &request, "admin", operation)
            .unwrap()
            .result,
        result.result
    );
    assert_eq!(
        storage_engine::canonical_json(pool.engine.state()).unwrap(),
        after
    );
    assert_eq!(pool.current_entry("b.txt").file_id, a.file_id);
    assert_eq!(pool.current_entry("a.txt").file_id, b.file_id);
    assert_eq!(
        pool.engine
            .file_context(&pool.repo.repository_id, "b.txt")
            .unwrap()
            .unwrap()
            .branch_id,
        a_context.branch_id
    );
    assert_eq!(
        pool.engine
            .file_context(&pool.repo.repository_id, "a.txt")
            .unwrap()
            .unwrap()
            .branch_id,
        b_context.branch_id
    );
    pool.reopen(|engine| {
        assert_eq!(
            storage_engine::canonical_json(engine.state()).unwrap(),
            after
        )
    });
}

#[test]
fn frozen_batch_nonparticipating_target_collision_is_atomic() {
    let mut pool = Pool::new();
    pool.save("a.txt", b"A");
    pool.save("b.txt", b"B");
    pool.save("occupied.txt", b"untouched");
    let branch = initialize(&mut pool, "");
    let before = storage_engine::canonical_json(pool.engine.state()).unwrap();
    let request = DirectoryNamespaceBatch {
        kind: storage_contract::DirectoryBatchKind::Rename,
        items: vec![
            pool.mapping(&branch, "a.txt", &branch, "b.txt"),
            pool.mapping(&branch, "b.txt", &branch, "occupied.txt"),
        ],
    };
    assert_eq!(
        pool.engine
            .directory_namespace_batch(
                &pool.repo.repository_id,
                &request,
                "admin",
                storage_contract::OperationId::generate()
            )
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::NameCollision
    );
    assert_eq!(
        storage_engine::canonical_json(pool.engine.state()).unwrap(),
        before
    );
    pool.reopen(|engine| {
        assert_eq!(
            storage_engine::canonical_json(engine.state()).unwrap(),
            before
        )
    });
}

#[test]
fn frozen_batch_overlapping_active_scopes_reject_before_losing_an_item() {
    let mut pool = Pool::new();
    pool.save("d/a.txt", b"A");
    pool.save("d/b.txt", b"B");
    let child = initialize(&mut pool, "d");
    let root = initialize(&mut pool, "");
    let before = storage_engine::canonical_json(pool.engine.state()).unwrap();
    let request = DirectoryNamespaceBatch {
        kind: storage_contract::DirectoryBatchKind::Rename,
        items: vec![
            pool.mapping(&root, "d/a.txt", &root, "d/a2.txt"),
            pool.mapping(&child, "d/b.txt", &child, "d/b2.txt"),
        ],
    };
    let result = pool.engine.directory_namespace_batch(
        &pool.repo.repository_id,
        &request,
        "admin",
        storage_contract::OperationId::generate(),
    );
    match result {
        Err(error) => assert_eq!(
            error.code(),
            storage_contract::ErrorCode::BatchScopeConflict
        ),
        Ok(_) => panic!(
            "overlapping scopes were published instead of rejected; final paths: {:?}",
            pool.engine
                .list_tree(&pool.repo.main_branch_id)
                .unwrap()
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>()
        ),
    }
    assert_eq!(
        storage_engine::canonical_json(pool.engine.state()).unwrap(),
        before
    );
    pool.reopen(|engine| {
        assert_eq!(
            storage_engine::canonical_json(engine.state()).unwrap(),
            before
        )
    });
}

#[test]
fn frozen_batch_directory_swap_preserves_child_identities_and_references() {
    let mut pool = Pool::new();
    pool.save("a/one.txt", b"A");
    pool.save("b/two.txt", b"B");
    let a_branch = initialize(&mut pool, "a");
    let b_branch = initialize(&mut pool, "b");
    let root = initialize(&mut pool, "");
    let a = pool.current_entry("a");
    let b = pool.current_entry("b");
    let a_file = pool.current_entry("a/one.txt");
    let b_file = pool.current_entry("b/two.txt");
    let items = vec![
        pool.mapping(&root, "a", &root, "b"),
        pool.mapping(&root, "b", &root, "a"),
    ];
    pool.batch(storage_contract::DirectoryBatchKind::Rename, items);
    assert_eq!(pool.current_entry("b").entry_id, a.entry_id);
    assert_eq!(pool.current_entry("a").entry_id, b.entry_id);
    assert_eq!(pool.current_entry("b/one.txt").file_id, a_file.file_id);
    assert_eq!(pool.current_entry("a/two.txt").file_id, b_file.file_id);
    assert_eq!(pool.branch(&a_branch).path, "b");
    assert_eq!(pool.branch(&b_branch).path, "a");
    let root_view = pool.branch(&root);
    for (directory, branch) in [(&a.entry_id, &a_branch), (&b.entry_id, &b_branch)] {
        let reference = &root_view.directory_refs[directory];
        assert_eq!(&reference.directory_branch_id, branch);
        assert_eq!(reference.tree_id, pool.branch(branch).head_tree_id);
    }
    let after = storage_engine::canonical_json(pool.engine.state()).unwrap();
    pool.reopen(|engine| {
        assert_eq!(
            storage_engine::canonical_json(engine.state()).unwrap(),
            after
        )
    });
}

#[test]
fn frozen_batch_read_only_parent_scope_does_not_block_child_copy() {
    let mut pool = Pool::new();
    pool.save("d/a.txt", b"source");
    let child = initialize(&mut pool, "d");
    let root = initialize(&mut pool, "");
    let original = pool.current_entry("d/a.txt");
    let items = vec![pool.mapping(&root, "d/a.txt", &child, "d/copy.txt")];
    pool.batch(storage_contract::DirectoryBatchKind::Copy, items);
    let copied = pool.current_entry("d/copy.txt");
    assert_ne!(copied.file_id, original.file_id);
    assert_eq!(pool.bytes(&copied.version_id), b"source");
    assert_eq!(pool.current_entry("d/a.txt").file_id, original.file_id);
}

#[test]
fn frozen_batch_deletes_distinct_same_directory_aliases_but_rejects_duplicate_source() {
    let mut pool = Pool::new();
    pool.save("a/report.txt", b"source");
    pool.save("b/stay.txt", b"untouched");
    let first_source = pool.register("a", "first");
    let second_source = pool.register("a", "second");
    let retained_source = pool.register("a", "retained");
    let target = pool.register("b", "target");
    let original = pool.entry(&first_source, "a/report.txt");
    for (source, destination) in [
        (&first_source, "b/first.txt"),
        (&second_source, "b/second.txt"),
    ] {
        let item = pool.mapping(source, "a/report.txt", &target, destination);
        pool.batch(storage_contract::DirectoryBatchKind::Move, vec![item]);
    }
    pool.select("b", "target");
    assert_eq!(
        pool.current_entry("b/first.txt").entry_id,
        original.entry_id
    );
    assert_eq!(
        pool.current_entry("b/second.txt").entry_id,
        original.entry_id
    );
    let first = DirectoryNamespaceItem {
        source: pool.source(&target, "b/first.txt"),
        target: None,
    };
    let second = DirectoryNamespaceItem {
        source: pool.source(&target, "b/second.txt"),
        target: None,
    };
    let before = storage_engine::canonical_json(pool.engine.state()).unwrap();
    let duplicate = DirectoryNamespaceBatch {
        kind: storage_contract::DirectoryBatchKind::Delete,
        items: vec![first.clone(), first.clone()],
    };
    assert_eq!(
        pool.engine
            .directory_namespace_batch(
                &pool.repo.repository_id,
                &duplicate,
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::BatchMappingConflict
    );
    assert_eq!(
        storage_engine::canonical_json(pool.engine.state()).unwrap(),
        before
    );
    let request = DirectoryNamespaceBatch {
        kind: storage_contract::DirectoryBatchKind::Delete,
        items: vec![first, second],
    };
    let operation = storage_contract::OperationId::generate();
    let receipt = pool
        .engine
        .directory_namespace_batch(&pool.repo.repository_id, &request, "admin", operation)
        .unwrap();
    assert_eq!(
        pool.engine
            .directory_namespace_batch(&pool.repo.repository_id, &request, "admin", operation)
            .unwrap()
            .result,
        receipt.result
    );
    let target_after = pool.branch(&target);
    assert_eq!(target_after.entries.len(), 1);
    assert_eq!(target_after.entries[0].path, "b/stay.txt");
    assert_eq!(
        pool.entry(&retained_source, "a/report.txt").file_id,
        original.file_id
    );
    assert_eq!(pool.bytes(&original.version_id), b"source");
    let after = storage_engine::canonical_json(pool.engine.state()).unwrap();
    pool.reopen(|engine| {
        assert_eq!(
            storage_engine::canonical_json(engine.state()).unwrap(),
            after
        );
    });
}
