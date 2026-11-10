use date_time::OffsetDateTime;
use storage_contract::disk_details::{
    DiskDetailsRecord, DiskHealth, DiskHealthAssessment, DiskHealthUnavailable, DiskMapping,
};
use web_interface::i18n::t;

#[derive(Debug, error_definition::Error)]
pub enum InvalidHealth {
    #[error("a SMART result requires a single-device mapping")]
    Mapping,
    #[error("the SMART observation time cannot be represented as UTC")]
    Timestamp(#[source] date_time::error::ComponentRange),
    #[error("the SMART result has no absolute device path")]
    DevicePath,
}

fn observation_time(milliseconds: u64) -> Result<OffsetDateTime, date_time::error::ComponentRange> {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(milliseconds) * 1_000_000)
}

pub(super) fn validate(record: &DiskDetailsRecord) -> Result<(), InvalidHealth> {
    if let DiskHealth::Smart {
        observed_at_unix_ms,
        device_path,
        ..
    } = &record.health
    {
        if record.mapping != DiskMapping::Single {
            return Err(InvalidHealth::Mapping);
        }
        if !device_path.starts_with("/dev/") || device_path.len() <= "/dev/".len() {
            return Err(InvalidHealth::DevicePath);
        }
        observation_time(*observed_at_unix_ms).map_err(InvalidHealth::Timestamp)?;
    }
    Ok(())
}

pub(super) struct Presentation {
    pub state: &'static str,
    pub status: String,
    pub fields: Vec<(String, String)>,
}

fn reason_label(reason: DiskHealthUnavailable) -> String {
    match reason {
        DiskHealthUnavailable::MappingUnknown => t(
            "未能确认目录对应的实际设备，未读取健康信息。",
            "The directory's device mapping is unconfirmed; health was not read.",
        ),
        DiskHealthUnavailable::MultipleDevices => t(
            "目录涉及多个设备，不能用一块硬盘的结果代表全部设备。",
            "The directory spans multiple devices; one disk's report cannot represent them all.",
        ),
        DiskHealthUnavailable::UnsupportedDevice => t(
            "当前设备不支持此只读健康查询。",
            "This device does not support the read-only health query.",
        ),
        DiskHealthUnavailable::ToolMissing => t(
            "服务器缺少此设备所需的查询支持，无法读取 SMART。",
            "Required device-query support is unavailable on the server; SMART could not be read.",
        ),
        DiskHealthUnavailable::PermissionDenied => t(
            "服务没有读取设备健康信息的权限；不会自动提升权限。",
            "The service lacks permission to read device health; permissions are not elevated automatically.",
        ),
        DiskHealthUnavailable::DeviceChanged => t(
            "读取期间目录或设备映射发生变化，结果已丢弃。请刷新。",
            "The directory or device mapping changed during the read. The report was discarded; refresh to retry.",
        ),
        DiskHealthUnavailable::Busy => t(
            "健康读取正在忙，请稍后刷新。",
            "Device-health reading is busy; refresh later.",
        ),
        DiskHealthUnavailable::TimedOut => t(
            "设备健康读取超时，未获得有效结果。",
            "The device-health read timed out; no valid report was received.",
        ),
        DiskHealthUnavailable::QueryFailed => t(
            "设备健康查询失败，不能据此判断硬盘健康。",
            "The device-health query failed; this does not establish disk health.",
        ),
        DiskHealthUnavailable::InvalidOutput => t(
            "设备健康响应格式无效，未采纳结果。",
            "The device-health response was invalid and was not accepted.",
        ),
        DiskHealthUnavailable::IncompleteOutput => t(
            "设备未提供完整的 SMART 总体状态，无法判断。",
            "The device did not provide a complete SMART overall status; health is unknown.",
        ),
        DiskHealthUnavailable::LowPower => t(
            "设备处于低功耗状态，未唤醒设备读取健康信息。",
            "The device is in a low-power state; it was not woken for a health read.",
        ),
    }
}

fn utc_label(milliseconds: u64) -> String {
    let value = observation_time(milliseconds)
        .expect("invariant: disk response boundary validates its observation time");
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        value.year(),
        u8::from(value.month()),
        value.day(),
        value.hour(),
        value.minute(),
        value.second(),
        value.millisecond(),
    )
}

pub(super) fn presentation(health: &DiskHealth) -> Presentation {
    match health {
        DiskHealth::Unavailable { reason } => Presentation {
            state: "unavailable",
            status: t(
                "硬盘健康：不可用／未知",
                "Disk health: unavailable / unknown",
            ),
            fields: vec![(t("原因", "Reason"), reason_label(*reason))],
        },
        DiskHealth::Smart {
            observed_at_unix_ms,
            device_path,
            assessment,
            temperature_celsius,
            power_on_hours,
        } => Presentation {
            state: "smart",
            status: match assessment {
                DiskHealthAssessment::Passed => t(
                    "硬盘健康：SMART 报告通过",
                    "Disk health: SMART reports passed",
                ),
                DiskHealthAssessment::Warning => t(
                    "硬盘健康：SMART 有告警记录",
                    "Disk health: SMART reports recorded warnings",
                ),
                DiskHealthAssessment::Failed => t(
                    "硬盘健康：SMART 报告异常",
                    "Disk health: SMART reports failure",
                ),
            },
            fields: vec![
                (t("检测设备", "Queried device"), device_path.clone()),
                (t("来源", "Source"), t("设备 SMART", "Device SMART")),
                (
                    t("采样时间（UTC）", "Observed at (UTC)"),
                    utc_label(*observed_at_unix_ms),
                ),
                (
                    t("温度", "Temperature"),
                    temperature_celsius
                        .map(|value| format!("{value} °C"))
                        .unwrap_or_else(|| t("未提供", "Not reported")),
                ),
                (
                    t("累计通电时间", "Power-on time"),
                    power_on_hours
                        .map(|value| format!("{value} {}", t("小时", "hours")))
                        .unwrap_or_else(|| t("未提供", "Not reported")),
                ),
            ],
        },
    }
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/admin/disk_health_tests.rs"]
mod tests;
