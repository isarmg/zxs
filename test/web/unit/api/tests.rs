use super::*;
#[test]
fn segment_encoding_is_native_safe_and_matches_uri_component_semantics() {
    assert_eq!(segment("AZaz09-_.!~*'()"), "AZaz09-_.!~*'()");
    assert_eq!(
        segment("a/b c?d=e&f#%+\\\u{0}"),
        "a%2Fb%20c%3Fd%3De%26f%23%25%2B%5C%00"
    );
    assert_eq!(segment("历史📁"), "%E5%8E%86%E5%8F%B2%F0%9F%93%81");
    assert_eq!(segment(""), "");
}
#[test]
fn late_login_cannot_restore_cleared_or_replace_newer_credentials() {
    let mut saved = Credentials::default();
    let request_generation = saved.generation;
    saved.replace(None, None);
    assert!(!saved.replace_if_current(request_generation, Some("late".into()), None));
    assert!(saved.token.is_none());
    let current = saved.generation;
    assert!(saved.replace_if_current(current, Some("new".into()), Some("csrf".into())));
    assert!(!saved.replace_if_current(current, Some("late".into()), None));
    assert_eq!(saved.token.as_deref(), Some("new"));
    assert_eq!(saved.csrf.as_deref(), Some("csrf"));
}
#[test]
fn an_old_unauthorized_response_cannot_revoke_a_new_login() {
    let mut saved = Credentials::default();
    saved.replace(Some("old-token".into()), Some("old-csrf".into()));
    let old_request = saved.generation;
    saved.replace(Some("new-token".into()), Some("new-csrf".into()));
    assert!(!saved.revoke(old_request));
    assert_eq!(saved.token.as_deref(), Some("new-token"));
    assert_eq!(saved.csrf.as_deref(), Some("new-csrf"));
    assert!(saved.revoke(saved.generation));
    assert!(saved.token.is_none());
    assert!(saved.csrf.is_none());
    assert!(!saved.revoke(old_request));
}
#[test]
fn restrict_transport_to_own_api() {
    assert!(validate_path("/api/repositories").is_ok());
    assert!(validate_path("/api/file-manager/disks/P01/entries?path=report..txt").is_ok());
    assert!(validate_path("/api/file-manager/disks/P01/entries?path=photos%2F2026").is_ok());
    for path in [
        "https://other/api/a",
        "//other/api/a",
        "/other/api/a",
        "/v1/repositories",
        "/api/../secret",
        "/api/%2e%2e/secret",
        "/api/.%2e/secret",
        "/api/a%2fb",
        "/api/a#secret",
        "/api/\\evil",
        "/api/\t../secret",
        "/api/file\nname",
    ] {
        assert!(validate_path(path).is_err(), "{path}");
    }
}

#[test]
fn beginning_logout_invalidates_pending_login_without_losing_revocation_credentials() {
    CREDENTIALS.with(|saved| {
        saved
            .borrow_mut()
            .replace(Some("active-token".into()), Some("active-csrf".into()));
    });
    let generation = CREDENTIALS.with(|saved| saved.borrow().generation);
    let mut state = crate::ui::State {
        authenticated: true,
        ..Default::default()
    };
    assert_eq!(crate::ui::begin_logout(&mut state), Some(1));
    assert_eq!(crate::ui::begin_logout(&mut state), None);
    CREDENTIALS.with(|saved| {
        let mut saved = saved.borrow_mut();
        assert!(!saved.replace_if_current(generation, Some("late-token".into()), None));
        assert_eq!(saved.token.as_deref(), Some("active-token"));
        assert_eq!(saved.csrf.as_deref(), Some("active-csrf"));
    });
    // Transport failure locks the UI but must retain the exact credential for
    // a later revocation retry, rather than replacing it with a new identity.
    state.session_transition = crate::ui::SessionTransition::LogoutUnconfirmed;
    state.authenticated = false;
    assert_eq!(crate::ui::begin_logout(&mut state), Some(2));
    CREDENTIALS.with(|saved| {
        let saved = saved.borrow();
        assert_eq!(saved.token.as_deref(), Some("active-token"));
        assert_eq!(saved.csrf.as_deref(), Some("active-csrf"));
    });
    clear_session();
}

#[test]
fn invalid_logout_success_preserves_credentials_and_the_revocation_retry_fence() {
    use crate::ui::{SessionTransition, State, begin_logout, complete_logout};
    use web_interface::prelude::*;
    Owner::new().with(|| {
        for value in [
            Value::Null,
            json!([]),
            json!(true),
            json!({}),
            json!({"logged_out":false}),
            json!({"logged_out":"true"}),
            json!({"logged_out":true,"admin":true}),
            json!({"logged_out":true,"extra":null}),
        ] {
            CREDENTIALS.with(|saved| {
                saved
                    .borrow_mut()
                    .replace(Some("original-bearer".into()), Some("original-csrf".into()));
            });
            let state = RwSignal::new(State {
                authenticated: true,
                principal: "owner".into(),
                ..Default::default()
            });
            state.update(|s| assert_eq!(begin_logout(s), Some(1)));
            let generation = CREDENTIALS.with(|saved| saved.borrow().generation);
            complete_logout(state, Ok(value));
            state.with_untracked(|s| {
                assert_eq!(s.session_transition, SessionTransition::LogoutUnconfirmed);
                assert!(!s.session_active());
                assert!(s.session_failure.is_some());
                assert_eq!(s.principal, "owner");
                assert_eq!(s.session_epoch, 1);
            });
            CREDENTIALS.with(|saved| {
                let saved = saved.borrow();
                assert_eq!(saved.token.as_deref(), Some("original-bearer"));
                assert_eq!(saved.csrf.as_deref(), Some("original-csrf"));
                assert_eq!(saved.generation, generation);
            });
            // A valid retry, not a fresh login, settles the original intent.
            state.update(|s| assert_eq!(begin_logout(s), Some(2)));
            complete_logout(state, Ok(json!({"logged_out":true})));
            state.with_untracked(|s| {
                assert_eq!(s.session_transition, SessionTransition::Idle);
                assert!(!s.authenticated);
                assert_eq!(s.session_epoch, 3);
            });
            CREDENTIALS.with(|saved| {
                let saved = saved.borrow();
                assert!(saved.token.is_none());
                assert!(saved.csrf.is_none());
            });
        }
    });
    clear_session();
}

fn session_response() -> Value {
    json!({"token":"new-token","csrf_token":"new-csrf","principal":"owner","expires_at":"9999999999"})
}
fn restored_response() -> Value {
    json!({"csrf_token":"new-csrf","principal":"owner"})
}

#[test]
fn malformed_authentication_success_cannot_install_or_restore_a_session() {
    let original = Credentials {
        token: Some("existing-token".into()),
        csrf: Some("existing-csrf".into()),
        generation: 10,
    };
    for login in [false, true] {
        let valid = if login {
            session_response()
        } else {
            restored_response()
        };
        let mut invalid = vec![Value::Null, json!({}), json!({"token":"new-token"})];
        for field in if login {
            vec!["principal", "csrf_token", "token", "expires_at"]
        } else {
            vec!["principal", "csrf_token"]
        } {
            let mut missing = valid.clone();
            missing.as_object_mut().unwrap().remove(field);
            invalid.push(missing);
            for replacement in [Value::Null, json!(""), json!(42)] {
                let mut response = valid.clone();
                response[field] = replacement;
                invalid.push(response);
            }
        }
        for (field, value) in [
            ("admin", json!(true)),
            ("repositories", json!([])),
            ("disabled", json!(false)),
            ("username", json!("owner")),
        ] {
            let mut response = valid.clone();
            response[field] = value;
            invalid.push(response);
        }
        if login {
            for expiry in ["0", "01", "18446744073709551616"] {
                let mut response = valid.clone();
                response["expires_at"] = expiry.into();
                invalid.push(response);
            }
        }
        for value in invalid {
            CREDENTIALS.with(|saved| *saved.borrow_mut() = original.clone());
            let result = if login {
                accept_login(original.generation, value)
            } else {
                accept_restore(original.generation, value)
            };
            let error = result.expect_err("incomplete or retired identities must not open the UI");
            assert_eq!(error.code, storage_contract::ErrorCode::InvalidSession);
            assert_eq!(error.status, 200);
            CREDENTIALS.with(|saved| {
                let saved = saved.borrow();
                assert_eq!(saved.generation, original.generation);
                assert_eq!(saved.token, original.token);
                assert_eq!(saved.csrf, original.csrf);
            });
        }
    }
    clear_session();
}

#[test]
fn validated_session_restoration_requires_the_original_credential_generation() {
    CREDENTIALS.with(|saved| *saved.borrow_mut() = Credentials::default());
    let response = restored_response();
    assert_eq!(
        accept_restore(0, response.clone()).unwrap(),
        Authenticated {
            principal: "owner".into()
        }
    );
    CREDENTIALS.with(|saved| {
        let saved = saved.borrow();
        assert!(
            saved.token.is_none(),
            "restoration uses the HTTP-only cookie"
        );
        assert_eq!(saved.csrf.as_deref(), Some("new-csrf"));
    });
    assert_eq!(
        accept_login(0, session_response()).unwrap(),
        Authenticated {
            principal: "owner".into()
        }
    );
    assert_eq!(
        accept_restore(0, response).unwrap_err().code,
        storage_contract::ErrorCode::SessionChanged
    );
    CREDENTIALS.with(|saved| {
        let saved = saved.borrow();
        assert_eq!(saved.generation, 1);
        assert_eq!(saved.token.as_deref(), Some("new-token"));
        assert_eq!(saved.csrf.as_deref(), Some("new-csrf"));
    });
    clear_session();
}

#[test]
fn invalid_api_path_preserves_the_contract_error_source() {
    use std::error::Error as _;
    let failure = validate_path("https://outside.example/api/a").unwrap_err();
    assert!(failure.source().unwrap().source().is_some());
    assert!(matches!(
        failure.source.as_deref(),
        Some(ErrorSource::Path(_))
    ));
}
