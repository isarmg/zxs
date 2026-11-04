use super::*;
use http_routing::{body::Body, http::Request as HttpRequest};
use service_stack::ServiceExt;

fn passwords(old: &str, new: &str) -> Result<PasswordChange, ApiError> {
    PasswordInput {
        expected_generation: 1,
        current_password: old.into(),
        new_password: new.into(),
    }
    .try_into()
}

#[test]
fn password_boundary_preserves_exact_bytes_and_rejects_line_injection_or_truncation() {
    let valid = passwords(" old %'密码 ", &"x".repeat(255)).unwrap();
    assert_eq!(valid.current, " old %'密码 ");
    assert_eq!(valid.new.len(), 255);
    for invalid in [
        "".into(),
        "x".repeat(256),
        "密码".repeat(43),
        "new\nsecret".into(),
        "new\rsecret".into(),
        "new\0secret".into(),
    ] {
        assert!(passwords(&invalid, "new").is_err());
        assert!(passwords("old", &invalid).is_err());
    }
}

#[test]
fn account_boundary_rejects_samba_domain_and_option_ambiguity() {
    for name in ["owner", "zxs-owner", "USER.name_1"] {
        assert!(LocalAccount::parse(name).is_some());
    }
    for name in [
        "",
        "SAM/user",
        "DOMAIN\\user",
        "user%secret",
        "user+domain",
        "x=y",
        "bad\nname",
        "汉字",
    ] {
        assert!(LocalAccount::parse(name).is_none(), "{name}");
    }
    assert!(LocalAccount::parse(&"a".repeat(256)).is_none());
}

#[test]
fn client_arguments_pin_loopback_and_never_include_passwords_or_root_flags() {
    let command = command(
        Path::new(CLIENT),
        Path::new("/private/client.conf"),
        &LocalAccount::parse("zxs-owner").unwrap(),
    );
    assert_eq!(command.get_program(), CLIENT);
    let arguments: Vec<_> = command.get_args().map(|v| v.to_str().unwrap()).collect();
    assert_eq!(
        arguments,
        [
            "-s",
            "-D",
            "0",
            "-r",
            "127.0.0.1",
            "-U",
            "zxs-owner",
            "-c",
            "/private/client.conf"
        ]
    );
    assert!(!arguments.contains(&"-a"));
    assert!(CLIENT_CONFIG.contains("client min protocol = SMB3_00"));
    assert!(CLIENT_CONFIG.contains("client smb encrypt = required"));
    assert!(!CLIENT_CONFIG.contains("passdb"));
}

fn script(root: &Path, name: &str, source: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = root.join(name);
    std::fs::write(&path, source).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    path
}

#[test]
fn child_receives_three_stdin_lines_and_success_is_confirmed_only_by_exit() {
    let root = tempfile::tempdir().unwrap();
    let program = script(
        root.path(),
        "client",
        "#!/bin/sh\nIFS= read -r old\nIFS= read -r new\nIFS= read -r confirm\n[ \"$old\" = 'old test secret' ] && [ \"$new\" = 'new test secret' ] && [ \"$confirm\" = \"$new\" ]\n",
    );
    let config = client_config().unwrap();
    assert_eq!(
        std::fs::read_to_string(config.path()).unwrap(),
        CLIENT_CONFIG
    );
    let mut child = start_client(
        &program,
        config.path(),
        &LocalAccount::parse("owner").unwrap(),
        &passwords("old test secret", "new test secret").unwrap(),
    )
    .unwrap();
    finish_client(&mut child, CLIENT_TIMEOUT).unwrap();
    assert!(child.wait().unwrap().success());
}

#[test]
fn child_failure_and_timeout_remain_unknown_and_are_reaped() {
    let root = tempfile::tempdir().unwrap();
    for (name, body, timeout) in [
        (
            "failure",
            "#!/bin/sh\nread a\nread b\nread c\nexit 1\n",
            CLIENT_TIMEOUT,
        ),
        (
            "timeout",
            "#!/bin/sh\nread a\nread b\nread c\nexec /bin/sleep 60\n",
            Duration::ZERO,
        ),
    ] {
        let program = script(root.path(), name, body);
        let config = client_config().unwrap();
        let mut child = start_client(
            &program,
            config.path(),
            &LocalAccount::parse("owner").unwrap(),
            &passwords("old", "new").unwrap(),
        )
        .unwrap();
        let error = finish_client(&mut child, timeout).unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_GATEWAY);
        assert!(!error.retryable);
        assert!(child.try_wait().unwrap().is_some());
        assert!(!format!("{error:?}").contains("old test secret"));
    }
}

#[test]
fn missing_client_is_known_not_to_have_mutated_password() {
    let root = tempfile::tempdir().unwrap();
    let error = start_client(
        &root.path().join("absent"),
        Path::new("/unused"),
        &LocalAccount::parse("owner").unwrap(),
        &passwords("old", "new").unwrap(),
    )
    .unwrap_err();
    assert_eq!(error.status, StatusCode::CONFLICT);
}

async fn application_with_authority(
    smb: bool,
) -> (
    tempfile::TempDir,
    http_routing::Router,
    String,
    Option<std::sync::Arc<crate::smb_account_binding::Authority>>,
) {
    let root = crate::tests::private_tempdir();
    let mut auth =
        crate::auth::AuthStore::initialize(root.path(), "admin", "long-test-password").unwrap();
    let token = auth.login("admin", "long-test-password").unwrap().token;
    let mut config = crate::ServerConfig::new(root.path());
    if smb {
        config.smb = Some(crate::SmbConfig {
            socket: root.path().join("smb.sock"),
            spool_dir: root.path().join("smb-spool"),
            allowed_uid: 0,
            spool_limits: crate::SpoolLimits::default(),
            username: "owner".into(),
            additional_users: vec!["other".into()],
        });
    }
    let config = config.admit().unwrap();
    let spool = crate::http_spool::HttpSpool::open_blocking(&config);
    let runtime = crate::ManagementRuntime::start(
        crate::tests::history_window_test_backend(root.path().join("content")),
        auth,
        &config,
    )
    .unwrap();
    let shutdown = std::sync::Arc::new(crate::service_lifecycle::EmbeddedManagementShutdown::new(
        runtime.backend.clone(),
        runtime.telemetry.clone(),
    ));
    let authority = runtime.smb_account.clone();
    let app = crate::router_shared(
        runtime.backend,
        runtime.auth,
        config,
        runtime.telemetry,
        spool,
        runtime.smb_account,
    )
    .unwrap()
    .layer(Extension(shutdown));
    (root, app, token, authority)
}

async fn application(smb: bool) -> (tempfile::TempDir, http_routing::Router, String) {
    let (root, app, token, _) = application_with_authority(smb).await;
    (root, app, token)
}

#[tokio::test]
async fn account_route_requires_auth_and_exposes_configuration_without_secrets() {
    let (_root, app, token) = application(true).await;
    for (token, status) in [
        (None, StatusCode::UNAUTHORIZED),
        (Some(token.as_str()), StatusCode::OK),
    ] {
        let mut request = HttpRequest::builder().uri("/api/smb/account");
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        if status == StatusCode::OK {
            let body = http_routing::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            let status: SmbAccountStatus = storage_contract::decode_strict(&body).unwrap();
            assert!(
                matches!(status,SmbAccountStatus::Configured { username,.. } if username=="owner")
            );
            assert!(
                !String::from_utf8(body.to_vec())
                    .unwrap()
                    .contains("long-test-password")
            );
        }
    }
}

#[tokio::test]
async fn password_endpoint_rejects_disabled_service_unknown_fields_and_cookie_csrf() {
    let (_root, app, token) = application(false).await;
    let csrf = authentication::derive_csrf_token(&token).unwrap();
    for (body, include_csrf, expected) in [
        (
            json!({"expected_generation":"1","current_password":"old","new_password":"new"}),
            false,
            StatusCode::FORBIDDEN,
        ),
        (
            json!({"expected_generation":"1","current_password":"old","new_password":"new","username":"other"}),
            true,
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"expected_generation":"1","current_password":"old","new_password":"new"}),
            true,
            StatusCode::CONFLICT,
        ),
    ] {
        let mut request = HttpRequest::builder()
            .method("POST")
            .uri("/api/smb/password")
            .header("host", "localhost:7400")
            .header("origin", "http://localhost:7400")
            .header("cookie", format!("zxs_session={token}"));
        if include_csrf {
            request = request.header(authentication::CSRF_HEADER, &csrf);
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
}

async fn json_request(
    app: http_routing::Router,
    token: &str,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri(path)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = http_routing::body::to_bytes(response.into_body(), 16384)
        .await
        .unwrap();
    (status, json_codec::from_slice(&bytes).unwrap())
}
#[tokio::test]
async fn account_save_reauthenticates_and_rejects_unlisted_stale_or_unauthenticated_mutations() {
    let (_root, app, token) = application(true).await;
    for (username, password, expected) in [
        ("outside", "long-test-password", StatusCode::FORBIDDEN),
        ("other", "synthetic-incorrect-secret", StatusCode::FORBIDDEN),
        ("other", "long-test-password", StatusCode::OK),
    ] {
        let (status,body)=json_request(app.clone(),&token,"/api/smb/account",json!({"username":username,"expected_generation":"1","administrator_password":password})).await;
        assert_eq!(status, expected, "{body}");
        if status == StatusCode::OK {
            assert_eq!(body["username"], "other");
            assert_eq!(body["generation"], "2");
            assert_eq!(body["candidates"], json!(["owner", "other"]));
        }
        assert!(!body.to_string().contains(password));
    }
    let (status,_)=json_request(app.clone(),&token,"/api/smb/account",json!({"username":"owner","expected_generation":"1","administrator_password":"long-test-password"})).await;
    assert_eq!(status, StatusCode::CONFLICT);
    // A password form for the previously active account must fail before any
    // Samba client availability check or secret dispatch, including as root.
    let (status, body) = json_request(
        app.clone(),
        &token,
        "/api/smb/password",
        json!({"expected_generation":"1","current_password":"old","new_password":"new"}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "SMB_REBIND_REQUIRED");
    let (status,_)=json_request(app,"invalid","/api/smb/account",json!({"username":"owner","expected_generation":"2","administrator_password":"long-test-password"})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
#[tokio::test]
async fn account_selection_requires_same_origin_and_csrf() {
    let (_root, app, token) = application(true).await;
    let response=app.oneshot(HttpRequest::builder().method("POST").uri("/api/smb/account")
        .header("host","localhost:7400").header("origin","http://localhost:7400")
        .header("cookie",format!("zxs_session={token}"))
        .body(Body::from(json!({"username":"other","expected_generation":"1","administrator_password":"long-test-password"}).to_string())).unwrap()).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
#[test]
fn account_password_proof_cannot_outlive_logout_or_web_password_rotation() {
    let root = crate::tests::private_tempdir();
    let mut auth =
        crate::auth::AuthStore::initialize(root.path(), "admin", "long-test-password").unwrap();
    let token = auth.login("admin", "long-test-password").unwrap().token;
    let identity = auth.authenticate(&token).unwrap();
    let proof = auth
        .login_challenge("admin")
        .unwrap()
        .verify("long-test-password")
        .unwrap();
    auth.logout(&token).unwrap();
    assert!(auth.revalidate_password_proof(&identity, &proof).is_err());
    let token = auth.login("admin", "long-test-password").unwrap().token;
    let identity = auth.authenticate(&token).unwrap();
    let proof = auth
        .login_challenge("admin")
        .unwrap()
        .verify("long-test-password")
        .unwrap();
    auth.change_password(&token, "long-test-password", "replacement-long-password")
        .unwrap();
    assert!(auth.revalidate_password_proof(&identity, &proof).is_err());
}

#[test]
fn password_client_rejects_privilege_changing_executable_modes() {
    for mode in [0o755, 0o700, 0o101] {
        assert!(client_mode_supported(mode));
    }
    for mode in [0o644, 0o600, 0o4755, 0o2755, 0o6755] {
        assert!(!client_mode_supported(mode));
    }
}

async fn read_account_batch(app: &http_routing::Router, token: &str) -> Vec<SmbAccountStatus> {
    let started = Instant::now();
    let mut requests = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let app = app.clone();
        let token = token.to_owned();
        requests.spawn(async move {
            let response = app
                .oneshot(
                    HttpRequest::builder()
                        .uri("/api/smb/account")
                        .header("authorization", format!("Bearer {token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = http_routing::body::to_bytes(response.into_body(), 16384)
                .await
                .unwrap();
            storage_contract::decode_strict::<SmbAccountStatus>(&bytes).unwrap()
        });
    }
    let auth = tokio::spawn(
        app.clone().oneshot(
            HttpRequest::builder()
                .uri("/api/auth/me")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        ),
    );
    let timer_started = Instant::now();
    let timer = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        timer_started.elapsed()
    });
    let mut statuses = Vec::new();
    while let Some(response) = requests.join_next().await {
        statuses.push(response.unwrap());
    }
    assert_eq!(auth.await.unwrap().unwrap().status(), StatusCode::OK);
    assert!(timer.await.unwrap() < Duration::from_millis(500));
    // A timeout future alone cannot detect an async handler that blocks the
    // only Tokio worker inside poll. Verify real elapsed time as well.
    assert!(started.elapsed() < Duration::from_millis(500));
    statuses
}

#[tokio::test(flavor = "current_thread")]
async fn account_get_does_not_wait_for_slow_smb_operation_or_pending_switch() {
    let (_root, app, token, authority) = application_with_authority(true).await;
    let authority = authority.unwrap();
    let (entered, admitted) = tokio::sync::oneshot::channel();
    let (release_operation, finish_operation) = std::sync::mpsc::channel();
    let reader = authority.clone();
    let operation = std::thread::spawn(move || {
        let binding = reader.read().unwrap();
        entered.send(()).unwrap();
        // Bounded native wait makes a broken synchronous GET fail, not hang
        // the entire test runtime and its own async timeout forever.
        let _ = finish_operation.recv_timeout(Duration::from_secs(3));
        drop(binding);
    });
    admitted.await.unwrap();

    let (attempting, attempt_started) = tokio::sync::oneshot::channel();
    let (locked, lock_acquired) = tokio::sync::oneshot::channel();
    let (release_switch, finish_switch) = std::sync::mpsc::channel();
    let (done, saved) = tokio::sync::oneshot::channel();
    let writer = authority.clone();
    let switch = std::thread::spawn(move || {
        attempting.send(()).unwrap();
        let result = writer.select(1, "other", || {
            let _ = locked.send(());
            finish_switch
                .recv_timeout(Duration::from_secs(3))
                .map_err(|_| ApiError::invalid("test switch release deadline exceeded"))
        });
        let _ = done.send(result);
    });
    attempt_started.await.unwrap();
    for status in read_account_batch(&app, &token).await {
        assert!(
            matches!(status, SmbAccountStatus::Configured { username, generation: 1, .. } if username == "owner")
        );
    }
    assert!(
        !switch.is_finished(),
        "the admitted operation must drain before selection"
    );
    release_operation.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), lock_acquired)
        .await
        .unwrap()
        .unwrap();
    operation.join().unwrap();

    // Deterministic writer-held phase: the authorization callback is inside
    // the operation write lock, so the old synchronous status() must block.
    for status in read_account_batch(&app, &token).await {
        assert!(
            matches!(status, SmbAccountStatus::Configured { username, generation: 1, .. } if username == "owner")
        );
    }
    release_switch.send(()).unwrap();
    let selected = tokio::time::timeout(Duration::from_secs(1), saved)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    switch.join().unwrap();
    assert_eq!(selected.generation, 2);
    for status in read_account_batch(&app, &token).await {
        assert!(
            matches!(status, SmbAccountStatus::Configured { username, generation: 2, .. } if username == "other")
        );
    }
}
