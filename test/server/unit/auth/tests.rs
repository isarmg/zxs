use super::*;

#[test]
fn the_initialized_username_is_the_only_identity_and_cannot_be_reinitialized() {
    let root = crate::tests::private_tempdir();
    let mut store =
        AuthStore::initialize(root.path(), " Owner.Ops ", "a-long-test-password").unwrap();
    let login = store.login("OWNER.OPS", "a-long-test-password").unwrap();
    assert_eq!(login.principal, "owner.ops");
    let identity = store.authenticate(&login.token).unwrap();
    assert_eq!(
        json_codec::to_value(&identity).unwrap(),
        json_codec::json!({"principal":"owner.ops"})
    );
    let public_login = json_codec::to_value(&login).unwrap();
    assert!(public_login.get("admin").is_none());
    assert!(public_login.get("repositories").is_none());
    for name in ["admin", "reader", "anonymous", "guest"] {
        assert!(store.login(name, "a-long-test-password").is_err());
        assert!(store.identity_for_principal(name).is_err());
    }
    let snapshot = store.snapshot().unwrap();
    assert!(snapshot.get("accounts").is_none());
    assert_eq!(snapshot["administrator"]["username"], "owner.ops");
    assert_eq!(snapshot["administrator"].as_object().unwrap().len(), 3);
    drop(store);
    let before = fs::read(root.path().join("auth.json")).unwrap();
    assert_eq!(
        AuthStore::initialize(root.path(), "another", "another-test-password")
            .err()
            .unwrap()
            .code,
        storage_contract::ErrorCode::AuthAlreadyInitialized
    );
    assert_eq!(fs::read(root.path().join("auth.json")).unwrap(), before);
}

#[test]
fn password_rotation_preserves_the_single_identity_revokes_every_session_and_survives_reopen() {
    let root = crate::tests::private_tempdir();
    let mut store =
        AuthStore::initialize(root.path(), "owner", "administrator-old-password").unwrap();
    let first = store.login("owner", "administrator-old-password").unwrap();
    let second = store.login("owner", "administrator-old-password").unwrap();
    let queued = store.authenticate(&second.token).unwrap();
    store
        .change_password(
            &first.token,
            "administrator-old-password",
            "administrator-new-password",
        )
        .unwrap();
    for token in [&first.token, &second.token] {
        assert_eq!(
            store.authenticate(token).unwrap_err().code,
            storage_contract::ErrorCode::Unauthorized
        );
    }
    assert!(store.revalidate_identity(&queued).is_err());
    assert!(store.login("owner", "administrator-old-password").is_err());
    assert!(store.database().unwrap().sessions.is_empty());
    drop(store);
    let mut store = AuthStore::open(root.path()).unwrap();
    let login = store.login("owner", "administrator-new-password").unwrap();
    assert_eq!(store.authenticate(&login.token).unwrap().principal, "owner");
    assert!(store.login("admin", "administrator-new-password").is_err());
}

#[test]
fn rejected_password_rotation_keeps_credentials_and_session_intact() {
    let root = crate::tests::private_tempdir();
    let mut store = AuthStore::initialize(root.path(), "admin", "a-long-test-password").unwrap();
    let login = store.login("admin", "a-long-test-password").unwrap();
    let before = store.snapshot().unwrap();
    assert_eq!(
        store
            .change_password(&login.token, "incorrect-password", "a-new-test-password")
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::CurrentPasswordInvalid
    );
    assert!(
        store
            .change_password(&login.token, "a-long-test-password", "short")
            .is_err()
    );
    assert_eq!(store.snapshot().unwrap(), before);
    assert!(store.authenticate(&login.token).is_ok());
    drop(store);
    let mut store = AuthStore::open(root.path()).unwrap();
    assert!(store.login("admin", "a-long-test-password").is_ok());
}

#[test]
fn missing_local_credentials_restore_only_from_authoritative_snapshot() {
    let root = crate::tests::private_tempdir();
    let mut store = AuthStore::initialize(root.path(), "admin", "a-long-test-password").unwrap();
    let login = store.login("admin", "a-long-test-password").unwrap();
    let snapshot = store.snapshot().unwrap();
    drop(store);
    fs::remove_file(root.path().join("auth.json")).unwrap();
    assert!(AuthStore::open(root.path()).is_err());
    assert!(!root.path().join("auth.json").exists());
    let restored = AuthStore::open_with_snapshot(root.path(), Some(snapshot)).unwrap();
    assert!(restored.authenticate(&login.token).is_ok());
}

#[test]
fn legacy_and_malformed_authorities_are_rejected_without_replacing_credentials() {
    let root = crate::tests::private_tempdir();
    let store = AuthStore::initialize(root.path(), "owner", "a-long-test-password").unwrap();
    let snapshot = store.snapshot().unwrap();
    drop(store);
    let path = root.path().join("auth.json");
    let previous = fs::read(&path).unwrap();
    let mut legacy = snapshot["administrator"].clone();
    legacy["admin"] = json_codec::json!(true);
    legacy["repositories"] = json_codec::json!([]);
    legacy["disabled"] = json_codec::json!(false);
    let mut cases = vec![
        json_codec::json!({"accounts":{"owner":legacy.clone()},"sessions":{}}),
        json_codec::json!({"accounts":{"owner":legacy.clone(),"reader":legacy},"sessions":{}}),
        json_codec::json!({"administrator":null,"sessions":{}}),
        json_codec::json!({"administrator":[snapshot["administrator"].clone()],"sessions":{}}),
        json_codec::json!({"sessions":{}}),
        json_codec::json!([snapshot["administrator"].clone(), {}]),
        json_codec::json!({"administrator":["owner",snapshot["administrator"]["password_hash"].clone(),"1"],"sessions":{}}),
    ];
    for (key, value) in [
        ("username", json_codec::json!("Owner")),
        ("password_hash", json_codec::json!("not-a-password-hash")),
        ("generation", json_codec::json!("0")),
        ("admin", json_codec::json!(false)),
        ("repositories", json_codec::json!(["repo-private"])),
        ("disabled", json_codec::json!(true)),
    ] {
        let mut invalid = snapshot.clone();
        invalid["administrator"][key] = value;
        cases.push(invalid);
    }
    let mut extra = snapshot.clone();
    extra["accounts"] = json_codec::json!({});
    cases.push(extra);
    let mut invalid_session = snapshot.clone();
    invalid_session["sessions"] =
        json_codec::json!({"invalid-token-hash":{"generation":"1","expires_at":"1"}});
    cases.push(invalid_session);
    let mut legacy_session = snapshot.clone();
    legacy_session["sessions"]["0".repeat(64)] =
        json_codec::json!({"username":"reader","generation":"1","expires_at":"1"});
    cases.push(legacy_session);
    let mut positional_session = snapshot.clone();
    positional_session["sessions"]["0".repeat(64)] = json_codec::json!(["1", "1"]);
    cases.push(positional_session);
    let mut too_many_sessions = snapshot.clone();
    for index in 0..=MAX_SESSIONS {
        too_many_sessions["sessions"][format!("{index:064x}")] =
            json_codec::json!({"generation":"1","expires_at":"1"});
    }
    cases.push(too_many_sessions);
    for invalid in cases {
        #[cfg(feature = "core")]
        assert!(AdmissionSnapshot::from_snapshot(&invalid).is_err());
        assert!(AuthStore::open_with_snapshot(root.path(), Some(invalid.clone())).is_err());
        assert_eq!(fs::read(&path).unwrap(), previous);
        fs::write(&path, json_codec::to_vec(&invalid).unwrap()).unwrap();
        let rejected = fs::read(&path).unwrap();
        assert!(AuthStore::open(root.path()).is_err());
        assert_eq!(fs::read(&path).unwrap(), rejected);
        fs::write(&path, &previous).unwrap();
    }
    assert!(AuthStore::open(root.path()).is_ok());
}

#[cfg(feature = "core")]
#[test]
fn admission_snapshot_projects_only_the_configured_administrator() {
    let root = crate::tests::private_tempdir();
    let store = AuthStore::initialize(root.path(), "owner", "a-long-test-password").unwrap();
    let admission = AdmissionSnapshot::from_snapshot(&store.snapshot().unwrap()).unwrap();
    assert_eq!(
        admission
            .revalidate_identity(&store.sole_identity().unwrap())
            .unwrap()
            .principal,
        "owner"
    );
    for name in ["admin", "reader", "anonymous", "guest", "Owner"] {
        assert!(
            admission
                .revalidate_identity(&Identity {
                    principal: name.into(),
                    admission_session: None,
                    smb_binding: None
                })
                .is_err()
        );
    }
}

#[test]
fn uncertain_backup_blocks_authentication_and_queued_password_changes_until_recovery() {
    let root = crate::tests::private_tempdir();
    let mut store = AuthStore::initialize(root.path(), "admin", "a-long-test-password").unwrap();
    let login = store.login("admin", "a-long-test-password").unwrap();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = calls.clone();
    store
        .attach_backup(Arc::new(move |_| {
            if count.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(())
            } else {
                Err(ApiError::new(
                    ::http::StatusCode::SERVICE_UNAVAILABLE,
                    storage_contract::ErrorCode::CommitInDoubt,
                    "publication interrupted",
                ))
            }
        }))
        .unwrap();
    assert!(store.login("admin", "a-long-test-password").is_err());
    assert_eq!(
        store.identity_for_principal("admin").unwrap_err().code,
        storage_contract::ErrorCode::RecoveryRequired
    );
    let before = store.snapshot().unwrap();
    assert_eq!(
        store
            .change_password(&login.token, "a-long-test-password", "a-new-test-password")
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::RecoveryRequired
    );
    assert_eq!(store.snapshot().unwrap(), before);
}

#[test]
fn detached_login_verification_cannot_cross_password_changes_or_recovery() {
    let root = crate::tests::private_tempdir();
    let mut store =
        AuthStore::initialize(root.path(), "owner", "administrator-old-password").unwrap();
    let login = store.login("owner", "administrator-old-password").unwrap();
    let proof = store
        .login_challenge("owner")
        .unwrap()
        .verify("administrator-old-password")
        .unwrap();
    store
        .change_password(
            &login.token,
            "administrator-old-password",
            "administrator-new-password",
        )
        .unwrap();
    assert_eq!(store.finish_login(proof).err().unwrap().status, 401);
    let proof = store
        .login_challenge("owner")
        .unwrap()
        .verify("administrator-new-password")
        .unwrap();
    store.blocked.store(true, Ordering::SeqCst);
    assert_eq!(
        store.finish_login(proof).err().unwrap().code,
        storage_contract::ErrorCode::RecoveryRequired
    );
    assert!(store.db.as_ref().unwrap().sessions.is_empty());
}

#[test]
fn detached_login_challenge_rejects_unknown_names_and_wrong_passwords() {
    let root = crate::tests::private_tempdir();
    let mut store = AuthStore::initialize(root.path(), "owner", "a-long-test-password").unwrap();
    for username in ["unknown", "admin", "guest", "", "bad name"] {
        assert!(
            store
                .login_challenge(username)
                .unwrap()
                .verify("a-long-test-password")
                .is_err()
        );
    }
    assert!(
        store
            .login_challenge("owner")
            .unwrap()
            .verify("wrong-password")
            .is_err()
    );
    let proof = store
        .login_challenge(" Owner ")
        .unwrap()
        .verify("a-long-test-password")
        .unwrap();
    let login = store.finish_login(proof).unwrap();
    assert_eq!(store.authenticate(&login.token).unwrap().principal, "owner");
}

#[test]
fn identity_refresh_preserves_internal_smb_binding_without_serializing_it() {
    let root = crate::tests::private_tempdir();
    let store = AuthStore::initialize(root.path(), "admin", "a-long-test-password").unwrap();
    let mut caller = store.identity_for_principal("admin").unwrap();
    caller.smb_binding = Some(SmbBinding {
        repository_id: "repository-one".into(),
        branch_id: "branch-one".into(),
        generation: 7,
    });
    let current = store.revalidate_identity(&caller).unwrap();
    assert_eq!(current.smb_binding, caller.smb_binding);
    assert_eq!(
        json_codec::to_value(&current).unwrap(),
        json_codec::json!({"principal":"admin"})
    );
    caller.principal = "reader".into();
    assert!(store.revalidate_identity(&caller).is_err());
}

#[test]
fn logout_revokes_only_the_selected_session_and_queued_identity() {
    let root = crate::tests::private_tempdir();
    let mut store = AuthStore::initialize(root.path(), "owner", "a-long-test-password").unwrap();
    let first = store.login("owner", "a-long-test-password").unwrap();
    let second = store.login("owner", "a-long-test-password").unwrap();
    let queued = store.authenticate(&first.token).unwrap();
    store.logout(&first.token).unwrap();
    assert!(store.authenticate(&first.token).is_err());
    assert!(store.revalidate_identity(&queued).is_err());
    assert!(store.authenticate(&second.token).is_ok());
    drop(store);
    let store = AuthStore::open(root.path()).unwrap();
    assert!(store.authenticate(&first.token).is_err());
    assert!(store.authenticate(&second.token).is_ok());
}

#[cfg(unix)]
#[test]
fn credential_lock_rejects_symlinks_hardlinks_and_public_files() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    for kind in ["symlink", "hardlink", "public"] {
        let root = crate::tests::private_tempdir();
        let target = root.path().join("unrelated");
        fs::write(&target, b"must remain unchanged").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        let path = root.path().join("auth.lock");
        match kind {
            "symlink" => symlink(&target, &path).unwrap(),
            "hardlink" => fs::hard_link(&target, &path).unwrap(),
            "public" => {
                fs::write(&path, b"").unwrap();
                fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            }
            _ => unreachable!(),
        }
        assert!(AuthStore::acquire_lock(root.path()).is_err(), "{kind}");
        assert_eq!(fs::read(&target).unwrap(), b"must remain unchanged");
    }
}

#[test]
fn credential_reader_enforces_limit_on_open_descriptor() {
    let root = crate::tests::private_tempdir();
    let path = root.path().join("credential");
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(&path)
        .unwrap()
        .set_len(MAX_STORE_BYTES as u64 + 1)
        .unwrap();
    assert!(read_credentials(&path).is_err());
}

#[cfg(unix)]
#[test]
fn credential_control_files_reject_fifos_without_waiting_for_a_peer() {
    use rustix::fs::{CWD, Mode, mkfifoat};
    let root = crate::tests::private_tempdir();
    mkfifoat(
        CWD,
        root.path().join("auth.lock"),
        Mode::from_raw_mode(0o600),
    )
    .unwrap();
    assert!(AuthStore::acquire_lock(root.path()).is_err());
    let credential = root.path().join("auth.json");
    mkfifoat(CWD, &credential, Mode::from_raw_mode(0o600)).unwrap();
    assert!(read_credentials(&credential).is_err());
}

#[cfg(unix)]
#[test]
fn credential_initialization_creates_private_roots_but_never_repairs_existing_ones() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let parent = crate::tests::private_tempdir();
    let root = parent.path().join("new").join("nested");
    drop(AuthStore::initialize(&root, "admin", "a-long-test-password").unwrap());
    for path in [&root, root.parent().unwrap()] {
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    let public = parent.path().join("public");
    fs::create_dir(&public).unwrap();
    fs::set_permissions(&public, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(AuthStore::initialize(&public, "admin", "a-long-test-password").is_err());
    assert_eq!(
        fs::metadata(&public).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert!(!public.join("auth.lock").exists());
    let alias = parent.path().join("alias");
    symlink(&root, &alias).unwrap();
    assert!(AuthStore::open(&alias).is_err());
}

#[test]
fn credential_store_supports_relative_private_root() {
    let mut builder = tempfile::Builder::new();
    builder.prefix("auth-relative-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    let root = builder.tempdir_in(".").unwrap();
    let path = root
        .path()
        .strip_prefix(std::env::current_dir().unwrap())
        .unwrap_or(root.path());
    let store = AuthStore::initialize(path, "admin", "a-long-test-password").unwrap();
    drop(store);
    assert!(AuthStore::open(path).is_ok());
}

#[cfg(unix)]
#[test]
fn credential_reopen_rejects_replaceable_ancestry_without_changing_it() {
    use std::os::unix::fs::PermissionsExt;
    let parent = crate::tests::private_tempdir();
    let root = parent.path().join("private");
    drop(AuthStore::initialize(&root, "admin", "a-long-test-password").unwrap());
    fs::set_permissions(parent.path(), fs::Permissions::from_mode(0o777)).unwrap();
    assert!(AuthStore::open(&root).is_err());
    assert_eq!(
        fs::metadata(parent.path()).unwrap().permissions().mode() & 0o777,
        0o777
    );
}

#[cfg(feature = "core")]
#[test]
fn unavailable_control_plane_never_invents_a_default_administrator() {
    let root = crate::tests::private_tempdir();
    let mut store = AuthStore::unavailable(root.path());
    assert!(!store.is_available());
    assert!(store.snapshot().is_err());
    assert!(store.sole_identity().is_err());
    assert!(store.identity_for_principal("admin").is_err());
    assert!(store.login("admin", "a-long-test-password").is_err());
    assert!(!root.path().join("management-auth.json").exists());
}

#[cfg(feature = "core")]
#[test]
fn snapshot_admission_preserves_binding_and_rechecks_session_revocation() {
    let root = crate::tests::private_tempdir();
    let mut store = AuthStore::initialize(root.path(), "owner", "a-long-test-password").unwrap();
    let login = store.login("owner", "a-long-test-password").unwrap();
    let mut caller = store.authenticate(&login.token).unwrap();
    caller.smb_binding = Some(SmbBinding {
        repository_id: "repository-one".into(),
        branch_id: "branch-one".into(),
        generation: 2,
    });
    let admission = AdmissionSnapshot::from_snapshot(&store.snapshot().unwrap()).unwrap();
    let refreshed = admission.revalidate_identity(&caller).unwrap();
    assert_eq!(refreshed.smb_binding, caller.smb_binding);
    assert_eq!(refreshed.admission_session, caller.admission_session);
    store.logout(&login.token).unwrap();
    let admission = AdmissionSnapshot::from_snapshot(&store.snapshot().unwrap()).unwrap();
    assert!(admission.revalidate_identity(&caller).is_err());
    let login = store.login("owner", "a-long-test-password").unwrap();
    let caller = store.authenticate(&login.token).unwrap();
    store
        .change_password(
            &login.token,
            "a-long-test-password",
            "rotated-test-password",
        )
        .unwrap();
    let admission = AdmissionSnapshot::from_snapshot(&store.snapshot().unwrap()).unwrap();
    assert!(admission.revalidate_identity(&caller).is_err());
}
