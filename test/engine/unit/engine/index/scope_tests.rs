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
    assert_eq!(scope_read_metrics(), ([260, 0, 0, 0, 0, 0], 64));
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
    assert_eq!(scope_read_metrics(), ([0; 6], 0));
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

fn file_branch(id: &str, file: Option<&str>, history: &str) -> Branch {
    Branch {
        branch_id: id.into(),
        history_tree_id: history.into(),
        name: id.into(),
        head_commit_id: "commit".into(),
        fork_commit_id: "commit".into(),
        branch_kind: BranchKind::File,
        file_id: file.map(str::to_owned),
        file_node_id: Some("a".into()),
        pending_file_versions: BTreeMap::new(),
        generation: 1,
        state: BranchState::Active,
    }
}
fn branch_count(engine: &Engine) -> Option<usize> {
    engine
        .with_history_scope("file", "history", |reader| {
            reader
                .branches()
                .try_fold(0, |count, branch| branch.map(|_| count + 1))
        })
        .unwrap()
}
#[test]
fn scope_branches_are_complete_scoped_borrowed_and_tracked() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = engine(&temp);
    let state = Arc::make_mut(&mut engine.state);
    for n in 0..130 {
        let id = format!("branch{n:03}");
        let mut branch = file_branch(&id, Some("file"), "history");
        // Match allowed_branch: state/kind are not extra membership filters.
        if n == 1 {
            branch.state = BranchState::Merged;
        }
        if n == 2 {
            branch.state = BranchState::Migrated;
        }
        if n == 3 {
            branch.branch_kind = BranchKind::Full;
        }
        state.branches.insert(id, branch);
    }
    for branch in [
        file_branch("directory", None, "history"),
        file_branch("otherfile", Some("other"), "history"),
        file_branch("otherhistory", Some("file"), "elsewhere"),
    ] {
        state.branches.insert(branch.branch_id.clone(), branch);
    }
    engine.rebuild_index().unwrap();
    reset_scope_read_metrics();
    assert_eq!(
        engine
            .with_history_scope("file", "history", |reader| {
                let mut count = 0;
                for branch in reader.branches() {
                    let branch = branch?;
                    assert!(std::ptr::eq(
                        branch,
                        &engine.state.branches[&branch.branch_id]
                    ));
                    count += 1;
                }
                Ok(count)
            })
            .unwrap(),
        Some(130)
    );
    assert_eq!(scope_read_metrics(), ([0, 0, 0, 0, 130, 0], 64));
    let before = Arc::clone(&engine.state);
    let state = Arc::make_mut(&mut engine.state);
    state.begin_tracking();
    state.branches.get_mut("branch000").unwrap().file_id = None;
    state.branches.get_mut("directory").unwrap().file_id = Some("file".into());
    state.branches.get_mut("branch001").unwrap().history_tree_id = "elsewhere".into();
    state.branches.remove("branch002");
    state.sequence += 1;
    engine.update_index(&before);
    assert_eq!(branch_count(&engine), Some(128));
    engine.rebuild_index().unwrap();
    assert_eq!(branch_count(&engine), Some(128));
}
#[test]
fn scope_branch_corruption_never_returns_partial_or_foreign_membership() {
    for statement in [
        "DELETE FROM history_scope_members WHERE kind=4 AND id='b'",
        "DELETE FROM history_scope_members WHERE kind=4 AND id='foreign';UPDATE history_scope_members SET id='foreign' WHERE kind=4 AND id='b'",
        "UPDATE history_scope_members SET id='directory' WHERE kind=4 AND id='b'",
        "UPDATE history_scope_members SET entry_key='0000000000000000' WHERE kind=4 AND id='b'",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let mut engine = engine(&temp);
        for branch in [
            file_branch("a", Some("file"), "history"),
            file_branch("b", Some("file"), "history"),
            file_branch("c", Some("file"), "history"),
            file_branch("foreign", Some("other"), "history"),
            file_branch("directory", None, "history"),
        ] {
            Arc::make_mut(&mut engine.state)
                .branches
                .insert(branch.branch_id.clone(), branch);
        }
        engine.rebuild_index().unwrap();
        sql(&engine.root.join("index.sqlite"), statement);
        assert_eq!(branch_count(&engine), None, "{statement}");
        assert!(!index_ready(&engine));
        engine.rebuild_index().unwrap();
        assert_eq!(branch_count(&engine), Some(3));
    }
}

fn file_merge(id: &str, file: &str, history: &str) -> FileBranchMerge {
    FileBranchMerge {
        merge_id: id.into(),
        history_tree_id: history.into(),
        file_id: file.into(),
        input_branch_ids: vec!["branch".into()],
        input_node_ids: vec!["a".into()],
        chosen_node_id: "a".into(),
        result_node_id: "a".into(),
        surviving_branch_id: "branch".into(),
        reused_node: true,
        parent_node_ids: vec![],
        operation_id: storage_contract::OperationId::generate(),
        sequence: 1,
    }
}
fn merge_count(engine: &Engine) -> Option<usize> {
    engine
        .with_history_scope("file", "history", |reader| {
            reader
                .merges()
                .try_fold(0, |count, merge| merge.map(|_| count + 1))
        })
        .unwrap()
}
#[test]
fn scope_merges_are_complete_borrowed_batched_and_track_moves() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = engine(&temp);
    for n in 0..130 {
        let id = format!("merge{n:03}");
        Arc::make_mut(&mut engine.state)
            .file_branch_merges
            .insert(id.clone(), file_merge(&id, "file", "history"));
    }
    for merge in [
        file_merge("otherfile", "other", "history"),
        file_merge("otherhistory", "file", "elsewhere"),
    ] {
        Arc::make_mut(&mut engine.state)
            .file_branch_merges
            .insert(merge.merge_id.clone(), merge);
    }
    engine.rebuild_index().unwrap();
    reset_scope_read_metrics();
    assert_eq!(
        engine
            .with_history_scope("file", "history", |reader| {
                let mut count = 0;
                for merge in reader.merges() {
                    let merge = merge?;
                    assert!(std::ptr::eq(
                        merge,
                        &engine.state.file_branch_merges[&merge.merge_id]
                    ));
                    count += 1;
                }
                Ok(count)
            })
            .unwrap(),
        Some(130)
    );
    assert_eq!(scope_read_metrics(), ([0, 0, 0, 0, 0, 130], 64));
    let before = Arc::clone(&engine.state);
    let state = Arc::make_mut(&mut engine.state);
    state.begin_tracking();
    state
        .file_branch_merges
        .get_mut("merge000")
        .unwrap()
        .file_id = "other".into();
    state
        .file_branch_merges
        .get_mut("merge001")
        .unwrap()
        .history_tree_id = "elsewhere".into();
    state.file_branch_merges.remove("merge002");
    state
        .file_branch_merges
        .insert("new".into(), file_merge("new", "file", "history"));
    state.sequence += 1;
    engine.update_index(&before);
    assert_eq!(merge_count(&engine), Some(128));
    engine.rebuild_index().unwrap();
    assert_eq!(merge_count(&engine), Some(128));
}
#[test]
fn scope_merge_corruption_rejects_missing_and_foreign_substitutes() {
    for statement in [
        "DELETE FROM history_scope_members WHERE kind=5 AND id='b'",
        "DELETE FROM history_scope_members WHERE kind=5 AND id='foreign';UPDATE history_scope_members SET id='foreign' WHERE kind=5 AND id='b'",
        "DELETE FROM history_scope_members WHERE kind=5 AND id='otherhistory';UPDATE history_scope_members SET id='otherhistory' WHERE kind=5 AND id='b'",
        "UPDATE history_scope_members SET entry_key='0000000000000000' WHERE kind=5 AND id='b'",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let mut engine = engine(&temp);
        for merge in [
            file_merge("a", "file", "history"),
            file_merge("b", "file", "history"),
            file_merge("c", "file", "history"),
            file_merge("foreign", "other", "history"),
            file_merge("otherhistory", "file", "elsewhere"),
        ] {
            Arc::make_mut(&mut engine.state)
                .file_branch_merges
                .insert(merge.merge_id.clone(), merge);
        }
        engine.rebuild_index().unwrap();
        sql(&engine.root.join("index.sqlite"), statement);
        assert_eq!(merge_count(&engine), None, "{statement}");
        assert!(!index_ready(&engine));
        engine.rebuild_index().unwrap();
        assert_eq!(merge_count(&engine), Some(3));
    }
}
