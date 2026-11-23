use super::*;

fn hex<const N: usize>(s: &str) -> [u8; N] {
    assert_eq!(s.len(), 2 * N);
    std::array::from_fn(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap())
}

#[test]
fn fresh_argon2id_verifier_matches_the_known_raw_tag() {
    // OpenSSL ARGON2ID, version 19, memory 65536 KiB, 3 iterations, 4 lanes.
    let salt = hex("1a2b3c4d5e6f708192a3b4c5d6e7f809");
    let tag = hex("cf7e9879a140233c9a38a16019d03dfcd127d1855386defe40fa2f39ab83e8c2");
    let password = "fresh workspace test secret";
    let verifier = Verifier::new(salt, tag);
    assert!(verifier.matches(password).unwrap());
    assert!(!verifier.matches("fresh workspace test secrets").unwrap());
    let mut changed = tag;
    changed[31] ^= 1;
    assert!(!Verifier::new(salt, changed).matches(password).unwrap());
}

#[test]
fn argon2id_uses_the_exact_password_bytes() {
    let verifier = Verifier::new(
        hex("000102030405060708090a0b0c0d0e0f"),
        hex("0225c3954d8493e58cba41547b19de49f3079a9507b30b4b687779eb13ee7fe8"),
    );
    assert!(verifier.matches("café\0中文").unwrap());
    assert!(!verifier.matches("café").unwrap());
    assert!(!verifier.matches("cafe\u{301}\0中文").unwrap());
    assert!(!verifier.matches("").unwrap());
    assert!(!verifier.matches(&"x".repeat(1025)).unwrap());
}

#[test]
fn login_accepts_only_the_exact_account_spelling() {
    assert_eq!(
        login_name(b"workspace.operator").unwrap(),
        "workspace.operator"
    );
    for invalid in [
        b"".as_slice(),
        b"Workspace.operator",
        b"-operator",
        b"workspace\x00operator",
        b"workspace/operator",
    ] {
        assert_eq!(
            login_name(invalid).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }
}
