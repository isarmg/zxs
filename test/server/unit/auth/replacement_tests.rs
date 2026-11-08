use super::*;

const OLD: &str = "old-password-for-reset-2026";
const NEW: &str = "new-password-for-reset-2026";
fn store(root: &Path) -> AuthStore {
    AuthStore::initialize_at(root, "admin", OLD, true).unwrap()
}

#[test]
fn prepared_reset_revokes_sessions_once_and_resolves_exact_published_state() {
    let root = tempfile::tempdir().unwrap();
    let mut auth = store(root.path());
    let session = auth.login("admin", OLD).unwrap();
    let before = auth.credential_revision().unwrap();
    let change = auth.prepare_system_password_replacement(NEW).unwrap();
    assert!(before == auth.credential_revision().unwrap());
    assert!(auth.authenticate(&session.token).is_ok());
    assert!(matches!(
        auth.apply_system_password_replacement(&change).unwrap(),
        PasswordReplacementOutcome::Published
    ));
    assert!(auth.authenticate(&session.token).is_err());
    assert_eq!(
        auth.credential_revision().unwrap().generation(),
        before.generation() + 1
    );
    assert!(matches!(
        auth.apply_system_password_replacement(&change).unwrap(),
        PasswordReplacementOutcome::ExistingDurablyVerified
    ));
    assert!(auth.login("admin", OLD).is_err());
    auth.login("admin", NEW).unwrap();
    // Later sessions are new authority. Replaying the old operation cannot erase them.
    assert!(auth.apply_system_password_replacement(&change).is_err());
}

#[test]
fn retained_replacement_reopens_without_another_generation_and_never_retains_plaintext() {
    let root = tempfile::tempdir().unwrap();
    let mut auth = store(root.path());
    let change = auth.prepare_system_password_replacement(NEW).unwrap();
    let bytes = json_codec::to_vec(&change).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(!text.contains(NEW));
    assert!(!text.contains(OLD));
    auth.apply_system_password_replacement(&change).unwrap();
    drop(auth);
    let mut reopened = AuthStore::open_system_management_directory(
        PrivateDirectory::open_trusted(root.path()).unwrap(),
    )
    .unwrap();
    let retained: PreparedPasswordReplacement = json_codec::from_slice(&bytes).unwrap();
    assert!(retained.verifies_repeated_password(NEW).unwrap());
    assert!(!retained.verifies_repeated_password(OLD).unwrap());
    assert!(matches!(
        reopened
            .apply_system_password_replacement(&retained)
            .unwrap(),
        PasswordReplacementOutcome::ExistingDurablyVerified
    ));
    assert_eq!(reopened.credential_revision().unwrap().generation(), 2);
}

#[test]
fn stale_or_nonmanagement_replacement_cannot_overwrite_authority() {
    let root = tempfile::tempdir().unwrap();
    let mut auth = store(root.path());
    let first = auth.prepare_system_password_replacement(NEW).unwrap();
    let second = auth
        .prepare_system_password_replacement("different-new-password-2026")
        .unwrap();
    auth.apply_system_password_replacement(&second).unwrap();
    let before = auth.credential_revision().unwrap();
    assert!(auth.apply_system_password_replacement(&first).is_err());
    assert!(before == auth.credential_revision().unwrap());
    let other = tempfile::tempdir().unwrap();
    let legacy = AuthStore::initialize(other.path(), "admin", OLD).unwrap();
    assert!(legacy.prepare_system_password_replacement(NEW).is_err());
}

#[test]
fn management_open_does_not_initialize_missing_credentials_or_accept_malformed_replacement() {
    let root = tempfile::tempdir().unwrap();
    assert!(
        AuthStore::open_system_management_directory(
            PrivateDirectory::open_trusted(root.path()).unwrap()
        )
        .is_err()
    );
    assert!(!root.path().join("management-auth.json").exists());
    let auth = store(root.path());
    let change = auth.prepare_system_password_replacement(NEW).unwrap();
    let mut value = json_codec::to_value(&change).unwrap();
    value["replacement"]["administrator"]["generation"] = "1".into();
    let malformed: PreparedPasswordReplacement = json_codec::from_value(value).unwrap();
    assert!(malformed.replacement_revision().is_err());
}
