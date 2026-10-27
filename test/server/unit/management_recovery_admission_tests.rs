//! Revoke only after offline open, before a new recovery action reaches the core.
use super::*;
use storage_engine::{CommitRequest, Config, Engine};

const PASSWORD: &str = "recovery-admission-password";
const NEW_PASSWORD: &str = "rotated-recovery-password";
const LOST_PAYLOAD: &[u8] = b"permanently lost recovery fixture bytes";

fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut result = BTreeMap::new();
    if !root.exists() {
        return result;
    }
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            result.extend(files(&path));
        } else {
            result.insert(path.clone(), fs::read(path).unwrap());
        }
    }
    result
}

fn evidence(data: &Path, disks: &BTreeMap<String, PathBuf>) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut result = files(&data.join("recovery-plans"));
    for disk in disks.values() {
        // Include all journal frames and immutable proofs, not just the current
        // state projection: a rejected request must not append recovery records.
        result.extend(files(disk));
    }
    result
}

struct Fixture {
    _root: tempfile::TempDir,
    backend: Arc<ManagementBackend>,
    auth: Arc<Mutex<AuthStore>>,
    input: Value,
}

impl Fixture {
    fn new(action: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("system");
        let management = root.path().join("management");
        let disks = BTreeMap::from([
            ("one".into(), root.path().join("one")),
            ("two".into(), root.path().join("two")),
            ("three".into(), root.path().join("three")),
        ]);
        let auth = AuthStore::initialize_management(&management, "admin", PASSWORD).unwrap();
        let mut engine =
            Engine::initialize(&data, Config::development(disks.values().cloned())).unwrap();
        engine
            .persist_auth_state(
                auth.snapshot().unwrap(),
                "system_auth",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
        let repo = engine
            .create_repository(
                &storage_contract::RepositoryName::parse("Recovery admission").unwrap(),
                "P01",
                3,
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        let staged = engine
            .stage_bytes(&workspace.workspace_id, "lost.txt", LOST_PAYLOAD)
            .unwrap();
        let primary = engine
            .config()
            .disks
            .iter()
            .find(|disk| disk.slot_id == repo.primary_slot_id)
            .unwrap();
        assert!(
            staged.source_path.starts_with(
                primary
                    .root
                    .join("staging/workspaces")
                    .join(&workspace.workspace_id)
            )
        );
        let operation = storage_contract::OperationId::generate();
        engine.inject_fault_once("after_accept_2");
        assert!(
            engine
                .commit_workspace(CommitRequest {
                    workspace_id: workspace.workspace_id,
                    operation_id: operation,
                    principal: "admin".into(),
                    expected_head: workspace.base_head,
                    fencing_token: workspace.fencing_token,
                    message: "candidate awaiting recovery".into(),
                    request_digest: None,
                })
                .is_err()
        );
        for disk in disks.values() {
            // Resolve the one immutable version descriptor on this member,
            // separately from the primary disk's private workspace source.
            let history = disk.join("histories").join(&repo.history_tree_id);
            let versions = fs::read_dir(history.join("versions"))
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect::<Vec<_>>();
            assert_eq!(versions.len(), 1);
            let version: storage_contract::FileVersion =
                json_codec::from_slice(&fs::read(versions[0].join("version.json")).unwrap())
                    .unwrap();
            assert_eq!(version.creation_name, "lost.txt");
            let payload = history
                .join("file-containers")
                .join(version.file_id)
                .join(version.creation_name)
                .join("versions")
                .join(version.storage_name);
            assert_ne!(payload, staged.source_path);
            assert_eq!(fs::read(&payload).unwrap(), LOST_PAYLOAD);
            fs::remove_file(payload).unwrap();
        }
        // stage_bytes stores its independent source on the primary disk too.
        // It must be the only surviving full copy, not a fourth replica.
        let remaining = files(root.path())
            .into_iter()
            .filter_map(|(path, bytes)| (bytes == LOST_PAYLOAD).then_some(path))
            .collect::<Vec<_>>();
        assert_eq!(
            remaining.as_slice(),
            std::slice::from_ref(&staged.source_path)
        );
        fs::remove_file(staged.source_path).unwrap();
        assert!(
            files(root.path())
                .values()
                .all(|bytes| bytes != LOST_PAYLOAD)
        );
        drop(engine);
        drop(auth);
        let input = if action == "recovery_commit" {
            let mut engine = Engine::open_for_recovery(&data).unwrap();
            let plan = engine
                .prepare_degraded_recovery(operation, "admin", "all original bytes are lost")
                .unwrap();
            json!({"recovery_id":plan.recovery_id,"digest":plan.digest,"accept_permanent_loss":true})
        } else {
            json!({"operation_id":operation,"reason":"all original bytes are lost"})
        };
        let (backend, auth) = open_service_with_disks(&data, &management, disks, true).unwrap();
        let auth = Arc::new(Mutex::new(auth));
        // No background recovery races with the deliberate offline fixture.
        *backend.inner.auth.lock().unwrap() = Some(auth.clone());
        Self {
            _root: root,
            backend,
            auth,
            input,
        }
    }
}

#[test]
fn published_recovery_reconciliation_keeps_known_publication_in_management_errors() {
    use storage_engine::{Error, RecoveryReconciliationStage};
    for fail_index in [true, false] {
        let fixture = Fixture::new("recovery_commit");
        let identity = {
            let mut auth = fixture.auth.lock().unwrap();
            let token = auth.login("admin", PASSWORD).unwrap().token;
            auth.authenticate(&token).unwrap()
        };
        let data = fixture.backend.inner.data_root.clone();
        let recovery_id = fixture.input["recovery_id"].as_str().unwrap();
        let plan_path = data
            .join("recovery-plans")
            .join(format!("{recovery_id}.json"));
        let plan_before = fs::read(&plan_path).unwrap();
        let plan: storage_engine::RecoveryPlan = json_codec::from_slice(&plan_before).unwrap();
        let obstruction = if fail_index {
            data.join("index.sqlite")
        } else {
            data.join("workspaces").join("broken-workspace")
        };
        let blocked_path = obstruction.clone();
        crate::setup_recovery::after_next_recovery_open(move || {
            if fail_index {
                fs::remove_file(&blocked_path).unwrap();
                fs::create_dir(&blocked_path).unwrap();
            } else {
                fs::create_dir(&blocked_path).unwrap();
                fs::write(blocked_path.join("workspace.json"), b"{}").unwrap();
            }
        });
        let error = fixture
            .backend
            .setup_operation(
                "recovery_commit",
                fixture.input.clone(),
                None,
                Some(&identity),
            )
            .unwrap_err();
        assert_eq!(error.status, ::http::StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(error.code, storage_contract::ErrorCode::RecoveryRequired);
        assert!(!error.retryable);
        let Some(crate::http_boundary::ApiErrorSource::Engine(
            Error::PublishedRecoveryReconciliation(context),
        )) = error.source.as_deref()
        else {
            panic!("management lost the precise post-publication recovery context");
        };
        assert_eq!(context.recovery_id(), recovery_id);
        assert_eq!(
            context.plan_digest(),
            fixture.input["digest"].as_str().unwrap()
        );
        assert_eq!(context.principal(), identity.principal);
        assert_eq!(context.isolation_generation(), plan.isolation_generation);
        assert_eq!(
            context.stage(),
            if fail_index {
                RecoveryReconciliationStage::IndexRebuild
            } else {
                RecoveryReconciliationStage::WorkspaceReconciliation
            }
        );
        let receipt = context.result();
        assert_eq!(receipt.operation_id, plan.operation_id);
        assert_eq!(
            receipt.commit_state,
            storage_contract::CommitState::PublishedDegraded
        );
        assert_eq!(
            receipt.protection_at_publish,
            storage_contract::ProtectionAtPublish::BelowTarget
        );
        assert!(error.message.contains("already published"));
        assert!(error.message.contains(&receipt.operation_id.to_string()));
        assert!(!error.message.contains(data.to_str().unwrap()));
        let wire = json_codec::to_vec(&json!({"error": &error})).unwrap();
        let decoded = storage_contract::client_protocol::decode_response(503, &wire).unwrap_err();
        assert!(storage_contract::client_protocol::mutation_outcome_unknown(
            decoded.status
        ));
        assert!(!decoded.retryable);
        assert_eq!(decoded.message, error.message);
        assert_eq!(
            decoded.request_id.as_deref(),
            Some(error.request_id.as_str())
        );
        assert_eq!(fs::read(&plan_path).unwrap(), plan_before);

        // This contextual response does not turn operation queries into an
        // offline evidence reader, nor relax startup while the obstruction remains.
        assert!(Engine::open(&data).is_err());
        assert!(Engine::open_for_recovery(&data).is_err());
        if !fail_index {
            fs::remove_file(obstruction.join("workspace.json")).unwrap();
        }
        fs::remove_dir(&obstruction).unwrap();
        let reopened = Engine::open(&data).unwrap();
        let published = reopened.operation(receipt.operation_id).unwrap();
        assert_eq!(published.commit_state, receipt.commit_state);
        assert_eq!(
            published.protection_at_publish,
            receipt.protection_at_publish
        );
        assert_eq!(published.request_digest, receipt.request_digest);
        assert_eq!(published.sequence, receipt.sequence);
        assert_eq!(fs::read(plan_path).unwrap(), plan_before);
        fixture.backend.stop_management();
        fixture.backend.drain_management().unwrap();
    }
}

#[test]
fn offline_recovery_revalidates_after_open_before_new_plan_or_publication() {
    for action in ["recovery_preview", "recovery_commit"] {
        for change_password in [false, true] {
            let f = Fixture::new(action);
            let (token, identity) = {
                let mut auth = f.auth.lock().unwrap();
                let token = auth.login("admin", PASSWORD).unwrap().token;
                let identity = auth.authenticate(&token).unwrap();
                (token, identity)
            };
            let before = Arc::new(Mutex::new(None));
            let captured = before.clone();
            let auth = f.auth.clone();
            let data = f.backend.inner.data_root.clone();
            let disks = f.backend.inner.setup_disks.clone();
            crate::setup_recovery::after_next_recovery_open(move || {
                let mut auth = auth.lock().unwrap();
                if change_password {
                    auth.change_password(&token, PASSWORD, NEW_PASSWORD)
                        .unwrap();
                } else {
                    auth.logout(&token).unwrap();
                }
                drop(auth);
                *captured.lock().unwrap() = Some(evidence(&data, &disks));
            });
            let error = f
                .backend
                .setup_operation(action, f.input.clone(), None, Some(&identity))
                .unwrap_err();
            assert_eq!(
                error.code,
                storage_contract::ErrorCode::Unauthorized,
                "{action}, password change: {change_password}"
            );
            let before = before
                .lock()
                .unwrap()
                .take()
                .expect("recovery open hook ran");
            assert_eq!(
                evidence(&f.backend.inner.data_root, &f.backend.inner.setup_disks),
                before,
                "rejected recovery changed plans, journals or publication proofs"
            );
            assert!(
                f.auth
                    .lock()
                    .unwrap()
                    .revalidate_identity(&identity)
                    .is_err()
            );

            // The exact same, valid request remains usable with fresh credentials.
            // This proves rejection did not come from a malformed/unsatisfiable plan.
            let fresh = {
                let mut auth = f.auth.lock().unwrap();
                let password = if change_password {
                    NEW_PASSWORD
                } else {
                    PASSWORD
                };
                let login = auth.login("admin", password).unwrap();
                auth.authenticate(&login.token).unwrap()
            };
            let result = f
                .backend
                .setup_operation(action, f.input, None, Some(&fresh));
            f.backend.stop_management();
            f.backend.drain_management().unwrap();
            let result = result.unwrap();
            if action == "recovery_commit" {
                assert_eq!(result["commit_state"], "PUBLISHED_DEGRADED");
            } else {
                assert!(result["recovery_id"].as_str().is_some());
            }
        }
    }
}

#[tokio::test]
async fn recovery_preview_http_parses_ids_before_lifecycle_and_keeps_auth_precedence() {
    use http_routing::{body::Body, http::Request};
    use service_stack::ServiceExt;

    let fixture = Fixture::new("recovery_preview");
    let config = crate::ServerConfig::loopback(&fixture.backend.inner.management_root);
    let spool = crate::http_spool::HttpSpool::open_blocking(&config);
    let telemetry = crate::telemetry::Telemetry::start(
        fixture.backend.clone(),
        &fixture._root.path().join("telemetry"),
    )
    .unwrap();
    let auth_reader = crate::lock(&fixture.auth).reader();
    let app = crate::router_shared(
        Arc::new(crate::AuthorizedBackend {
            backend: fixture.backend.clone(),
            auth: auth_reader,
        }),
        fixture.auth.clone(),
        config.admit().unwrap(),
        telemetry,
        spool,
    )
    .unwrap();
    let token = fixture
        .auth
        .lock()
        .unwrap()
        .login("admin", PASSWORD)
        .unwrap()
        .token;
    let request = |token: Option<&str>, input: Value| {
        let mut request = Request::builder()
            .method("POST")
            .uri("/api/management/recovery-preview")
            .header("content-type", "application/json");
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        request
            .body(Body::from(json_codec::to_vec(&input).unwrap()))
            .unwrap()
    };
    let before = evidence(
        &fixture.backend.inner.data_root,
        &fixture.backend.inner.setup_disks,
    );
    let system_before = files(&fixture.backend.inner.data_root);
    // A valid request hits this existing lifecycle reservation; malformed IDs must not.
    fixture
        .backend
        .inner
        .recovering
        .store(true, Ordering::SeqCst);
    let cases = [
        (json!({"reason":"boundary test"}), "INVALID_REQUEST"),
        (
            json!({"operation_id":"","reason":"boundary test"}),
            "INVALID_REQUEST",
        ),
        (
            json!({"operation_id":null,"reason":"boundary test"}),
            "INVALID_REQUEST",
        ),
        (
            json!({"operation_id":17,"reason":"boundary test"}),
            "INVALID_REQUEST",
        ),
        (
            json!({"operation_id":"op_short","reason":"boundary test"}),
            "INVALID_ID",
        ),
        (
            json!({"operation_id":"../../outside","reason":"boundary test"}),
            "INVALID_ID",
        ),
        (
            json!({"operation_id":"op_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA","reason":"boundary test"}),
            "INVALID_ID",
        ),
        (
            json!({"operation_id":"ws_00000000000000000000000000000000","reason":"boundary test"}),
            "INVALID_ID",
        ),
    ];
    for (input, code) in &cases {
        let response = app
            .clone()
            .oneshot(request(Some(&token), input.clone()))
            .await
            .unwrap();
        assert_eq!(response.status(), ::http::StatusCode::BAD_REQUEST);
        let bytes = http_routing::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let body: Value = json_codec::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], *code, "{input}");
        assert!(fixture.backend.inner.recovering.load(Ordering::SeqCst));
        assert_eq!(
            evidence(
                &fixture.backend.inner.data_root,
                &fixture.backend.inner.setup_disks
            ),
            before
        );
        assert_eq!(files(&fixture.backend.inner.data_root), system_before);
    }
    let response = app
        .clone()
        .oneshot(request(Some(&token), fixture.input.clone()))
        .await
        .unwrap();
    assert_eq!(response.status(), ::http::StatusCode::CONFLICT);
    let bytes = http_routing::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: Value = json_codec::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], "STORAGE_RETRY_RUNNING");
    fixture.auth.lock().unwrap().logout(&token).unwrap();
    for token in [None, Some(token.as_str())] {
        for (input, _) in &cases {
            let response = app
                .clone()
                .oneshot(request(token, input.clone()))
                .await
                .unwrap();
            assert_eq!(response.status(), ::http::StatusCode::UNAUTHORIZED);
            let bytes = http_routing::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap();
            let body: Value = json_codec::from_slice(&bytes).unwrap();
            assert_eq!(body["error"]["code"], "UNAUTHORIZED");
        }
    }
    assert_eq!(
        evidence(
            &fixture.backend.inner.data_root,
            &fixture.backend.inner.setup_disks
        ),
        before
    );
    assert_eq!(files(&fixture.backend.inner.data_root), system_before);
    fixture
        .backend
        .inner
        .recovering
        .store(false, Ordering::SeqCst);
}
