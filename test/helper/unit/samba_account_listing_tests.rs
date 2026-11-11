use super::*;

#[test]
fn native_listing_retains_empty_sets_and_exact_account_fields() {
    assert!(account_listing(b"").unwrap().is_empty());
    let accounts = account_listing(b"second:4294967295:Display:Name\nfirst:1001:\n").unwrap();
    assert_eq!(accounts.len(), 2);
    assert_eq!(accounts["first"].uid, 1001);
    assert_eq!(accounts["first"].description, "");
    assert_eq!(accounts["second"].uid, u32::MAX);
    assert_eq!(accounts["second"].description, "Display:Name");
}

#[test]
fn native_listing_rejects_incomplete_control_and_repeated_records() {
    for bytes in [
        b"\n".as_slice(),
        b"share\n",
        b"share:1001\n",
        b":1001:Name\n",
        b"share:1001:Name\nshare:1002:Other\n",
        b"share:1001:Name\n\n",
        b"share:1001:Tab\tName\n",
        b"share:1001:Nul\0Name\n",
    ] {
        assert!(account_listing(bytes).is_err());
    }
}

#[test]
fn native_listing_text_keeps_its_utf8_source() {
    let error = account_listing(b"share:1001:\xff\n").err().unwrap();
    assert!(matches!(
        error.downcast_ref::<ObservationError>(),
        Some(ObservationError::NativeAccountsText(_))
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
fn native_listing_uid_keeps_its_parse_source() {
    use std::num::IntErrorKind;

    for (uid, kind) in [
        ("", IntErrorKind::Empty),
        ("bad", IntErrorKind::InvalidDigit),
        ("-1", IntErrorKind::InvalidDigit),
        ("4294967296", IntErrorKind::PosOverflow),
    ] {
        let record = format!("share:{uid}:Name\n");
        let error = account_listing(record.as_bytes()).err().unwrap();
        assert!(matches!(
            error.downcast_ref::<ObservationError>(),
            Some(ObservationError::NativeAccountUid(_))
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
fn record_controls_are_rejected_before_native_uid_decode() {
    let error = account_listing(b"share:bad:Tab\tName\n").err().unwrap();
    assert!(error.downcast_ref::<ObservationError>().is_none());
    assert!(error.source().is_none());
}
