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

fn stored_revision() -> json_codec::Value {
    json_codec::json!({
        "parent_device":"17", "parent_inode":"24",
        "file":{"device":"17","inode":"301","bytes":"4096","uid":0,"gid":0,
        "mode":0o100600_u32,"links":"1","modified_seconds":"-10","modified_nanos":"2",
        "changed_seconds":"12","changed_nanos":"340"}
    })
}

#[test]
fn historical_reset_requires_the_same_private_parent_and_single_link_database() {
    let source = stored_revision();
    let original: Revision = json_codec::from_value(source.clone()).unwrap();
    let mut updated = source.clone();
    updated["file"]["bytes"] = "8192".into();
    updated["file"]["changed_seconds"] = "99".into();
    let after: Revision = json_codec::from_value(updated).unwrap();
    assert!(original.accepts_after(&after));

    for (key, wrong) in [
        ("device", json_codec::json!("18")), ("inode", json_codec::json!("302")),
        ("links", json_codec::json!("2")), ("uid", json_codec::json!(99)),
        ("mode", json_codec::json!(0o100644_u32)), ("mode", json_codec::json!(0o040600_u32)),
        ("bytes", json_codec::json!("0")), ("bytes", json_codec::json!("09")),
        ("modified_nanos", json_codec::json!("1000000000")), ("changed_seconds", json_codec::json!("-0")),
    ] {
        let mut value = source.clone();
        value["file"][key] = wrong;
        assert!(!original.accepts_after(&json_codec::from_value(value).unwrap()), "{key}");
    }
    for (key, wrong) in [("parent_device","9"), ("parent_inode","25"), ("parent_inode","024")] {
        let mut value = source.clone();
        value[key] = wrong.into();
        assert!(!original.accepts_after(&json_codec::from_value(value).unwrap()), "{key}");
    }
}

#[test]
fn enrollment_only_allows_a_new_inode_after_a_genuinely_absent_database() {
    let source = stored_revision();
    let baseline = |before: json_codec::Value| -> enrollment::Baseline {
        json_codec::from_value(json_codec::json!({
            "parent_device":"17", "parent_inode":"24",
            "database":before, "accounts_digest":"a".repeat(64)
        })).unwrap()
    };
    let original = baseline(source["file"].clone());
    let absent = baseline(json_codec::Value::Null);
    let mut after = source;
    after["file"]["inode"] = "302".into();
    let created: Revision = json_codec::from_value(after.clone()).unwrap();
    assert!(absent.accepts_after(&created));
    assert!(!original.accepts_after(&created));
    after["parent_inode"] = "25".into();
    assert!(!absent.accepts_after(&json_codec::from_value(after.clone()).unwrap()));
    after["parent_inode"] = "24".into();
    after["file"]["links"] = "2".into();
    assert!(!absent.accepts_after(&json_codec::from_value(after).unwrap()));
}
