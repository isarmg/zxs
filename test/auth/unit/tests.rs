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

#[test]
fn origin_policy_preserves_direct_http_and_exact_browser_serialization() {
    let empty: [&[u8]; 0] = [];
    for (host, origin) in [
        ("192.0.2.20:7400", "http://192.0.2.20:7400"),
        ("STORAGE.EXAMPLE:80", "http://storage.example"),
        ("[2001:0db8::20]:80", "http://[2001:db8::20]"),
        ("0.0.0.0:7400", "http://0.0.0.0:7400"),
    ] {
        let proof = verify_same_origin(
            None,
            OriginRequirement::Required,
            &[origin],
            &[host],
            &empty,
        )
        .unwrap()
        .unwrap();
        assert_eq!(proof.as_str(), origin);
        for wrong in [
            format!("{origin}/"),
            origin.replace("http:", "https:"),
            format!("{origin}:80"),
        ] {
            assert!(
                verify_same_origin(None, OriginRequirement::Required, &[wrong], &[host], &empty)
                    .is_err()
            );
        }
    }
}

#[test]
fn external_https_preserves_internal_host_and_exact_origin() {
    let external = ExternalOrigin::parse("HTTPS://EXAMPLE.COM:443").unwrap();
    assert_eq!(external.as_str(), "https://example.com");
    for hosts in [vec![], vec!["127.0.0.1:7400"]] {
        assert!(
            verify_same_origin(
                Some(&external),
                OriginRequirement::Required,
                &["https://example.com"],
                &hosts,
                &["same-origin"]
            )
            .is_ok()
        );
    }
    for wrong in [
        "https://EXAMPLE.COM",
        "https://example.com:443",
        "http://example.com",
        "null",
        "https://example.com/",
        "https://example.com?x",
        "https://user@example.com",
        "https://evil.example",
    ] {
        assert_eq!(
            verify_same_origin(
                Some(&external),
                OriginRequirement::Required,
                &[wrong],
                &["internal:7400"],
                &["same-origin"]
            ),
            Err(Error::OriginMismatch)
        );
    }
    for invalid in [
        "http://example.com",
        "https://example.com/",
        "https://example.com/path",
        "https://user@example.com",
        "https://example.com?x",
        "https://example.com#x",
    ] {
        assert!(ExternalOrigin::parse(invalid).is_err());
    }
}

#[test]
fn origin_policy_rejects_duplicate_and_joined_security_fields() {
    let origin = ["http://example.com"];
    let host = ["example.com"];
    let site = ["same-origin"];
    for (origins, hosts, sites) in [
        (vec![origin[0], origin[0]], host.to_vec(), site.to_vec()),
        (
            origin.to_vec(),
            vec![host[0], "evil.example"],
            site.to_vec(),
        ),
        (origin.to_vec(), host.to_vec(), vec![site[0], "cross-site"]),
        (
            vec!["http://example.com,http://evil.example"],
            host.to_vec(),
            site.to_vec(),
        ),
        (
            origin.to_vec(),
            vec!["example.com,evil.example"],
            site.to_vec(),
        ),
        (
            origin.to_vec(),
            host.to_vec(),
            vec!["same-origin,cross-site"],
        ),
        (origin.to_vec(), host.to_vec(), vec!["cross-site"]),
    ] {
        assert!(
            verify_same_origin(None, OriginRequirement::Required, &origins, &hosts, &sites)
                .is_err()
        );
    }
    let empty: [&str; 0] = [];
    assert!(
        verify_same_origin(None, OriginRequirement::Optional, &empty, &host, &empty)
            .unwrap()
            .is_none()
    );
    assert!(
        verify_same_origin(
            None,
            OriginRequirement::Optional,
            &empty,
            &host,
            &["cross-site"]
        )
        .is_ok()
    );
    assert!(require_same_origin_fetch_metadata(&["cross-site"]).is_err());
    assert!(require_same_origin_fetch_metadata(&empty).is_ok());
    assert!(verify_same_origin(None, OriginRequirement::Required, &empty, &host, &empty).is_err());
    assert!(verify_same_origin(None, OriginRequirement::Required, &origin, &empty, &site).is_err());
    assert!(verify_same_origin(None, OriginRequirement::Required, &origin, &host, &empty).is_ok());
    assert!(
        verify_same_origin(
            None,
            OriginRequirement::Optional,
            &empty,
            &host,
            &["same-origin", "cross-site"]
        )
        .is_err()
    );
}

#[test]
fn origin_url_failure_retains_concrete_source() {
    use std::error::Error as _;
    let error = ExternalOrigin::parse("https://[").unwrap_err();
    assert!(
        error
            .source()
            .unwrap()
            .is::<url_encoding::address::ParseError>()
    );
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

#[path = "../origin_vectors.rs"]
mod origin_vectors;

#[test]
fn shared_deployment_origin_vectors() {
    for &(external, host, origin, accepted) in origin_vectors::ORIGIN_VECTORS {
        let external = external.map(|value| ExternalOrigin::parse(value).unwrap());
        assert_eq!(
            verify_same_origin(
                external.as_ref(),
                OriginRequirement::Required,
                &[origin],
                &[host],
                &[] as &[&str]
            )
            .is_ok(),
            accepted,
            "{external:?} {host} {origin}"
        );
    }
}

#[test]
fn malformed_authorities_and_non_ascii_headers_fail_at_the_shared_boundary() {
    for host in [
        "user@example.com",
        "example.com/path",
        "example.com?x",
        "example.com#x",
        "example.com\\evil",
        "[",
        "::1",
        "example.com,evil",
    ] {
        assert!(
            verify_same_origin(
                None,
                OriginRequirement::Required,
                &["http://example.com"],
                &[host],
                &[] as &[&str]
            )
            .is_err(),
            "{host}"
        );
    }
    for origin in [
        b"http://example.com ".as_slice(),
        b"http://example.com\n",
        b"\xff",
    ] {
        assert!(
            verify_same_origin(
                None,
                OriginRequirement::Required,
                &[origin],
                &["example.com"],
                &[] as &[&str]
            )
            .is_err()
        );
    }
}
