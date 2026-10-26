use super::*;
use http_routing::{Router, body::Body, http::Request};
use json_codec::{Value, json};
use service_stack::ServiceExt;
use std::{sync::Mutex, time::Duration};

const OLD_PASSWORD: &str = "password-change-old-password";
const NEW_PASSWORD: &str = "password-change-new-password";

struct UnusedBackend;
impl crate::Backend for UnusedBackend {
    fn operation(
        &self,
        _: &crate::auth::Identity,
        _: storage_contract::OperationId,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        unreachable!("test does not query operations")
    }
    fn commit_workspace(
        &self,
        _: &crate::auth::Identity,
        _: crate::CommitWorkspace,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        unreachable!("test does not commit workspaces")
    }

    fn snapshot_at(
        &self,
        _: &crate::auth::Identity,
        _: &str,
        _: &str,
    ) -> Result<std::sync::Arc<storage_contract::Commit>, crate::ApiError> {
        unreachable!("this test backend does not serve snapshots")
    }

    fn authorize_current(
        &self,
        _identity: &crate::auth::Identity,
        _branch: &str,
    ) -> Result<(), crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }

    fn read_directory(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadDirectory<'_>,
    ) -> Result<crate::DirectorySnapshot, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }

    fn authorize_workspace_selection(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
        _path: &name_policy::PortablePath,
    ) -> Result<(), crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }

    fn create_workspace(
        &self,
        _identity: &crate::auth::Identity,
        _command: crate::CreateWorkspace,
    ) -> Result<storage_contract::Workspace, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn abort_workspace(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
    ) -> Result<(), crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }

    fn workspace(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
    ) -> Result<storage_contract::Workspace, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn published_upload_status(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
        _item: &str,
    ) -> Result<Option<crate::PublishedUploadStatus>, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn seal_workspace(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
    ) -> Result<storage_contract::SealedWorkspace, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn commit_workspace_public(
        &self,
        _identity: &crate::auth::Identity,
        _command: storage_contract::PublicCommitEnvelope,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn authorize_upload(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
        _logical_size: Option<u64>,
    ) -> Result<(), crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }

    fn lookup_current(
        &self,
        _identity: &crate::auth::Identity,
        _branch: &str,
        _path: &name_policy::LookupPath,
    ) -> Result<Option<storage_contract::Entry>, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }

    fn lookup_at(
        &self,
        _: &crate::auth::Identity,
        _: &str,
        _: &str,
        _: &name_policy::LookupPath,
    ) -> Result<Option<storage_contract::Entry>, crate::ApiError> {
        unreachable!("this test backend does not serve snapshot lookups")
    }

    fn call(&self, _: crate::BackendAction, _: &Identity, _: Value) -> Result<Value, ApiError> {
        Err(ApiError::unsupported())
    }
    fn upload(
        &self,
        _: &Identity,
        _: &str,
        _: &str,
        _: &name_policy::PortablePath,
        _: &crate::UploadSource,
    ) -> Result<storage_contract::StagedFile, ApiError> {
        Err(ApiError::unsupported())
    }
    fn upload_generation(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
        _item: &str,
        _path: &name_policy::PortablePath,
        _source: &crate::UploadSource,
        _generation: u64,
    ) -> Result<storage_contract::StagedFile, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }

    fn content(&self, _: &Identity, _: &str) -> Result<crate::Content, ApiError> {
        Err(ApiError::unsupported())
    }
}

fn application(root: &Path, store: AuthStore) -> (Router, Arc<Mutex<AuthStore>>) {
    let config = crate::ServerConfig::loopback(root);
    let spool = crate::http_spool::HttpSpool::open_blocking(&config);
    let runtime = crate::ManagementRuntime::start(Arc::new(UnusedBackend), store, &config).unwrap();
    let auth = runtime.auth.clone();
    let app = crate::router_shared(
        runtime.backend,
        runtime.auth,
        config.admit().unwrap(),
        runtime.telemetry,
        spool,
    )
    .unwrap();
    (app, auth)
}

fn password_request(token: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/api/auth/password")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"current_password":OLD_PASSWORD,"new_password":NEW_PASSWORD}).to_string(),
        ))
        .unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn password_work_does_not_hold_the_authority_lock_or_block_session_authentication() {
    let root = crate::tests::private_tempdir();
    let mut store = AuthStore::initialize(root.path(), "owner", OLD_PASSWORD).unwrap();
    let login = store.login("owner", OLD_PASSWORD).unwrap();
    let (started, mut work) = tokio::sync::mpsc::unbounded_channel();
    let (resume, wait) = std::sync::mpsc::channel();
    let wait = Mutex::new(wait);
    store.password_work_hook = Some(Arc::new(move || {
        started.send(()).unwrap();
        // Independent watchdog bounds regressions without depending on Tokio.
        wait.lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
    }));
    let (app, auth) = application(root.path(), store);
    let changing = tokio::spawn(app.clone().oneshot(password_request(&login.token)));
    let mut concurrent_login = None;
    for stage in ["verify current password", "hash new password"] {
        tokio::time::timeout(Duration::from_secs(10), work.recv())
            .await
            .unwrap()
            .unwrap();
        let unlocked = auth.try_lock().is_ok();
        let response = if unlocked {
            Some(
                tokio::time::timeout(
                    Duration::from_secs(2),
                    app.clone().oneshot(
                        Request::builder()
                            .uri("/api/auth/me")
                            .header("authorization", format!("Bearer {}", login.token))
                            .body(Body::empty())
                            .unwrap(),
                    ),
                )
                .await,
            )
        } else {
            None
        };
        if unlocked && stage == "hash new password" {
            let response = tokio::time::timeout(
                Duration::from_secs(5),
                app.clone().oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/auth/login")
                        .header("content-type", "application/json")
                        .body(Body::from(
                            json!({"username":"owner","password":OLD_PASSWORD}).to_string(),
                        ))
                        .unwrap(),
                ),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(response.status(), 200);
            let bytes = http_routing::body::to_bytes(response.into_body(), 8192)
                .await
                .unwrap();
            concurrent_login = Some(json_codec::from_slice::<LoginResult>(&bytes).unwrap());
        }
        resume.send(()).unwrap();
        assert!(unlocked, "authority lock held during {stage}");
        assert_eq!(response.unwrap().unwrap().unwrap().status(), 200, "{stage}");
    }
    let response = changing.await.unwrap().unwrap();
    assert_eq!(response.status(), 200);
    assert!(
        response.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );
    assert!(auth.lock().unwrap().authenticate(&login.token).is_err());
    assert!(
        auth.lock()
            .unwrap()
            .authenticate(&concurrent_login.unwrap().token)
            .is_err()
    );
}

#[test]
fn concurrent_password_preparations_publish_once_and_revoke_every_old_session_and_login_proof() {
    let root = crate::tests::private_tempdir();
    let mut store = AuthStore::initialize(root.path(), "owner", OLD_PASSWORD).unwrap();
    let first = store.login("owner", OLD_PASSWORD).unwrap();
    let second = store.login("owner", OLD_PASSWORD).unwrap();
    let pending_login = store
        .login_challenge("owner")
        .unwrap()
        .verify(OLD_PASSWORD)
        .unwrap();
    let store = Mutex::new(store);
    let prepared = std::sync::Barrier::new(2);
    let passwords = [NEW_PASSWORD, "password-change-competing-password"];
    let results = std::thread::scope(|scope| {
        let workers = [&first.token, &second.token]
            .into_iter()
            .zip(passwords)
            .map(|(token, password)| {
                let store = &store;
                let prepared = &prepared;
                scope.spawn(move || {
                    let challenge = store
                        .lock()
                        .unwrap()
                        .password_change_challenge(token)
                        .unwrap();
                    let replacement = challenge.prepare(OLD_PASSWORD, password).unwrap();
                    // Both proofs must use the old baseline before either publishes.
                    prepared.wait();
                    store.lock().unwrap().finish_password_change(replacement)
                })
            })
            .collect::<Vec<_>>();
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>()
    });
    let winner = results.iter().position(Result::is_ok).unwrap();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results[1 - winner].as_ref().unwrap_err().code,
        storage_contract::ErrorCode::Unauthorized
    );
    let mut store = store.into_inner().unwrap();
    assert_eq!(store.database().unwrap().administrator.generation, 2);
    assert!(store.database().unwrap().sessions.is_empty());
    assert!(store.authenticate(&first.token).is_err());
    assert!(store.authenticate(&second.token).is_err());
    assert_eq!(
        store.finish_login(pending_login).err().unwrap().code,
        storage_contract::ErrorCode::Unauthorized
    );
    drop(store);
    let mut store = AuthStore::open(root.path()).unwrap();
    assert!(store.authenticate(&first.token).is_err());
    assert!(store.authenticate(&second.token).is_err());
    assert!(store.login("owner", passwords[winner]).is_ok());
    assert!(store.login("owner", OLD_PASSWORD).is_err());
    assert!(store.login("owner", passwords[1 - winner]).is_err());
}

#[test]
fn prepared_password_change_rechecks_session_and_exact_credentials_without_mutation() {
    let different_hash = primitives::hash_password("independently-recovered-password").unwrap();
    for invalidation in [
        "logout",
        "expiry",
        "session generation",
        "credential generation",
        "hash",
        "principal",
        "blocked",
    ] {
        let root = crate::tests::private_tempdir();
        let mut store = AuthStore::initialize(root.path(), "owner", OLD_PASSWORD).unwrap();
        let login = store.login("owner", OLD_PASSWORD).unwrap();
        let prepared = store
            .password_change_challenge(&login.token)
            .unwrap()
            .prepare(OLD_PASSWORD, NEW_PASSWORD)
            .unwrap();
        let key = primitives::token_hash_hex(&login.token);
        match invalidation {
            "logout" => store.logout(&login.token).unwrap(),
            "expiry" => {
                store
                    .database_mut()
                    .unwrap()
                    .sessions
                    .get_mut(&key)
                    .unwrap()
                    .expires_at = 0
            }
            "session generation" => {
                store
                    .database_mut()
                    .unwrap()
                    .sessions
                    .get_mut(&key)
                    .unwrap()
                    .generation = 0
            }
            "credential generation" => {
                let database = store.database_mut().unwrap();
                database.administrator.generation += 1;
                // Keep the live session valid to exercise the credential baseline check.
                database.sessions.get_mut(&key).unwrap().generation =
                    database.administrator.generation;
                assert!(store.authenticate(&login.token).is_ok());
            }
            "hash" => {
                store.database_mut().unwrap().administrator.password_hash = different_hash.clone();
                assert!(store.authenticate(&login.token).is_ok());
            }
            "principal" => store.database_mut().unwrap().administrator.username = "another".into(),
            "blocked" => store.blocked.store(true, Ordering::SeqCst),
            _ => unreachable!(),
        }
        let before = store.snapshot().unwrap();
        let disk = fs::read(root.path().join("auth.json")).unwrap();
        let error = store.finish_password_change(prepared).unwrap_err();
        assert_eq!(
            error.code,
            if invalidation == "blocked" {
                storage_contract::ErrorCode::RecoveryRequired
            } else {
                storage_contract::ErrorCode::Unauthorized
            },
            "{invalidation}"
        );
        assert_eq!(store.snapshot().unwrap(), before, "{invalidation}");
        assert_eq!(
            fs::read(root.path().join("auth.json")).unwrap(),
            disk,
            "{invalidation}"
        );
    }
}

#[test]
fn failed_prepared_password_publication_restores_sessions_created_during_preparation() {
    for uncertain in [false, true] {
        let root = crate::tests::private_tempdir();
        let mut store = AuthStore::initialize(root.path(), "owner", OLD_PASSWORD).unwrap();
        let original = store.login("owner", OLD_PASSWORD).unwrap();
        let prepared = store
            .password_change_challenge(&original.token)
            .unwrap()
            .prepare(OLD_PASSWORD, NEW_PASSWORD)
            .unwrap();
        let concurrent = store.login("owner", OLD_PASSWORD).unwrap();
        let before = store.snapshot().unwrap();
        let disk = fs::read(root.path().join("auth.json")).unwrap();
        store.backup = Some(Arc::new(move |_| {
            Err(ApiError::new(
                if uncertain {
                    ::http::StatusCode::SERVICE_UNAVAILABLE
                } else {
                    ::http::StatusCode::CONFLICT
                },
                if uncertain {
                    storage_contract::ErrorCode::CommitInDoubt
                } else {
                    storage_contract::ErrorCode::CommitPreparationBusy
                },
                "injected publication failure",
            ))
        }));
        assert_eq!(
            store.finish_password_change(prepared).unwrap_err().code,
            if uncertain {
                storage_contract::ErrorCode::CommitInDoubt
            } else {
                storage_contract::ErrorCode::CommitPreparationBusy
            }
        );
        assert_eq!(store.snapshot().unwrap(), before);
        assert_eq!(fs::read(root.path().join("auth.json")).unwrap(), disk);
        for token in [&original.token, &concurrent.token] {
            if uncertain {
                assert_eq!(
                    store.authenticate(token).unwrap_err().code,
                    storage_contract::ErrorCode::RecoveryRequired
                );
            } else {
                assert!(store.authenticate(token).is_ok());
            }
        }
        if !uncertain {
            drop(store);
            let store = AuthStore::open(root.path()).unwrap();
            assert_eq!(store.snapshot().unwrap(), before);
            assert!(store.authenticate(&original.token).is_ok());
            assert!(store.authenticate(&concurrent.token).is_ok());
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn http_session_reads_continue_during_durable_logout_then_observe_revocation() {
    let root = crate::tests::private_tempdir();
    let mut store = AuthStore::initialize(root.path(), "owner", OLD_PASSWORD).unwrap();
    let login = store.login("owner", OLD_PASSWORD).unwrap();
    let (started, mut work) = tokio::sync::mpsc::unbounded_channel();
    let (resume, wait) = std::sync::mpsc::channel();
    let wait = Mutex::new(wait);
    store.backup = Some(Arc::new(move |_| {
        started.send(()).unwrap();
        wait.lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        Ok(())
    }));
    let (app, auth) = application(root.path(), store);
    let token = login.token.clone();
    let writer = tokio::task::spawn_blocking(move || auth.lock().unwrap().logout(&token));
    tokio::time::timeout(Duration::from_secs(10), work.recv())
        .await
        .unwrap()
        .unwrap();
    let request = || {
        Request::builder()
            .uri("/api/auth/me")
            .header("authorization", format!("Bearer {}", login.token))
            .body(Body::empty())
            .unwrap()
    };
    let response =
        tokio::time::timeout(Duration::from_secs(2), app.clone().oneshot(request())).await;
    // Release the writer even if the read regresses, so the assertion cannot strand it.
    resume.send(()).unwrap();
    assert_eq!(response.unwrap().unwrap().status(), 200);
    writer.await.unwrap().unwrap();
    assert_eq!(app.oneshot(request()).await.unwrap().status(), 401);
}
