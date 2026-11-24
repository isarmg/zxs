use super::{
    Principal, SelectedWorkspace, bound_origin, existing_basic_header,
    require_existing_basic_authority,
};
use std::sync::Arc;

#[test]
fn existing_basic_keeps_its_exact_local_owner_but_uses_the_current_verified_authority() {
    let stored = r#"{"origin":"https://example.invalid:443","account":"fixture","trust":"historical OS trust"}"#;
    let saved: Principal = serde_json::from_str(stored).unwrap();
    let refreshed = Principal {
        trust: "refreshed OS trust".into(),
        ..saved.clone()
    };
    let owner = Arc::new(());
    let other = Arc::new(());
    let selection = SelectedWorkspace {
        owner: Arc::clone(&owner),
        saved: saved.clone(),
    };
    assert_ne!(saved, refreshed);
    assert!(bound_origin(&saved, "https://EXAMPLE.invalid", "fixture").is_ok());
    assert!(bound_origin(&saved, "https://example.invalid:8443", "fixture").is_err());
    assert!(bound_origin(&saved, "https://example.invalid", "foreign").is_err());
    assert!(require_existing_basic_authority(&saved, &refreshed).is_ok());

    // Synthetic principal here exercises only the post-verified gate. Actual
    // production H2Channel must complete native TLS and ALPN before this point.
    let secret = "fixture-password-not-a-real-secret";
    let auth = existing_basic_header(&selection, &refreshed, &owner, &saved, secret).unwrap();
    assert!(auth.is_sensitive());
    assert!(auth.as_bytes().starts_with(b"Basic "));
    assert!(existing_basic_header(&selection, &refreshed, &other, &saved, secret).is_err());
    assert!(existing_basic_header(&selection, &refreshed, &owner, &refreshed, secret).is_err());
    let wrong = Principal {
        origin: "https://other.invalid:443".into(),
        ..refreshed
    };
    assert!(existing_basic_header(&selection, &wrong, &owner, &saved, secret).is_err());
    assert_eq!(serde_json::to_string(&selection.saved).unwrap(), stored);
}
