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
    assert!(account_listing(b"share:4294967295:Name\n", "share", u32::MAX).is_ok());
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

#[test]
fn parameter_text_keeps_its_utf8_source() {
    let error = scalar(b"tdbsam\xff").unwrap_err();
    assert!(matches!(
        error.downcast_ref::<ObservationError>(),
        Some(ObservationError::ParameterText(_))
    ));
    let source = error
        .source()
        .unwrap()
        .downcast_ref::<std::str::Utf8Error>()
        .unwrap();
    assert_eq!(source.valid_up_to(), 6);
    assert_eq!(source.error_len(), Some(1));
}

#[test]
fn existing_account_text_keeps_its_utf8_source() {
    let error = account_listing(b"share:1001:\xff\n", "share", 1001).unwrap_err();
    assert!(matches!(
        error.downcast_ref::<ObservationError>(),
        Some(ObservationError::ExistingAccountText(_))
    ));
    let source = error
        .source()
        .unwrap()
        .downcast_ref::<std::str::Utf8Error>()
        .unwrap();
    assert_eq!(source.valid_up_to(), 11);
    assert_eq!(source.error_len(), Some(1));
}

#[test]
fn existing_account_uid_keeps_its_parse_source() {
    use std::num::IntErrorKind;

    for (uid, kind) in [
        ("", IntErrorKind::Empty),
        ("bad", IntErrorKind::InvalidDigit),
        ("-1", IntErrorKind::InvalidDigit),
        ("4294967296", IntErrorKind::PosOverflow),
    ] {
        let record = format!("share:{uid}:Name\n");
        let error = account_listing(record.as_bytes(), "share", 1001).unwrap_err();
        assert!(matches!(
            error.downcast_ref::<ObservationError>(),
            Some(ObservationError::ExistingAccountUid(_))
        ));
        let source = error
            .source()
            .unwrap()
            .downcast_ref::<std::num::ParseIntError>()
            .unwrap();
        assert_eq!(source.kind(), &kind);
    }
}

#[test]
fn record_count_rejection_still_precedes_existing_uid_decode() {
    let error = account_listing(b"share:bad:Name\nother:1002:Name\n", "share", 1001)
        .unwrap_err();
    assert!(error.downcast_ref::<ObservationError>().is_none());
    assert!(error.source().is_none());
}
