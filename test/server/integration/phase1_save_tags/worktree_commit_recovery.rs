//! The HTTP owner gate must admit recovery without opening the frozen view.
use super::{
    CommitWorktree, EditWorktree, Fixture, Http, OperationId, SnapshotHandle,
    WorktreeCommitOutcome, WorktreeCommitReceipt, cold, commit_route, content_route,
    initialized, instance, resume,
};

#[tokio::test]
async fn exact_authenticated_commit_reaches_its_existing_tail_through_tcp_after_restart() {
    let fixture = Fixture::new(&["primary"]).await;
    fixture.save("D01", "original.txt", b"immutable original").await;
    let entry = fixture.entry("D01", "original.txt").await;
    let history = fixture.history(entry["file_id"].as_str().unwrap()).await;
    let repository = history["identity"]["context"]["repository_id"].as_str().unwrap();
    let http = Http::start(fixture.app.clone(), &fixture.token).await;
    let (context, source) = initialized(&http, repository).await;
    let target = instance(&http, repository, &context, source, "worktree-main-001").await;
    let bytes = b"one independently sealed edit";
    let edit = EditWorktree {
        target, expected: http.status(target).await.revision,
        path: name_policy::PortablePath::parse("original.txt").unwrap(),
        size: bytes.len() as u64,
        content_hash: storage_contract::ContentHash::from_digest(blake3::hash(bytes)),
        operation_id: OperationId::generate().unwrap(),
    };
    let (status, revision) = http.edit(&edit, bytes).await;
    assert_eq!(status, 200, "{revision}");
    let request = CommitWorktree {
        target, expected: json_codec::from_value(revision).unwrap(),
        message: "exact commit after durable node".into(),
        operation_id: OperationId::generate().unwrap(),
    };
    let route = commit_route(target);
    let receipt_route = format!("/api/repositories/{repository}/directory-worktrees/commits/{}",
        request.operation_id);
    let headers = [("content-type", "application/json".into())];
    let body = json_codec::to_vec(&request).unwrap();
    http.close().await;
    let (root, mut engine, token) = cold(fixture).await;
    engine.inject_fault_once("after_worktree_node_publication");
    let fixture = resume(root, engine, token);
    let http = Http::start(fixture.app.clone(), &fixture.token).await;
    assert_ne!(http.raw(true, "POST", &route, &headers, &body).await.0, 200);
    let pending: WorktreeCommitReceipt =
        json_codec::from_value(http.json("GET", &receipt_route, None).await).unwrap();
    assert_eq!(pending.request, request);
    assert_eq!(pending.result, WorktreeCommitOutcome::Pending {});
    http.close().await;

    let (root, engine, token) = cold(fixture).await;
    let versions = engine.state().versions.clone();
    let operations = engine.state().operations.clone();
    let fixture = resume(root, engine, token);
    let http = Http::start(fixture.app.clone(), &fixture.token).await;
    assert_eq!(http.raw(false, "POST", &route, &headers, &body).await.0, 401);
    let read = format!("{}?publication={}&path=original.txt",
        content_route(target), target.publication);
    assert_eq!(http.raw(true, "GET", &read, &[], &[]).await.0, 503);
    let mut changed = request.clone();
    changed.message.push_str(" not the original");
    assert_eq!(http.raw(true, "POST", &route, &headers, &json_codec::to_vec(&changed).unwrap())
        .await.0, 409);
    changed.operation_id = OperationId::generate().unwrap();
    assert_eq!(http.raw(true, "POST", &route, &headers, &json_codec::to_vec(&changed).unwrap())
        .await.0, 503);
    // The original's FILE and node are already permanent. Only the missing
    // ledger/SYSTEM completion may be published; the receipt alone is not success.
    let node: SnapshotHandle = json_codec::from_value(http.json("POST", &route,
        Some(json_codec::to_value(&request).unwrap())).await).unwrap();
    let complete: WorktreeCommitReceipt =
        json_codec::from_value(http.json("GET", &receipt_route, None).await).unwrap();
    assert_eq!(complete.request, request);
    assert_eq!(complete.result, WorktreeCommitOutcome::Complete { snapshot: node });
    assert_eq!(http.read(target, "original.txt").await.as_slice(), bytes);
    http.close().await;
    let (_root, engine, _token) = cold(fixture).await;
    assert_eq!(engine.state().versions, versions);
    assert_eq!(engine.state().operations.len(), operations.len() + 1);
    for (id, original) in operations.iter() {
        assert_eq!(engine.state().operations.get(id), Some(original));
    }
}
