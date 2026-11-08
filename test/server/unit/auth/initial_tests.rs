use super::*;
const PASSWORD: &str = "first-management-password-2026";
#[test]
fn first_candidate_uses_current_policy_and_resolves_exact_creation() {
    let root = tempfile::tempdir().unwrap();
    let candidate = PreparedInitialAdministrator::new("admin", PASSWORD).unwrap();
    let encoded = json_codec::to_vec(&candidate).unwrap();
    assert!(!std::str::from_utf8(&encoded).unwrap().contains(PASSWORD));
    let (mut auth, outcome) = AuthStore::create_initial_system_management(
        PrivateDirectory::open_trusted(root.path()).unwrap(),
        &candidate,
    )
    .unwrap();
    assert!(matches!(outcome, InitialManagementOutcome::Created));
    assert_eq!(auth.credential_revision().unwrap().generation(), 1);
    assert!(candidate.verifies_repeated_password(PASSWORD).unwrap());
    assert!(
        !candidate
            .verifies_repeated_password("different-password-2026")
            .unwrap()
    );
    drop(auth);
    let retained: PreparedInitialAdministrator = json_codec::from_slice(&encoded).unwrap();
    let (reopened, outcome) = AuthStore::create_initial_system_management(
        PrivateDirectory::open_trusted(root.path()).unwrap(),
        &retained,
    )
    .unwrap();
    assert!(matches!(
        outcome,
        InitialManagementOutcome::ExistingDurablyVerified
    ));
    auth = reopened;
    auth.login("admin", PASSWORD).unwrap();
    drop(auth);
    // An initial request never erases a later login or rewrites authority.
    assert!(
        AuthStore::create_initial_system_management(
            PrivateDirectory::open_trusted(root.path()).unwrap(),
            &retained
        )
        .is_err()
    );
}
#[test]
fn different_initial_account_or_password_never_overwrites_existing_credentials() {
    let root = tempfile::tempdir().unwrap();
    let candidate = PreparedInitialAdministrator::new("admin", PASSWORD).unwrap();
    let (auth, _) = AuthStore::create_initial_system_management(
        PrivateDirectory::open_trusted(root.path()).unwrap(),
        &candidate,
    )
    .unwrap();
    drop(auth);
    let before = std::fs::read(root.path().join("management-auth.json")).unwrap();
    for (name, password) in [
        ("other-admin", PASSWORD),
        ("admin", "different-password-2026"),
    ] {
        let different = PreparedInitialAdministrator::new(name, password).unwrap();
        assert!(
            AuthStore::create_initial_system_management(
                PrivateDirectory::open_trusted(root.path()).unwrap(),
                &different
            )
            .is_err()
        );
        assert_eq!(
            std::fs::read(root.path().join("management-auth.json")).unwrap(),
            before
        );
    }
}
#[test]
fn retained_initial_state_requires_generation_one_and_no_sessions() {
    let candidate = PreparedInitialAdministrator::new("admin", PASSWORD).unwrap();
    let mut value = json_codec::to_value(&candidate).unwrap();
    value["database"]["administrator"]["generation"] = "2".into();
    let invalid: PreparedInitialAdministrator = json_codec::from_value(value).unwrap();
    assert!(invalid.credential_revision().is_err());
}
