use super::*;
use std::sync::{Arc, Mutex};
use storage_server::Backend;

#[test]
fn offline_fixture_preserves_real_management_authority_with_a_corrupt_pool() {
    let root = tempfile::tempdir().unwrap();
    let password = "offline-fixture-admin-password";
    super::super::fixture::initialize_management(&root, password).unwrap();
    let source = root.path().join("pool");
    let offline = root.path().join("offline-management");
    prepare_offline(&source, &offline).unwrap();
    let pool = offline.join("pool");
    let management = offline.join("management");
    let (backend, mut auth) = storage_server::management::open_service(&pool, &management).unwrap();
    assert!(
        auth.is_available(),
        "offline fixture must preserve usable system credentials"
    );
    use std::os::unix::fs::PermissionsExt;
    for directory in [&offline, &pool, &management] {
        assert_eq!(
            fs::metadata(directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    assert_eq!(
        fs::metadata(management.join("management-auth.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let session = auth.login("smoke-admin", password).unwrap();
    let identity = auth.authenticate(&session.token).unwrap();
    let auth = Arc::new(Mutex::new(auth));
    backend.start_management(auth.clone());
    wait_for("corrupt fixture storage discovery", || {
        let status = backend.management_status();
        if status.state() == storage_server::StorageState::Unavailable && !status.recovering() {
            Ok(())
        } else {
            Err(format!("unexpected storage state: {status:?}").into())
        }
    })
    .unwrap();
    backend.retry_storage(&identity).unwrap();
    wait_for("corrupt fixture storage retry", || {
        let status = backend.management_status();
        if status.state() == storage_server::StorageState::Unavailable && !status.recovering() {
            Ok(())
        } else {
            Err(format!("unexpected storage state: {status:?}").into())
        }
    })
    .unwrap();
    let mut auth = auth.lock().unwrap();
    assert!(auth.authenticate(&session.token).is_ok());
    auth.logout(&session.token).unwrap();
    assert!(auth.authenticate(&session.token).is_err());
    drop(auth);
    backend.drain_management().unwrap();
}
