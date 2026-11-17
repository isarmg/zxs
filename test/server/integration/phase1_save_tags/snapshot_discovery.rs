//! The browser discovers a real registered branch through the authenticated router.
//! These fixtures use independent temporary roots, not an HTTP backend mock.
use super::{Body, Fixture, Request, ServiceExt, StatusCode};
use json_codec::{Value, json};
use storage_contract::snapshot_worktree::discovery::{
    DirectorySnapshotHead, SnapshotHeadDiscovery,
};
use storage_contract::{OperationId, RepositoryId};

async fn get(fixture: &Fixture, route: &str, authenticated: bool) -> (StatusCode, Value) {
    let mut request = Request::builder().uri(route);
    if authenticated {
        request = request.header("authorization", format!("Bearer {}", fixture.token));
    }
    let response = fixture.app.clone().oneshot(request.body(Body::empty()).unwrap())
        .await.unwrap();
    let status = response.status();
    let bytes = http_routing::body::to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    (status, json_codec::from_slice(&bytes).unwrap())
}

async fn discover(fixture: &Fixture, repository: RepositoryId) -> DirectorySnapshotHead {
    let route = format!("/api/repositories/{repository}/snapshot-head");
    let (status, value) = get(fixture, &route, true).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let result: DirectorySnapshotHead = json_codec::from_value(value).unwrap();
    assert_eq!(result.repository_id, repository);
    assert!(result.context.source.root.is_none());
    result
}

#[tokio::test]
async fn snapshot_head_http_is_authenticated_scoped_read_only_and_cold() {
    let fixture = Fixture::new(&["primary", "other"]).await;
    fixture.save("D01", "first.txt", b"real first body").await;
    fixture.save("D02", "other.txt", b"different root body").await;
    let first = fixture.entry("D01", "first.txt").await;
    let other = fixture.entry("D02", "other.txt").await;
    let repository = |history: Value| -> RepositoryId {
        history["identity"]["context"]["repository_id"].as_str().unwrap().parse().unwrap()
    };
    let first_repository = repository(fixture.history(first["file_id"].as_str().unwrap()).await);
    let other_repository = repository(fixture.history(other["file_id"].as_str().unwrap()).await);
    let route = format!("/api/repositories/{first_repository}/snapshot-head");
    assert_eq!(get(&fixture, &route, false).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(get(&fixture, "/api/repositories/not-an-id/snapshot-head", true).await.0,
        StatusCode::BAD_REQUEST);
    let absent = discover(&fixture, first_repository).await;
    assert_eq!(absent.head, SnapshotHeadDiscovery::NotInitialized);
    assert_eq!(discover(&fixture, first_repository).await, absent);
    assert!(!fixture.root.path().join("primary/common-data/.zxs").exists());
    assert!(!fixture.root.path().join("other/common-data/.zxs").exists());

    let request = json!({
        "host": absent.context.host,
        "source": absent.context.source,
        "operation_id": OperationId::generate().unwrap(),
    });
    let initialized = fixture.json("POST", "/api/directory-snapshots", Some(request.clone())).await;
    let initialized: storage_contract::snapshot_worktree::SnapshotHandle =
        json_codec::from_value(initialized).unwrap();
    let observed = discover(&fixture, first_repository).await;
    let SnapshotHeadDiscovery::Head { snapshot, branch_name } = &observed.head else {
        panic!("the real completed initial branch must be discoverable");
    };
    assert_eq!(*snapshot, initialized);
    assert_eq!(branch_name.as_str(), "main");
    assert_eq!(discover(&fixture, other_repository).await.head,
        SnapshotHeadDiscovery::NotInitialized);
    assert_eq!(fixture.body(first["version_id"].as_str().unwrap()).await, b"real first body");

    let fixture = fixture.reopen().await;
    assert_eq!(discover(&fixture, first_repository).await, observed);
    assert_eq!(discover(&fixture, other_repository).await.head,
        SnapshotHeadDiscovery::NotInitialized);
    let retry = fixture.json("POST", "/api/directory-snapshots", Some(request)).await;
    assert_eq!(json_codec::from_value::<storage_contract::snapshot_worktree::SnapshotHandle>(retry)
        .unwrap(), initialized);
    assert_eq!(discover(&fixture, first_repository).await, observed);
    assert!(!fixture.root.path().join("other/common-data/.zxs").exists());
}
