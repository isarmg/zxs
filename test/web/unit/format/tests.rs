use super::*;
#[test]
fn human_sizes_use_binary_units_in_both_languages() {
    assert_eq!(bytes(1024), "1.0 KiB");
    assert_eq!(bytes(42), "42 B");
    assert!(bytes(u64::MAX).ends_with("EiB"));
}

#[test]
fn server_utc_timestamps_are_displayed_without_browser_conversion() {
    for value in ["2026-10-24T01:00:00Z", "2026-10-24T01:00:00.123Z"] {
        assert_eq!(timestamp(value), value);
    }
    assert_eq!(timestamp(""), t("尚无记录", "No record yet"));
    assert_eq!(timestamp("—"), t("尚无记录", "No record yet"));
}

#[test]
fn human_sizes_cover_every_binary_unit_boundary() {
    assert_eq!(bytes(0), "0 B");
    assert_eq!(bytes(1023), "1023 B");
    for (power, unit) in ["KiB", "MiB", "GiB", "TiB", "PiB", "EiB"]
        .into_iter()
        .enumerate()
    {
        let boundary = 1_u64 << (10 * (power + 1));
        assert_eq!(bytes(boundary), format!("1.0 {unit}"));
        assert_eq!(bytes(boundary + 1), format!("1.0 {unit}"));
    }
    assert_eq!(bytes(u64::MAX), "16.0 EiB");
}
