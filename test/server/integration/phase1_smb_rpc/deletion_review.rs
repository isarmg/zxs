use super::*;
use storage_contract::file_deletion_review::{BranchPage, ParentPage, Summary};

fn oversized_seed() -> (tempfile::TempDir, Engine, Repository, Entry, Entry) {
    let (root, mut engine, repo, a, b) = seed();
    let branch = engine
        .state()
        .branches
        .values()
        .find(|branch| {
            branch.file_id.as_deref() == Some(a.file_id.as_str()) && branch.name == "review"
        })
        .unwrap()
        .branch_id
        .clone();
    let sources = historical_sources(&mut engine, &a.file_id, &branch);
    let expected = engine.file_branch_head(&branch).unwrap();
    let inputs = sources
        .iter()
        .map(|node| storage_contract::FileHistoryMergeInput {
            branch_id: branch.clone(),
            source_node_id: node.clone(),
            expected_node_id: expected.clone(),
        })
        .collect::<Vec<_>>();
    // Seed already owns the shared branch and one review branch. Each real
    // historical merge derives one branch per input, retaining frozen aliases.
    // 40 * 100 + 95 gives 4095 more branches without thousands of independent
    // database installations exhausting their unchanged retention budget.
    for batch in 0..41 {
        let count = if batch == 40 { 95 } else { 100 };
        let receipt = engine
            .merge_file_history(storage_engine::FileHistoryMergeRequest {
                file_id: &a.file_id,
                inputs: &inputs[..count],
                chosen_node_id: &sources[0],
                name: &storage_contract::BranchName::parse(&format!("review-{batch}")).unwrap(),
                location: None,
                principal: "alice",
                operation_id: OperationId::generate().unwrap(),
            })
            .unwrap();
        assert_eq!(receipt.result["directory_branch_ids"], json!([]));
        assert_eq!(engine.file_branch_head(&branch).unwrap(), expected);
    }
    assert_eq!(
        engine
            .namespace_tree(&repo.main_branch_id)
            .unwrap()
            .get("A.txt"),
        Some(&a)
    );
    (root, engine, repo, a, b)
}

fn historical_sources(engine: &mut Engine, file: &str, branch: &str) -> Vec<String> {
    let mut sources = Vec::new();
    // The review limit needs distinct real history sources. Container labels
    // cannot manufacture nodes, so each source is a genuine content revision.
    for index in 0..100 {
        let node = write_history_content(engine, branch, "A.txt", format!("source {index}").as_bytes());
        assert_eq!(engine.state().versions[&node].file_id, file);
        sources.push(node);
    }
    sources
}

fn delete(
    engine: &mut Engine,
    repo: &Repository,
    entry: &Entry,
    operation: OperationId,
) {
    engine
        .soft_delete_file(storage_engine::DeleteFileRequest {
            target: storage_engine::FileDeletionTarget {
                repository_id: &HistoryRepositoryId::parse(&repo.repository_id).unwrap(),
                file_id: &HistoryFileId::parse(&entry.file_id).unwrap(),
                path: &name_policy::LookupPath::parse(entry.path.as_str()).unwrap(),
            },
            principal: "alice",
            operation_id: operation,
        })
        .unwrap();
}

async fn login(app: &http_routing::Router) -> String {
    let (status, login) = request(
        app,
        None,
        "POST",
        "/api/auth/login",
        json!({"username": "alice", "password": "phase-one-smb-password"}),
    )
    .await;
    assert_eq!(status, http::StatusCode::OK, "{login}");
    login["token"].as_str().unwrap().into()
}

async fn all_branches(
    app: &http_routing::Router,
    token: &str,
    base: &str,
    node: &storage_contract::FileDeletionNode,
) {
    let mut branches = std::collections::BTreeMap::new();
    let mut after = None;
    loop {
        let mut url = format!("{base}/branches?deletion_node_id={}", node.node_id);
        if let Some(cursor) = after {
            url.push_str(&format!("&after={cursor}"));
        }
        let (status, page) = request(app, Some(token), "GET", &url, json!({})).await;
        assert_eq!(status, http::StatusCode::OK, "{page}");
        let page: BranchPage = json_codec::from_value(page).unwrap();
        assert_eq!(page.file_id.to_string(), node.file_id);
        assert_eq!(page.deletion_node_id.to_string(), node.node_id);
        assert!(page.entries.len() <= 100);
        for entry in page.entries {
            assert!(
                branches
                    .insert(entry.branch_id.to_string(), entry.node_id.to_string())
                    .is_none()
            );
        }
        after = page.next_after;
        if after.is_none() {
            break;
        }
    }
    assert_eq!(branches, node.branch_heads);
    let last = node.branch_heads.last_key_value().unwrap().0;
    let url = format!("{base}/branches?deletion_node_id={}&after={last}", node.node_id);
    let (status, page) = request(app, Some(token), "GET", &url, json!({})).await;
    assert_eq!(status, http::StatusCode::OK, "{page}");
    assert_eq!(page.get("next_after"), Some(&Value::Null));
    let page: BranchPage = json_codec::from_value(page).unwrap();
    assert_eq!(page.file_id.to_string(), node.file_id);
    assert_eq!(page.deletion_node_id.to_string(), node.node_id);
    assert!(page.entries.is_empty());
    assert!(page.next_after.is_none());
}

async fn all_parents(
    app: &http_routing::Router,
    token: &str,
    base: &str,
    node: &storage_contract::FileDeletionNode,
) {
    let mut parents = Vec::new();
    let mut offset = 0;
    loop {
        let url = format!(
            "{base}/parents?deletion_node_id={}&offset={offset}",
            node.node_id,
        );
        let (status, page) = request(app, Some(token), "GET", &url, json!({})).await;
        assert_eq!(status, http::StatusCode::OK, "{page}");
        let page: ParentPage = json_codec::from_value(page).unwrap();
        assert_eq!(page.offset, offset);
        assert_eq!(page.file_id.to_string(), node.file_id);
        assert_eq!(page.deletion_node_id.to_string(), node.node_id);
        assert!(page.entries.len() <= 100);
        parents.extend(page.entries.into_iter().map(|id| id.to_string()));
        match page.next_offset {
            Some(next) => offset = next,
            None => break,
        }
    }
    assert_eq!(parents, node.parent_node_ids);
    let end = node.parent_node_ids.len();
    let url = format!("{base}/parents?deletion_node_id={}&offset={end}", node.node_id);
    let (status, page) = request(app, Some(token), "GET", &url, json!({})).await;
    assert_eq!(status, http::StatusCode::OK, "{page}");
    assert_eq!(page.get("next_offset"), Some(&Value::Null));
    let page: ParentPage = json_codec::from_value(page).unwrap();
    assert_eq!(page.file_id.to_string(), node.file_id);
    assert_eq!(page.deletion_node_id.to_string(), node.node_id);
    assert_eq!(page.offset, end as u64);
    assert!(page.entries.is_empty());
    assert!(page.next_offset.is_none());
    let url = format!(
        "{base}/parents?deletion_node_id={}&offset={}",
        node.node_id,
        end + 1,
    );
    let (status, error) = request(app, Some(token), "GET", &url, json!({})).await;
    assert_eq!(status, http::StatusCode::BAD_REQUEST);
    assert_eq!(error["error"]["code"], "INVALID_REQUEST");
}

async fn bad_queries(
    app: &http_routing::Router,
    token: &str,
    base: &str,
    node: &storage_contract::FileDeletionNode,
) {
    let branch = node.branch_heads.first_key_value().unwrap().0;
    for suffix in [
        "&after=not-a-branch".to_owned(),
        format!("&after={branch}&after={branch}"),
    ] {
        let url = format!("{base}/branches?deletion_node_id={}{}", node.node_id, suffix);
        let (status, error) = request(app, Some(token), "GET", &url, json!({})).await;
        assert_eq!(status, http::StatusCode::BAD_REQUEST);
        assert_eq!(error["error"]["code"], "INVALID_REQUEST");
    }
    for suffix in [
        "&offset=18446744073709551615",
        "&offset=-1",
        "&offset=0&offset=1",
        "&offset=01",
        "&unexpected=true",
    ] {
        let url = format!("{base}/parents?deletion_node_id={}{}", node.node_id, suffix);
        assert_eq!(
            request(app, Some(token), "GET", &url, json!({})).await.0,
            http::StatusCode::BAD_REQUEST
        );
    }
}

#[test]
fn oversized_deletion_is_paged_and_undo_retains_the_complete_durable_receipt() {
    let (root, mut engine, repo, a, b) = oversized_seed();
    let deletion_operation = OperationId::generate().unwrap();
    delete(&mut engine, &repo, &a, deletion_operation);
    let node = engine.file_deletion(&a.file_id).unwrap().clone();
    assert_eq!(node.branch_heads.len(), 4097);
    assert!(node.parent_node_ids.len() > 100);
    let undo_operation = OperationId::generate().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let compact = runtime.block_on(async {
        let backend = Arc::new(EngineBackend::new(engine));
        let auth = backend
            .protect_auth(
                AuthStore::initialize(
                    &root.path().join("state"),
                    "alice",
                    "phase-one-smb-password",
                )
                .unwrap(),
            )
            .unwrap();
        let identity = auth.sole_identity().unwrap();
        let app = storage_server::router(
            backend.clone(),
            auth,
            storage_server::ServerConfig::new(&root.path().join("state")),
        )
        .unwrap();
        let token = login(&app).await;
        let token = token.as_str();
        let base = format!("/api/files/{}/deletion", a.file_id);
        let summary_url = format!("{base}/summary?deletion_node_id={}", node.node_id);
        assert_eq!(
            request(&app, None, "GET", &summary_url, json!({})).await.0,
            http::StatusCode::UNAUTHORIZED
        );
        for collection in ["branches", "parents"] {
            let url = format!("{base}/{collection}?deletion_node_id={}", node.node_id);
            assert_eq!(
                request(&app, None, "GET", &url, json!({})).await.0,
                http::StatusCode::UNAUTHORIZED
            );
        }
        let (status, oversized) = request(&app, Some(token), "GET", &base, json!({})).await;
        assert_eq!(status, http::StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(oversized["error"]["code"], "RESOURCE_LIMIT_EXCEEDED");
        let (status, summary) =
            request(&app, Some(token), "GET", &summary_url, json!({})).await;
        assert_eq!(status, http::StatusCode::OK, "{summary}");
        let summary: Summary = json_codec::from_value(summary).unwrap();
        assert_eq!(summary.branch_count, node.branch_heads.len() as u64);
        assert_eq!(summary.parent_count, node.parent_node_ids.len() as u64);
        all_branches(&app, token, &base, &node).await;
        all_parents(&app, token, &base, &node).await;
        bad_queries(&app, token, &base, &node).await;
        let mut smb = identity.clone();
        smb.smb_binding = Some(storage_server::auth::SmbBinding {
            repository_id: repo.repository_id.clone(),
            branch_id: repo.main_branch_id.clone(),
            generation: repo.smb_binding_generation,
        });
        assert_eq!(
            backend
                .file_deletion_summary(
                    &smb,
                    a.file_id.parse().unwrap(),
                    node.node_id.parse().unwrap(),
                )
                .unwrap_err()
                .code,
            ErrorCode::Forbidden
        );
        assert_eq!(
            backend
                .file_deletion_branches(
                    &smb,
                    a.file_id.parse().unwrap(),
                    node.node_id.parse().unwrap(),
                    None,
                )
                .unwrap_err()
                .code,
            ErrorCode::Forbidden
        );
        assert_eq!(
            backend
                .file_deletion_parents(
                    &smb,
                    a.file_id.parse().unwrap(),
                    node.node_id.parse().unwrap(),
                    0,
                )
                .unwrap_err()
                .code,
            ErrorCode::Forbidden
        );
        let body = json!({"deletion_node_id": node.node_id, "operation_id": undo_operation});
        let undo_url = format!("{base}/undo-summary");
        let first = request(&app, Some(token), "POST", &undo_url, body.clone()).await;
        assert_eq!(first.0, http::StatusCode::OK, "{}", first.1);
        assert!(first.1["result"].get("removed_deletion_node").is_none());
        assert_eq!(
            first.1["result"]["removed_deletion_summary"],
            json_codec::to_value(summary).unwrap()
        );
        let stored = backend.operation_outcome(&identity, undo_operation).unwrap();
        assert_eq!(
            stored.result["removed_deletion_node"],
            json_codec::to_value(&node).unwrap()
        );
        assert_eq!(first.1["request_digest"], stored.request_digest);
        let outcome_url = format!("/api/file-deletion-undos/{undo_operation}");
        assert_eq!(
            request(&app, None, "GET", &outcome_url, json!({})).await.0,
            http::StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(&app, Some(token), "GET", &outcome_url, json!({})).await,
            first
        );
        assert_eq!(
            request(&app, Some(token), "POST", &undo_url, body).await,
            first
        );
        assert_eq!(
            request(&app, Some(token), "GET", &summary_url, json!({})).await.0,
            http::StatusCode::NOT_FOUND
        );
        let wrong_kind = format!("/api/file-deletion-undos/{deletion_operation}");
        assert_eq!(
            request(&app, Some(token), "GET", &wrong_kind, json!({})).await.0,
            http::StatusCode::BAD_REQUEST
        );
        first.1
    });
    runtime.shutdown_timeout(Duration::from_secs(2));
    let mut engine = Engine::open(&root.path().join("state")).unwrap();
    assert!(engine.file_deletion(&a.file_id).is_none());
    for entry in [&a, &b] {
        delete(&mut engine, &repo, entry, OperationId::generate().unwrap());
    }
    let replacement = engine.file_deletion(&a.file_id).unwrap().node_id.clone();
    assert_ne!(replacement, node.node_id);
    let other_deletion = engine.file_deletion(&b.file_id).unwrap().node_id.clone();
    let foreign_operation = OperationId::generate().unwrap();
    engine
        .undo_file_deletion(storage_engine::UndoFileDeletionRequest {
            file_id: &HistoryFileId::parse(&b.file_id).unwrap(),
            deletion_node_id: &other_deletion,
            principal: "bob",
            operation_id: foreign_operation,
        })
        .unwrap();
    let backend = Arc::new(EngineBackend::new(engine));
    let auth = backend
        .protect_auth(AuthStore::open(&root.path().join("state")).unwrap())
        .unwrap();
    let identity = auth.sole_identity().unwrap();
    let full = backend.operation_outcome(&identity, undo_operation).unwrap();
    assert_eq!(
        full.result["removed_deletion_node"],
        json_codec::to_value(&node).unwrap()
    );
    assert_eq!(compact["request_digest"], full.request_digest);
    assert_eq!(compact["operation_id"], json_codec::to_value(undo_operation).unwrap());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let app = storage_server::router(
            backend.clone(),
            auth,
            storage_server::ServerConfig::new(&root.path().join("state")),
        )
        .unwrap();
        let token = login(&app).await;
        let token = token.as_str();
        let outcome = format!("/api/file-deletion-undos/{undo_operation}");
        assert_eq!(
            request(&app, Some(token), "GET", &outcome, json!({})).await,
            (http::StatusCode::OK, compact)
        );
        let foreign = format!("/api/file-deletion-undos/{foreign_operation}");
        assert_eq!(
            request(&app, Some(token), "GET", &foreign, json!({})).await.0,
            http::StatusCode::FORBIDDEN
        );
        for collection in ["summary", "branches", "parents"] {
            let stale = format!(
                "/api/files/{}/deletion/{collection}?deletion_node_id={}",
                a.file_id, node.node_id,
            );
            let (status, error) = request(&app, Some(token), "GET", &stale, json!({})).await;
            assert_eq!(status, http::StatusCode::NOT_FOUND);
            assert_eq!(error["error"]["code"], "FILE_NODE_NOT_FOUND");
        }
        let rejected = request(
            &app,
            Some(token),
            "POST",
            &format!("/api/files/{}/deletion/undo-summary", a.file_id),
            json!({
                "deletion_node_id": node.node_id,
                "operation_id": OperationId::generate().unwrap(),
            }),
        )
        .await;
        assert_eq!(rejected.0, http::StatusCode::NOT_FOUND);
        let current = backend
            .file_deletion_summary(
                &identity,
                a.file_id.parse().unwrap(),
                replacement.parse().unwrap(),
            )
            .unwrap();
        assert_eq!(current.node_id.to_string(), replacement);
    });
    runtime.shutdown_timeout(Duration::from_secs(2));
}
