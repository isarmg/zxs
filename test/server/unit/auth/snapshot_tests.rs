//! Committed reads are independent of the serialized credential writer.
use super::*;
use std::sync::{Mutex, mpsc};
use std::time::Duration;

const PASSWORD: &str = "committed-auth-snapshot-password";

#[test]
fn pending_login_is_invisible_without_blocking_committed_reads() {
    let root = crate::tests::private_tempdir();
    let mut store = AuthStore::initialize(root.path(), "owner", PASSWORD).unwrap();
    let first = store.login("owner", PASSWORD).unwrap();
    let reader = store.reader();
    let first_identity = reader.authenticate(&first.token).unwrap();
    let verified = store
        .login_challenge("owner")
        .unwrap()
        .verify(PASSWORD)
        .unwrap();
    let (entered, candidate) = mpsc::channel();
    let (resume, wait) = mpsc::channel();
    let wait = Mutex::new(wait);
    // The production backup interface runs after staging sync, before publication.
    store.backup = Some(Arc::new(move |snapshot| {
        entered.send(snapshot).unwrap();
        wait.lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        Ok(())
    }));
    let writer = std::thread::spawn(move || {
        let result = store.finish_login(verified);
        (store, result)
    });
    let candidate = candidate.recv_timeout(Duration::from_secs(10)).unwrap();
    let pending_key = candidate["sessions"]
        .as_object()
        .unwrap()
        .keys()
        .find(|key| Some(key.as_str()) != first_identity.admission_session.as_deref())
        .unwrap()
        .clone();
    let pending = Identity {
        principal: "owner".into(),
        admission_session: Some(pending_key),
        smb_binding: None,
    };
    let existing = reader.authenticate(&first.token);
    let unpublished = reader.revalidate_identity(&pending);
    resume.send(()).unwrap();
    let (store, result) = writer.join().unwrap();
    assert!(existing.is_ok());
    assert_eq!(
        unpublished.unwrap_err().code,
        storage_contract::ErrorCode::Unauthorized
    );
    let second = result.unwrap();
    assert!(reader.authenticate(&second.token).is_ok());
    assert!(reader.revalidate_identity(&pending).is_ok());
    drop(store);
    assert_eq!(
        reader.authenticate(&first.token).unwrap_err().code,
        storage_contract::ErrorCode::Unauthorized
    );
}

#[test]
fn failed_publication_never_exposes_candidate_and_preserves_failure_source() {
    for uncertain in [false, true] {
        let root = crate::tests::private_tempdir();
        let mut store = AuthStore::initialize(root.path(), "owner", PASSWORD).unwrap();
        let login = store.login("owner", PASSWORD).unwrap();
        let reader = store.reader();
        let before = fs::read(root.path().join("auth.json")).unwrap();
        store.backup = Some(Arc::new(move |_| {
            Err(ApiError::new(
                ::http::StatusCode::CONFLICT,
                if uncertain {
                    storage_contract::ErrorCode::CommitInDoubt
                } else {
                    storage_contract::ErrorCode::CommitPreparationBusy
                },
                "backup unavailable",
            )
            .with_source(std::io::Error::other("backup device failed")))
        }));
        let error = store.logout(&login.token).unwrap_err();
        assert!(std::error::Error::source(&error).is_some());
        assert_eq!(fs::read(root.path().join("auth.json")).unwrap(), before);
        if uncertain {
            assert_eq!(
                reader.authenticate(&login.token).unwrap_err().code,
                storage_contract::ErrorCode::RecoveryRequired
            );
        } else {
            assert!(reader.authenticate(&login.token).is_ok());
        }
    }
}

#[test]
fn committed_revocations_refresh_existing_readers() {
    let root = crate::tests::private_tempdir();
    let mut store = AuthStore::initialize(root.path(), "owner", PASSWORD).unwrap();
    let reader = store.reader();
    let first = store.login("owner", PASSWORD).unwrap();
    let identity = reader.authenticate(&first.token).unwrap();
    store.logout(&first.token).unwrap();
    assert_eq!(
        reader.revalidate_identity(&identity).unwrap_err().status,
        401
    );
    let second = store.login("owner", PASSWORD).unwrap();
    let identity = reader.authenticate(&second.token).unwrap();
    store
        .change_password(&second.token, PASSWORD, "replacement-snapshot-password")
        .unwrap();
    assert_eq!(
        reader.revalidate_identity(&identity).unwrap_err().status,
        401
    );
}

#[cfg(feature = "core")]
#[test]
fn setup_and_recovery_keep_existing_reader_connected_to_new_authority() {
    let root = crate::tests::private_tempdir();
    let mut store = AuthStore::unavailable(root.path());
    let reader = store.reader();
    assert_eq!(
        reader.authenticate("invalid").unwrap_err().code,
        storage_contract::ErrorCode::RecoveryRequired
    );
    let replacement = AuthStore::initialize_management(root.path(), "owner", PASSWORD).unwrap();
    store.replace_authority(replacement);
    let login = store.login("owner", PASSWORD).unwrap();
    assert!(reader.authenticate(&login.token).is_ok());
    let replacement_root = crate::tests::private_tempdir();
    let replacement =
        AuthStore::initialize_management(replacement_root.path(), "other", PASSWORD).unwrap();
    store.replace_authority(replacement);
    assert_eq!(reader.authenticate(&login.token).unwrap_err().status, 401);
    let new_login = store.login("other", PASSWORD).unwrap();
    assert!(reader.authenticate(&new_login.token).is_ok());
    drop(store);
    assert_eq!(
        reader.authenticate(&new_login.token).unwrap_err().status,
        401
    );
}

#[test]
fn in_flight_channel_reference_does_not_extend_authority_after_store_drop() {
    let root = crate::tests::private_tempdir();
    let mut store = AuthStore::initialize(root.path(), "owner", PASSWORD).unwrap();
    let login = store.login("owner", PASSWORD).unwrap();
    let reader = store.reader();
    // Model references briefly held inside another concurrent reader.
    let _committed = reader.committed.upgrade().unwrap();
    let _blocked = reader.blocked.upgrade().unwrap();
    drop(store);
    assert_eq!(
        reader.authenticate(&login.token).unwrap_err().code,
        storage_contract::ErrorCode::RecoveryRequired
    );
}

#[cfg(feature = "core")]
#[test]
fn failed_system_authority_rename_blocks_readers_without_publishing_revocation() {
    let root = crate::tests::private_tempdir();
    let mut store = AuthStore::initialize_management(root.path(), "owner", PASSWORD).unwrap();
    let login = store.login("owner", PASSWORD).unwrap();
    let reader = store.reader();
    let before = store.snapshot().unwrap();
    let durable = fs::read(&store.path).unwrap();
    // A real filesystem failure after temporary file sync, not a business hook.
    let obstruction = root.path().join("replacement-directory");
    fs::create_dir(&obstruction).unwrap();
    let original = std::mem::replace(&mut store.path, obstruction);
    let error = store.logout(&login.token).unwrap_err();
    assert!(matches!(
        error.source.as_deref(),
        Some(crate::http_boundary::ApiErrorSource::Io(_))
    ));
    assert_eq!(store.snapshot().unwrap(), before);
    assert_eq!(fs::read(&original).unwrap(), durable);
    assert_eq!(
        reader.authenticate(&login.token).unwrap_err().code,
        storage_contract::ErrorCode::RecoveryRequired
    );
    assert!(fs::read_dir(root.path()).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".tmp-")
    }));
}

#[test]
fn sole_identity_reads_committed_authority_while_writer_mutex_is_held() {
    let root = crate::tests::private_tempdir();
    let writer = Mutex::new(AuthStore::initialize(root.path(), "owner", PASSWORD).unwrap());
    let guard = writer.lock().unwrap();
    let reader = guard.reader();
    let (report, result) = mpsc::channel();
    let read = std::thread::spawn(move || report.send(reader.sole_identity()).unwrap());
    let identity = result.recv_timeout(Duration::from_secs(5));
    drop(guard);
    read.join().unwrap();
    let identity = identity
        .expect("committed reader must not wait for credential writer")
        .unwrap();
    assert_eq!(identity.principal, "owner");
    assert!(identity.admission_session.is_none());
    assert!(identity.smb_binding.is_none());
}

#[test]
fn sole_identity_rejects_blocked_and_dropped_authority_without_retaining_file_lock() {
    let root = crate::tests::private_tempdir();
    let store = AuthStore::initialize(root.path(), "owner", PASSWORD).unwrap();
    let reader = store.reader();
    assert_eq!(reader.sole_identity().unwrap().principal, "owner");
    store.blocked.store(true, Ordering::SeqCst);
    assert_eq!(
        reader.sole_identity().unwrap_err().code,
        storage_contract::ErrorCode::RecoveryRequired
    );
    drop(store);
    assert_eq!(
        reader.sole_identity().unwrap_err().code,
        storage_contract::ErrorCode::Unauthorized
    );
    // A surviving reader neither retains the OS lock nor adopts a new store.
    let replacement = AuthStore::open(root.path()).unwrap();
    assert_eq!(
        replacement.reader().sole_identity().unwrap().principal,
        "owner"
    );
    assert_eq!(
        reader.sole_identity().unwrap_err().code,
        storage_contract::ErrorCode::Unauthorized
    );
}

#[test]
fn sole_identity_rejects_store_drop_even_with_in_flight_channel_owners() {
    let root = crate::tests::private_tempdir();
    let store = AuthStore::initialize(root.path(), "owner", PASSWORD).unwrap();
    let reader = store.reader();
    let _committed = reader.committed.upgrade().unwrap();
    let _blocked = reader.blocked.upgrade().unwrap();
    drop(store);
    assert_eq!(
        reader.sole_identity().unwrap_err().code,
        storage_contract::ErrorCode::RecoveryRequired
    );
    let replacement = AuthStore::open(root.path()).unwrap();
    assert_eq!(replacement.sole_identity().unwrap().principal, "owner");
}
