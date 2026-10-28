//! Login admission through the actual HTTP router and persistent account store.
use http_routing::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use json_codec::json;
use service_stack::ServiceExt;
use std::{net::SocketAddr, path::Path, sync::Arc};
use storage_contract::Config;
use storage_engine::Engine;
use storage_server::{ServerConfig, auth::AuthStore, engine_backend::EngineBackend};

fn application(root: &Path, proxy: bool) -> Router {
    let engine = Engine::initialize(
        &root.join("system"),
        Config::development([root.join("d1"), root.join("d2"), root.join("d3")]),
    )
    .unwrap();
    let backend = Arc::new(EngineBackend::new(engine));
    let auth = backend
        .protect_auth(
            AuthStore::initialize(&root.join("system"), "admin", "integration-test-password")
                .unwrap(),
        )
        .unwrap();
    let mut config = ServerConfig::loopback(&root.join("system"));
    if proxy {
        config.external_origin = Some("https://storage.example.org".into());
        config.trusted_proxy_ips = vec!["127.0.0.1".parse().unwrap()];
    }
    storage_server::router(backend, auth, config).unwrap()
}

async fn login(app: &Router, forwarded: Option<&str>, password: &str) -> StatusCode {
    login_as(app, forwarded, " Admin ", password).await
}

async fn login_as(
    app: &Router,
    forwarded: Option<&str>,
    username: &str,
    password: &str,
) -> StatusCode {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/api/auth/login")
        .header("content-type", "application/json")
        .extension(ConnectInfo(
            "127.0.0.1:12345".parse::<SocketAddr>().unwrap(),
        ));
    if let Some(ip) = forwarded {
        builder = builder.header("x-forwarded-for", ip);
    }
    let response = app
        .clone()
        .oneshot(
            builder
                .body(Body::from(
                    json!({
                        "username": username, "password": password,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    response.status()
}

#[tokio::test]
async fn trusted_proxy_preserves_source_limits_without_hard_locking_victim_on_other_source() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path(), true);
    for _ in 0..10 {
        assert_eq!(
            login(&app, Some("192.0.2.1"), "wrong-password").await,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        login(&app, Some("192.0.2.1"), "integration-test-password").await,
        StatusCode::TOO_MANY_REQUESTS
    );
    let start = std::time::Instant::now();
    assert_eq!(
        login(&app, Some("192.0.2.2"), "integration-test-password").await,
        StatusCode::OK
    );
    assert!(start.elapsed() >= std::time::Duration::from_millis(250));
    assert_eq!(
        login(&app, None, "integration-test-password").await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        login(
            &app,
            Some("192.0.2.1, 192.0.2.2"),
            "integration-test-password"
        )
        .await,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn default_direct_transport_ignores_spoofed_header_rotation() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path(), false);
    for index in 1..=10 {
        assert_eq!(
            login(&app, Some(&format!("192.0.2.{index}")), "wrong-password").await,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        login(&app, Some("192.0.2.11"), "integration-test-password").await,
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn invalid_username_requests_do_not_consume_valid_source_budget() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path(), true);
    for username in ["x".repeat(1000), "管理员".repeat(100), "é".into()] {
        for _ in 0..11 {
            assert_eq!(
                login_as(&app, Some("192.0.2.1"), &username, "wrong-password").await,
                StatusCode::UNAUTHORIZED
            );
        }
    }
    assert_eq!(
        login(&app, Some("192.0.2.1"), "integration-test-password").await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn alternating_unknown_and_known_names_share_the_same_source_budget() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path(), true);
    for index in 0..10 {
        let username = if index % 2 == 0 { "unknown" } else { " Admin " };
        assert_eq!(
            login_as(&app, Some("192.0.2.1"), username, "wrong-password").await,
            StatusCode::UNAUTHORIZED
        );
    }
    for username in ["unknown", "admin", "another"] {
        assert_eq!(
            login_as(
                &app,
                Some("192.0.2.1"),
                username,
                "integration-test-password"
            )
            .await,
            StatusCode::TOO_MANY_REQUESTS
        );
    }
    // The real password cannot turn an unknown-name challenge into a session.
    assert_eq!(
        login_as(
            &app,
            Some("192.0.2.2"),
            "unknown",
            "integration-test-password"
        )
        .await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        login(&app, Some("192.0.2.2"), "integration-test-password").await,
        StatusCode::OK
    );
}
