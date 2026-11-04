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

#[test]
fn account_setup_fixture_has_no_disk_candidates_or_pool_configuration() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let fixture = SetupFixture::new(root.path(), "account-only-fixture").unwrap();
    assert!(!fixture.pool.join("config.json").exists());
    assert!(!fixture.root.join("storage-parent").exists());
    for path in [&fixture.root, &fixture.pool, &fixture.management] {
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    assert!(!fixture.management.join("bootstrap.key").exists());
}

#[test]
fn browser_storage_fixture_selects_only_an_empty_private_child() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let fixture = SetupFixture::new(root.path(), "nested-directory-fixture").unwrap();
    let storage = fixture.storage_directory().unwrap();
    assert_eq!(
        storage,
        fixture.root.join("storage-parent/selected-storage")
    );
    assert!(fs::read_dir(&storage).unwrap().next().is_none());
    assert_eq!(
        fs::metadata(&storage).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::read(storage.parent().unwrap().join("outside-selection.txt")).unwrap(),
        b"outside selected storage\n"
    );
    assert!(!fixture.pool.join("config.json").exists());
}

#[test]
fn completion_fixture_has_real_activation_proof_and_existing_account() {
    // Inspection intentionally rejects tmpfs. Keep this test on the repository's
    // filesystem instead of weakening the production candidate validation.
    let root = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let fixture = SetupFixture::new(root.path(), "activation-completion-fixture").unwrap();
    let storage = prepare_interruption(&fixture, "fixture-only-long-password").unwrap();
    assert_single_storage(&fixture, &storage).unwrap();
    assert!(fixture.pool.join("management-authority.json").exists());
    assert!(fixture.management.join("management-auth.json").exists());
    assert!(fixture.pool.join("storage-activation-intent.json").exists());
    assert!(fixture.pool.join("storage-activation-ready.json").exists());
    assert!(
        !fixture
            .pool
            .join("storage-activation-complete.json")
            .exists()
    );
    let service = storage_server::setup_recovery::SetupRecovery::new(
        fixture.pool.clone(),
        fixture.management.clone(),
        std::collections::BTreeMap::new(),
    )
    .unwrap();
    assert!(service.storage_status().completion_available);
}
