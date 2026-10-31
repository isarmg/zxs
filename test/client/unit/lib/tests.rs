use super::*;
#[test]
fn http_and_https_origins_accept_remote_and_loopback_hosts() {
    for address in [
        "http://example.org",
        "http://192.0.2.10:7400",
        "http://[2001:db8::10]:7400",
        "http://127.0.0.1:7400",
        "http://localhost:7400",
        "http://[::1]:7400",
        "https://example.org",
        "https://192.0.2.10:7400",
        "https://[2001:db8::10]:7400",
    ] {
        assert!(Client::connect(address).is_ok(), "rejected {address}");
    }
}

#[test]
fn non_origin_and_credential_urls_are_rejected() {
    for address in [
        "ftp://example.org",
        "file:///tmp/server",
        "https://name:secret@example.org",
        "http://name:secret@example.org",
        "http://name@example.org",
        "http://example.org/api/",
        "http://example.org?key=value",
        "http://example.org#fragment",
    ] {
        assert!(Client::connect(address).is_err(), "accepted {address}");
    }
}
#[test]
fn route_segments_cannot_escape_scope() {
    assert_eq!(segment("a/b?x=1"), "a%2Fb%3Fx%3D1");
    for address in ["http://example.org", "https://example.org"] {
        let client = Client::connect(address).unwrap();
        for route in [
            "/api/%2e%2e/private",
            "/api/.%2E/private",
            "/api/a\t/../private",
            "/api/a%2fb",
            "http://other.example/api/files",
            "//other.example/api/files",
        ] {
            assert!(
                client.url(route).is_err(),
                "accepted {route:?} for {address}"
            );
        }
        assert!(client.url("/api/files?path=a/../b").is_ok());
    }
}

#[test]
fn segment_preserves_exact_public_encoding_vectors() {
    for (value, expected) in [
        ("", ""),
        ("AZaz09*-._", "AZaz09*-._"),
        ("..", ".."),
        ("a b+c", "a%20b%2Bc"),
        ("  +  ", "%20%20%2B%20%20"),
        ("!~'()", "%21%7E%27%28%29"),
        ("a/b?x=1", "a%2Fb%3Fx%3D1"),
        ("%#&", "%25%23%26"),
        ("\0\r\n\t\u{7f}", "%00%0D%0A%09%7F"),
        ("中文", "%E4%B8%AD%E6%96%87"),
        ("é", "%C3%A9"),
        ("e\u{301}", "e%CC%81"),
        ("😀", "%F0%9F%98%80"),
        ("\u{10ffff}", "%F4%8F%BF%BF"),
    ] {
        assert_eq!(segment(value), expected, "input {value:?}");
    }
}

// Keep the previous public byte contract only as a test oracle.
fn original_segment(value: &str) -> String {
    url_encoding::form::byte_serialize(value.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

#[test]
fn segment_matches_original_for_all_ascii_and_adjacent_pairs() {
    let all_ascii: String = (0u8..=127).map(char::from).collect();
    assert_eq!(segment(&all_ascii), original_segment(&all_ascii));
    for first in 0u8..=127 {
        for second in 0u8..=127 {
            let value = format!("A{}{} Z+~", char::from(first), char::from(second));
            assert_eq!(segment(&value), original_segment(&value), "input {value:?}");
        }
    }
}

#[test]
fn segment_matches_original_for_generated_utf8_strings() {
    let boundaries = [
        0x7f, 0x80, 0x7ff, 0x800, 0xd7ff, 0xe000, 0xffff, 0x10000, 0x10ffff,
    ];
    for codepoint in boundaries.into_iter().chain((0..=0x10ffff).step_by(997)) {
        // Surrogates are not valid UTF-8 string input to the public API.
        let Some(character) = char::from_u32(codepoint) else {
            continue;
        };
        let value = format!("AZ {character}+中e\u{301}{character}~*'() ");
        assert_eq!(segment(&value), original_segment(&value), "input {value:?}");
    }
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
