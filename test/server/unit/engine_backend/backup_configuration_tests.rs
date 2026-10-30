use super::*;
use crate::auth::AuthStore;
use std::sync::Arc;

fn fixture() -> (
    tempfile::TempDir,
    Engine,
    storage_contract::Repository,
    Identity,
) {
    let root = crate::tests::private_tempdir();
    let config = storage_engine::Config::development([
        root.path().join("d1"),
        root.path().join("d2"),
        root.path().join("d3"),
    ]);
    let mut engine = Engine::initialize(&root.path().join("system"), config).unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("BackupRead").unwrap(),
            "P01",
            3,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let identity = install_test_administrator(&mut engine, "owner");
    (root, engine, repo, identity)
}

#[test]
fn captured_configuration_finishes_without_writer_and_keeps_original_generations() {
    let (_root, engine, repo, identity) = fixture();
    let expected = engine
        .backup_configuration(&repo.repository_id, "")
        .unwrap();
    let backend = EngineBackend::new(engine);
    let view = backend
        .engine
        .lock()
        .unwrap()
        .backup_configuration_read_view();
    let mut writer = backend.engine.lock().unwrap();
    writer
        .configure_backups(storage_engine::BackupConfigurationRequest {
            repository_id: &repo.repository_id,
            path: "",
            target_copies: 1,
            backup_prefixes: &std::collections::BTreeMap::new(),
            expected_generation: 0,
            principal: "owner",
            operation_id: storage_contract::OperationId::generate(),
        })
        .unwrap();
    let repository = repo.repository_id.clone();
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        let control = crate::tests::history_read_control();
        let observe = || control.checkpoint().map_err(storage_engine::Error::from);
        send.send(view.read(&repository, "", &observe).map_err(error))
            .unwrap();
    });
    let result = receive.recv_timeout(std::time::Duration::from_secs(30));
    drop(writer);
    worker.join().unwrap();
    let snapshot = backend
        .finish_backup_configuration(
            &identity,
            result.expect("backup read waited for the writer mutex"),
            &crate::tests::history_read_control(),
        )
        .unwrap();
    assert_eq!(snapshot, expected);
    assert_eq!(
        backend
            .backup_configuration(
                &identity,
                crate::ReadBackupConfiguration {
                    repository_id: repo.repository_id,
                    path: String::new()
                },
                &crate::tests::history_read_control(),
            )
            .unwrap()
            .generation,
        1
    );
}

#[test]
fn completion_reauthorizes_success_error_and_cancellation_for_every_auth_mode() {
    for managed in [false, true] {
        for password_change in [false, true] {
            let (root, engine, repo, _) = fixture();
            let backend = Arc::new(EngineBackend::new(engine));
            let auth =
                AuthStore::initialize(root.path(), "owner", "integration-test-password").unwrap();
            let mut auth = if managed {
                backend.set_management_auth(auth.reader()).unwrap();
                auth
            } else {
                backend.protect_auth(auth).unwrap()
            };
            let token = auth
                .login("owner", "integration-test-password")
                .unwrap()
                .token;
            let identity = auth.authenticate(&token).unwrap();
            let control = crate::tests::history_read_control();
            let view = backend
                .engine
                .lock()
                .unwrap()
                .backup_configuration_read_view();
            let observe = || control.checkpoint().map_err(storage_engine::Error::from);
            let captured = view.read(&repo.repository_id, "", &observe).map_err(error);
            let failure = || {
                ApiError::new(
                    ::http::StatusCode::INTERNAL_SERVER_ERROR,
                    storage_contract::ErrorCode::ObjectCorrupt,
                    "backup namespace corrupt",
                )
            };
            assert_eq!(
                backend
                    .finish_backup_configuration(&identity, Err(failure()), &control)
                    .unwrap_err()
                    .code,
                storage_contract::ErrorCode::ObjectCorrupt
            );
            let cancelled = crate::tests::history_read_control();
            drop(cancelled.wait_owner());
            let observe_cancelled = || cancelled.checkpoint().map_err(storage_engine::Error::from);
            assert_eq!(
                view.read(&repo.repository_id, "", &observe_cancelled)
                    .unwrap_err()
                    .code(),
                storage_contract::ErrorCode::RequestTimeout
            );
            assert_eq!(
                backend
                    .backup_configuration(
                        &identity,
                        crate::ReadBackupConfiguration {
                            repository_id: repo.repository_id.clone(),
                            path: String::new(),
                        },
                        &cancelled
                    )
                    .unwrap_err()
                    .code,
                storage_contract::ErrorCode::RequestTimeout
            );
            if password_change {
                auth.change_password(
                    &token,
                    "integration-test-password",
                    "new-integration-test-password",
                )
                .unwrap();
            } else {
                auth.logout(&token).unwrap();
            }
            for result in [captured, Err(failure())] {
                assert_eq!(
                    backend
                        .finish_backup_configuration(&identity, result, &control)
                        .unwrap_err()
                        .code,
                    storage_contract::ErrorCode::Unauthorized
                );
            }
            assert_eq!(
                backend
                    .finish_backup_configuration(&identity, Err(failure()), &cancelled)
                    .unwrap_err()
                    .code,
                storage_contract::ErrorCode::Unauthorized
            );
        }
    }
}

#[tokio::test]
async fn configuration_wire_is_complete_bounded_and_leased_until_last_chunk_drops() {
    use futures::StreamExt;
    let (_root, engine, repo, _) = fixture();
    let snapshot = engine
        .backup_configuration(&repo.repository_id, "")
        .unwrap();
    let response =
        crate::backend_backup_configuration::BackupConfigurationResponse::from(snapshot.clone());
    let expected = json_codec::to_vec(&response).unwrap();
    let encoded = crate::backend_file_history::encode_response_bounded(response).unwrap();
    assert_eq!(encoded.body(), expected);
    let slots = Arc::new(tokio::sync::Semaphore::new(1));
    let limits = crate::TransferLimits::default();
    let response = encoded.into_leased_response(
        slots.clone().try_acquire_owned().unwrap(),
        &limits,
        tokio::time::Instant::now() + limits.total_timeout,
        None,
    );
    let mut stream = response.into_body().into_data_stream();
    let chunk = stream.next().await.unwrap().unwrap();
    assert!(stream.next().await.is_none());
    drop(stream);
    assert_eq!(slots.available_permits(), 0);
    drop(chunk);
    assert_eq!(slots.available_permits(), 1);
    let mut oversized = snapshot;
    oversized.scope.path = "x".repeat(storage_contract::client_protocol::MAX_RESPONSE_BYTES);
    assert_eq!(
        crate::backend_file_history::encode_response_bounded(
            crate::backend_backup_configuration::BackupConfigurationResponse::from(oversized),
        )
        .err()
        .unwrap()
        .code,
        storage_contract::ErrorCode::ResourceLimitExceeded
    );
}
