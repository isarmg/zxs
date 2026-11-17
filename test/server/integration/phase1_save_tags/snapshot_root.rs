//! Register an actually empty root before its first formal snapshot, without a
//! placeholder file, folder or historical file node.
use super::{Body, Fixture, Request, ServiceExt, StatusCode};
use json_codec::{Value, json};
use storage_contract::OperationId;
use storage_contract::snapshot_worktree::root::RegisteredSnapshotRoot;

async fn post(
    fixture: &Fixture,
    slot: &str,
    authenticated: bool,
    body: Value,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri(format!("/api/file-manager/disks/{slot}/snapshot-root"))
        .header("content-type", "application/json");
    if authenticated {
        request = request.header("authorization", format!("Bearer {}", fixture.token));
    }
    let response = fixture
        .app
        .clone()
        .oneshot(
            request
                .body(Body::from(json_codec::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = http_routing::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    (status, json_codec::from_slice(&bytes).unwrap())
}

async fn listing(fixture: &Fixture, slot: &str) -> Value {
    fixture
        .json(
            "GET",
            &format!("/api/file-manager/disks/{slot}/entries?path="),
            None,
        )
        .await
}

#[tokio::test]
async fn empty_root_registration_http_reaches_snapshot_without_placeholder_files() {
    let fixture = Fixture::new(&["primary", "other"]).await;
    let before = listing(&fixture, "D01").await;
    assert!(before["context"].is_null());
    assert!(before["entries"].as_array().unwrap().is_empty());
    let request = json!({"operation_id": OperationId::generate().unwrap()});
    let (status, value) = post(&fixture, "D01", true, request.clone()).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let registered: RegisteredSnapshotRoot = json_codec::from_value(value.clone()).unwrap();
    assert_eq!(registered.slot_id.as_str(), "D01");
    let after = listing(&fixture, "D01").await;
    assert_eq!(
        after["context"]["repository_id"],
        registered.repository_id.to_string()
    );
    assert!(after["entries"].as_array().unwrap().is_empty());
    let common = fixture.root.path().join("primary/common-data");
    assert_eq!(std::fs::read_dir(&common).unwrap().count(), 0);
    assert_eq!(
        post(&fixture, "D01", true, request.clone()).await,
        (StatusCode::OK, value.clone())
    );

    // A different unused operation may observe this existing root, but the
    // endpoint never invents a creation receipt for that observation.
    let other_request = json!({"operation_id": OperationId::generate().unwrap()});
    assert_eq!(
        post(&fixture, "D01", true, other_request).await,
        (StatusCode::OK, value.clone())
    );
    assert_eq!(
        post(&fixture, "D02", true, request.clone()).await.0,
        StatusCode::CONFLICT
    );
    assert!(listing(&fixture, "D02").await["context"].is_null());

    let context = fixture
        .json(
            "GET",
            &format!(
                "/api/repositories/{}/snapshot-root-context",
                registered.repository_id,
            ),
            None,
        )
        .await;
    let initialize = json!({
        "host": context["host"], "source": context["source"],
        "operation_id": OperationId::generate().unwrap(),
    });
    let snapshot = fixture
        .json("POST", "/api/directory-snapshots", Some(initialize.clone()))
        .await;
    assert!(common.join(".zxs").is_dir());
    assert!(
        listing(&fixture, "D01").await["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let fixture = fixture.reopen().await;
    assert_eq!(
        post(&fixture, "D01", true, request).await,
        (StatusCode::OK, value)
    );
    assert_eq!(
        fixture
            .json("POST", "/api/directory-snapshots", Some(initialize))
            .await,
        snapshot
    );
    assert!(
        listing(&fixture, "D01").await["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(listing(&fixture, "D02").await["context"].is_null());
}

#[tokio::test]
async fn empty_root_registration_rejects_auth_and_shape_before_creating_namespace() {
    let fixture = Fixture::new(&["primary"]).await;
    let request = json!({"operation_id": OperationId::generate().unwrap()});
    assert_eq!(
        post(&fixture, "D01", false, request.clone()).await.0,
        StatusCode::UNAUTHORIZED
    );
    for invalid in [
        json!({}),
        json!([]),
        json!({
            "operation_id": request["operation_id"], "path": "placeholder",
        }),
        json!({"operation_id": "not-an-operation"}),
    ] {
        assert_eq!(
            post(&fixture, "D01", true, invalid).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        post(&fixture, "D99", true, request).await.0,
        StatusCode::NOT_FOUND
    );
    assert!(listing(&fixture, "D01").await["context"].is_null());
    assert_eq!(
        std::fs::read_dir(fixture.root.path().join("primary/common-data"))
            .unwrap()
            .count(),
        0
    );
}

#[tokio::test]
async fn empty_root_registration_does_not_turn_a_backup_into_a_primary() {
    let root = tempfile::tempdir().unwrap();
    let system = root.path().join("system");
    let config = storage_contract::Config::development([
        (
            name_policy::Name::parse("Primary").unwrap(),
            root.path().join("primary"),
        ),
        (
            name_policy::Name::parse("Backup").unwrap(),
            root.path().join("backup"),
        ),
    ])
    .unwrap();
    let engine = storage_engine::Engine::initialize(&system, config).unwrap();
    let auth = storage_server::auth::AuthStore::initialize(
        &system,
        "admin",
        "snapshot-root-test-password",
    )
    .unwrap();
    let app = super::router(engine, auth, &system);
    let login = super::request(
        &app,
        None,
        "POST",
        "/api/auth/login",
        Some(json!({
            "username": "admin", "password": "snapshot-root-test-password",
        })),
    )
    .await;
    let fixture = Fixture {
        root,
        app,
        token: login["token"].as_str().unwrap().into(),
    };
    let (status, error) = post(
        &fixture,
        "B01",
        true,
        json!({"operation_id": OperationId::generate().unwrap()}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{error}");
    assert!(listing(&fixture, "P01").await["context"].is_null());
    for root in ["primary", "backup"] {
        assert_eq!(
            std::fs::read_dir(fixture.root.path().join(root).join("common-data"))
                .unwrap()
                .count(),
            0
        );
    }
}
