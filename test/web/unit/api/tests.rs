use super::*;

#[test]
fn invalid_domain_response_retains_uncertainty_and_the_original_source() {
    use std::error::Error as _;

    let source = json_codec::from_str::<Value>("{").unwrap_err();
    let original = source.to_string();
    let error = Error::invalid_response(
        "The original operation is unconfirmed.",
        ErrorSource::FileDeletion(source),
    );
    assert_eq!(error.code, storage_contract::ErrorCode::InvalidResponse);
    assert_eq!(error.status, 200);
    assert_eq!(error.message, "The original operation is unconfirmed.");
    assert!(!error.retryable);
    assert!(error.request_id.is_none());
    assert!(error.mutation_outcome_unknown());
    assert!(matches!(
        error.source.as_deref(),
        Some(ErrorSource::FileDeletion(_))
    ));
    let cause = error.source().unwrap().source().unwrap();
    assert!(cause.is::<json_codec::Error>());
    assert_eq!(cause.to_string(), original);

    let cloned = error.clone();
    assert!(std::sync::Arc::ptr_eq(
        error.source.as_ref().unwrap(),
        cloned.source.as_ref().unwrap()
    ));
}

#[test]
fn old_401_keeps_new_credentials_and_their_queued_management_read() {
    use std::{
        future::Future,
        pin::Pin,
        task::{Context, Poll, Waker},
    };
    clear_session();
    let old = CREDENTIALS.with(|saved| saved.borrow().generation);
    CREDENTIALS.with(|saved| {
        saved
            .borrow_mut()
            .replace(Some("new-token".into()), Some("new-csrf".into()))
    });
    let mut current = MANAGEMENT_READS.with(read_queue::Queue::acquire).unwrap();
    revoke_response_credentials(old);
    assert!(matches!(
        Pin::new(&mut current).poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Ok(_))
    ));
    CREDENTIALS.with(|saved| assert_eq!(saved.borrow().token.as_deref(), Some("new-token")));
    clear_session();
}

#[test]
fn current_401_and_logout_clear_queued_reads_without_a_network_attempt() {
    use std::{
        future::Future,
        pin::Pin,
        task::{Context, Poll, Waker},
    };
    for logout in [false, true] {
        clear_session();
        let mut queued = MANAGEMENT_READS.with(read_queue::Queue::acquire).unwrap();
        if logout {
            invalidate_session_work();
        } else {
            let generation = CREDENTIALS.with(|saved| saved.borrow().generation);
            revoke_response_credentials(generation);
        }
        assert!(matches!(
            Pin::new(&mut queued).poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(Err(read_queue::AdmissionError::Invalidated))
        ));
    }
    clear_session();
}

#[test]
fn local_queue_full_is_distinct_from_the_unchanged_remote_429() {
    let local = read_admission_error(read_queue::AdmissionError::Full);
    assert_eq!(local.status, 0);
    assert_eq!(local.code, storage_contract::ErrorCode::ResourceLimit);
    let remote = response::decode(429, br#"{"error":{"code":"RESOURCE_LIMIT_EXCEEDED","message":"capacity reached","retryable":true,"request_id":"original-response"}}"#).unwrap_err();
    assert_eq!(remote.status, 429);
    assert_eq!(
        remote.code,
        storage_contract::ErrorCode::ResourceLimitExceeded
    );
    assert_eq!(remote.message, "capacity reached");
    assert_eq!(remote.request_id.as_deref(), Some("original-response"));
    assert!(remote.retryable);
}
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
        let mut invalid = vec![
            Value::Null,
            json!({}),
            json!({"token":"new-token"}),
            json!(["owner", "new-csrf"]),
            json!(["owner", "new-csrf", "new-token", "9999999999"]),
            if login {
                restored_response()
            } else {
                session_response()
            },
        ];
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
            for expiry in ["0", "01", "+1", "-1", " 1", "1 ", "1.0", "18446744073709551616"] {
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

#[test]
fn endpoint_validation_status_keeps_request_identity_and_source() {
    use std::error::Error as _;
    let mut failure = Error::from(crate::admin::ResponseError::Contract("plan digest"));
    failure.request_id = Some("request-admin".into());
    let source = failure.source.clone().unwrap();
    for status in [200, 201, 202, 204] {
        let failure = failure.clone().with_status(status);
        assert_eq!(failure.status, status);
        assert_eq!(failure.request_id.as_deref(), Some("request-admin"));
        assert!(std::sync::Arc::ptr_eq(
            failure.source.as_ref().unwrap(),
            &source
        ));
        assert!(failure.source().is_some());
        assert!(failure.mutation_outcome_unknown());
    }
}

#[test]
fn setup_status_is_a_bodyless_read() {
    let (method, path, body) = SetupCall::Status.into_request();
    assert_eq!(method, "GET");
    assert_eq!(path, "/api/setup/status");
    assert!(body.is_none());
}

#[test]
fn authentication_dtos_accept_the_existing_wire_domain_without_normalizing_identity() {
    for expires_at in ["1", "18446744073709551615"] {
        let wire = format!(
            r#"{{"principal":" ","csrf_token":" ","token":" ","expires_at":"{expires_at}"}}"#
        );
        let value: Value = json_codec::from_str(&wire).unwrap();
        let response = session::decode::<LoginResponse>(value).unwrap();
        assert_eq!(response.principal.as_str(), " ");
        assert_eq!(response.csrf_token.0, " ");
        assert_eq!(response.token.0, " ");
    }
    let wire = r#"{"principal":"owner","csrf_token":"new-csrf"}"#;
    let response = session::decode::<SessionResponse>(json_codec::from_str(wire).unwrap()).unwrap();
    assert_eq!(response.principal.as_str(), "owner");
    assert_eq!(response.csrf_token.0, "new-csrf");
}

#[test]
fn authentication_decode_failure_retains_its_source_without_changing_the_error_code() {
    use std::error::Error as _;

    let failure = accept_login(0, json!({"principal":null})).unwrap_err();
    assert_eq!(failure.code, storage_contract::ErrorCode::InvalidSession);
    assert_eq!(failure.status, 200);
    assert!(matches!(
        failure.source.as_deref(),
        Some(ErrorSource::Session(_))
    ));
    assert!(failure.source().unwrap().source().unwrap().is::<session::DecodeError>());
}

#[test]
fn authentication_diagnostics_redact_response_values_and_unknown_field_names() {
    use std::error::Error as _;

    for login in [false, true] {
        let valid = if login {
            session_response()
        } else {
            restored_response()
        };
        let mut invalid = vec![json!("response-secret-marker")];
        for field in ["principal", "csrf_token", "token", "expires_at"] {
            let mut response = valid.clone();
            response[field] = json!(8723941029384756_u64);
            invalid.push(response);
        }
        let mut response = valid;
        response["response-secret-marker"] = json!("response-secret-marker");
        invalid.push(response);
        for value in invalid {
            let failure = if login {
                accept_login(0, value).unwrap_err()
            } else {
                accept_restore(0, value).unwrap_err()
            };
            assert_eq!(failure.code, storage_contract::ErrorCode::InvalidSession);
            assert_eq!(failure.status, 200);
            let mut diagnostics = format!("{failure:?} {failure} {:?}", failure.clone());
            let mut source = failure.source();
            while let Some(cause) = source {
                diagnostics.push_str(&format!(" {cause:?} {cause}"));
                assert!(!cause.is::<json_codec::Error>());
                source = cause.source();
            }
            assert!(!diagnostics.contains("response-secret-marker"));
            assert!(!diagnostics.contains("8723941029384756"));
        }
    }
}
