use super::*;
use storage_contract::ErrorCode;

fn disk(level: u32, online: bool, issue: Option<DiskObservationIssue>) -> Disk {
    Disk {
        name: "actual-directory".into(),
        slot_id: "registered-slot".into(),
        level: NonZeroU32::new(level).unwrap(),
        online,
        writable: level == 1,
        available_bytes: None,
        observation_failure: issue,
    }
}

#[test]
fn level_labels_use_the_reported_number_not_backup_count_or_path() {
    assert_eq!(
        level(NonZeroU32::new(1).unwrap()),
        t("第 1 级 · 主盘", "Level 1 · Primary")
    );
    for tier in [2, 3, 7, u32::MAX] {
        assert_eq!(
            level(NonZeroU32::new(tier).unwrap()),
            format!("{} {tier} · {}", t("层级", "Level"), t("备份盘", "Backup"))
        );
    }
}

#[test]
fn offline_does_not_hide_backup_role_or_read_only_mode() {
    let row = disk(3, false, None);
    assert_eq!(
        level(row.level),
        format!("{} 3 · {}", t("层级", "Level"), t("备份盘", "Backup"))
    );
    assert_eq!(access(row.writable), t("只读", "Read only"));
    assert_eq!(observation(&row), t("离线", "Offline"));
}

#[test]
fn online_backup_remains_read_only_without_becoming_a_primary() {
    for tier in [2, 3, 9] {
        let row = disk(tier, true, None);
        assert_eq!(access(row.writable), t("只读", "Read only"));
        assert_eq!(observation(&row), t("在线", "Online"));
    }
}

#[test]
fn primary_mode_does_not_assert_live_write_authority() {
    let mut row = disk(1, false, None);
    assert_eq!(access(row.writable), t("读写目录", "Read/write directory"));
    assert_eq!(observation(&row), t("离线", "Offline"));
    row.writable = false;
    assert_eq!(access(row.writable), t("只读", "Read only"));
    assert_eq!(level(row.level), t("第 1 级 · 主盘", "Level 1 · Primary"));
}

#[test]
fn identity_failure_does_not_infer_physical_offline_or_hidden_membership() {
    let row = disk(
        2,
        false,
        Some(DiskObservationIssue::Seal(ErrorCode::IoError)),
    );
    assert_eq!(
        observation(&row),
        t(
            "不可访问（身份观测失败）",
            "Unavailable (identity observation failed)"
        )
    );
    assert_eq!(access(row.writable), t("只读", "Read only"));
}

#[test]
fn capacity_failure_does_not_turn_an_online_disk_into_an_offline_one() {
    let row = disk(
        2,
        true,
        Some(DiskObservationIssue::Capacity(ErrorCode::IoError)),
    );
    assert_eq!(
        observation(&row),
        t("在线（容量不可用）", "Online (capacity unavailable)")
    );
    let row = disk(
        2,
        false,
        Some(DiskObservationIssue::Capacity(ErrorCode::DiskUnavailable)),
    );
    assert_eq!(
        observation(&row),
        t(
            "不可访问（容量观测失败）",
            "Unavailable (capacity observation failed)"
        )
    );
}

#[test]
fn labels_only_borrow_observations_and_do_not_change_any_mode_or_identity() {
    let row = disk(
        3,
        false,
        Some(DiskObservationIssue::Seal(ErrorCode::IoError)),
    );
    let original = row.clone();
    let _ = level(row.level);
    let _ = access(row.writable);
    let _ = observation(&row);
    assert_eq!(row, original);
}
