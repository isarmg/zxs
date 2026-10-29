//! Projection failure must not turn an authoritative receipt into an HTTP error.
use super::projection_failure::{fail_projection, projection_sequence, receipt_outcome};
use super::*;

async fn sequence(app: &Router, token: &str) -> u64 {
    success(app, token, "GET", "/readyz", None).await["sequence"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

async fn assert_index_unavailable(app: &Router, token: &str) {
    let ready = success(app, token, "GET", "/readyz", None).await;
    assert_eq!(ready["ready"], true);
    assert_eq!(
        ready["history_index"],
        json!({"state":"unavailable","code":"INDEX_REBUILD_FAILED"})
    );
    assert!(!ready.to_string().contains("injected post-publication"));
}

#[tokio::test]
async fn metadata_projection_failure_returns_published_http_receipt_and_same_id_replay() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "original.txt", b"unchanged content").await;
    let request =
        manager_namespace_request(&app, &token, "rename", "original.txt", Some("renamed.txt"))
            .await;
    let operation = request["operation_id"].as_str().unwrap();
    let index = root.path().join("system/index.sqlite");
    let before = sequence(&app, &token).await;
    let projected_before = fail_projection(&index);
    assert_eq!(projected_before, before.to_string());
    let route = "/api/file-manager/disks/P01/namespace-batch";

    let saved = success(&app, &token, "POST", route, Some(request.clone())).await;
    assert!(saved.get("current_health").is_none());
    assert_eq!(saved["operation_id"], operation);
    assert_eq!(saved["commit_state"], "PUBLISHED");
    assert_eq!(saved["publication_sequence"], (before + 1).to_string());
    assert_eq!(saved["protection_at_publish"], "NOT_APPLICABLE");
    assert_eq!(projection_sequence(&index), projected_before);
    assert_index_unavailable(&app, &token).await;
    let queried = success(
        &app,
        &token,
        "GET",
        &format!("/api/operations/{operation}"),
        None,
    )
    .await;
    capture_wire("namespace-batch-observation.json", &queried);
    let outcome = success(
        &app,
        &token,
        "GET",
        &format!("/api/operations/{operation}/outcome"),
        None,
    )
    .await;
    assert!(outcome.get("current_health").is_none());
    assert_eq!(outcome, saved);
    capture_wire("namespace-batch-outcome.json", &outcome);
    assert_eq!(receipt_outcome(&queried), receipt_outcome(&saved));
    let replay = success(&app, &token, "POST", route, Some(request.clone())).await;
    assert!(replay.get("current_health").is_none());
    assert_eq!(receipt_outcome(&replay), receipt_outcome(&saved));
    assert_eq!(sequence(&app, &token).await, before + 1);
    assert_eq!(projection_sequence(&index), projected_before);

    drop(app);
    let recovered = reopen_after_router_drop(&root.path().join("system")).await;
    assert_eq!(recovered.state().sequence, before + 1);
    let operation = operation.parse::<storage_contract::OperationId>().unwrap();
    // The just-published receipt is the newest record of the resident tail.
    let receipt = &recovered
        .state()
        .operations
        .tail_record(operation)
        .unwrap()
        .result;
    assert_eq!(
        receipt.commit_state,
        storage_contract::CommitState::Published
    );
    assert_eq!(receipt.sequence, before + 1);
    assert_eq!(projection_sequence(&index), (before + 1).to_string());
}

#[tokio::test]
async fn native_client_receives_published_content_receipt_when_projection_update_fails() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    let workspace = success(
        &app,
        &token,
        "POST",
        "/api/file-manager/disks/P01/write-sessions",
        Some(json!({"path":"saved.txt"})),
    )
    .await;
    let workspace_id = workspace["workspace_id"].as_str().unwrap();
    upload(
        &app,
        &token,
        workspace_id,
        &new_id("entry"),
        workspace["relative_path"].as_str().unwrap(),
        b"published despite unavailable projection",
    )
    .await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut client =
        storage_client::Client::connect(&format!("http://{}", listener.local_addr().unwrap()))
            .unwrap();
    client.set_token(Some(token.clone()));
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let served = app.clone();
    let server = tokio::spawn(async move {
        http_routing::serve(listener, served)
            .with_graceful_shutdown(async { stopped.await.unwrap() })
            .await
            .unwrap();
    });
    let operation = new_id("op");
    let request = client
        .prepare_commit(
            workspace_id,
            &operation,
            "save with unavailable index",
            None,
        )
        .await
        .unwrap();
    let before = sequence(&app, &token).await;
    let index = root.path().join("system/index.sqlite");
    let projected_before = fail_projection(&index);
    assert_eq!(projected_before, before.to_string());

    let saved = client.commit_prepared(&request).await.unwrap();
    assert!(saved.get("current_health").is_none());
    assert_eq!(saved["operation_id"], operation);
    assert_eq!(saved["commit_state"], "PUBLISHED");
    assert_eq!(saved["operation_kind"], "content_save");
    assert_eq!(
        saved["result"]["publication_sequence"],
        (before + 1).to_string()
    );
    assert_eq!(saved["result"]["outcome"], "COMMITTED_AND_PROTECTED");
    assert_eq!(projection_sequence(&index), projected_before);
    assert_index_unavailable(&app, &token).await;
    let route = format!("/api/operations/{operation}");
    let queried = client.request("GET", &route, None).await.unwrap();
    assert!(queried["current_health"]["checked_at"].is_string());
    assert_eq!(receipt_outcome(&queried), receipt_outcome(&saved));
    let http_query = success(&app, &token, "GET", &route, None).await;
    assert_eq!(receipt_outcome(&http_query), receipt_outcome(&saved));
    let replay = client.commit_prepared(&request).await.unwrap();
    assert!(replay.get("current_health").is_none());
    assert_eq!(receipt_outcome(&replay), receipt_outcome(&saved));
    assert_eq!(sequence(&app, &token).await, before + 1);
    assert_eq!(projection_sequence(&index), projected_before);
    stop.send(()).unwrap();
    server.await.unwrap();
    drop(client);
    drop(app);
    let recovered = reopen_after_router_drop(&root.path().join("system")).await;
    assert_eq!(recovered.state().sequence, before + 1);
    let operation = operation.parse::<storage_contract::OperationId>().unwrap();
    // The just-published receipt is the newest record of the resident tail.
    let receipt = &recovered
        .state()
        .operations
        .tail_record(operation)
        .unwrap()
        .result;
    assert_eq!(
        receipt.commit_state,
        storage_contract::CommitState::Published
    );
    assert_eq!(receipt.sequence, before + 1);
    assert_eq!(projection_sequence(&index), (before + 1).to_string());
}
