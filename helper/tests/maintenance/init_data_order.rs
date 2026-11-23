use super::*;

#[test]
fn cli_requires_an_explicit_target_and_the_full_role_shape() {
    let parse = |words: &[&str]| {
        crate::headless_setup_command::Setup::parse(
            words.iter().copied().map(std::ffi::OsString::from),
        )
    };
    assert!(parse(&["init-data", "/new", "/system", "/recycle", "/old", "/new"]).is_ok());
    assert!(parse(&["init-data", "/new", "/system", "/recycle"]).is_err());
    assert!(parse(&["init-data", "relative", "/system", "/recycle", "/data"]).is_err());
    assert!(parse(&["web", "init-data", "/new", "/system", "/recycle", "/new"]).is_err());
}

#[test]
fn fixed_order_binds_pool_token_and_raw_target_path() {
    let roles = [[11, 12, 13], [21, 22, 23], [31, 32, 33]];
    let raw = encode([1; 32], [2; 32], [3; 16], [4; 32], &roles, b"/disk/\xff").unwrap();
    assert_eq!(&raw[..8], b"ZXINITD1");
    assert_eq!(&raw[8..40], &[1; 32]);
    assert_eq!(&raw[40..72], &[2; 32]);
    assert_eq!(&raw[72..88], &[3; 16]);
    assert_eq!(&raw[88..120], &[4; 32]);
    assert_eq!(raw[120], 3);
    let identities: Vec<_> = roles
        .into_iter()
        .flatten()
        .flat_map(u64::to_be_bytes)
        .collect();
    assert_eq!(&raw[121..193], identities.as_slice());
    assert_eq!(&raw[193..195], &7_u16.to_be_bytes());
    assert_eq!(&raw[195..], b"/disk/\xff");
    assert_ne!(step(1, 4), step(2, 4));
    assert_ne!(step(3, 2), step(3, 1));
}
