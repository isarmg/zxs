use super::*;
use json_codec::json;

fn op() -> OperationId {
    OperationId::generate().unwrap()
}
fn fixture() -> (tempfile::TempDir, Engine) {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([temp.path().join("primary")]).unwrap();
    let engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
    (temp, engine)
}
fn repository(engine: &mut Engine, name: &str) -> Repository {
    engine
        .create_repository(
            &RepositoryName::parse(name).unwrap(),
            "P01",
            1,
            "admin",
            op(),
        )
        .unwrap()
}
fn save(engine: &mut Engine, repo: &Repository, path: &str) {
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, path, b"global path fixture")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: op(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "global path fixture".into(),
            request_digest: None,
        })
        .unwrap();
}
fn rename(engine: &mut Engine, repo: &Repository, source: &str, destination: &str) {
    engine
        .file_namespace(
            &repo.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::Rename, source, Some(destination))
                .unwrap(),
            "admin",
            op(),
        )
        .unwrap();
}
fn expected_operations(engine: &Engine) -> Vec<OperationResult> {
    let mut rows: Vec<_> = engine.state.operations.values().cloned().collect();
    rows.sort_by_key(|row| (row.sequence, row.operation_id));
    rows
}
fn operations(engine: &Engine, limit: usize) -> Vec<OperationResult> {
    let mut cursor = None;
    let mut rows = Vec::new();
    loop {
        let page = engine
            .system_operation_records(cursor.as_ref(), limit)
            .unwrap();
        assert_eq!(page.sequence, engine.state.sequence);
        rows.extend(page.records);
        cursor = page.next;
        if cursor.is_none() {
            return rows;
        }
    }
}
fn sql(engine: &Engine, statement: &'static str) {
    platform_io::sqlite::block_on_sqlite_connection(async {
        let mut connection = platform_io::sqlite::open_index_connection(
            &engine.root.join("index.sqlite"),
            false,
            None,
        )
        .await
        .unwrap();
        connection.execute(statement).await.unwrap();
        connection.close().await.unwrap();
    });
}

#[test]
fn real_publications_store_complete_global_receipts_and_exterior_path_rows_then_cold_rebuild() {
    let (temp, mut engine) = fixture();
    assert!(
        engine
            .system_operation_records(None, 10)
            .unwrap()
            .records
            .is_empty()
    );
    let left = repository(&mut engine, "left");
    let right = repository(&mut engine, "right");
    save(&mut engine, &left, "a.txt");
    save(&mut engine, &right, "other.txt");
    rename(&mut engine, &left, "a.txt", "b.txt");
    rename(&mut engine, &right, "other.txt", "renamed.txt");
    let expected = expected_operations(&engine);
    assert_eq!(operations(&engine, 1), expected);
    let page = engine.system_file_path_nodes(None, 100).unwrap();
    assert_eq!(page.records.len(), 2);
    assert_eq!(page.records[0].previous_path, "a.txt");
    assert_eq!(page.records[0].path, "b.txt");
    assert_eq!(page.records[1].history_tree_id, right.history_tree_id);
    let actual: String = platform_io::sqlite::block_on_sqlite_connection(async {
        let mut c =
            platform_io::sqlite::open_index_read_connection(&engine.root.join("index.sqlite"))
                .await
                .unwrap();
        let value = sqlite_access::query_scalar(
            "SELECT payload FROM system_changes WHERE kind=1 ORDER BY sequence_key,id LIMIT 1",
        )
        .fetch_one(&mut c)
        .await
        .unwrap();
        c.close().await.unwrap();
        value
    });
    assert_eq!(actual.as_bytes(), encode(&page.records[0]).unwrap());
    let saved_paths = page.records;
    drop(engine);
    // The database is derived: deleting it must neither invent an empty pool
    // nor discard the publications used to reconstruct these complete rows.
    std::fs::remove_file(temp.path().join("system/index.sqlite")).unwrap();
    let engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(operations(&engine, 2), expected);
    assert_eq!(
        engine.system_file_path_nodes(None, 100).unwrap().records,
        saved_paths
    );
}

#[test]
fn cursor_is_state_and_family_bound_and_invalid_limits_do_not_disable_the_database() {
    let (_temp, mut engine) = fixture();
    repository(&mut engine, "one");
    repository(&mut engine, "two");
    let cursor = engine
        .system_operation_records(None, 1)
        .unwrap()
        .next
        .unwrap();
    assert!(matches!(
        engine.system_file_path_nodes(Some(&cursor), 1),
        Err(SystemChangeReadError::Request(_))
    ));
    for limit in [0, 101, usize::MAX] {
        assert!(matches!(
            engine.system_operation_records(None, limit),
            Err(SystemChangeReadError::Request(_))
        ));
    }
    let (_other_temp, mut other) = fixture();
    // Same State bytes and sequence in a different Engine must not adopt the
    // first Engine's cursor; neither may a rebuilt projection of this State.
    other.state = Arc::clone(&engine.state);
    other.rebuild_index().unwrap();
    assert!(matches!(
        other.system_operation_records(Some(&cursor), 1),
        Err(SystemChangeReadError::Request(_))
    ));
    engine.rebuild_index().unwrap();
    assert!(matches!(
        engine.system_operation_records(Some(&cursor), 1),
        Err(SystemChangeReadError::Request(_))
    ));
    let cursor = engine
        .system_operation_records(None, 1)
        .unwrap()
        .next
        .unwrap();
    repository(&mut engine, "three");
    assert!(
        matches!(engine.system_operation_records(Some(&cursor), 1), Err(SystemChangeReadError::Request(error)) if error.code()==ErrorCode::HistoryChanged)
    );
    assert!(matches!(engine.index_health(), IndexHealth::Ready { .. }));
    assert_eq!(operations(&engine, 2), expected_operations(&engine));
}

#[test]
fn missing_extra_mutated_rows_order_keys_marker_and_schema_fail_closed_and_latch() {
    for statement in [
        "DELETE FROM system_changes WHERE id=(SELECT id FROM system_changes WHERE kind=0 ORDER BY sequence_key,id LIMIT 1)",
        "INSERT INTO system_changes VALUES(0,'',X'0000000000000000','{}')",
        "UPDATE system_changes SET payload='{}' WHERE kind=0",
        "UPDATE system_changes SET sequence_key=X'FFFFFFFFFFFFFFFF' WHERE kind=0",
        "PRAGMA ignore_check_constraints=ON;UPDATE system_changes SET sequence_key=X'01' WHERE kind=0",
        "UPDATE current_state SET sequence='99999'",
        "DROP INDEX system_change_order",
        "DROP TABLE system_changes;CREATE VIEW system_changes AS SELECT 0 AS kind,'' AS id,X'0000000000000000' AS sequence_key,'{}' AS payload",
        "CREATE TRIGGER injected_change AFTER INSERT ON system_changes BEGIN SELECT RAISE(ABORT,'injected');END",
        "UPDATE system_changes SET payload=hex(zeroblob(200000)) WHERE kind=0",
        "UPDATE system_changes SET payload=CAST(zeroblob(67108865) AS TEXT) WHERE kind=0",
    ] {
        let (_temp, mut engine) = fixture();
        repository(&mut engine, "one");
        repository(&mut engine, "two");
        let expected = expected_operations(&engine);
        sql(&engine, statement);
        assert!(
            matches!(
                engine.system_operation_records(None, 1),
                Err(SystemChangeReadError::Unavailable(_))
            ),
            "{statement}"
        );
        assert!(matches!(engine.index_health(), IndexHealth::Unavailable(_)));
        assert!(engine.system_operation_records(None, 100).is_err());
        engine.rebuild_index().unwrap();
        assert_eq!(operations(&engine, 1), expected, "{statement}");
    }
}

#[test]
fn projection_failure_never_rolls_back_an_already_published_operation() {
    let (temp, mut engine) = fixture();
    repository(&mut engine, "before");
    sql(
        &engine,
        "CREATE TRIGGER fail_system_change BEFORE INSERT ON system_changes BEGIN SELECT RAISE(ABORT,'injected');END;",
    );
    let published = repository(&mut engine, "after");
    assert_eq!(
        engine.state.repositories[&published.repository_id],
        published
    );
    assert!(!engine.is_read_only());
    assert!(matches!(
        engine.system_operation_records(None, 100),
        Err(SystemChangeReadError::Unavailable(_))
    ));
    let expected = expected_operations(&engine);
    drop(engine);
    let engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(operations(&engine, 100), expected);
}

#[test]
fn exact_delta_handles_replacement_removal_unsigned_order_and_unknown_lineage() {
    let (_temp, mut engine) = fixture();
    repository(&mut engine, "one");
    repository(&mut engine, "two");
    let base = Arc::clone(&engine.state);
    let state = Arc::make_mut(&mut engine.state);
    state.begin_tracking();
    let ids: Vec<_> = state.operations.keys().copied().collect();
    state.operations.get_mut(&ids[0]).unwrap().sequence = u64::MAX;
    state.operations.get_mut(&ids[0]).unwrap().result = json!({"changed":"same identity"});
    state.operations.get_mut(&ids[1]).unwrap().sequence = 1 << 63;
    state.sequence = u64::MAX;
    engine.update_index(&base);
    assert_eq!(operations(&engine, 1), expected_operations(&engine));
    let base = Arc::clone(&engine.state);
    let mut next = (*base).clone();
    next.operations.remove(&ids[0]);
    // A serialization round trip deliberately removes tracked-map lineage.
    let next: State = storage_contract::decode_strict(&canonical_json(&next).unwrap()).unwrap();
    engine.state = Arc::new(next);
    engine.update_index(&base);
    assert_eq!(operations(&engine, 1), expected_operations(&engine));
}

#[test]
fn retained_path_projection_is_not_filtered_by_current_placement() {
    let (_temp, mut engine) = fixture();
    let repo = repository(&mut engine, "paths");
    save(&mut engine, &repo, "before");
    rename(&mut engine, &repo, "before", "after");
    let rows = engine.system_file_path_nodes(None, 100).unwrap().records;
    // Isolated projection test, not admission of this synthetic business State.
    let base = Arc::clone(&engine.state);
    Arc::make_mut(&mut engine.state).placements.clear();
    engine.update_index(&base);
    assert_eq!(
        engine.system_file_path_nodes(None, 100).unwrap().records,
        rows
    );
    let base = Arc::clone(&engine.state);
    let state = Arc::make_mut(&mut engine.state);
    state.begin_tracking();
    let id = rows[0].node_id.clone();
    let row = state.file_path_nodes.get_mut(&id).unwrap();
    row.previous_path = "different previous path".into();
    row.sequence = u64::MAX;
    state.sequence = u64::MAX;
    engine.update_index(&base);
    assert_eq!(
        engine.system_file_path_nodes(None, 1).unwrap().records[0],
        engine.state.file_path_nodes[&id]
    );
    let base = Arc::clone(&engine.state);
    Arc::make_mut(&mut engine.state).file_path_nodes.remove(&id);
    engine.update_index(&base);
    assert!(
        engine
            .system_file_path_nodes(None, 1)
            .unwrap()
            .records
            .is_empty()
    );
}

#[test]
fn payload_byte_budget_pages_without_omitting_the_boundary_row() {
    let (_temp, mut engine) = fixture();
    repository(&mut engine, "one");
    repository(&mut engine, "two");
    let base = Arc::clone(&engine.state);
    let state = Arc::make_mut(&mut engine.state);
    state.begin_tracking();
    let ids: Vec<_> = state.operations.keys().copied().collect();
    for id in ids {
        state.operations.get_mut(&id).unwrap().result = json!("x".repeat(MAX_PAGE_BYTES / 2));
    }
    // Isolated projection-size fixture; this does not claim admission of a
    // new business request with this synthetic receipt payload.
    engine.update_index(&base);
    let first = engine.system_operation_records(None, 100).unwrap();
    assert_eq!(first.records.len(), 1);
    assert!(encode(&first.records[0]).unwrap().len() <= MAX_PAGE_BYTES);
    let second = engine
        .system_operation_records(first.next.as_ref(), 100)
        .unwrap();
    assert_eq!(second.records.len(), 1);
    assert_ne!(
        first.records[0].operation_id,
        second.records[0].operation_id
    );
    assert!(second.next.is_none());
}

#[test]
fn named_database_replacement_is_preserved_and_not_read_as_current_system_history() {
    let (_temp, mut engine) = fixture();
    repository(&mut engine, "one");
    let path = engine.root.join("index.sqlite");
    std::fs::rename(&path, engine.root.join("original-index.sqlite")).unwrap();
    std::fs::write(&path, b"unknown replacement").unwrap();
    assert!(matches!(
        engine.system_operation_records(None, 100),
        Err(SystemChangeReadError::Unavailable(_))
    ));
    assert_eq!(std::fs::read(path).unwrap(), b"unknown replacement");
}

fn catalog(engine: &Engine, kind: SystemCatalogKind, limit: usize) -> Vec<SystemCatalogRecord> {
    let mut cursor = None;
    let mut rows = Vec::new();
    loop {
        let page = engine
            .system_catalog_records(kind, cursor.as_ref(), limit)
            .unwrap();
        assert_eq!(page.sequence, engine.state.sequence);
        rows.extend(page.records);
        cursor = page.next;
        if cursor.is_none() {
            return rows;
        }
    }
}

#[test]
fn global_catalog_retains_full_namespace_rows_across_repositories_and_cold_rebuild() {
    let (temp, mut engine) = fixture();
    let left = repository(&mut engine, "left");
    let right = repository(&mut engine, "right");
    save(&mut engine, &left, "folder/same.txt");
    save(&mut engine, &right, "folder/same.txt");
    rename(&mut engine, &left, "folder/same.txt", "folder/moved.txt");
    let expected: Vec<_> = engine
        .state
        .commits
        .iter()
        .map(|(id, value)| SystemCatalogRecord {
            id: id.clone(),
            value: SystemCatalogValue::NamespaceCommit(value.clone()),
        })
        .collect();
    assert_eq!(
        catalog(&engine, SystemCatalogKind::NamespaceCommit, 1),
        expected
    );
    assert_eq!(catalog(&engine, SystemCatalogKind::Repository, 1).len(), 2);
    assert!(expected.iter().any(|row| matches!(&row.value,
        SystemCatalogValue::NamespaceCommit(commit) if commit.entries.iter().any(|entry| entry.path() == "folder/same.txt"))));
    assert!(expected.iter().any(|row| matches!(&row.value,
        SystemCatalogValue::NamespaceCommit(commit) if commit.entries.iter().any(|entry| entry.path() == "folder/moved.txt"))));
    drop(engine);
    let reopened = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(
        catalog(&reopened, SystemCatalogKind::NamespaceCommit, 2),
        expected
    );
}

#[test]
fn catalog_location_replacements_removals_and_long_keys_have_exact_projection() {
    let (_temp, mut engine) = fixture();
    let repo = repository(&mut engine, "files");
    save(&mut engine, &repo, "before.txt");
    let file = engine
        .state
        .versions
        .values()
        .next()
        .unwrap()
        .file_id
        .clone();
    let before = Arc::clone(&engine.state);
    let next = Arc::make_mut(&mut engine.state);
    next.begin_tracking();
    next.sequence += 1;
    next.file_locations.insert(file.clone(), "after.txt".into());
    let long_key = "selection-key".repeat(100);
    next.directory_selections
        .insert(long_key.clone(), "selection".into());
    engine.update_index(&before);
    assert_eq!(
        catalog(&engine, SystemCatalogKind::FileLocation, 1),
        vec![SystemCatalogRecord {
            id: file.clone(),
            value: SystemCatalogValue::FileLocation("after.txt".into()),
        }]
    );
    assert!(
        catalog(&engine, SystemCatalogKind::DirectorySelection, 10)
            .iter()
            .any(|record| record.id == long_key)
    );
    let before = Arc::clone(&engine.state);
    let next = Arc::make_mut(&mut engine.state);
    next.begin_tracking();
    next.sequence += 1;
    next.file_locations.remove(&file);
    next.directory_selections.remove(&long_key);
    engine.update_index(&before);
    assert!(catalog(&engine, SystemCatalogKind::FileLocation, 10).is_empty());
    assert!(
        !catalog(&engine, SystemCatalogKind::DirectorySelection, 10)
            .iter()
            .any(|record| record.id == long_key)
    );
}

#[test]
fn catalog_cursor_cannot_cross_family_or_publication() {
    let (_temp, mut engine) = fixture();
    repository(&mut engine, "left");
    repository(&mut engine, "right");
    let page = engine
        .system_catalog_records(SystemCatalogKind::Repository, None, 1)
        .unwrap();
    let cursor = page.next.as_ref().unwrap();
    assert!(matches!(
        engine.system_catalog_records(SystemCatalogKind::Branch, Some(cursor), 1),
        Err(SystemChangeReadError::Request(_))
    ));
    assert!(matches!(
        engine.system_operation_records(Some(cursor), 1),
        Err(SystemChangeReadError::Request(_))
    ));
    repository(&mut engine, "third");
    assert!(matches!(
        engine.system_catalog_records(SystemCatalogKind::Repository, Some(cursor), 1),
        Err(SystemChangeReadError::Request(_))
    ));
    assert!(matches!(engine.index_health(), IndexHealth::Ready { .. }));
}

#[test]
fn catalog_rejects_missing_and_extra_first_rows_without_false_empty_success() {
    for statement in [
        "DELETE FROM system_changes WHERE kind=2",
        "INSERT INTO system_changes VALUES(2,'',X'0000000000000000','{}')",
        "UPDATE system_changes SET payload='{}' WHERE kind=2",
    ] {
        let (_temp, mut engine) = fixture();
        repository(&mut engine, "repo");
        sql(&engine, statement);
        assert!(matches!(
            engine.system_catalog_records(SystemCatalogKind::Repository, None, 1),
            Err(SystemChangeReadError::Unavailable(_))
        ));
        assert!(matches!(engine.index_health(), IndexHealth::Unavailable(_)));
    }
}
