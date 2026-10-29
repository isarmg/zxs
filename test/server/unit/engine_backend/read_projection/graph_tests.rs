use super::*;
use storage_contract::history_graph::GraphItems;

fn graph_request(
    history: &str,
    collection: &str,
    cursor: Option<&str>,
    limit: usize,
) -> crate::ReadHistoryGraph {
    let mut query = std::collections::BTreeMap::from([
        ("collection".into(), collection.into()),
        ("limit".into(), limit.to_string()),
    ]);
    if let Some(cursor) = cursor {
        query.insert("cursor".into(), cursor.into());
    }
    crate::ReadHistoryGraph::page(history.into(), query).unwrap()
}
fn control() -> crate::GraphReadControl {
    crate::GraphReadControl::until(std::time::Instant::now() + std::time::Duration::from_secs(30))
}

#[test]
fn bounded_graph_pages_preserve_scope_order_and_release_state_before_encoding() {
    let (root, mut engine, repo, _) = fixture();
    save_projection_file(&mut engine, &repo.main_branch_id, "file.txt", b"first");
    save_projection_file(&mut engine, &repo.main_branch_id, "file.txt", b"second");
    let other = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Other").unwrap(),
            "P01",
            3,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let other_head = &engine.state().branches[&other.main_branch_id].head_commit_id;
    let excluded = engine.state().commits.get_shared(other_head).unwrap();
    let expected = engine
        .graph(&repo.history_tree_id)
        .unwrap()
        .commits
        .into_iter()
        .map(|commit| (commit.sequence, commit.commit_id))
        .collect::<Vec<_>>();
    let (backend, auth, _token, identity) = authenticated_backend(engine, root.path());
    let mut seen = Vec::new();
    let mut next = None;
    let mut frozen = None;
    loop {
        let response = backend
            .history_graph(
                &identity,
                graph_request(&repo.history_tree_id, "commits", next.as_deref(), 1),
                &control(),
            )
            .unwrap();
        let crate::HistoryGraphResponse::Graph(page) = &response else {
            panic!("expected graph page")
        };
        let GraphItems::Commits(commits) = &page.items else {
            panic!("expected commits")
        };
        assert!(commits.len() <= 1);
        for commit in commits {
            assert_eq!(commit.history_tree_id, repo.history_tree_id);
            seen.push((commit.sequence, commit.commit_id.clone()));
        }
        assert_eq!(
            Arc::strong_count(&excluded),
            2,
            "bounded DTO must not retain an old State or unrelated commit"
        );
        next = page.next_cursor.clone();
        if frozen.is_none() {
            frozen = Some(response);
        }
        if next.is_none() {
            break;
        }
    }
    assert_eq!(seen, expected);
    let frozen = frozen.unwrap();
    let bytes = json_codec::to_vec(&frozen).unwrap();
    {
        let mut engine = backend.engine.lock().unwrap();
        save_projection_file(&mut engine, &repo.main_branch_id, "later.txt", b"later");
    }
    struct Probe<'a>(&'a EngineBackend, &'a crate::HistoryGraphResponse);
    impl Serialize for Probe<'_> {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let available = std::thread::scope(|scope| {
                scope
                    .spawn(|| self.0.engine.try_lock().is_ok())
                    .join()
                    .unwrap()
            });
            if !available {
                return Err(S::Error::custom(
                    "graph serialization retained engine mutex",
                ));
            }
            self.1.serialize(serializer)
        }
    }
    assert_eq!(
        json_codec::to_vec(&Probe(&backend, &frozen)).unwrap(),
        bytes
    );
    drop(auth);
    drop(backend);
    assert_eq!(json_codec::to_vec(&frozen).unwrap(), bytes);
    assert_eq!(Arc::strong_count(&excluded), 1);
}

#[test]
fn bounded_graph_rechecks_authority_before_missing_history_and_stale_cursor() {
    let (root, mut engine, repo, _) = fixture();
    save_projection_file(&mut engine, &repo.main_branch_id, "file.txt", b"first");
    let (backend, mut auth, token, identity) = authenticated_backend(engine, root.path());
    let missing = backend
        .history_graph(
            &identity,
            graph_request("missing", "commits", None, 1),
            &control(),
        )
        .unwrap_err();
    assert_eq!(missing.code, storage_contract::ErrorCode::NotFound);
    let crate::HistoryGraphResponse::Graph(page) = backend
        .history_graph(
            &identity,
            graph_request(&repo.history_tree_id, "commits", None, 1),
            &control(),
        )
        .unwrap()
    else {
        panic!("expected graph")
    };
    let cursor = page.next_cursor.unwrap();
    {
        let mut engine = backend.engine.lock().unwrap();
        save_projection_file(&mut engine, &repo.main_branch_id, "later.txt", b"later");
    }
    assert_eq!(
        backend
            .history_graph(
                &identity,
                graph_request(&repo.history_tree_id, "commits", Some(&cursor), 1),
                &control()
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::HistoryChanged
    );
    auth.logout(&token).unwrap();
    for history in [&repo.history_tree_id, "missing"] {
        assert_eq!(
            backend
                .history_graph(
                    &identity,
                    graph_request(history, "commits", None, 1),
                    &control()
                )
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::Unauthorized
        );
    }
}

#[test]
fn bounded_graph_text_reads_share_root_and_preserve_current_record_fields() {
    let (root, mut engine, repo, _) = fixture();
    save_projection_file(&mut engine, &repo.main_branch_id, "file.txt", b"first");
    let head = engine.state().branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let expected = engine.state().commits[&head].message.clone();
    let (backend, _auth, _token, identity) = authenticated_backend(engine, root.path());
    let crate::HistoryGraphResponse::Graph(page) = backend
        .history_graph(
            &identity,
            graph_request(&repo.history_tree_id, "commits", None, 128),
            &control(),
        )
        .unwrap()
    else {
        panic!("expected graph")
    };
    let request = crate::ReadHistoryGraph::commit_field(
        repo.history_tree_id.clone(),
        head,
        "message".into(),
        std::collections::BTreeMap::from([("history_root".into(), page.history_root.clone())]),
    )
    .unwrap();
    let crate::HistoryGraphResponse::Text(text) = backend
        .history_graph(&identity, request, &control())
        .unwrap()
    else {
        panic!("expected text")
    };
    assert_eq!(text.history_root, page.history_root);
    assert_eq!(text.text.as_deref(), Some(expected.as_str()));
    assert_eq!(text.total_bytes, expected.len() as u64);
    assert!(text.next_offset.is_none());
}
