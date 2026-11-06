use super::*;

#[test]
fn only_a_current_disk_list_has_no_read_notice() {
    assert!(DiskRead::Current.message().is_none());
    for state in [
        DiskRead::Reading,
        DiskRead::RefreshingCached,
        DiskRead::Unconfirmed,
        DiskRead::UnconfirmedCached,
    ] {
        assert!(state.message().is_some_and(|message| !message.is_empty()));
    }
}

#[test]
fn unconfirmed_empty_is_not_a_successful_empty_or_offline_disk() {
    assert_eq!(
        DiskRead::Unconfirmed.message(),
        Some(t(
            "尚未取得磁盘列表，请重新读取。",
            "The disk list is not available yet. Read it again.",
        ))
    );
    assert_eq!(DiskRead::Unconfirmed.marker(), "unconfirmed");
    assert!(DiskRead::Unconfirmed.can_retry());
}

#[test]
fn cached_read_failure_keeps_a_notice_and_an_explicit_retry() {
    assert_eq!(
        DiskRead::UnconfirmedCached.message(),
        Some(t(
            "磁盘列表尚未更新，当前显示上次读取结果。请重新读取以确认可用性。",
            "The disk list has not been refreshed. Showing the previous result; read it again to check availability.",
        ))
    );
    assert_eq!(DiskRead::UnconfirmedCached.marker(), "unconfirmed-cached");
    assert!(DiskRead::UnconfirmedCached.can_retry());
}

#[test]
fn active_reads_do_not_offer_duplicate_retries() {
    for state in [DiskRead::Reading, DiskRead::RefreshingCached] {
        assert!(!state.can_retry());
    }
    assert_eq!(DiskRead::Reading.marker(), "reading");
    assert_eq!(DiskRead::RefreshingCached.marker(), "refreshing-cached");
    assert_ne!(
        DiskRead::Reading.message(),
        DiskRead::RefreshingCached.message()
    );
}

#[test]
fn completed_read_removes_the_notice_and_retry() {
    assert_eq!(DiskRead::Current.marker(), "current");
    assert!(!DiskRead::Current.can_retry());
    assert!(DiskRead::Current.message().is_none());
}
