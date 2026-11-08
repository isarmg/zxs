use super::*;
#[test]
fn parameter_boundary_requires_one_nonempty_scalar() {
    assert_eq!(scalar(b"tdbsam\n").unwrap(), "tdbsam");
    assert_eq!(scalar(b"/var/lib/samba/private\n").unwrap(), PRIVATE);
    for bad in [
        b"".as_slice(),
        b"\n",
        b"tdbsam\nother",
        b"tdbsam\0",
        b"\xff",
    ] {
        assert!(scalar(bad).is_err());
    }
}
#[test]
fn exact_single_existing_account_is_required_without_hash_export() {
    assert!(account_listing(b"share:1001:Display Name\n", "share", 1001).is_ok());
    assert!(account_listing(b"share:1001:\n", "share", 1001).is_ok());
    for bad in [
        b"".as_slice(),
        b"share:1001\n",
        b"other:1001:Name\n",
        b"share:1002:Name\n",
        b"share:bad:Name\n",
        b"share:1001:Name\nother:1002:Name\n",
        b"share:1001:Name\n\n",
    ] {
        assert!(account_listing(bad, "share", 1001).is_err());
    }
}
