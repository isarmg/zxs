//! Labels describe the registered role and one observation.
use crate::files::model::Disk;
use std::num::NonZeroU32;
use storage_contract::DiskObservationIssue;
use web_interface::i18n::t;

pub(super) fn level(level: NonZeroU32) -> String {
    if level.get() == 1 {
        t("第 1 级 · 主盘", "Level 1 · Primary")
    } else {
        format!(
            "{} {} · {}",
            t("层级", "Level"),
            level,
            t("备份盘", "Backup")
        )
    }
}

pub(super) fn access(writable: bool) -> String {
    if writable {
        t("读写目录", "Read/write directory")
    } else {
        t("只读", "Read only")
    }
}

pub(super) fn observation(disk: &Disk) -> String {
    match disk.observation_failure {
        Some(DiskObservationIssue::Seal(_)) => t(
            "不可访问（身份观测失败）",
            "Unavailable (identity observation failed)",
        ),
        Some(DiskObservationIssue::Capacity(_)) if disk.online => {
            t("在线（容量不可用）", "Online (capacity unavailable)")
        }
        Some(DiskObservationIssue::Capacity(_)) => t(
            "不可访问（容量观测失败）",
            "Unavailable (capacity observation failed)",
        ),
        None if disk.online => t("在线", "Online"),
        None => t("离线", "Offline"),
    }
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/view/disk_status_tests.rs"]
mod tests;
