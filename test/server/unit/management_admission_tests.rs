//! Startup diagnostics and request-local setup admission, without background workers.
use super::*;
use std::{
    io::{Read, Seek},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
};
use storage_contract::ErrorCode;

const SECRET: &str = "credential-secret-must-not-be-logged";

fn capture<T>(action: impl FnOnce() -> T) -> (T, String) {
    let mut file = tempfile::tempfile().unwrap();
    let subscriber = tracing::collector::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(Arc::new(file.try_clone().unwrap()))
        .finish();
    let result = tracing::subscriber::with_default(subscriber, action);
    file.rewind().unwrap();
    let mut output = String::new();
    file.read_to_string(&mut output).unwrap();
    (result, output)
}

fn write_private(path: &Path, bytes: &[u8]) {
    fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .unwrap()
        .write_all(bytes)
        .unwrap();
}

#[test]
fn startup_credential_json_diagnostics_redact_both_decoder_boundaries() {
    let documents = [
        format!(r#"{{"{SECRET}":1,"{SECRET}":2}}"#),
        format!(
            r#"{{"administrator":{{"username":"admin","password_hash":"{SECRET}","generation":"1","{SECRET}":true}},"sessions":{{}}}}"#
        ),
        format!(
            r#"{{"administrator":{{"username":["{SECRET}"],"password_hash":"{SECRET}","generation":"1"}},"sessions":{{}}}}"#
        ),
        format!(r#"{{"administrator":"{SECRET}""#),
    ];
    for document in documents {
        let root = crate::tests::private_tempdir();
        let data = root.path().join("data");
        let management = root.path().join("management");
        create_private_directory(&management).unwrap();
        let credentials = management.join("management-auth.json");
        write_private(&credentials, document.as_bytes());
        let ((backend, auth), output) = capture(|| open_service(&data, &management).unwrap());
        assert!(!auth.is_available());
        assert_eq!(
            auth.authenticate(SECRET).unwrap_err().code,
            ErrorCode::RecoveryRequired
        );
        assert!(backend.inner.backend().is_err());
        assert_eq!(fs::read(credentials).unwrap(), document.as_bytes());
        assert_eq!(
            output
                .matches("management credentials require recovery")
                .count(),
            1,
            "{output}"
        );
        assert!(output.contains("credential JSON"), "{output}");
        assert!(
            output.contains("line ") && output.contains("column "),
            "{output}"
        );
        assert!(!output.contains(SECRET), "{output}");
        assert!(!output.contains("password_hash"), "{output}");
        assert!(data.is_dir());
        assert_eq!(fs::read_dir(&data).unwrap().count(), 0);
    }
}

#[test]
fn startup_reports_the_credential_lock_cause_without_enabling_authentication() {
    let root = crate::tests::private_tempdir();
    let data = root.path().join("data");
    let management = root.path().join("management");
    let held = AuthStore::initialize_management(&management, "admin", SECRET).unwrap();
    let bytes = fs::read(management.join("management-auth.json")).unwrap();
    let expected = match AuthStore::open_management(&management) {
        Ok(_) => panic!("the original store still holds the credential lock"),
        Err(error) => {
            assert_eq!(error.code, ErrorCode::AuthStoreBusy);
            std::error::Error::source(&error).unwrap().to_string()
        }
    };
    let ((_backend, auth), output) = capture(|| open_service(&data, &management).unwrap());
    assert!(!auth.is_available());
    assert!(auth.authenticate(SECRET).is_err());
    assert_eq!(
        output
            .matches("management credentials require recovery")
            .count(),
        1,
        "{output}"
    );
    assert!(
        output.contains("AUTH_STORE_BUSY") && output.contains(&expected),
        "{output}"
    );
    assert!(!output.contains(SECRET));
    assert_eq!(
        fs::read(management.join("management-auth.json")).unwrap(),
        bytes
    );
    assert!(held.is_available());
}

#[test]
fn startup_reports_authority_io_failure_once_and_preserves_the_fence() {
    let root = crate::tests::private_tempdir();
    let data = root.path().join("data");
    let management = root.path().join("management");
    drop(AuthStore::initialize_management(&management, "admin", SECRET).unwrap());
    create_private_directory(&data).unwrap();
    let marker = data.join("management-authority.json");
    std::os::unix::fs::symlink("missing-authority", &marker).unwrap();
    let expected = claim_authority(&data).unwrap_err();
    assert_eq!(expected.code, ErrorCode::IoError);
    let cause = std::error::Error::source(&expected).unwrap().to_string();
    let ((_backend, auth), output) = capture(|| open_service(&data, &management).unwrap());
    assert!(!auth.is_available());
    assert_eq!(
        auth.authenticate(SECRET).unwrap_err().code,
        ErrorCode::RecoveryRequired
    );
    assert_eq!(
        output
            .matches("cannot durably establish management authority")
            .count(),
        1,
        "{output}"
    );
    assert!(!output.contains("management credentials require recovery"));
    assert!(
        output.contains("IO_ERROR") && output.contains(&cause),
        "{output}"
    );
    assert!(!output.contains(SECRET));
    assert_eq!(
        fs::read_link(marker).unwrap(),
        Path::new("missing-authority")
    );
}

#[test]
fn authority_marker_stays_private_and_rejects_public_or_aliased_files() {
    let root = crate::tests::private_tempdir();
    let data = root.path().join("data");
    create_private_directory(&data).unwrap();
    assert_eq!(fs::metadata(&data).unwrap().mode() & 0o777, 0o700);
    claim_authority(&data).unwrap();
    let marker = data.join("management-authority.json");
    let metadata = fs::metadata(&marker).unwrap();
    assert_eq!(metadata.mode() & 0o777, 0o600);
    assert_eq!(metadata.nlink(), 1);
    let bytes = fs::read(&marker).unwrap();
    claim_authority(&data).unwrap();

    fs::set_permissions(&marker, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(claim_authority(&data).unwrap_err().status, 400);
    assert_eq!(fs::read(&marker).unwrap(), bytes);
    fs::set_permissions(&marker, fs::Permissions::from_mode(0o600)).unwrap();
    let alias = data.join("authority-alias.json");
    fs::hard_link(&marker, &alias).unwrap();
    assert_eq!(claim_authority(&data).unwrap_err().status, 400);
    assert_eq!(fs::read(&marker).unwrap(), bytes);
    assert_eq!(fs::read(alias).unwrap(), bytes);
}

fn commands() -> Vec<(&'static str, Value, bool)> {
    vec![
        (
            "activate",
            json!({"selections":[],"mode":"development"}),
            true,
        ),
        ("restore_system", json!({"disk_ids":["disk"]}), true),
        (
            "recovery_preview",
            json!({"operation_id":"op_00000000000000000000000000000001","reason":"operator reason"}),
            false,
        ),
        (
            "recovery_commit",
            json!({"recovery_id":"recovery","digest":"digest","accept_permanent_loss":false}),
            true,
        ),
        ("offline_check", json!({"disk_id":"disk"}), false),
    ]
}

#[test]
fn admitted_setup_commands_keep_owned_fields_and_resume_policy() {
    for (action, input, resumes) in commands() {
        let command = SetupCommand::parse(action, input).unwrap();
        assert_eq!(command.resumes_storage(), resumes, "{action}");
        match command {
            SetupCommand::Activate(request) => {
                assert_eq!(action, "activate");
                assert!(request.selections.is_empty());
                assert_eq!(request.mode, storage_engine::Mode::Development);
            }
            SetupCommand::CompleteActivation => panic!("completion is separately tested"),
            SetupCommand::RestoreSystem(request) => {
                assert_eq!(action, "restore_system");
                assert_eq!(request.disk_ids, ["disk"]);
                assert!(!request.accept_stale_credentials);
            }
            SetupCommand::RecoveryPreview(request) => {
                assert_eq!(action, "recovery_preview");
                assert_eq!(
                    request.operation_id.to_string(),
                    "op_00000000000000000000000000000001"
                );
                assert_eq!(request.reason, "operator reason");
            }
            SetupCommand::RecoveryCommit(request) => {
                assert_eq!(action, "recovery_commit");
                assert_eq!(request.recovery_id, "recovery");
                assert_eq!(request.digest, "digest");
                assert!(!request.accept_permanent_loss);
            }
            SetupCommand::OfflineCheck(disk) => {
                assert_eq!(action, "offline_check");
                assert_eq!(disk, "disk");
            }
        }
    }
}

#[test]
fn setup_dto_errors_and_unknown_actions_keep_auth_and_lifecycle_precedence() {
    let root = crate::tests::private_tempdir();
    let data = root.path().join("data");
    let management = root.path().join("management");
    drop(AuthStore::initialize_management(&management, "admin", SECRET).unwrap());
    let (backend, mut auth) = open_service(&data, &management).unwrap();
    let login = auth.login("admin", SECRET).unwrap();
    let identity = auth.authenticate(&login.token).unwrap();
    *backend.inner.auth.lock().unwrap() = Some(Arc::new(Mutex::new(auth)));
    let credentials = fs::read(management.join("management-auth.json")).unwrap();
    let marker = fs::read(data.join("management-authority.json")).unwrap();
    backend.inner.recovering.store(true, Ordering::SeqCst);
    for (action, input, _) in commands() {
        let error = backend
            .setup_operation(action, input.clone(), None, Some(&identity))
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::StorageRetryRunning, "{action}");
        for malformed in [json!({}), json!(null), {
            let mut unknown = input.clone();
            unknown["unknown"] = json!(true);
            unknown
        }] {
            let error = backend
                .setup_operation(action, malformed, None, Some(&identity))
                .unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidRequest, "{action}");
            assert_eq!(error.status, ::http::StatusCode::BAD_REQUEST, "{action}");
            assert!(backend.inner.recovering.load(Ordering::SeqCst));
        }
    }
    let error = backend
        .setup_operation("unknown", json!(null), None, None)
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Unauthorized);
    let error = backend
        .setup_operation("unknown", json!(null), None, Some(&identity))
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::UnsupportedOperation);
    assert!(backend.inner.recovering.load(Ordering::SeqCst));
    assert_eq!(
        fs::read(management.join("management-auth.json")).unwrap(),
        credentials
    );
    assert_eq!(
        fs::read(data.join("management-authority.json")).unwrap(),
        marker
    );
    assert!(!data.join("config.json").exists());
    assert!(backend.inner.state.lock().unwrap().backend.is_none());
    backend.inner.recovering.store(false, Ordering::SeqCst);
}

#[test]
fn complete_activation_is_an_empty_command_and_bootstrap_disk_commands_are_removed() {
    assert!(matches!(
        SetupCommand::parse("complete_activation", json!({})).unwrap(),
        SetupCommand::CompleteActivation
    ));
    for body in [
        json!(null),
        json!({"selections":[]}),
        json!({"username":"admin"}),
    ] {
        assert!(SetupCommand::parse("complete_activation", body).is_err());
    }
    for action in ["initialize_preset", "complete_initialization"] {
        assert!(SetupCommand::parse(action, json!({})).is_err());
    }
    assert!(
        SetupCommand::parse(
            "initialize",
            json!({"username":"admin","password":SECRET,"disk_ids":[]})
        )
        .is_err()
    );
}
