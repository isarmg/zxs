use super::super::foreign_disk_fixture::*;
use super::*;
use storage_contract::storage_directories::{DirectorySource, DiscoveredDiskState};
use storage_engine::disk_authority::{
    Digest, FileWrite, Identity as DiskIdentity, IndependentDisk, Origin, Role, Token,
};

fn independent(f: &Fixture) -> PathBuf {
    let path = f.source.parent().unwrap().join("independent");
    let token = |value| Token::new(value).unwrap();
    let disk = IndependentDisk::initialize(
        &path,
        DiskIdentity {
            origin: Origin {
                pool_id: token("origin_pool"),
                disk_id: token("origin_disk"),
                slot_id: token("origin_slot"),
                binding_generation: 1,
            },
            incarnation: Digest::body(b"independent"),
            role: Role::Primary,
        },
    )
    .unwrap();
    let review = disk
        .review_file(FileWrite {
            operation: storage_contract::OperationId::generate().unwrap(),
            expected_base: None,
            history: &token("history"),
            file: &token("file"),
            bytes: b"retained content",
        })
        .unwrap();
    disk.execute(&review).unwrap();
    path
}
fn observed(f: &Fixture, path: &Path) -> DirectorySource {
    let candidate = f
        .backend
        .discovered_disks(&f.identity)
        .unwrap()
        .candidates
        .into_iter()
        .find(|candidate| Path::new(&candidate.path) == path)
        .unwrap();
    let DiscoveredDiskState::Unsupported {
        source: Some(source),
        ..
    } = candidate.state
    else {
        panic!("independent format must never be offered as empty or managed-format admission")
    };
    source
}
fn preview(f: &Fixture, source: DirectorySource) -> Result<IndependentDiskPreview, ApiError> {
    let output = f.backend.setup_operation(
        "independent_disk_preview",
        json_codec::to_value(IndependentDiskPreviewRequest { source }).unwrap(),
        None,
        Some(&f.identity),
    )?;
    Ok(json_codec::from_value(output).unwrap())
}

#[test]
fn authenticated_independent_preview_is_read_only_even_without_body_bytes() {
    let f = fixture();
    let path = independent(&f);
    for entry in fs::read_dir(path.join("bodies")).unwrap() {
        fs::remove_file(entry.unwrap().path()).unwrap();
    }
    let source = observed(&f, &path);
    let before = bytes(&path);
    let known = bytes(&f.known);
    let output = preview(&f, source.clone()).unwrap();
    assert_eq!(output.source, source);
    assert_eq!(output.topology.nodes().len(), 1);
    assert_eq!(output.topology.root().sequence, 1);
    assert!(output.topology.relations().is_empty());
    assert_eq!(bytes(&path), before);
    assert_eq!(bytes(&f.known), known);
}

#[test]
fn independent_preview_keeps_scope_identity_lifecycle_and_revocation_boundaries() {
    let f = fixture();
    let path = independent(&f);
    let source = observed(&f, &path);
    let nested = path.join("objects");
    for forbidden in [f.known.as_path(), nested.as_path(), f._root.path()] {
        let mut altered = source.clone();
        altered.path = forbidden.to_string_lossy().into_owned();
        assert!(preview(&f, altered).is_err());
    }
    let mut altered = source.clone();
    altered.identity.inode += 1;
    assert!(preview(&f, altered).is_err());
    let gate = crate::lock(&f.backend.inner.storage_addition);
    assert_eq!(
        preview(&f, source.clone()).unwrap_err().code,
        storage_contract::ErrorCode::StorageRetryRunning
    );
    drop(gate);
    let auth = crate::lock(&f.backend.inner.auth).clone().unwrap();
    crate::lock(&auth).logout(&f.token).unwrap();
    assert_eq!(
        preview(&f, source).unwrap_err().status,
        http::StatusCode::UNAUTHORIZED
    );
}

#[test]
fn independent_preview_rejects_wrong_format_corruption_and_execution_fields() {
    let f = fixture();
    assert!(preview(&f, source(&f)).is_err());
    let path = independent(&f);
    let source = observed(&f, &path);
    let mut request = json_codec::to_value(IndependentDiskPreviewRequest {
        source: source.clone(),
    })
    .unwrap();
    request["execute"] = true.into();
    assert!(
        f.backend
            .setup_operation("independent_disk_preview", request, None, Some(&f.identity))
            .is_err()
    );
    let metadata = fs::read_dir(path.join("objects"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::write(metadata, b"corrupt metadata").unwrap();
    let before = bytes(&path);
    let error = preview(&f, source).unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::RecoveryRequired);
    assert!(matches!(
        error.source.as_deref(),
        Some(crate::http_boundary::ApiErrorSource::IndependentSource(_))
    ));
    assert_eq!(error.message, "independent disk source inspection failed");
    assert_eq!(bytes(&path), before);
}

fn http_app(f: &Fixture) -> http_routing::Router {
    let config = crate::ServerConfig::new(&f.backend.inner.management_root);
    let spool = crate::http_spool::HttpSpool::open_blocking(&config);
    let telemetry =
        crate::telemetry::Telemetry::start(f.backend.clone(), &f._root.path().join("telemetry"))
            .unwrap();
    let auth = crate::lock(&f.backend.inner.auth).clone().unwrap();
    let reader = crate::lock(&auth).reader();
    crate::router_shared(
        Arc::new(crate::AuthorizedBackend {
            backend: f.backend.clone(),
            auth: reader,
        }),
        auth,
        config.admit().unwrap(),
        telemetry,
        spool,
        None,
    )
    .unwrap()
}

async fn http_preview(
    app: &http_routing::Router,
    path: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> (http::StatusCode, Value) {
    use http_routing::{body::Body, http::Request};
    use service_stack::ServiceExt;
    let mut request = Request::builder()
        .method("POST")
        .uri(path)
        .header("host", "127.0.0.1:7400")
        .header("origin", "http://127.0.0.1:7400")
        .header("content-type", "application/json");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = http_routing::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    (status, json_codec::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn actual_http_independent_preview_keeps_source_unchanged_and_auth_boundaries() {
    use http::StatusCode;
    const ENDPOINT: &str = "/api/admin/storage/independent/preview";
    let f = fixture();
    let path = independent(&f);
    for entry in fs::read_dir(path.join("bodies")).unwrap() {
        fs::remove_file(entry.unwrap().path()).unwrap();
    }
    let source = observed(&f, &path);
    let app = http_app(&f);
    let before = bytes(&path);
    let known = bytes(&f.known);
    let input = json_codec::to_vec(&IndependentDiskPreviewRequest {
        source: source.clone(),
    })
    .unwrap();
    let bearer = format!("Bearer {}", f.token);
    let authorized = [("authorization", bearer.as_str())];
    let cookie = format!("zxs_session={}", f.token);
    let csrf = authentication::derive_csrf_token(&f.token).unwrap();
    assert_eq!(
        http_preview(&app, ENDPOINT, &[], b"not json".to_vec())
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        http_preview(&app, ENDPOINT, &[("cookie", &cookie)], input.clone())
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let (status, value) = http_preview(
        &app,
        ENDPOINT,
        &[("cookie", &cookie), ("x-csrf-token", &csrf)],
        input.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let output: IndependentDiskPreview = json_codec::from_value(value).unwrap();
    assert_eq!(output.source, source);
    assert_eq!(output.topology.root().sequence, 1);
    assert_eq!(output.topology.nodes().len(), 1);
    let (status, value) = http_preview(&app, ENDPOINT, &authorized, input.clone()).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(
        http_preview(
            &app,
            &format!("{ENDPOINT}?execute=true"),
            &authorized,
            input.clone(),
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let mut unknown: Value = json_codec::from_slice(&input).unwrap();
    unknown["execute"] = true.into();
    assert_eq!(
        http_preview(
            &app,
            ENDPOINT,
            &authorized,
            json_codec::to_vec(&unknown).unwrap(),
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        http_preview(&app, ENDPOINT, &authorized, vec![b' '; 64 * 1024 + 1])
            .await
            .0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let auth = crate::lock(&f.backend.inner.auth).clone().unwrap();
    crate::lock(&auth).logout(&f.token).unwrap();
    assert_eq!(
        http_preview(&app, ENDPOINT, &authorized, input).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(bytes(&path), before);
    assert_eq!(bytes(&f.known), known);
}

#[tokio::test]
async fn actual_http_independent_preview_rejects_replaced_source_without_writes() {
    let f = fixture();
    let path = independent(&f);
    let source = observed(&f, &path);
    let app = http_app(&f);
    let original = bytes(&path);
    let moved = path.with_extension("moved");
    fs::rename(&path, &moved).unwrap();
    let replacement = independent(&f);
    let before = bytes(&replacement);
    let bearer = format!("Bearer {}", f.token);
    let (status, error) = http_preview(
        &app,
        "/api/admin/storage/independent/preview",
        &[("authorization", &bearer)],
        json_codec::to_vec(&IndependentDiskPreviewRequest { source }).unwrap(),
    )
    .await;
    assert_eq!(status, http::StatusCode::CONFLICT, "{error}");
    assert_eq!(error["error"]["code"], "DISK_CHANGED", "{error}");
    assert_eq!(bytes(&moved), original);
    assert_eq!(bytes(&replacement), before);
}
