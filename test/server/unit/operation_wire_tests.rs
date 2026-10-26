use super::*;
use std::{
    error::Error as _,
    time::{Duration, UNIX_EPOCH},
};

#[test]
fn system_clock_health_time_preserves_fraction_and_calendar_boundary() {
    assert_eq!(checked_at(UNIX_EPOCH).unwrap(), "1970-01-01T00:00:00Z");
    assert_eq!(
        checked_at(UNIX_EPOCH + Duration::new(42, 123_000_000)).unwrap(),
        "1970-01-01T00:00:42.123Z"
    );
    let last = UNIX_EPOCH + Duration::new(253_402_300_799, 999_999_999);
    assert_eq!(checked_at(last).unwrap(), "9999-12-31T23:59:59.999999999Z");
    let error = checked_at(last + Duration::from_nanos(1)).unwrap_err();
    assert_eq!(error.status, ::http::StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(error.code, storage_contract::ErrorCode::IoError);
    assert!(
        error
            .source()
            .unwrap()
            .source()
            .unwrap()
            .is::<date_time::error::ComponentRange>()
    );
    let error = checked_at(UNIX_EPOCH - Duration::from_nanos(1)).unwrap_err();
    assert_eq!(error.status, ::http::StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        error
            .source()
            .unwrap()
            .source()
            .unwrap()
            .is::<std::time::SystemTimeError>()
    );
}
