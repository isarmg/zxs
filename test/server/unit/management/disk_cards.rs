use super::*;
use http_routing::{body::Body, http::Request};
use service_stack::ServiceExt;
use storage_contract::disk_details::{
    DiskHealth, DiskHealthUnavailable, DiskMapping, DiskNoteRequest,
};

struct Fixture {
    backend: Arc<ManagementBackend>,
    auth: Option<AuthStore>,
    identity: Identity,
    token: String,
    csrf: String,
    disk_id: String,
    disk_root: PathBuf,
    management: PathBuf,
    _root: tempfile::TempDir,
}

fn fixture() -> Fixture {
    fixture_for_mode(storage_contract::Mode::Development)
}

fn fixture_for_mode(mode: storage_contract::Mode) -> Fixture {
    let root = crate::tests::private_tempdir();
    let data = root.path().join("data");
    let disk_root = root.path().join("simulated-disk");
    let management = root.path().join("management");
    let mut config = storage_contract::Config::development([disk_root.clone()]).unwrap();
    config.mode = mode;
    let engine = storage_engine::Engine::initialize(&data, config).unwrap();
    let disk_id = engine.config().disks[0].disk_id.clone();
    drop(
        AuthStore::initialize_management(&management, "admin", "disk-card-test-password").unwrap(),
    );
    let (backend, mut auth) = open_service(&data, &management).unwrap();
    let login = auth.login("admin", "disk-card-test-password").unwrap();
    let identity = auth.authenticate(&login.token).unwrap();
    let engine = Arc::new(EngineBackend::new(engine));
    engine.set_management_auth(auth.reader()).unwrap();
    backend.inner.set(Some(engine), StorageState::ReadOnly);
    Fixture {
        _root: root,
        backend,
        auth: Some(auth),
        identity,
        token: login.token,
        csrf: login.csrf_token,
        disk_id,
        disk_root,
        management,
    }
}

fn command(fixture: &Fixture, expected: &str, note: &str) -> DiskNoteRequest {
    DiskNoteRequest {
        expected_disk_id: fixture.disk_id.clone(),
        expected_note: expected.into(),
        note: note.into(),
    }
}

#[test]
fn development_has_no_hardware_probe_target_but_keeps_registration_and_notes() {
    let fixture = fixture();
    assert_eq!(
        disk_hardware_root(&storage_contract::Mode::Development, &fixture.disk_root),
        None
    );
    assert_eq!(
        disk_hardware_root(&storage_contract::Mode::Production, &fixture.disk_root),
        Some(fixture.disk_root.as_path())
    );
    let before = fs::read(fixture.disk_root.join("disk.json")).unwrap();
    let details = fixture
        .backend
        .disk_details(&fixture.identity, "p01")
        .unwrap();
    assert_eq!(details.slot_id, "P01");
    assert_eq!(details.disk_id, fixture.disk_id);
    assert_eq!(details.root_path, fixture.disk_root.to_str().unwrap());
    assert_eq!(
        details.health,
        DiskHealth::Unavailable {
            reason: DiskHealthUnavailable::MappingUnknown
        }
    );
    assert_eq!(details.mapping, DiskMapping::Unknown);
    assert_eq!(details.mount_path, None);
    assert_eq!(details.device_path, None);
    assert_eq!(details.filesystem_uuid, None);
    assert_eq!(details.model, None);
    assert_eq!(details.serial, None);
    assert_eq!(details.capacity_bytes, None);
    fixture
        .backend
        .update_disk_note(
            &fixture.identity,
            "P01",
            command(&fixture, "", "自由备注\n\t第二行"),
        )
        .unwrap();
    assert_eq!(
        fixture
            .backend
            .disk_details(&fixture.identity, "P01")
            .unwrap()
            .note,
        "自由备注\n\t第二行"
    );
    assert_eq!(
        fs::read(fixture.disk_root.join("disk.json")).unwrap(),
        before
    );
    assert_eq!(
        fixture.backend.management_status().state(),
        StorageState::ReadOnly
    );
}

#[test]
fn wrong_registered_disk_or_unknown_slot_cannot_write_a_note() {
    let fixture = fixture();
    let mut request = command(&fixture, "", "not written");
    request.expected_disk_id = "../../outside".into();
    assert_eq!(
        fixture
            .backend
            .update_disk_note(&fixture.identity, "P01", request)
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::DiskIdentityMismatch
    );
    assert_eq!(
        fixture
            .backend
            .update_disk_note(
                &fixture.identity,
                "missing",
                command(&fixture, "", "not written")
            )
            .unwrap_err()
            .status,
        http::StatusCode::NOT_FOUND
    );
    assert!(!fixture.management.join("disk-notes").exists());
}

#[test]
fn concurrent_note_updates_are_compare_and_set_not_lost_updates() {
    let fixture = fixture();
    let start = Arc::new(std::sync::Barrier::new(3));
    let workers: Vec<_> = ["first", "second"]
        .into_iter()
        .map(|note| {
            let backend = Arc::clone(&fixture.backend);
            let identity = fixture.identity.clone();
            let request = command(&fixture, "", note);
            let start = Arc::clone(&start);
            std::thread::spawn(move || {
                start.wait();
                backend.update_disk_note(&identity, "P01", request)
            })
        })
        .collect();
    start.wait();
    let results: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| result
                .as_ref()
                .is_err_and(|error| error.status == http::StatusCode::CONFLICT))
            .count(),
        1
    );
}

#[test]
fn queued_note_save_revalidates_revoked_session_before_publication() {
    let mut fixture = fixture();
    let lock = crate::lock(&fixture.backend.inner.disk_notes);
    let backend = Arc::clone(&fixture.backend);
    let identity = fixture.identity.clone();
    let request = command(&fixture, "", "must not persist");
    let start = Arc::new(std::sync::Barrier::new(2));
    let ready = Arc::clone(&start);
    let worker = std::thread::spawn(move || {
        ready.wait();
        backend.update_disk_note(&identity, "P01", request)
    });
    start.wait();
    fixture
        .auth
        .as_mut()
        .unwrap()
        .logout(&fixture.token)
        .unwrap();
    drop(lock);
    assert_eq!(
        worker.join().unwrap().unwrap_err().status,
        http::StatusCode::UNAUTHORIZED
    );
    assert!(!fixture.management.join("disk-notes").exists());
}

async fn http_request(
    app: &http_routing::Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: String,
) -> (http::StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = http_routing::body::to_bytes(response.into_body(), 2 * crate::JSON_BODY_LIMIT)
        .await
        .unwrap();
    (status, json_codec::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn disk_card_http_roundtrip_preserves_cas_identity_authentication_and_body_boundaries() {
    let mut fixture = fixture();
    let app = crate::router(
        fixture.backend.clone(),
        fixture.auth.take().unwrap(),
        crate::ServerConfig::new(&fixture.management),
    )
    .unwrap();
    // Await the real startup recovery worker instead of racing admission or sleeping.
    let recovery = crate::lock(&fixture.backend.inner.recovery_worker).take();
    if let Some(recovery) = recovery {
        tokio::task::spawn_blocking(move || recovery.join().unwrap())
            .await
            .unwrap();
    }
    let path = "/api/admin/disks/P01/details";
    let note_path = "/api/admin/disks/P01/note";
    assert_eq!(
        http_request(&app, "GET", path, None, String::new()).await.0,
        http::StatusCode::UNAUTHORIZED
    );
    let (status, details) =
        http_request(&app, "GET", path, Some(&fixture.token), String::new()).await;
    assert_eq!(status, http::StatusCode::OK, "{details}");
    assert_eq!(details["disk_id"], fixture.disk_id);
    assert_eq!(details["mapping"], "unknown");
    assert_eq!(
        details["health"],
        json!({"state":"unavailable","reason":"mapping_unknown"})
    );
    let body = json_codec::to_string(&command(&fixture, "", "remark\n<text>")).unwrap();
    let (status, saved) =
        http_request(&app, "PUT", note_path, Some(&fixture.token), body.clone()).await;
    assert_eq!(status, http::StatusCode::OK, "{saved}");
    assert_eq!(
        saved,
        json!({"slot_id":"P01", "disk_id":fixture.disk_id, "note":"remark\n<text>"})
    );
    assert_eq!(
        http_request(&app, "PUT", note_path, Some(&fixture.token), body)
            .await
            .0,
        http::StatusCode::OK
    );
    let conflict = json_codec::to_string(&command(&fixture, "", "stale")).unwrap();
    assert_eq!(
        http_request(&app, "PUT", note_path, Some(&fixture.token), conflict)
            .await
            .0,
        http::StatusCode::CONFLICT
    );
    for body in [
        "null".to_owned(),
        "[]".to_owned(),
        format!(
            r#"{{"expected_disk_id":"{}","expected_note":"","note":"a","note":"b"}}"#,
            fixture.disk_id
        ),
    ] {
        assert_eq!(
            http_request(&app, "PUT", note_path, Some(&fixture.token), body)
                .await
                .0,
            http::StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        http_request(
            &app,
            "PUT",
            note_path,
            Some(&fixture.token),
            " ".repeat(crate::JSON_BODY_LIMIT + 1)
        )
        .await
        .0,
        http::StatusCode::PAYLOAD_TOO_LARGE
    );
    let cookie_request = |csrf: Option<&str>| {
        let mut request = Request::builder()
            .method("PUT")
            .uri(note_path)
            .header("content-type", "application/json")
            .header("cookie", format!("zxs_session={}", fixture.token))
            .header("host", "127.0.0.1:7400")
            .header("origin", "http://127.0.0.1:7400");
        if let Some(csrf) = csrf {
            request = request.header("x-csrf-token", csrf);
        }
        request
            .body(Body::from(
                json_codec::to_string(&command(&fixture, "remark\n<text>", "")).unwrap(),
            ))
            .unwrap()
    };
    assert_eq!(
        app.clone()
            .oneshot(cookie_request(None))
            .await
            .unwrap()
            .status(),
        http::StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.clone()
            .oneshot(cookie_request(Some(&fixture.csrf)))
            .await
            .unwrap()
            .status(),
        http::StatusCode::OK
    );
    let (_, cleared) = http_request(&app, "GET", path, Some(&fixture.token), String::new()).await;
    assert_eq!(cleared["note"], "");
    fixture.backend.stop_management();
}

#[test]
fn unavailable_production_registration_never_observes_replacement_filesystem_or_blocks_notes() {
    use std::os::unix::fs::PermissionsExt;
    for replace_directory in [false, true] {
        let fixture = fixture_for_mode(storage_contract::Mode::Production);
        let seal = fs::read(fixture.disk_root.join("disk.json")).unwrap();
        if replace_directory {
            fs::rename(
                &fixture.disk_root,
                fixture._root.path().join("retired-disk"),
            )
            .unwrap();
            fs::create_dir(&fixture.disk_root).unwrap();
            fs::set_permissions(&fixture.disk_root, fs::Permissions::from_mode(0o700)).unwrap();
            // Even copying the old seal cannot bless this different directory inode.
            fs::write(fixture.disk_root.join("disk.json"), &seal).unwrap();
        } else {
            fs::remove_file(fixture.disk_root.join("disk.json")).unwrap();
        }
        let details = fixture
            .backend
            .disk_details(&fixture.identity, "P01")
            .unwrap();
        assert_eq!(
            details.health,
            DiskHealth::Unavailable {
                reason: DiskHealthUnavailable::MappingUnknown
            }
        );
        assert_eq!(details.mapping, DiskMapping::Unknown);
        assert_eq!(details.device_path, None);
        assert_eq!(details.mount_path, None);
        assert_eq!(details.filesystem_uuid, None);
        assert_eq!(details.capacity_bytes, None);
        assert_eq!(details.root_path, fixture.disk_root.to_str().unwrap());
        fixture
            .backend
            .update_disk_note(
                &fixture.identity,
                "P01",
                command(&fixture, "", "盘已离线\n等待检修"),
            )
            .unwrap();
        assert_eq!(
            fixture
                .backend
                .disk_details(&fixture.identity, "P01")
                .unwrap()
                .note,
            "盘已离线\n等待检修"
        );
    }
}

#[test]
fn note_survives_management_and_engine_reopen_without_entering_the_data_seal() {
    let fixture = fixture();
    fixture
        .backend
        .update_disk_note(
            &fixture.identity,
            "P01",
            command(&fixture, "", "重启后保留\nlocal system note"),
        )
        .unwrap();
    let Fixture {
        _root,
        backend,
        auth,
        token,
        disk_id,
        disk_root,
        management,
        ..
    } = fixture;
    let seal = fs::read(disk_root.join("disk.json")).unwrap();
    drop(backend);
    drop(auth);
    let data = _root.path().join("data");
    let (backend, auth) = open_service(&data, &management).unwrap();
    let engine = Arc::new(EngineBackend::new(
        storage_engine::Engine::open(&data).unwrap(),
    ));
    engine.set_management_auth(auth.reader()).unwrap();
    backend.inner.set(Some(engine), StorageState::Ready);
    let identity = auth.authenticate(&token).unwrap();
    let card = backend.disk_details(&identity, "P01").unwrap();
    assert_eq!(card.disk_id, disk_id);
    assert_eq!(card.note, "重启后保留\nlocal system note");
    assert_eq!(fs::read(disk_root.join("disk.json")).unwrap(), seal);
}

#[test]
fn stopped_production_directory_keeps_unknown_card_without_reopening_target() {
    let fixture = fixture_for_mode(storage_contract::Mode::Production);
    let backend = fixture.backend.inner.backend().unwrap();
    let mut stop = backend
        .with_disk_registration(&fixture.identity, "P01", |_, engine| {
            engine
                .inject_disk_quiesce("P01")
                .map_err(ApiError::invalid_source)
        })
        .unwrap();
    stop.release_control();
    assert!(stop.is_drained().unwrap());
    fs::rename(
        &fixture.disk_root,
        fixture._root.path().join("stopped-disk"),
    )
    .unwrap();
    let details = fixture
        .backend
        .disk_details(&fixture.identity, "P01")
        .unwrap();
    assert_eq!(details.disk_id, fixture.disk_id);
    assert_eq!(details.root_path, fixture.disk_root.to_str().unwrap());
    assert_eq!(details.mapping, DiskMapping::Unknown);
    assert_eq!(
        details.health,
        DiskHealth::Unavailable {
            reason: DiskHealthUnavailable::MappingUnknown,
        }
    );
    assert_eq!(details.capacity_bytes, None);
    assert_eq!(details.device_path, None);
    assert_eq!(details.mount_path, None);
    assert!(stop.is_drained().unwrap());
}
