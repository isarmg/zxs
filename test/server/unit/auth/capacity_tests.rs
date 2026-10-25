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
    assert_eq!(store.finish_login(proof(&store)).err().unwrap().status, 429);
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
    let before = store.snapshot().unwrap();
    let disk = fs::read(root.path().join("auth.json")).unwrap();
    assert_eq!(
        store
            .change_password(
                &login.token,
                "capacity-test-password",
                "rotated-capacity-password"
            )
            .unwrap_err()
            .status,
        429
    );
    assert_eq!(store.snapshot().unwrap(), before);
    assert_eq!(fs::read(root.path().join("auth.json")).unwrap(), disk);
    assert!(store.authenticate(&login.token).is_ok());
}
