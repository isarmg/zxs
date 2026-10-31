use super::*;

fn scope_count(engine: &Engine) -> Option<usize> {
    engine
        .with_history_scope("file", "history", |reader| {
            reader
                .nodes()
                .try_fold(0, |count, node| node.map(|_| count + 1))
        })
        .unwrap()
}

#[test]
fn scope_membership_batches_and_nested_streams_match_state() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = engine(&temp);
    for n in 0..130 {
        let id = format!("v{n:03}");
        Arc::make_mut(&mut engine.state)
            .versions
            .insert(id.clone(), version(&id, "file", n));
    }
    // The outer cursor holds its batch while a separate cursor completes.
    engine.rebuild_index().unwrap();
    reset_scope_read_metrics();
    let result = engine
        .with_history_scope("file", "history", |reader| {
            let mut outer = reader.versions();
            let first = outer.next().unwrap()?;
            assert!(std::ptr::eq(first, &engine.state.versions["v000"]));
            assert_eq!(reader.versions().collect::<Result<Vec<_>>>()?.len(), 130);
            assert_eq!(outer.collect::<Result<Vec<_>>>()?.len(), 129);
            assert_eq!(
                reader.commit_entries().collect::<Result<Vec<_>>>()?.len(),
                0
            );
            Ok(first.version_id.clone())
        })
        .unwrap();
    assert_eq!(result, Some("v000".into()));
    assert_eq!(scope_read_metrics(), ([260, 0, 0, 0], 64));
    assert_eq!(scope_count(&engine), Some(130));
}

#[test]
fn scope_missing_middle_wrong_scope_and_forged_substitution_disable_and_rebuild() {
    for statement in [
        "DELETE FROM history_scope_members WHERE kind=0 AND id='c'",
        "DELETE FROM history_scope_members WHERE kind=0 AND id='other_0';UPDATE history_scope_members SET id='other_0' WHERE kind=0 AND id='c'",
        "UPDATE history_scope_members SET entry_key='0000000000000000' WHERE kind=0 AND id='c'",
        "UPDATE history_scope_members SET history_tree_id='wrong' WHERE kind=0 AND id='c'",
        "UPDATE current_state SET sequence='999'",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let mut engine = engine(&temp);
        populate(&mut engine);
        sql(&engine.root.join("index.sqlite"), statement);
        assert_eq!(scope_count(&engine), None, "{statement}");
        assert!(!index_ready(&engine));
        assert_eq!(index_failure(&engine).stage(), IndexFailureStage::Read);
        engine.rebuild_index().unwrap();
        assert_eq!(scope_count(&engine), Some(5));
    }
}

#[test]
fn scope_unconsumed_or_swallowed_failure_never_accepts_partial_result() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = engine(&temp);
    populate(&mut engine);
    assert_eq!(
        engine
            .with_history_scope("file", "history", |reader| {
                let _first = reader.versions().next().unwrap()?;
                Ok(1)
            })
            .unwrap(),
        None
    );
    engine.rebuild_index().unwrap();
    sql(
        &engine.root.join("index.sqlite"),
        "DELETE FROM history_scope_members WHERE kind=0 AND id='c'",
    );
    assert_eq!(
        engine
            .with_history_scope("file", "history", |reader| {
                for result in reader.versions() {
                    let _ignored = result;
                }
                Ok(1)
            })
            .unwrap(),
        None
    );
    assert!(
        index_failure(&engine)
            .cause()
            .to_string()
            .contains("incomplete scope")
    );
}

#[test]
fn scope_update_moves_members_and_owner_covers_multiple_histories() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = engine(&temp);
    populate(&mut engine);
    let previous = Arc::clone(&engine.state);
    let state = Arc::make_mut(&mut engine.state);
    state.begin_tracking();
    state.versions.get_mut("a").unwrap().history_tree_id = "different".into();
    state.sequence += 1;
    engine.update_index(&previous);
    assert_eq!(scope_count(&engine), Some(4));
    assert_eq!(
        engine
            .with_history_scope("file", "different", |r| r
                .versions()
                .try_fold(0, |n, v| v.map(|_| n + 1)))
            .unwrap(),
        Some(1)
    );
    assert_eq!(
        engine
            .indexed_history_owner(
                "file",
                &storage_contract::history_window::HistoryWindowCancellation::new()
            )
            .unwrap()
            .unwrap()
            .version_id,
        "a"
    );
    assert!(
        engine
            .indexed_history_owner(
                "absent",
                &storage_contract::history_window::HistoryWindowCancellation::new()
            )
            .unwrap()
            .is_none()
    );
}

fn commit_with_entries() -> Commit {
    let entry = Entry {
        entry_id: "entry".into(),
        path: name_policy::PortablePath::parse("file.txt").unwrap(),
        kind: NamespaceEntryKind::File,
        file_id: "file".into(),
        version_id: "a".into(),
        size: 0,
    };
    Commit {
        commit_id: "commit".into(),
        history_tree_id: "history".into(),
        kind: "save".into(),
        parent_commit_ids: vec![],
        tree_id: "tree".into(),
        sequence: 1,
        operation_id: storage_contract::OperationId::generate(),
        author: "author".into(),
        message: String::new(),
        entries: vec![entry.clone(), entry],
        namespace_deletions: None,
        pending_file_versions: BTreeMap::new(),
        provenance: None,
    }
}
#[test]
fn scope_commit_members_are_entry_positions_not_deduplicated_file_ids() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = engine(&temp);
    let commit = commit_with_entries();
    Arc::make_mut(&mut engine.state)
        .commits
        .insert(commit.commit_id.clone(), commit);
    engine.rebuild_index().unwrap();
    assert_eq!(
        engine
            .with_history_scope("file", "history", |reader| {
                let entries = reader.commit_entries().collect::<Result<Vec<_>>>()?;
                assert_eq!(entries.len(), 2);
                assert!(std::ptr::eq(
                    entries[0].1,
                    &engine.state.commits["commit"].entries[0]
                ));
                assert!(std::ptr::eq(
                    entries[1].1,
                    &engine.state.commits["commit"].entries[1]
                ));
                Ok(entries.len())
            })
            .unwrap(),
        Some(2)
    );
    sql(
        &engine.root.join("index.sqlite"),
        "UPDATE history_scope_members SET entry_key='0000000000000002' WHERE entry_key='0000000000000001'",
    );
    assert_eq!(
        engine
            .with_history_scope("file", "history", |reader| reader
                .commit_entries()
                .try_fold(0, |n, v| v.map(|_| n + 1)))
            .unwrap(),
        None
    );
    assert!(
        index_failure(&engine)
            .cause()
            .to_string()
            .contains("unknown scope commit entry")
    );
}

#[test]
fn scope_callback_error_preserves_original_error_and_healthy_index() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = engine(&temp);
    populate(&mut engine);
    let error = engine
        .with_history_scope::<()>("file", "history", |reader| {
            let _first = reader.versions().next().unwrap()?;
            Err(Error::new(
                ErrorCode::InvalidRequest,
                "caller cursor invalid",
            ))
        })
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        Error::new(ErrorCode::InvalidRequest, "caller cursor invalid").to_string()
    );
    assert!(index_ready(&engine));
    assert_eq!(scope_count(&engine), Some(5));
}

#[test]
fn scope_snapshot_is_send_sync_and_holds_old_wal_view_across_update_and_rebuild() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<OwnedScopeRead>();
    let temp = tempfile::tempdir().unwrap();
    let mut engine = engine(&temp);
    populate(&mut engine);
    let before = Arc::clone(&engine.state);
    let captured = engine
        .capture_history_scope("file", "history")
        .unwrap()
        .unwrap();
    reset_scope_read_metrics();
    assert_eq!(scope_read_metrics(), ([0; 4], 0));
    let state = Arc::make_mut(&mut engine.state);
    state.begin_tracking();
    state.versions.remove("a");
    state.sequence += 1;
    engine.update_index(&before);
    assert!(index_ready(&engine));
    engine.rebuild_index().unwrap();
    assert_eq!(
        captured
            .with_state(&before, |r| r
                .versions()
                .try_fold(0, |n, v| v.map(|_| n + 1)))
            .unwrap(),
        Some(5)
    );
    engine.record_scope_failure(&captured);
    assert!(index_ready(&engine));
    assert_eq!(scope_count(&engine), Some(4));
    assert_eq!(captured.with_state(&before, |_| Ok(99)).unwrap(), None);
}

#[test]
fn scope_snapshot_drop_releases_unconsumed_wal_reader() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = engine(&temp);
    populate(&mut engine);
    let snapshot = engine
        .capture_history_scope("file", "history")
        .unwrap()
        .unwrap();
    let before = Arc::clone(&engine.state);
    let state = Arc::make_mut(&mut engine.state);
    state.begin_tracking();
    state.versions.remove("a");
    state.sequence += 1;
    engine.update_index(&before);
    drop(snapshot);
    let path = engine.root.join("index.sqlite");
    platform_io::sqlite::block_on_sqlite_connection(async {
        let mut c = platform_io::sqlite::open_index_connection(&path, false, None)
            .await
            .unwrap();
        let row = sqlite_access::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .fetch_one(&mut c)
            .await
            .unwrap();
        assert_eq!(
            row.get::<i64, _>(0),
            0,
            "dropped read snapshot must not retain WAL lock"
        );
        c.close().await.unwrap();
    });
}

#[test]
fn scope_old_snapshot_failure_does_not_disable_rebuilt_generation() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = engine(&temp);
    populate(&mut engine);
    sql(
        &engine.root.join("index.sqlite"),
        "DELETE FROM history_scope_members WHERE kind=0 AND id='c'",
    );
    let snapshot = engine
        .capture_history_scope("file", "history")
        .unwrap()
        .unwrap();
    engine.rebuild_index().unwrap();
    assert_eq!(
        snapshot
            .with_state(&engine.state, |r| r
                .versions()
                .try_fold(0, |n, v| v.map(|_| n + 1)))
            .unwrap(),
        None
    );
    engine.record_scope_failure(&snapshot);
    assert!(index_ready(&engine));
    assert_eq!(scope_count(&engine), Some(5));
}
