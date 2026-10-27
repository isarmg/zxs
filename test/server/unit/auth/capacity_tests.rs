//! Seed sessions directly so capacity tests do not hash dozens of passwords.
use super::*;

fn proof(store: &AuthStore) -> VerifiedLogin {
    let administrator = &store.database().unwrap().administrator;
    VerifiedLogin {
        username: administrator.username.clone(),
        password_hash: administrator.password_hash.clone(),
        generation: administrator.generation,
    }
}

fn seed(store: &mut AuthStore, index: usize, generation: u64, expires_at: u64) {
    store.database_mut().unwrap().sessions.insert(
        format!("{index:064x}"),
        Session {
            generation,
            expires_at,
        },
    );
}

fn seed_invalid(store: &mut AuthStore, count: usize) {
    for index in 0..count {
        let (generation, expires_at) = if index % 2 == 0 {
            (1, 0)
        } else {
            (0, u64::MAX)
        };
        seed(store, index, generation, expires_at);
    }
}

#[test]
fn expired_and_revoked_sessions_do_not_exhaust_capacity_after_restart() {
    let root = crate::tests::private_tempdir();
    let mut store = AuthStore::initialize(root.path(), "owner", "capacity-test-password").unwrap();
    seed_invalid(&mut store, MAX_SESSIONS);
    store.persist(false).unwrap();
    drop(store);
    let mut store = AuthStore::open(root.path()).unwrap();
    let login = store.finish_login(proof(&store)).unwrap();
    assert_eq!(store.database().unwrap().sessions.len(), 1);
    drop(store);
    let store = AuthStore::open(root.path()).unwrap();
    assert_eq!(store.database().unwrap().sessions.len(), 1);
    assert!(store.authenticate(&login.token).is_ok());
}

#[test]
fn the_single_session_limit_is_enforced_without_mutating_state() {
    let root = crate::tests::private_tempdir();
    let mut store = AuthStore::initialize(root.path(), "owner", "capacity-test-password").unwrap();
    let active = store.finish_login(proof(&store)).unwrap();
    for index in 0..MAX_SESSIONS - 1 {
        seed(&mut store, index, 1, u64::MAX);
    }
    store.persist(false).unwrap();
    let before = store.snapshot().unwrap();
    let error = store.finish_login(proof(&store)).err().unwrap();
    assert_eq!(error.status, 429);
    assert_eq!(
        error.code,
        storage_contract::ErrorCode::ResourceLimitExceeded
    );
    assert!(error.retryable);
    assert_eq!(store.snapshot().unwrap(), before);
    store
        .change_password(
            &active.token,
            "capacity-test-password",
            "rotated-capacity-password",
        )
        .unwrap();
    assert!(store.database().unwrap().sessions.is_empty());
    assert!(store.authenticate(&active.token).is_err());
    let login = store.finish_login(proof(&store)).unwrap();
    drop(store);
    let store = AuthStore::open(root.path()).unwrap();
    assert_eq!(store.database().unwrap().sessions.len(), 1);
    assert!(store.authenticate(&login.token).is_ok());
}

#[test]
fn failed_login_password_rotation_and_logout_restore_exact_sessions() {
    for mutation in ["login", "password", "logout"] {
        let root = crate::tests::private_tempdir();
        let mut store =
            AuthStore::initialize(root.path(), "owner", "capacity-test-password").unwrap();
        let active = store.finish_login(proof(&store)).unwrap();
        seed_invalid(&mut store, 4);
        store.persist(false).unwrap();
        let before = store.snapshot().unwrap();
        let disk = fs::read(root.path().join("auth.json")).unwrap();
        store.backup = Some(Arc::new(|_| {
            Err(ApiError::conflict(
                storage_contract::ErrorCode::CommitPreparationBusy,
                "injected persistence failure",
            ))
        }));
        let error = match mutation {
            "login" => store.finish_login(proof(&store)).err().unwrap(),
            "password" => store
                .change_password(
                    &active.token,
                    "capacity-test-password",
                    "rotated-capacity-password",
                )
                .unwrap_err(),
            "logout" => store.logout(&active.token).unwrap_err(),
            _ => unreachable!(),
        };
        assert_eq!(
            error.code,
            storage_contract::ErrorCode::CommitPreparationBusy
        );
        assert_eq!(store.snapshot().unwrap(), before);
        assert_eq!(fs::read(root.path().join("auth.json")).unwrap(), disk);
        assert!(store.authenticate(&active.token).is_ok());
        drop(store);
        let store = AuthStore::open(root.path()).unwrap();
        assert_eq!(store.snapshot().unwrap(), before);
        assert!(store.authenticate(&active.token).is_ok());
    }
}

#[test]
fn successful_password_rotation_clears_active_expired_and_revoked_sessions() {
    let root = crate::tests::private_tempdir();
    let mut store = AuthStore::initialize(root.path(), "owner", "capacity-test-password").unwrap();
    let first = store.finish_login(proof(&store)).unwrap();
    let second = store.finish_login(proof(&store)).unwrap();
    seed_invalid(&mut store, 4);
    store
        .change_password(
            &first.token,
            "capacity-test-password",
            "rotated-capacity-password",
        )
        .unwrap();
    assert!(store.database().unwrap().sessions.is_empty());
    assert!(store.authenticate(&first.token).is_err());
    assert!(store.authenticate(&second.token).is_err());
    drop(store);
    assert!(
        AuthStore::open(root.path())
            .unwrap()
            .database()
            .unwrap()
            .sessions
            .is_empty()
    );
}

#[test]
fn exhausted_generation_rejects_password_change_without_revivifying_sessions() {
    let root = crate::tests::private_tempdir();
    let mut store = AuthStore::initialize(root.path(), "owner", "capacity-test-password").unwrap();
    store.database_mut().unwrap().administrator.generation = u64::MAX;
    let login = store.finish_login(proof(&store)).unwrap();
    let revoked = primitives::random_token().unwrap();
    store.database_mut().unwrap().sessions.insert(
        primitives::token_hash_hex(&revoked),
        Session {
            generation: 1,
            expires_at: u64::MAX,
        },
    );
    seed(&mut store, 0, u64::MAX, 0);
    store.persist(false).unwrap();
    drop(store);
    let mut store = AuthStore::open(root.path()).unwrap();
    let before = store.snapshot().unwrap();
    let disk = fs::read(root.path().join("auth.json")).unwrap();
    let wrong_password = store
        .change_password(
            &login.token,
            "incorrect-capacity-password",
            "rotated-capacity-password",
        )
        .unwrap_err();
    assert_eq!(wrong_password.status, 403);
    assert_eq!(
        wrong_password.code,
        storage_contract::ErrorCode::CurrentPasswordInvalid
    );
    let error = store
        .change_password(
            &login.token,
            "capacity-test-password",
            "rotated-capacity-password",
        )
        .unwrap_err();
    assert_eq!(error.status, 409);
    assert_eq!(error.code, storage_contract::ErrorCode::AuthStateInvalid);
    assert!(!error.retryable);
    assert!(error.source.is_none());
    assert_eq!(error.message, "administrator generation has been exhausted");
    assert_eq!(store.snapshot().unwrap(), before);
    assert_eq!(fs::read(root.path().join("auth.json")).unwrap(), disk);
    assert!(store.authenticate(&login.token).is_ok());
    assert_eq!(store.authenticate(&revoked).unwrap_err().status, 401);
    drop(store);
    let store = AuthStore::open(root.path()).unwrap();
    assert_eq!(store.snapshot().unwrap(), before);
    assert!(store.authenticate(&login.token).is_ok());
    assert_eq!(store.authenticate(&revoked).unwrap_err().status, 401);
}

#[test]
fn final_available_generation_still_rotates_and_revokes_old_sessions() {
    let root = crate::tests::private_tempdir();
    let mut store = AuthStore::initialize(root.path(), "owner", "capacity-test-password").unwrap();
    store.database_mut().unwrap().administrator.generation = u64::MAX - 1;
    let login = store.finish_login(proof(&store)).unwrap();
    store
        .change_password(
            &login.token,
            "capacity-test-password",
            "rotated-capacity-password",
        )
        .unwrap();
    assert_eq!(store.database().unwrap().administrator.generation, u64::MAX);
    assert!(store.database().unwrap().sessions.is_empty());
    assert_eq!(store.authenticate(&login.token).unwrap_err().status, 401);
    drop(store);
    let mut store = AuthStore::open(root.path()).unwrap();
    assert_eq!(store.database().unwrap().administrator.generation, u64::MAX);
    assert!(store.database().unwrap().sessions.is_empty());
    assert_eq!(
        store
            .login("owner", "capacity-test-password")
            .err()
            .unwrap()
            .status,
        401
    );
    assert!(store.login("owner", "rotated-capacity-password").is_ok());
}
