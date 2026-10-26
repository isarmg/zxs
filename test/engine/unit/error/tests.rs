use super::*;
use std::error::Error as _;

#[test]
fn contextual_io_preserves_origin_and_explicit_retry_policy() {
    let error = Error::from(std::io::Error::from_raw_os_error(28))
        .context(ErrorCode::RecoveryRequired, "cannot read recovery evidence")
        .retryable();
    assert_eq!(error.code(), ErrorCode::RecoveryRequired);
    assert_eq!(error.message(), "cannot read recovery evidence");
    assert!(error.is_retryable());
    let origin = error.source().unwrap().source().unwrap();
    assert_eq!(
        origin
            .downcast_ref::<std::io::Error>()
            .unwrap()
            .raw_os_error(),
        Some(28)
    );
    let remapped = error.context(ErrorCode::StateInvalid, "invalid recovered state");
    assert!(!remapped.is_retryable());
}

#[test]
fn filesystem_context_keeps_durability_and_path_status_policy() {
    let error = crate::disk::fs_error(platform_io::fs_safety::Error::PublishedDurabilityUnknown(
        std::io::Error::from_raw_os_error(28),
    ));
    assert_eq!(error.code(), ErrorCode::DurabilityUnknown);
    assert!(error.to_string().starts_with("DURABILITY_UNKNOWN: "));
    let origin = error.source().unwrap().source().unwrap();
    assert_eq!(
        origin
            .downcast_ref::<std::io::Error>()
            .unwrap()
            .raw_os_error(),
        Some(28)
    );
    for (raw, expected) in [
        (20, ErrorCode::UnsafePath),
        (40, ErrorCode::UnsafePath),
        (28, ErrorCode::NoSpace),
        (5, ErrorCode::IoError),
    ] {
        let error = crate::disk::fs_error(platform_io::fs_safety::Error::Io(
            std::io::Error::from_raw_os_error(raw),
        ));
        assert_eq!(error.code(), expected);
        let filesystem = error.source().unwrap();
        assert!(filesystem.is::<platform_io::fs_safety::Error>());
        assert_eq!(
            filesystem
                .source()
                .unwrap()
                .downcast_ref::<std::io::Error>()
                .unwrap()
                .raw_os_error(),
            Some(raw)
        );
    }
}

#[test]
fn sqlite_and_payload_errors_retain_concrete_causes() {
    let error = Error::from(sqlite_access::Error::Io(std::io::Error::from_raw_os_error(
        5,
    )));
    assert_eq!(error.code(), ErrorCode::IndexRebuildFailed);
    assert!(error.source().unwrap().is::<sqlite_access::Error>());
    assert_eq!(
        error
            .source()
            .unwrap()
            .source()
            .unwrap()
            .downcast_ref::<std::io::Error>()
            .unwrap()
            .raw_os_error(),
        Some(5)
    );
    let error = Error::from(platform_io::payload::Error::Digest).context(
        ErrorCode::RecoveryRequired,
        "workspace mirror verification failed",
    );
    assert_eq!(error.code(), ErrorCode::RecoveryRequired);
    assert!(
        error
            .source()
            .unwrap()
            .source()
            .unwrap()
            .is::<platform_io::payload::Error>()
    );
}

#[test]
fn out_of_rfc3339_clock_range_preserves_format_cause() {
    let timestamp = date_time::Date::from_calendar_date(-1, date_time::Month::January, 1)
        .unwrap()
        .midnight()
        .assume_utc();
    let source = timestamp
        .format(&date_time::format_description::well_known::Rfc3339)
        .unwrap_err();
    let error = Error::from(source);
    assert_eq!(error.code(), ErrorCode::ContainerLayoutInvalid);
    assert!(error.source().unwrap().is::<date_time::error::Format>());
}

#[test]
fn cleanup_failure_preserves_both_errors_and_original_status() {
    let error = Error::Cleanup {
        operation: Box::new(Error::from(std::io::Error::from_raw_os_error(28))),
        cleanup: std::io::Error::from_raw_os_error(13),
    }
    .retryable();
    assert_eq!(error.code(), ErrorCode::NoSpace);
    assert!(error.is_retryable());
    assert!(error.message().contains("cleanup failed"));
    assert!(error.source().unwrap().to_string().starts_with("NO_SPACE:"));
    let Error::Cleanup { operation, cleanup } = error else {
        panic!("cleanup cause lost")
    };
    assert_eq!(operation.code(), ErrorCode::NoSpace);
    assert_eq!(cleanup.raw_os_error(), Some(13));
}
