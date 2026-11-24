use super::{Duration, Instant, Ordering, check_login_header, decode_token, login_request};
use super::{Principal, ReadPurpose, SelectedWorkspace, bound_origin};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use std::sync::Arc;

#[test]
fn credential_bootstrap_is_exactly_empty_and_origin_bound() {
    let request = login_request().unwrap();
    assert_eq!(request.method(), Method::POST);
    assert_eq!(request.uri().to_string(), "/zxs/v1/sessions");
    assert!(request.uri().authority().is_none());
    assert!(request.uri().query().is_none());
    assert_eq!(
        request
            .headers()
            .get_all(header::CONTENT_LENGTH)
            .iter()
            .count(),
        1
    );
    assert_eq!(request.headers()[header::CONTENT_LENGTH], "0");
    assert!(!request.headers().contains_key(header::AUTHORIZATION));
    assert!(!request.headers().contains_key(header::TRANSFER_ENCODING));
    assert!(!request.headers().contains_key(header::CONNECTION));

    let original = Principal {
        origin: "https://example.invalid:443".to_owned(),
        account: "selected".to_owned(),
        trust: "exact old native digest".to_owned(),
    };
    assert!(bound_origin(&original, "https://EXAMPLE.invalid", "selected").is_ok());
    for (origin, account) in [
        ("https://other.invalid", "selected"),
        ("https://example.invalid:444", "selected"),
        ("https://example.invalid?alternate", "selected"),
        ("https://example.invalid", "someone"),
        ("https://example.invalid", "SELECTED"),
    ] {
        assert!(bound_origin(&original, origin, account).is_err());
    }
}

#[test]
fn current_native_receipt_identity_does_not_change_local_custody_or_payload_scope() {
    let enrolled = r#"{"origin":"https://example.invalid:443","account":"selected","trust":"original digest"}"#;
    let original: Principal = serde_json::from_str(enrolled).unwrap();
    let current = Principal {
        trust: "current OS observation".to_owned(),
        ..original.clone()
    };
    let owner = Arc::new(());
    let workspace = SelectedWorkspace {
        owner: Arc::clone(&owner),
        saved: original.clone(),
    };

    assert_ne!(original, current);
    // This covers binding policy only, not the mandatory TLS/ALPN handshake.
    assert!(
        ReadPurpose::Receipt
            .require_peer_identity(&original, &current)
            .is_ok()
    );
    assert!(
        ReadPurpose::Payload
            .require_peer_identity(&original, &original)
            .is_ok()
    );
    assert!(
        ReadPurpose::Payload
            .require_peer_identity(&original, &current)
            .is_err()
    );
    for changed in [
        Principal {
            origin: "https://example.invalid:444".to_owned(),
            ..current.clone()
        },
        Principal {
            origin: "https://other.invalid:443".to_owned(),
            ..current.clone()
        },
        Principal {
            account: "other-user".to_owned(),
            ..current
        },
    ] {
        assert!(
            ReadPurpose::Receipt
                .require_peer_identity(&original, &changed)
                .is_err()
        );
    }
    let changed_saved = Principal {
        trust: "not the exact saved record".to_owned(),
        ..original.clone()
    };
    assert!(workspace.matches(&owner, &original));
    assert!(!workspace.matches(&owner, &changed_saved));
    assert!(!workspace.matches(&Arc::new(()), &original));
    assert_eq!(serde_json::to_string(&workspace.saved).unwrap(), enrolled);
}

#[test]
fn exact_token_shape_lifetime_and_header_sensitivity() {
    let token = URL_SAFE_NO_PAD.encode([11; 32]);
    let clock = Instant::now();
    let encode = |secret: &str, scheme: &str, life: &str| {
        format!(r#"{{"access_token":"{secret}","token_type":"{scheme}","expires_in":{life}}}"#)
    };
    let value = encode(&token, "Bearer", "900");
    let live = decode_token(value.as_bytes(), clock).unwrap();
    assert!(live.auth.is_sensitive());
    assert_eq!(live.auth.as_bytes().len(), 50);
    assert!(live.auth.as_bytes().starts_with(b"Bearer "));
    live.require_live(clock + Duration::from_secs(899)).unwrap();
    assert!(live.require_live(clock + Duration::from_secs(900)).is_err());
    live.rejected.store(true, Ordering::Relaxed);
    assert!(live.require_live(clock).is_err());

    let alternate_tail = format!("{}B", &token[..42]);
    for bad in [
        encode(&URL_SAFE_NO_PAD.encode([0; 32]), "Bearer", "900"),
        encode(&format!("{token}="), "Bearer", "900"),
        encode(&alternate_tail, "Bearer", "900"),
        encode("secret-value-must-not-leak", "Bearer", "900"),
        encode(&token, "Basic", "900"),
        encode(&token, "Bearer", "900.0"),
        encode(&token, "Bearer", "901"),
        format!(
            r#"{{"access_token":"{token}","access_token":"{token}","token_type":"Bearer","expires_in":900}}"#
        ),
        format!(
            r#"{{"access_token":"{token}","token_type":"Bearer","expires_in":900,"extra":true}}"#
        ),
    ] {
        let Err(error) = decode_token(bad.as_bytes(), clock) else {
            panic!("accepted a malformed synthetic session reply")
        };
        assert!(!error.to_string().contains("secret-value-must-not-leak"));
    }
}

#[test]
fn session_errors_are_bounded_and_never_a_credential() {
    let mut headers = HeaderMap::new();
    assert!(check_login_header(StatusCode::OK, &headers).is_err());
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    assert!(check_login_header(StatusCode::OK, &headers).is_ok());
    headers.append(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    assert!(check_login_header(StatusCode::OK, &headers).is_err());

    headers.clear();
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from_static("0"));
    for n in ["1", "60"] {
        headers.insert(header::RETRY_AFTER, HeaderValue::from_static(n));
        assert_eq!(
            check_login_header(StatusCode::TOO_MANY_REQUESTS, &headers)
                .unwrap_err()
                .to_string(),
            format!("session login is limited; explicitly try again in {n} seconds")
        );
    }
    for n in ["0", "01", "61", "999", "1, 2"] {
        headers.insert(header::RETRY_AFTER, HeaderValue::from_static(n));
        assert!(
            check_login_header(StatusCode::TOO_MANY_REQUESTS, &headers)
                .unwrap_err()
                .to_string()
                .contains("no exact bounded Retry-After")
        );
    }
    headers.insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    assert!(
        check_login_header(StatusCode::UNAUTHORIZED, &headers)
            .unwrap_err()
            .to_string()
            .contains("no fallback")
    );
    headers.insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_static("Basic realm=\"zxs\", charset=\"UTF-8\""),
    );
    assert!(
        check_login_header(StatusCode::UNAUTHORIZED, &headers)
            .unwrap_err()
            .to_string()
            .contains("rejected its Basic login")
    );
    assert!(check_login_header(StatusCode::TEMPORARY_REDIRECT, &headers).is_err());
    assert!(check_login_header(StatusCode::SERVICE_UNAVAILABLE, &headers).is_err());
}

#[test]
fn prompt_time_revocation_prevents_even_constructing_basic() {
    use super::{basic_header, invalid, prompt_bound_basic};
    use std::cell::Cell;
    use zeroize::Zeroizing;

    let selected = Cell::new(true);
    let checked = Cell::new(false);
    let constructed = Cell::new(false);
    let refused = prompt_bound_basic(
        || {
            selected.set(false);
            Ok(Zeroizing::new("synthetic prompt input".to_owned()))
        },
        || {
            checked.set(true);
            if selected.get() {
                Ok(())
            } else {
                Err(invalid("original workspace revoked during prompt"))
            }
        },
        |password| {
            constructed.set(true);
            basic_header("fixture", password)
        },
    );
    assert!(refused.is_err());
    assert!(checked.get());
    assert!(!constructed.get());

    checked.set(false);
    let allowed = prompt_bound_basic(
        || Ok(Zeroizing::new("synthetic prompt input".to_owned())),
        || {
            checked.set(true);
            Ok(())
        },
        |password| {
            assert!(checked.get());
            basic_header("fixture", password)
        },
    )
    .unwrap();
    assert!(allowed.is_sensitive());
}
