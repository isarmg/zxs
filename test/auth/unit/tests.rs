use super::*;

#[test]
fn administrator_username_has_one_canonical_policy() {
    assert_eq!(
        normalize_administrator_username(" Admin.Ops ").unwrap(),
        "admin.ops"
    );
    for valid in ["adm", "admin..ops", "admin__ops", "admin--ops"] {
        assert_eq!(normalize_administrator_username(valid).unwrap(), valid);
        require_canonical_administrator_username(valid).unwrap();
    }
    let maximum = format!("a{}z", "b".repeat(62));
    assert_eq!(normalize_administrator_username(&maximum).unwrap(), maximum);
    require_canonical_administrator_username(&maximum).unwrap();
    assert_eq!(normalize_administrator_username("Admin").unwrap(), "admin");
    assert!(require_canonical_administrator_username("Admin").is_err());
    for invalid in [
        "ad",
        ".admin",
        "admin-",
        "admin@example.test",
        "admin+ops",
        "admin ops",
        "管理员",
        "admin\t",
    ] {
        assert!(
            normalize_administrator_username(invalid).is_err(),
            "{invalid}"
        );
        assert!(
            require_canonical_administrator_username(invalid).is_err(),
            "{invalid}"
        );
    }
    assert!(normalize_administrator_username(&"a".repeat(65)).is_err());
    assert!(require_canonical_administrator_username(&"a".repeat(65)).is_err());
}

#[test]
fn only_the_current_password_policy_verifies() {
    let encoded = hash_password("correct horse battery staple").unwrap();
    require_current_password_hash(&encoded).unwrap();
    assert!(verify_password("correct horse battery staple", &encoded));
    assert!(!verify_password("wrong-password", &encoded));
    assert!(!verify_password("short", &encoded));

    let weak = Argon2::new(
        Algorithm::Argon2id,
        Version::V0x13,
        Params::new(8_192, 1, 1, Some(ARGON2_OUTPUT_BYTES)).unwrap(),
    )
    .hash_password(
        b"correct horse battery staple",
        &SaltString::generate(&mut argon2::password_hash::random_generator_core::OsRng),
    )
    .unwrap()
    .to_string();
    assert!(matches!(
        require_current_password_hash(&weak),
        Err(Error::InvalidPasswordHash)
    ));
    assert!(!verify_password("correct horse battery staple", &weak));
    assert!(require_current_password_hash("not-a-password-hash").is_err());
}

#[test]
fn csrf_derivation_is_canonical_stable_and_domain_separated() {
    let token = "A".repeat(43);
    let derived = derive_csrf_token(&token).unwrap();
    assert_eq!(derived, "Gt5U2H4mxXFOTGk8VgxGCUw3suwJxxqfr8KOuL-F4D8");
    assert_eq!(derive_csrf_token(&token).unwrap(), derived);
    assert_ne!(token, derived);
    assert_ne!(
        derive_csrf_token(&random_token().unwrap()).unwrap(),
        derived
    );
    for value in [
        "".to_owned(),
        "B".repeat(43),
        format!("{token}="),
        "A".repeat(42),
    ] {
        assert!(derive_csrf_token(&value).is_err());
    }
}

#[test]
fn token_generation_hashing_and_cookie_parsing_are_bounded() {
    let token = random_token().unwrap();
    assert!(is_token_shape(&token));
    let digest = token_hash(&token);
    assert_eq!(token_hash_hex(&token).len(), 64);
    assert!(token_matches_hash(&token, &digest));
    assert!(!token_matches_hash("another-token", &digest));
    let noncanonical_shape = "noncanonical-session-token";
    assert!(!token_matches_hash(
        noncanonical_shape,
        &token_hash(noncanonical_shape)
    ));
    assert!(!token_matches_hash(&token, &digest[..31]));
    assert!(!is_token_shape(&"B".repeat(SESSION_TOKEN_ENCODED_BYTES)));
    let cookie_header = format!("first=x; admin_session={token}; last=y");
    assert_eq!(
        parse_cookie_value(&cookie_header, "admin_session"),
        Some(token.as_str())
    );
    assert_eq!(parse_cookie_value("admin_session=", "admin_session"), None);
    assert_eq!(
        parse_cookie_value("admin_session=first; admin_session=second", "admin_session"),
        None
    );
}

fn verify_origin(
    mode: AdministratorOriginMode,
    origin: &str,
    host: &str,
    site: &str,
) -> Result<VerifiedSameOrigin, Error> {
    require_administrator_same_origin(
        mode,
        &[origin.as_bytes()],
        &[host.as_bytes()],
        &[site.as_bytes()],
    )
}

#[test]
fn production_origin_is_https_and_compares_normalized_effective_authority() {
    let verified = verify_origin(
        AdministratorOriginMode::ProductionHttps,
        "https://Console.Example.TEST:443",
        "console.example.test",
        "same-origin",
    )
    .unwrap();
    assert_eq!(verified.scheme(), "https");
    assert_eq!(verified.host(), "console.example.test");
    assert_eq!(verified.port(), 443);

    let ipv6 = verify_origin(
        AdministratorOriginMode::ProductionHttps,
        "https://[2001:0db8:0:0:0:0:0:1]:8443",
        "[2001:db8::1]:8443",
        "same-origin",
    )
    .unwrap();
    assert_eq!(ipv6.host(), "2001:db8::1");
    assert_eq!(ipv6.port(), 8443);
}

#[test]
fn websocket_origin_accepts_missing_fetch_metadata_without_weakening_http() {
    let empty: [&[u8]; 0] = [];
    let origin = [b"https://Console.Example.TEST:443".as_slice()];
    let host = [b"console.example.test".as_slice()];
    let verified = require_administrator_websocket_origin(
        AdministratorOriginMode::ProductionHttps,
        &origin,
        &host,
        &empty,
    )
    .unwrap();
    assert_eq!(verified.scheme(), "https");
    assert_eq!(verified.host(), "console.example.test");
    assert_eq!(verified.port(), 443);
    assert_eq!(
        require_administrator_same_origin(
            AdministratorOriginMode::ProductionHttps,
            &origin,
            &host,
            &empty,
        ),
        Err(Error::MissingSecurityHeader {
            name: SEC_FETCH_SITE_HEADER
        })
    );
    assert_eq!(
        require_administrator_websocket_origin(
            AdministratorOriginMode::ProductionHttps,
            &origin,
            &host,
            &[b"same-origin".as_slice()],
        )
        .unwrap(),
        verified
    );
    let loopback = require_administrator_websocket_origin(
        AdministratorOriginMode::LoopbackDevelopmentHttp,
        &[b"http://[0:0:0:0:0:0:0:1]".as_slice()],
        &[b"[::1]:80".as_slice()],
        &empty,
    )
    .unwrap();
    assert_eq!(loopback.host(), "::1");
    assert_eq!(loopback.port(), 80);
}

#[test]
fn websocket_origin_requires_one_origin_and_host_even_without_fetch_metadata() {
    let empty: [&[u8]; 0] = [];
    let origin = [b"https://console.example.test".as_slice()];
    let host = [b"console.example.test".as_slice()];
    for (origins, hosts, error) in [
        (
            empty.to_vec(),
            host.to_vec(),
            Error::MissingSecurityHeader {
                name: ORIGIN_HEADER,
            },
        ),
        (
            origin.to_vec(),
            empty.to_vec(),
            Error::MissingSecurityHeader { name: HOST_HEADER },
        ),
        (
            vec![origin[0], origin[0]],
            host.to_vec(),
            Error::DuplicateSecurityHeader {
                name: ORIGIN_HEADER,
                actual: 2,
            },
        ),
        (
            origin.to_vec(),
            vec![host[0], host[0]],
            Error::DuplicateSecurityHeader {
                name: HOST_HEADER,
                actual: 2,
            },
        ),
    ] {
        assert_eq!(
            require_administrator_websocket_origin(
                AdministratorOriginMode::ProductionHttps,
                &origins,
                &hosts,
                &empty,
            ),
            Err(error)
        );
    }
}

#[test]
fn websocket_origin_rejects_supplied_invalid_or_multiple_fetch_metadata() {
    let origin = [b"https://console.example.test".as_slice()];
    let host = [b"console.example.test".as_slice()];
    for site in [
        "",
        "same-site",
        "cross-site",
        "none",
        "same-origin,cross-site",
        "same-origin ",
    ] {
        assert_eq!(
            require_administrator_websocket_origin(
                AdministratorOriginMode::ProductionHttps,
                &origin,
                &host,
                &[site.as_bytes()],
            ),
            Err(Error::InvalidSecurityHeaderValue {
                name: SEC_FETCH_SITE_HEADER
            })
        );
    }
    assert_eq!(
        require_administrator_websocket_origin(
            AdministratorOriginMode::ProductionHttps,
            &origin,
            &host,
            &[b"same-origin".as_slice(), b"same-origin".as_slice()],
        ),
        Err(Error::DuplicateSecurityHeader {
            name: SEC_FETCH_SITE_HEADER,
            actual: 2
        })
    );
}

#[test]
fn websocket_origin_keeps_scheme_authority_and_loopback_boundaries() {
    let empty: [&[u8]; 0] = [];
    for (mode, origin, host, error) in [
        (
            AdministratorOriginMode::ProductionHttps,
            "http://console.example.test",
            "console.example.test",
            Error::UnexpectedOriginScheme { expected: "https" },
        ),
        (
            AdministratorOriginMode::ProductionHttps,
            "https://other.example.test",
            "console.example.test",
            Error::OriginHostMismatch,
        ),
        (
            AdministratorOriginMode::ProductionHttps,
            "https://console.example.test:444",
            "console.example.test",
            Error::OriginHostMismatch,
        ),
        (
            AdministratorOriginMode::ProductionHttps,
            "https://console.example.test/",
            "console.example.test",
            Error::InvalidSecurityHeaderValue {
                name: ORIGIN_HEADER,
            },
        ),
        (
            AdministratorOriginMode::ProductionHttps,
            "null",
            "console.example.test",
            Error::InvalidSecurityHeaderValue {
                name: ORIGIN_HEADER,
            },
        ),
        (
            AdministratorOriginMode::LoopbackDevelopmentHttp,
            "http://console.example.test",
            "console.example.test",
            Error::DevelopmentHostIsNotLoopback,
        ),
    ] {
        assert_eq!(
            require_administrator_websocket_origin(
                mode,
                &[origin.as_bytes()],
                &[host.as_bytes()],
                &empty,
            ),
            Err(error)
        );
    }
}

#[test]
fn loopback_http_mode_accepts_only_actual_loopback_authorities() {
    for (origin, host, expected) in [
        ("http://LOCALHOST:3000", "localhost:3000", "localhost"),
        ("http://127.0.0.42:8080", "127.0.0.42:8080", "127.0.0.42"),
        ("http://[0:0:0:0:0:0:0:1]", "[::1]:80", "::1"),
    ] {
        let verified = verify_origin(
            AdministratorOriginMode::LoopbackDevelopmentHttp,
            origin,
            host,
            "same-origin",
        )
        .unwrap();
        assert_eq!(verified.host(), expected);
    }

    assert!(matches!(
        verify_origin(
            AdministratorOriginMode::LoopbackDevelopmentHttp,
            "http://development.example.test",
            "development.example.test",
            "same-origin",
        ),
        Err(Error::DevelopmentHostIsNotLoopback)
    ));
}

#[test]
fn same_origin_policy_rejects_missing_duplicate_cross_site_and_mismatch() {
    let origin = [b"https://console.example.test".as_slice()];
    let host = [b"console.example.test".as_slice()];
    let site = [b"same-origin".as_slice()];
    let empty: [&[u8]; 0] = [];

    assert!(matches!(
        require_administrator_same_origin(
            AdministratorOriginMode::ProductionHttps,
            &empty,
            &host,
            &site,
        ),
        Err(Error::MissingSecurityHeader {
            name: ORIGIN_HEADER
        })
    ));
    assert!(matches!(
        require_administrator_same_origin(
            AdministratorOriginMode::ProductionHttps,
            &origin,
            &empty,
            &site,
        ),
        Err(Error::MissingSecurityHeader { name: HOST_HEADER })
    ));
    assert!(matches!(
        require_administrator_same_origin(
            AdministratorOriginMode::ProductionHttps,
            &origin,
            &host,
            &empty,
        ),
        Err(Error::MissingSecurityHeader {
            name: SEC_FETCH_SITE_HEADER
        })
    ));

    let duplicate_origin = [origin[0], origin[0]];
    assert!(matches!(
        require_administrator_same_origin(
            AdministratorOriginMode::ProductionHttps,
            &duplicate_origin,
            &host,
            &site,
        ),
        Err(Error::DuplicateSecurityHeader {
            name: ORIGIN_HEADER,
            actual: 2
        })
    ));
    let duplicate_host = [host[0], host[0]];
    assert!(matches!(
        require_administrator_same_origin(
            AdministratorOriginMode::ProductionHttps,
            &origin,
            &duplicate_host,
            &site,
        ),
        Err(Error::DuplicateSecurityHeader {
            name: HOST_HEADER,
            actual: 2
        })
    ));
    let duplicate_site = [site[0], site[0]];
    assert!(matches!(
        require_administrator_same_origin(
            AdministratorOriginMode::ProductionHttps,
            &origin,
            &host,
            &duplicate_site,
        ),
        Err(Error::DuplicateSecurityHeader {
            name: SEC_FETCH_SITE_HEADER,
            actual: 2
        })
    ));
    assert!(matches!(
        verify_origin(
            AdministratorOriginMode::ProductionHttps,
            "https://console.example.test",
            "console.example.test",
            "cross-site",
        ),
        Err(Error::InvalidSecurityHeaderValue {
            name: SEC_FETCH_SITE_HEADER
        })
    ));
    assert!(matches!(
        verify_origin(
            AdministratorOriginMode::ProductionHttps,
            "https://console.example.test:444",
            "console.example.test",
            "same-origin",
        ),
        Err(Error::OriginHostMismatch)
    ));
}

#[test]
fn same_origin_policy_rejects_noncanonical_origin_and_host_syntax() {
    for origin in [
        "http://console.example.test",
        "HTTPS://console.example.test",
    ] {
        assert!(matches!(
            verify_origin(
                AdministratorOriginMode::ProductionHttps,
                origin,
                "console.example.test",
                "same-origin",
            ),
            Err(Error::UnexpectedOriginScheme { expected: "https" })
        ));
    }

    for origin in [
        "https://user@console.example.test",
        "https://console.example.test/",
        "https://console.example.test?query",
        "https://console.example.test#fragment",
        "https://console.example.test:0",
        "https://console.example.test:0443",
        "https://console_example.test",
        "https://console.example.test.",
    ] {
        assert!(matches!(
            verify_origin(
                AdministratorOriginMode::ProductionHttps,
                origin,
                "console.example.test",
                "same-origin",
            ),
            Err(Error::InvalidSecurityHeaderValue {
                name: ORIGIN_HEADER
            })
        ));
    }

    for host in [
        "https://console.example.test",
        "user@console.example.test",
        "console.example.test/path",
        "console.example.test?query",
        "console.example.test:0443",
        "127.000.000.001",
        "::1",
    ] {
        assert!(matches!(
            verify_origin(
                AdministratorOriginMode::ProductionHttps,
                "https://console.example.test",
                host,
                "same-origin",
            ),
            Err(Error::InvalidSecurityHeaderValue { name: HOST_HEADER })
        ));
    }
}

#[test]
fn singleton_security_and_csrf_headers_fail_closed() {
    let token = random_token().unwrap();
    let values = [token.as_bytes()];
    assert_eq!(
        require_single_security_header_value(CSRF_HEADER, &values).unwrap(),
        token
    );
    assert_eq!(require_single_csrf_token(&values).unwrap(), token);
    let digest = token_hash(&token);
    require_csrf_token_matches_hash(&values, &digest).unwrap();

    let empty: [&[u8]; 0] = [];
    assert!(matches!(
        require_single_csrf_token(&empty),
        Err(Error::MissingSecurityHeader { name: CSRF_HEADER })
    ));
    let duplicate = [token.as_bytes(), token.as_bytes()];
    assert!(matches!(
        require_single_csrf_token(&duplicate),
        Err(Error::DuplicateSecurityHeader {
            name: CSRF_HEADER,
            actual: 2
        })
    ));
    assert!(matches!(
        require_single_csrf_token(&[b"value,second".as_slice()]),
        Err(Error::InvalidSecurityHeaderValue { name: CSRF_HEADER })
    ));
    let noncanonical_token = "B".repeat(SESSION_TOKEN_ENCODED_BYTES);
    assert!(matches!(
        require_single_csrf_token(&[noncanonical_token.as_bytes()]),
        Err(Error::InvalidCsrfToken)
    ));
    assert!(matches!(
        require_csrf_token_matches_hash(
            &[noncanonical_token.as_bytes()],
            &token_hash(&noncanonical_token)
        ),
        Err(Error::InvalidCsrfToken)
    ));
    for invalid_length in [digest[..31].to_vec(), vec![0; TOKEN_HASH_BYTES + 1]] {
        assert!(!token_matches_hash(&token, &invalid_length));
        assert!(matches!(
            require_csrf_token_matches_hash(&values, &invalid_length),
            Err(Error::CsrfTokenMismatch)
        ));
    }
    let mut wrong_digest = digest;
    wrong_digest[0] ^= 1;
    assert!(matches!(
        require_csrf_token_matches_hash(&values, &wrong_digest),
        Err(Error::CsrfTokenMismatch)
    ));

    let non_utf8 = [&[0xff_u8][..]];
    assert!(matches!(
        require_single_security_header_value(ORIGIN_HEADER, &non_utf8),
        Err(Error::InvalidSecurityHeaderValue {
            name: ORIGIN_HEADER
        })
    ));
    assert!(matches!(
        require_single_security_header_value(ORIGIN_HEADER, &[b" leading".as_slice()]),
        Err(Error::InvalidSecurityHeaderValue {
            name: ORIGIN_HEADER
        })
    ));
}

#[test]
fn password_hash_propagates_entropy_failure_without_panicking() {
    let error = hash_password_with_entropy("correct horse battery staple", |_| {
        Err(Error::Random(system_entropy::Error::UNEXPECTED))
    })
    .unwrap_err();
    assert_eq!(error, Error::Random(system_entropy::Error::UNEXPECTED));
}

#[test]
fn password_hash_validates_before_requesting_entropy() {
    let error = hash_password_with_entropy("short", |_| {
        panic!("invalid passwords must not request entropy")
    })
    .unwrap_err();
    assert_eq!(error, Error::InvalidPassword);
}

#[test]
fn password_hash_uses_the_supplied_full_length_salt() {
    let encoded = hash_password_with_entropy("contract-fixture-password", |bytes| {
        assert_eq!(bytes.len(), ARGON2_SALT_BYTES);
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = index as u8;
        }
        Ok(())
    })
    .unwrap();
    require_current_password_hash(&encoded).unwrap();
    assert!(verify_password("contract-fixture-password", &encoded));
    let vectors = credential_validation_vectors().unwrap();
    assert_eq!(
        encoded,
        vectors["password_hash"]["encoded"].as_str().unwrap()
    );
}

#[test]
fn external_authentication_failures_retain_concrete_sources() {
    use std::error::Error as _;
    let error = require_current_password_hash("not-a-password-hash").unwrap_err();
    assert!(error.source().unwrap().is::<argon2::password_hash::Error>());
    let error = Error::Random(system_entropy::Error::UNEXPECTED);
    assert!(error.source().unwrap().is::<system_entropy::Error>());
}
