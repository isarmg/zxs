use super::*;
#[test]
fn plaintext_remote_and_credential_urls_are_rejected() {
    assert!(Client::connect("http://example.org").is_err());
    assert!(Client::connect("https://name:secret@example.org").is_err());
    assert!(Client::connect("http://127.0.0.1:7400").is_ok());
    assert!(Client::connect("https://example.org").is_ok());
}
#[test]
fn route_segments_cannot_escape_scope() {
    assert_eq!(segment("a/b?x=1"), "a%2Fb%3Fx%3D1");
    let client = Client::connect("https://example.org").unwrap();
    for route in [
        "/api/%2e%2e/private",
        "/api/.%2E/private",
        "/api/a\t/../private",
        "/api/a%2fb",
    ] {
        assert!(client.url(route).is_err(), "accepted {route:?}");
    }
    assert!(client.url("/api/files?path=a/../b").is_ok());
}
#[test]
fn session_debug_never_prints_credentials() {
    let session = Session {
        token: "private-bearer".into(),
        csrf_token: "private-csrf".into(),
        principal: "alice".into(),
        expires_at: 10,
    };
    let debug = format!("{session:?}");
    assert!(!debug.contains(&session.token));
    assert!(!debug.contains(&session.csrf_token));
    assert!(debug.contains("alice"));
}

#[test]
fn session_response_is_exact_and_contains_no_roles() {
    let valid = json!({"token":"secret-token","csrf_token":"secret-csrf","principal":"owner","expires_at":"100"});
    assert_eq!(Session::decode(valid.clone()).unwrap().principal, "owner");
    for field in ["token", "csrf_token", "principal", "expires_at"] {
        let mut missing = valid.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(Session::decode(missing).is_err());
        for value in [Value::Null, json!(""), json!(1)] {
            let mut invalid = valid.clone();
            invalid[field] = value;
            assert!(Session::decode(invalid).is_err());
        }
    }
    for (field, value) in [
        ("admin", json!(true)),
        ("repositories", json!([])),
        ("disabled", json!(false)),
    ] {
        let mut invalid = valid.clone();
        invalid[field] = value;
        assert!(Session::decode(invalid).is_err());
    }
    for expiry in ["0", "01", "18446744073709551616"] {
        let mut invalid = valid.clone();
        invalid["expires_at"] = expiry.into();
        assert!(Session::decode(invalid).is_err());
    }
}

#[test]
fn malformed_external_values_retain_sources() {
    use std::error::Error as _;
    let error = match Client::connect("not a URL") {
        Ok(_) => panic!("invalid URL accepted"),
        Err(error) => error,
    };
    assert!(
        error
            .source()
            .unwrap()
            .is::<url_encoding::address::ParseError>()
    );
    let error = Session::decode(json!({"token": 1})).unwrap_err();
    assert!(error.source().unwrap().is::<json_codec::Error>());
}
