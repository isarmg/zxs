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
    let prepared =
        storage_server::startup_admission::PreparedService::open(&pool, &management).unwrap();
    let (backend, mut auth) = storage_server::management::open_prepared_service_with_options(
        &prepared,
        storage_server::management::ManagementOptions::default(),
    )
    .unwrap();
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
    for name in ["config.json", "management-authority.json"] {
        assert_eq!(
            fs::metadata(pool.join(name)).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert_eq!(
        fs::read(pool.join("management-authority.json")).unwrap(),
        fs::read(source.join("management-authority.json")).unwrap()
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
fn empty_fixture_is_rejected_without_creating_setup_or_configuration() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let fixture = PreparedFixture::new(root.path(), "account-only-fixture").unwrap();
    assert!(!fixture.pool.join("config.json").exists());
    assert!(!fixture.root.join("storage-parent").exists());
    for path in [&fixture.root, &fixture.pool, &fixture.management] {
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    assert!(!fixture.management.join("bootstrap.key").exists());
    assert!(matches!(
        storage_server::startup_admission::PreparedService::open(
            &fixture.pool,
            &fixture.management
        ),
        Err(storage_server::startup_admission::Error::StorageNotPrepared)
    ));
    assert!(fs::read_dir(&fixture.pool).unwrap().next().is_none());
    assert!(fs::read_dir(&fixture.management).unwrap().next().is_none());
}

#[test]
fn browser_storage_fixture_selects_only_an_empty_private_child() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let fixture = PreparedFixture::new(root.path(), "nested-directory-fixture").unwrap();
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
fn primary_fixture_completes_activation_before_the_daemon_is_launched() {
    // Inspection intentionally rejects tmpfs. Keep this test on the repository's
    // filesystem instead of weakening the production candidate validation.
    let root = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let fixture = PreparedFixture::new(root.path(), "offline-prepared-fixture").unwrap();
    let storage = prepare_primary(&fixture, "fixture-only-long-password").unwrap();
    assert_single_storage(&fixture, &storage).unwrap();
    assert!(fixture.pool.join("management-authority.json").exists());
    assert!(fixture.management.join("management-auth.json").exists());
    assert!(fixture.pool.join("storage-activation-intent.json").exists());
    assert!(fixture.pool.join("storage-activation-ready.json").exists());
    assert!(
        fixture
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
    service.activation_admitted().unwrap();
    storage_server::startup_admission::PreparedService::open(&fixture.pool, &fixture.management)
        .unwrap();
    let engine = storage_engine::Engine::open(&fixture.pool).unwrap();
    assert!(engine.auth_state_ref().is_some());
}
