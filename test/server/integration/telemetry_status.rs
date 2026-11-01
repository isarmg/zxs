//! Host metrics use real collectors and the real storage backend behind auth.
use http_routing::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use json_codec::{Value, json};
use service_stack::ServiceExt;
use std::sync::Arc;
use storage_contract::Config;
use storage_engine::Engine;
use storage_server::{ServerConfig, auth::AuthStore, engine_backend::EngineBackend};

async fn request(
    app: &Router,
    token: Option<&str>,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(if body.is_some() { "POST" } else { "GET" })
        .uri(path);
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(
            request
                .header("content-type", "application/json")
                .body(
                    body.map(|value| Body::from(json_codec::to_vec(&value).unwrap()))
                        .unwrap_or_else(Body::empty),
                )
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let value = json_codec::from_slice(
        &http_routing::body::to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    (status, value)
}

#[tokio::test]
async fn real_status_samples_host_and_registered_disks_only_for_the_authenticated_account() {
    let root = tempfile::tempdir().unwrap();
    let system = root.path().join("system");
    let config = Config::development([
        root.path().join("d1"),
        root.path().join("d2"),
        root.path().join("d3"),
    ])
    .unwrap();
    let backend = Arc::new(EngineBackend::new(
        Engine::initialize(&system, config).unwrap(),
    ));
    let auth = AuthStore::initialize(&system, "admin", "monitor-test-password").unwrap();
    let auth = backend.protect_auth(auth).unwrap();
    let app = storage_server::router(backend, auth, ServerConfig::new(&system)).unwrap();
    assert_eq!(
        request(&app, None, "/api/system/status", None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&app, None, "/api/system/history?range=1h", None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let (status, rejected) = request(
        &app,
        None,
        "/api/auth/login",
        Some(json!({"username":"reader","password":"monitor-reader-password"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(rejected.get("token").is_none());
    let (_, admin) = request(
        &app,
        None,
        "/api/auth/login",
        Some(json!({"username":"admin","password":"monitor-test-password"})),
    )
    .await;
    let (status, sample) = request(&app, admin["token"].as_str(), "/api/system/status", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        sample["cpu"]["logical_count"]
            .as_u64()
            .is_some_and(|count| count > 0)
    );
    assert!(
        sample["memory"]["total_bytes"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            > 0
    );
    let disks = sample["disks"].as_array().unwrap();
    assert_eq!(disks.len(), 3);
    for disk in disks {
        let total = disk["total_bytes"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap();
        let available = disk["available_bytes"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap();
        assert!(total >= available && total > 0);
        assert_eq!(disk["simulation"], true);
        assert!(disk["power_watts"].is_null());
        assert_eq!(disk["power_unavailable_reason"], "simulated_disk");
        assert!(
            disk["usage_percent"]
                .as_f64()
                .is_some_and(|value| (0.0..=100.0).contains(&value))
        );
        assert_eq!(disk["read_only"], disk["id"] != "P01");
        assert!(disk.get("root").is_none() && disk.get("physical_device").is_none());
    }
    assert!(!sample.to_string().contains(root.path().to_str().unwrap()));
    assert!(sample["cpu"].get("power_watts").is_some());
    assert!(
        sample["cpu"]["power_watts"].is_number()
            || sample["cpu"]["power_unavailable_reason"].is_string()
    );
    tokio::time::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL).await;
    let (status, sample) = request(&app, admin["token"].as_str(), "/api/system/status", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        sample["cpu"]["usage_percent"]
            .as_f64()
            .is_some_and(|value| (0.0..=100.0).contains(&value))
    );
    let token = admin["token"].as_str();
    let (status, before) = request(&app, token, "/api/system/history?range=1min", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!before["samples"].as_array().unwrap().is_empty());
    let last_before = before["samples"].as_array().unwrap().last().unwrap()["collected_at"]
        .as_str()
        .unwrap()
        .to_string();
    let (_, ready_before) = request(&app, None, "/readyz", None).await;
    // No monitoring requests occur during this interval: the sampler must
    // collect real history independently, without a browser remaining open.
    tokio::time::sleep(std::time::Duration::from_millis(5300)).await;
    let (_, ready_after) = request(&app, None, "/readyz", None).await;
    assert_eq!(
        ready_before["sequence"], ready_after["sequence"],
        "monitoring must not publish business history"
    );
    let (_, after) = request(&app, token, "/api/system/history?range=1min", None).await;
    assert!(
        after["samples"]
            .as_array()
            .unwrap()
            .iter()
            .any(|sample| sample["collected_at"].as_str().unwrap() > last_before.as_str())
    );
    assert!(!after.to_string().contains(root.path().to_str().unwrap()));
    for (range, interval) in [
        ("1min", 5),
        ("30min", 5),
        ("1h", 10),
        ("24h", 240),
        ("1month", 7200),
    ] {
        let (status, history) = request(
            &app,
            token,
            &format!("/api/system/history?range={range}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(history["range"], range);
        assert_eq!(history["interval_seconds"], interval);
        assert!(history["samples"].as_array().unwrap().len() <= 361);
    }
    assert_eq!(
        request(&app, token, "/api/system/history?range=1year", None)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert!(system.join("telemetry-cache").is_dir());
    assert!(
        std::fs::read_dir(system.join("telemetry-cache"))
            .unwrap()
            .any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("recent-"))
    );
}
