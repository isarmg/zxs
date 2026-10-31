use super::*;

async fn response(app: &Router, method: &str, path: &str, token: Option<&str>) -> Response {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}
async fn json(response: Response) -> Value {
    json_codec::from_slice(
        &http_routing::body::to_bytes(response.into_body(), 8192)
            .await
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn unavailable_http_spool_isolates_only_upload_staging() {
    for failure in [
        "occupied",
        "occupied_parent",
        "locked",
        "fifo_lock",
        "invalid_inventory",
    ] {
        let root = crate::tests::private_tempdir();
        let mut config = ServerConfig::loopback(root.path());
        let _owner = match failure {
            "occupied" => {
                std::fs::write(&config.spool_dir, b"keep this evidence").unwrap();
                None
            }
            "occupied_parent" => {
                let parent = root.path().join("not-a-directory");
                std::fs::write(&parent, b"keep this evidence").unwrap();
                config.spool_dir = parent.join("api-spool");
                None
            }
            "locked" => {
                config.prepare_spool().unwrap();
                Some(
                    spool_budget::SpoolBudget::recover(
                        &config.spool_dir,
                        config.http_spool_limits.clone(),
                    )
                    .unwrap(),
                )
            }
            "fifo_lock" => {
                config.prepare_spool().unwrap();
                rustix::fs::mkfifoat(
                    rustix::fs::CWD,
                    config.spool_dir.join(".budget.lock"),
                    rustix::fs::Mode::from_raw_mode(0o600),
                )
                .unwrap();
                None
            }
            _ => {
                std::fs::create_dir_all(config.spool_dir.join("unexpected-directory")).unwrap();
                None
            }
        };
        let app = application_config(root.path(), config.clone());
        assert_eq!(
            response(&app, "GET", "/healthz", None).await.status(),
            StatusCode::OK,
            "{failure}"
        );
        let status = json(response(&app, "GET", "/api/management/status", None).await).await;
        assert_eq!(status["authentication"], "ready");
        assert_eq!(status["auxiliary"]["http_uploads"], "unavailable");
        assert_eq!(status["auxiliary"]["web_assets"], "not_configured");
        assert!(!status.to_string().contains(root.path().to_str().unwrap()));
        let (token, _, cookie) = authenticated(&app).await;
        for path in [
            "/api/auth/me",
            "/api/system/storage",
            "/api/system/status",
            "/api/capabilities",
        ] {
            assert_eq!(
                response(&app, "GET", path, Some(&token)).await.status(),
                StatusCode::OK,
                "{failure}: {path}"
            );
        }
        // Content streaming does not depend on HTTP upload staging.
        assert_eq!(
            response(&app, "GET", "/api/versions/version/content", Some(&token))
                .await
                .status(),
            StatusCode::OK
        );
        for (method, path) in [
            ("PUT", "/api/workspaces/ws/files/item/content?path=file"),
            ("GET", "/api/workspaces/ws/files/item/upload-status"),
        ] {
            assert_eq!(
                response(&app, method, path, None).await.status(),
                StatusCode::UNAUTHORIZED
            );
            let unavailable = response(&app, method, path, Some(&token)).await;
            assert_eq!(unavailable.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(
                json(unavailable).await["error"]["code"],
                "HTTP_UPLOADS_UNAVAILABLE"
            );
        }
        // Disabling staging is not an authentication or CSRF fallback.
        let csrf = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/workspaces/ws/files/item/content?path=file")
                    .header("cookie", &cookie)
                    .header("host", "127.0.0.1:7400")
                    .header("origin", "http://127.0.0.1:7400")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(csrf.status(), StatusCode::FORBIDDEN);
        if failure == "occupied" {
            assert_eq!(
                std::fs::read(&config.spool_dir).unwrap(),
                b"keep this evidence"
            );
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn rejected_spool_symlink_does_not_touch_its_target() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    for suffix in ["plain", "slash", "dot"] {
        let root = crate::tests::private_tempdir();
        let target = root.path().join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut config = ServerConfig::loopback(root.path());
        symlink(&target, &config.spool_dir).unwrap();
        config.spool_dir = match suffix {
            "slash" => PathBuf::from(format!("{}/", config.spool_dir.display())),
            "dot" => config.spool_dir.join("."),
            _ => config.spool_dir,
        };
        let app = application_config(root.path(), config);
        assert_eq!(
            response(&app, "GET", "/healthz", None).await.status(),
            StatusCode::OK
        );
        assert_eq!(
            json(response(&app, "GET", "/api/management/status", None).await).await["auxiliary"]["http_uploads"],
            "unavailable"
        );
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert!(!target.join(".budget.lock").exists());
    }
}

#[tokio::test]
async fn missing_or_rejected_web_assets_leave_management_apis_but_not_a_fake_ui() {
    for failure in ["missing", "empty", "ordinary_file", "oversized"] {
        let root = crate::tests::private_tempdir();
        let assets = root.path().join("web");
        if failure == "ordinary_file" {
            std::fs::write(&assets, "not a directory").unwrap();
        } else if failure != "missing" {
            std::fs::create_dir(&assets).unwrap();
            if failure == "oversized" {
                let file = std::fs::File::create(assets.join("index.html")).unwrap();
                file.set_len(64 * 1024 * 1024 + 1).unwrap();
            }
        }
        let mut config = ServerConfig::loopback(root.path());
        config.web_dir = Some(assets);
        let app = application_config(root.path(), config);
        assert_assets_unavailable(&app).await;
        let (token, _, _) = authenticated(&app).await;
        assert_eq!(
            response(&app, "GET", "/api/auth/me", Some(&token))
                .await
                .status(),
            StatusCode::OK
        );
    }
}

async fn assert_assets_unavailable(app: &Router) {
    assert_eq!(
        response(app, "GET", "/healthz", None).await.status(),
        StatusCode::OK
    );
    let status = json(response(app, "GET", "/api/management/status", None).await).await;
    assert_eq!(status["auxiliary"]["web_assets"], "unavailable");
    for path in ["/", "/index.html", "/pkg/app.wasm"] {
        let unavailable = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header(header::IF_NONE_MATCH, "*")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unavailable.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(unavailable.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(unavailable.headers()["x-content-type-options"], "nosniff");
        assert!(unavailable.headers().get(header::ETAG).is_none());
        assert_eq!(
            json(unavailable).await["error"]["code"],
            "WEB_ASSETS_UNAVAILABLE"
        );
    }
    for path in ["/api", "/api/unknown"] {
        let missing = response(app, "GET", path, None).await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        assert_eq!(json(missing).await["error"]["code"], "NOT_FOUND");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn rejected_asset_links_and_special_files_never_enable_the_static_file_service() {
    use std::os::unix::fs::symlink;
    for failure in [
        "root_link",
        "root_link_slash",
        "root_link_dot",
        "parent_link",
        "file_link",
        "directory_link",
        "compressed_link",
        "special_file",
    ] {
        let root = crate::tests::private_tempdir();
        let assets = root.path().join("web");
        let target = root.path().join("outside");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("index.html"), "private target").unwrap();
        if failure.starts_with("root_link") || failure == "parent_link" {
            symlink(&target, &assets).unwrap();
            if failure == "parent_link" {
                std::fs::create_dir(target.join("nested")).unwrap();
                std::fs::write(target.join("nested/index.html"), "private nested target").unwrap();
            }
        } else {
            std::fs::create_dir(&assets).unwrap();
            std::fs::write(assets.join("index.html"), "public fixture").unwrap();
            match failure {
                "file_link" => {
                    symlink(target.join("index.html"), assets.join("secret.txt")).unwrap()
                }
                "directory_link" => symlink(&target, assets.join("pkg")).unwrap(),
                "compressed_link" => {
                    symlink(target.join("index.html"), assets.join("index.html.gz")).unwrap()
                }
                _ => rustix::fs::mkfifoat(
                    rustix::fs::CWD,
                    assets.join("pipe"),
                    rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
                )
                .unwrap(),
            }
        }
        let mut config = ServerConfig::loopback(root.path());
        config.web_dir = Some(match failure {
            "root_link_slash" => PathBuf::from(format!("{}/", assets.display())),
            "root_link_dot" => assets.join("."),
            "parent_link" => assets.join("nested"),
            _ => assets,
        });
        assert_assets_unavailable(&application_config(root.path(), config)).await;
        assert_eq!(
            std::fs::read(target.join("index.html")).unwrap(),
            b"private target"
        );
    }
}

#[test]
fn invalid_server_security_and_admission_configuration_still_fails_closed() {
    for invalid in [
        "origin",
        "proxy",
        "spool_limits",
        "transfer_limits",
        "upload_limit",
    ] {
        let root = crate::tests::private_tempdir();
        let mut config = ServerConfig::loopback(root.path());
        config.web_dir = Some(root.path().join("missing-web"));
        std::fs::write(&config.spool_dir, "unavailable").unwrap();
        match invalid {
            "origin" => config.external_origin = Some("http://example.invalid".into()),
            "proxy" => config.trusted_proxy_ips.push("192.0.2.1".parse().unwrap()),
            "spool_limits" => config.http_spool_limits.bytes_total = 0,
            "transfer_limits" => config.http_transfer_limits.slots = 0,
            "upload_limit" => config.max_upload_bytes = 0,
            _ => unreachable!("unknown invalid configuration fixture"),
        }
        let auth = AuthStore::initialize(root.path(), "admin", "a-long-test-password").unwrap();
        let result = router(
            Arc::new(TestBackend {
                path: root.path().join("content"),
            }),
            auth,
            config,
        );
        assert!(result.is_err(), "{invalid}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn combined_auxiliary_failures_preserve_management_and_rejected_cache_target() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let root = crate::tests::private_tempdir();
    let target = root.path().join("external-cache-target");
    std::fs::create_dir(&target).unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o750)).unwrap();
    let sentinel = target.join("sentinel");
    std::fs::write(&sentinel, b"preserve external evidence").unwrap();
    std::fs::set_permissions(&sentinel, std::fs::Permissions::from_mode(0o640)).unwrap();
    let cache = root.path().join("telemetry-cache");
    symlink(&target, &cache).unwrap();
    let mut config = ServerConfig::loopback(root.path());
    config.web_dir = Some(root.path().join("missing-web"));
    std::fs::write(&config.spool_dir, b"preserve spool evidence").unwrap();
    let app = application_config(root.path(), config.clone());

    // Optional failures compose: no fake UI or unbudgeted upload fallback,
    // while management, authentication and in-memory telemetry stay usable.
    assert_assets_unavailable(&app).await;
    let status = json(response(&app, "GET", "/api/management/status", None).await).await;
    assert_eq!(status["authentication"], "ready");
    assert_eq!(status["auxiliary"]["http_uploads"], "unavailable");
    let (token, _, _) = authenticated(&app).await;
    for path in [
        "/api/auth/me",
        "/api/system/storage",
        "/api/system/status",
        "/api/system/history?range=1min",
        "/api/capabilities",
        "/api/versions/version/content",
    ] {
        let reply = response(&app, "GET", path, Some(&token)).await;
        assert_eq!(reply.status(), StatusCode::OK, "{path}");
        http_routing::body::to_bytes(reply.into_body(), 1024 * 1024)
            .await
            .unwrap();
    }
    for (method, path) in [
        ("PUT", "/api/workspaces/ws/files/item/content?path=file"),
        ("GET", "/api/workspaces/ws/files/item/upload-status"),
    ] {
        assert_eq!(
            response(&app, method, path, None).await.status(),
            StatusCode::UNAUTHORIZED
        );
        let reply = response(&app, method, path, Some(&token)).await;
        assert_eq!(reply.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            json(reply).await["error"]["code"],
            "HTTP_UPLOADS_UNAVAILABLE"
        );
    }
    drop(app);
    assert_eq!(std::fs::read_link(&cache).unwrap(), target);
    assert_eq!(
        std::fs::read(&sentinel).unwrap(),
        b"preserve external evidence"
    );
    assert_eq!(
        std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o750
    );
    assert_eq!(
        std::fs::metadata(&sentinel).unwrap().permissions().mode() & 0o777,
        0o640
    );
    assert_eq!(std::fs::read_dir(&target).unwrap().count(), 1);
    assert_eq!(
        std::fs::read(&config.spool_dir).unwrap(),
        b"preserve spool evidence"
    );
    assert!(!config.web_dir.unwrap().exists());
}
