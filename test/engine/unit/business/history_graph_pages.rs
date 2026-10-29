use super::*;
use storage_contract::{
    BranchKind, BranchState, CommitNamespace, HistoryState, NamespaceDeletions, NamespaceEntryKind,
    NamespaceRefKind, NamespaceReference, OperationId,
};

fn id(prefix: &str, n: u64) -> String {
    format!("{prefix}_{n:032x}")
}
fn fixture() -> Arc<State> {
    let mut state = State::default();
    let history = id("history", 1);
    state.histories.insert(
        history.clone(),
        HistoryTree {
            history_tree_id: history.clone(),
            repository_id: id("repo", 1),
            root_commit_id: id("commit", 1),
            main_branch_id: Some(id("br", 1)),
            generation: 1,
            state: HistoryState::Active,
            provenance: None,
        },
    );
    for n in 1..=4 {
        let commit_id = id("commit", n);
        state.commits.insert(
            commit_id.clone(),
            Commit {
                commit_id,
                history_tree_id: history.clone(),
                kind: "save".into(),
                parent_commit_ids: if n == 1 {
                    Vec::new()
                } else {
                    vec![id("commit", n - 1)]
                },
                tree_id: "namespace".into(),
                sequence: if n == 1 { 0 } else { 1 },
                operation_id: OperationId::generate(),
                author: "author".into(),
                message: "message".into(),
                namespace: CommitNamespace {
                    root: NamespaceReference {
                        kind: NamespaceRefKind::Page,
                        hash: "a".repeat(64),
                    },
                    entries: 0,
                },
                namespace_deletions: None,
                pending_file_versions: BTreeMap::new(),
                provenance: None,
            },
        );
        let branch_id = id("br", n);
        state.branches.insert(
            branch_id.clone(),
            Branch {
                branch_id,
                history_tree_id: history.clone(),
                name: format!("branch{n}"),
                head_commit_id: id("commit", n),
                fork_commit_id: id("commit", 1),
                branch_kind: BranchKind::Full,
                file_id: None,
                file_node_id: None,
                pending_file_versions: BTreeMap::new(),
                generation: 1,
                state: BranchState::Active,
            },
        );
    }
    state.merge_links.insert(
        id("link", 1),
        MergeLink {
            merge_link_id: id("link", 1),
            history_tree_id: history.clone(),
            source_branch_id: id("br", 1),
            source_commit_id: id("commit", 1),
            target_branch_id: id("br", 2),
            target_commit_id: id("commit", 2),
            author: "author".into(),
            operation_id: OperationId::generate(),
        },
    );
    state.history_boundaries.insert(
        id("boundary", 1),
        HistoryBoundary {
            boundary_id: id("boundary", 1),
            history_tree_id: history.clone(),
            local_commit_id: id("commit", 1),
            local_branch_id: None,
            relation: "split".into(),
            external_history_tree_id: id("history", 2),
            external_commit_id: id("commit", 50),
            source_merge_link_id: None,
        },
    );
    let mut foreign = state.commits[&id("commit", 1)].clone();
    foreign.commit_id = id("commit", 0);
    foreign.history_tree_id = id("history", 2);
    state.commits.insert(foreign.commit_id.clone(), foreign);
    Arc::new(state)
}
fn plan(state: &Arc<State>) -> HistoryGraphReadPlan {
    HistoryGraphReadPlan {
        state: Arc::clone(state),
        history_id: id("history", 1),
        root: HistoryRoot::parse(&"a".repeat(64)).unwrap(),
        membership: None,
    }
}
fn page_ids(items: GraphItems) -> Vec<String> {
    match items {
        GraphItems::Commits(records) => {
            records.into_iter().map(|record| record.commit_id).collect()
        }
        GraphItems::Branches(records) => {
            records.into_iter().map(|record| record.branch_id).collect()
        }
        GraphItems::MergeLinks(records) => records
            .into_iter()
            .map(|record| record.merge_link_id)
            .collect(),
        GraphItems::BoundaryMarks(records) => records
            .into_iter()
            .map(|record| record.boundary_id)
            .collect(),
    }
}

#[test]
fn graph_one_record_pages_preserve_same_sequence_id_order_and_complete_collections() {
    let state = fixture();
    for (collection, expected) in [
        (
            GraphCollection::Commits,
            (1..=4).map(|n| id("commit", n)).collect::<Vec<_>>(),
        ),
        (
            GraphCollection::Branches,
            (1..=4).map(|n| id("br", n)).collect(),
        ),
        (GraphCollection::MergeLinks, vec![id("link", 1)]),
        (GraphCollection::BoundaryMarks, vec![id("boundary", 1)]),
    ] {
        let mut cursor = None;
        let mut actual = Vec::new();
        loop {
            let page = plan(&state)
                .read_page(
                    collection,
                    cursor.as_ref(),
                    PageLimit::new(1).unwrap(),
                    || Ok(()),
                )
                .unwrap();
            assert!(page_ids(page.items.clone()).len() <= 1);
            actual.extend(page_ids(page.items));
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(actual, expected);
    }
}

#[test]
fn graph_cursor_rejects_wrong_collection_foreign_record_and_wrong_sequence() {
    let state = fixture();
    for key in [
        GraphKey::Id { id: id("br", 1) },
        GraphKey::Commit {
            sequence: 0,
            commit_id: id("commit", 0),
        },
        GraphKey::Commit {
            sequence: 100,
            commit_id: id("commit", 1),
        },
    ] {
        let error = plan(&state)
            .read_page(
                GraphCollection::Commits,
                Some(&key),
                PageLimit::new(1).unwrap(),
                || Ok(()),
            )
            .err()
            .unwrap();
        assert_eq!(error.code(), ErrorCode::InvalidCursor);
    }
}

#[test]
fn graph_text_slices_reconstruct_utf8_and_preserve_absence_and_empty() {
    let text = "a🦀中\n\"b".repeat(20);
    let mut offset = 0;
    let mut result = String::new();
    loop {
        let part = text_slice(Some(&text), offset, TextLimit::new(4).unwrap()).unwrap();
        assert!(part.text.as_ref().unwrap().len() <= 4);
        assert_eq!(part.total_bytes, text.len());
        result.push_str(part.text.as_ref().unwrap());
        match part.next_offset {
            Some(next) => {
                assert!(next > offset);
                offset = next;
            }
            None => break,
        }
    }
    assert_eq!(result, text);
    assert_eq!(
        text_slice(None, 0, TextLimit::new(4).unwrap())
            .unwrap()
            .text,
        None
    );
    assert_eq!(
        text_slice(Some(""), 0, TextLimit::new(4).unwrap())
            .unwrap()
            .text,
        Some(String::new())
    );
    assert!(text_slice(Some("🦀"), 1, TextLimit::new(4).unwrap()).is_err());
    assert!(text_slice(Some("a"), usize::MAX, TextLimit::new(4).unwrap()).is_err());
    assert!(text_slice(None, 1, TextLimit::new(4).unwrap()).is_err());
}

#[test]
fn graph_all_large_text_fields_stay_deferred_and_are_individually_readable() {
    let mut state = fixture();
    let state_mut = Arc::make_mut(&mut state);
    let text = "🦀".repeat(8192);
    let commit = state_mut.commits.get_mut(&id("commit", 1)).unwrap();
    commit.kind = text.clone();
    commit.tree_id = text.clone();
    commit.author = text.clone();
    commit.message = text.clone();
    commit.provenance = Some(text.clone());
    state_mut
        .histories
        .get_mut(&id("history", 1))
        .unwrap()
        .provenance = Some(text.clone());
    state_mut
        .merge_links
        .get_mut(&id("link", 1))
        .unwrap()
        .author = text.clone();
    let boundary = state_mut
        .history_boundaries
        .get_mut(&id("boundary", 1))
        .unwrap();
    boundary.relation = text.clone();
    boundary.source_merge_link_id = Some(text.clone());
    let page = plan(&state)
        .read_page(
            GraphCollection::Commits,
            None,
            PageLimit::new(1).unwrap(),
            || Ok(()),
        )
        .unwrap();
    let GraphItems::Commits(records) = page.items else {
        panic!("expected commits")
    };
    let deferred = TextSummary::Deferred {
        bytes: text.len() as u64,
    };
    assert_eq!(records[0].kind, deferred);
    assert_eq!(records[0].tree_id, deferred);
    assert_eq!(records[0].author, deferred);
    assert_eq!(records[0].message, deferred);
    assert_eq!(records[0].provenance, Some(deferred.clone()));
    assert_eq!(page.history.provenance, Some(deferred));
    let mut targets = vec![
        TextTarget::History {
            field: HistoryField::Provenance,
        },
        TextTarget::MergeLink {
            id: id("link", 1),
            field: MergeLinkField::Author,
        },
        TextTarget::Boundary {
            id: id("boundary", 1),
            field: BoundaryField::Relation,
        },
        TextTarget::Boundary {
            id: id("boundary", 1),
            field: BoundaryField::SourceMergeLinkId,
        },
    ];
    for field in [
        CommitField::Kind,
        CommitField::TreeId,
        CommitField::Author,
        CommitField::Message,
        CommitField::Provenance,
    ] {
        targets.push(TextTarget::Commit {
            id: id("commit", 1),
            field,
        });
    }
    for target in targets {
        let part = plan(&state)
            .read_text(&target, 0, TextLimit::new(4).unwrap(), || Ok(()))
            .unwrap();
        assert_eq!(part.text, Some("🦀".into()));
        assert_eq!(part.total_bytes, text.len());
        assert_eq!(part.next_offset, Some(4));
    }
}

#[test]
fn graph_auxiliary_targets_cannot_cross_history() {
    let state = fixture();
    let target = TextTarget::Commit {
        id: id("commit", 0),
        field: CommitField::Message,
    };
    assert_eq!(
        plan(&state)
            .read_text(&target, 0, TextLimit::new(4).unwrap(), || Ok(()))
            .err()
            .unwrap()
            .code(),
        ErrorCode::NotFound
    );
    assert_eq!(
        plan(&state)
            .read_pending(
                &PendingTarget::Commit {
                    id: id("commit", 0)
                },
                None,
                PageLimit::new(1).unwrap(),
                || Ok(())
            )
            .err()
            .unwrap()
            .code(),
        ErrorCode::NotFound
    );
    assert_eq!(
        plan(&state)
            .read_deletions(&id("commit", 0), 0, PageLimit::new(1).unwrap(), || Ok(()))
            .err()
            .unwrap()
            .code(),
        ErrorCode::NotFound
    );
}

#[test]
fn graph_pending_pages_use_exclusive_keys_without_skipping() {
    let mut state = fixture();
    let records = (1..=5)
        .map(|n| (id("file", n), id("ver", n)))
        .collect::<BTreeMap<_, _>>();
    let state_mut = Arc::make_mut(&mut state);
    state_mut
        .commits
        .get_mut(&id("commit", 1))
        .unwrap()
        .pending_file_versions = records.clone();
    state_mut
        .branches
        .get_mut(&id("br", 1))
        .unwrap()
        .pending_file_versions = records.clone();
    for target in [
        PendingTarget::Commit {
            id: id("commit", 1),
        },
        PendingTarget::Branch { id: id("br", 1) },
    ] {
        let mut after = None;
        let mut actual = BTreeMap::new();
        loop {
            let page = plan(&state)
                .read_pending(
                    &target,
                    after.as_deref(),
                    PageLimit::new(2).unwrap(),
                    || Ok(()),
                )
                .unwrap();
            actual.extend(
                page.items
                    .into_iter()
                    .map(|record| (record.file_id, record.version_id)),
            );
            after = page.next;
            if after.is_none() {
                break;
            }
        }
        assert_eq!(actual, records);
        assert!(
            plan(&state)
                .read_pending(
                    &target,
                    Some(&id("file", 100)),
                    PageLimit::new(1).unwrap(),
                    || Ok(())
                )
                .is_err()
        );
    }
}

#[test]
fn graph_deletion_pages_preserve_order_duplicates_timestamp_and_absence() {
    let mut state = fixture();
    let paths = ["b", "a", "b"]
        .into_iter()
        .map(|path| DeletedPath {
            path: path.into(),
            kind: NamespaceEntryKind::File,
        })
        .collect::<Vec<_>>();
    Arc::make_mut(&mut state)
        .commits
        .get_mut(&id("commit", 1))
        .unwrap()
        .namespace_deletions = Some(NamespaceDeletions {
        timestamp: 42,
        paths: paths.clone(),
    });
    let mut actual = Vec::new();
    let mut offset = 0;
    loop {
        let page = plan(&state)
            .read_deletions(&id("commit", 1), offset, PageLimit::new(1).unwrap(), || {
                Ok(())
            })
            .unwrap();
        assert_eq!(page.timestamp, Some(42));
        assert_eq!(page.total, 3);
        actual.extend(page.items);
        match page.next_offset {
            Some(next) => offset = next,
            None => break,
        }
    }
    assert_eq!(actual, paths);
    let absent = plan(&state)
        .read_deletions(&id("commit", 2), 0, PageLimit::new(1).unwrap(), || Ok(()))
        .unwrap();
    assert_eq!(absent.timestamp, None);
    assert_eq!(absent.total, 0);
    assert!(absent.items.is_empty());
    assert!(
        plan(&state)
            .read_deletions(
                &id("commit", 1),
                usize::MAX,
                PageLimit::new(1).unwrap(),
                || Ok(())
            )
            .is_err()
    );
}

#[test]
fn graph_cancellation_observes_foreign_rows_and_releases_the_old_state() {
    let state = fixture();
    let retained = Arc::downgrade(&state);
    let captured = plan(&state);
    drop(state);
    let mut checks = 0;
    let error = captured
        .read_page(
            GraphCollection::Commits,
            None,
            PageLimit::new(1).unwrap(),
            || {
                checks += 1;
                if checks == 2 {
                    return Err(Error::new(ErrorCode::ResourceLimitExceeded, "cancelled"));
                }
                Ok(())
            },
        )
        .err()
        .unwrap();
    assert_eq!(error.code(), ErrorCode::ResourceLimitExceeded);
    assert_eq!(
        checks, 2,
        "first candidate is a foreign-history row and must still check cancellation"
    );
    assert!(retained.upgrade().is_none());
}

#[test]
fn graph_successful_projection_does_not_retain_old_state_or_shared_records() {
    let state = fixture();
    let retained = Arc::downgrade(&state);
    let captured = plan(&state);
    drop(state);
    let page = captured
        .read_page(
            GraphCollection::Commits,
            None,
            PageLimit::new(1).unwrap(),
            || Ok(()),
        )
        .unwrap();
    assert!(retained.upgrade().is_none());
    assert_eq!(page_ids(page.items), vec![id("commit", 1)]);
}

#[test]
fn graph_real_capture_keeps_unrelated_history_root_and_reopens_without_index_build() {
    let temporary = tempfile::tempdir().unwrap();
    let system = temporary.path().join("system");
    let mut engine = Engine::initialize(
        &system,
        crate::Config::development([
            temporary.path().join("p"),
            temporary.path().join("b"),
            temporary.path().join("t"),
        ]),
    )
    .unwrap();
    let repository = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Graph").unwrap(),
            "P01",
            3,
            "admin",
            OperationId::generate(),
        )
        .unwrap();
    let first = engine
        .history_graph_read_plan(&repository.history_tree_id)
        .unwrap();
    let first_root = first.root().clone();
    assert!(first.membership.is_some());
    first
        .read_page(
            GraphCollection::Commits,
            None,
            PageLimit::new(1).unwrap(),
            || Ok(()),
        )
        .unwrap();
    engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Other graph").unwrap(),
            "P01",
            3,
            "admin",
            OperationId::generate(),
        )
        .unwrap();
    assert_eq!(
        engine
            .history_graph_read_plan(&repository.history_tree_id)
            .unwrap()
            .root(),
        &first_root
    );
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "file.txt", b"body")
        .unwrap();
    engine
        .commit_workspace(crate::CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "change".into(),
            request_digest: None,
        })
        .unwrap();
    let changed = engine
        .history_graph_read_plan(&repository.history_tree_id)
        .unwrap();
    assert_ne!(changed.root(), &first_root);
    let changed_root = changed.root().clone();
    drop(changed);
    drop(engine);
    let engine = Engine::open(&system).unwrap();
    assert!(engine.available_history_owners().is_none());
    let cold = engine
        .history_graph_read_plan(&repository.history_tree_id)
        .unwrap();
    assert_eq!(cold.root(), &changed_root);
    assert!(cold.membership.is_none());
    let page = cold
        .read_page(
            GraphCollection::Commits,
            None,
            PageLimit::new(128).unwrap(),
            || Ok(()),
        )
        .unwrap();
    assert_eq!(page_ids(page.items).len(), 2);
    assert!(engine.available_history_owners().is_none());
}

#[test]
fn graph_summary_and_worst_escaped_page_fit_the_protocol_byte_bound() {
    let mut state = fixture();
    let source = Arc::make_mut(&mut state)
        .commits
        .get_mut(&id("commit", 1))
        .unwrap();
    // JSON's six-byte escape is an upper bound even for stored text with fewer escapes.
    let inline = "\u{0001}".repeat(storage_contract::client_protocol::MAX_INLINE_TEXT_BYTES);
    source.kind = inline.clone();
    source.tree_id = inline.clone();
    source.author = inline.clone();
    source.message = inline.clone();
    source.provenance = Some(inline);
    let summary = commit_summary(source);
    assert!(json_codec::to_vec(&summary).unwrap().len() < 16 * 1024);
    let page = GraphPage {
        history: history_summary(&state.histories[&id("history", 1)]),
        history_root: "a".repeat(64),
        items: GraphItems::Commits(vec![summary; MAX_GRAPH_RECORDS]),
        next_cursor: Some("x".repeat(32768)),
    };
    assert!(
        json_codec::to_vec(&page).unwrap().len()
            < storage_contract::client_protocol::MAX_HISTORY_PAGE_BYTES
    );
    let deletion = DeletedPath {
        path: "\u{0001}".repeat(768),
        kind: NamespaceEntryKind::Directory,
    };
    assert!(json_codec::to_vec(&deletion).unwrap().len() < 8 * 1024);
    let page = DeletionsPage {
        history_tree_id: id("history", 1),
        history_root: "a".repeat(64),
        commit_id: id("commit", 1),
        timestamp: Some(u64::MAX),
        total: u64::MAX,
        offset: u64::MAX,
        records: vec![deletion; MAX_GRAPH_RECORDS],
        next_offset: None,
    };
    assert!(
        json_codec::to_vec(&page).unwrap().len()
            < storage_contract::client_protocol::MAX_HISTORY_PAGE_BYTES
    );
}
