use super::*;
use json_codec::json;

fn smart(assessment: DiskHealthAssessment) -> DiskHealth {
    DiskHealth::Smart {
        observed_at_unix_ms: 1_700_000_000_123,
        device_path: "/dev/nvme0n1".into(),
        assessment,
        temperature_celsius: Some(0),
        power_on_hours: Some(0),
    }
}

fn card(health: DiskHealth) -> DiskDetailsRecord {
    DiskDetailsRecord {
        slot_id: "P01".into(),
        disk_id: "zxs-id".into(),
        root_path: "/data/zxs".into(),
        mount_path: Some("/data".into()),
        model: None,
        serial: None,
        capacity_bytes: None,
        device_path: Some("/dev/nvme0n1p1".into()),
        filesystem_uuid: None,
        mapping: DiskMapping::Single,
        health,
        note: String::new(),
    }
}

#[test]
fn health_projection_keeps_actual_query_device_utc_time_and_zero_measurements() {
    let record = card(smart(DiskHealthAssessment::Passed));
    validate(&record).unwrap();
    let shown = presentation(&record.health);
    assert_eq!(shown.state, "smart");
    assert_eq!(shown.status, "Disk health: SMART reports passed");
    assert!(
        shown
            .fields
            .iter()
            .any(|(_, value)| value == "/dev/nvme0n1")
    );
    assert!(
        !shown
            .fields
            .iter()
            .any(|(_, value)| value == "/dev/nvme0n1p1")
    );
    assert!(
        shown
            .fields
            .iter()
            .any(|(_, value)| value == "2023-11-14T22:13:20.123Z")
    );
    assert!(shown.fields.iter().any(|(_, value)| value == "0 °C"));
    assert!(shown.fields.iter().any(|(_, value)| value == "0 hours"));
}

#[test]
fn missing_optional_measurements_are_not_zero_or_a_failed_health_report() {
    let health = DiskHealth::Smart {
        observed_at_unix_ms: 0,
        device_path: "/dev/sda".into(),
        assessment: DiskHealthAssessment::Failed,
        temperature_celsius: None,
        power_on_hours: None,
    };
    let shown = presentation(&health);
    assert_eq!(shown.status, "Disk health: SMART reports failure");
    assert_eq!(
        shown
            .fields
            .iter()
            .filter(|(_, value)| value == "Not reported")
            .count(),
        2
    );
    assert!(
        shown
            .fields
            .iter()
            .any(|(_, value)| value == "1970-01-01T00:00:00.000Z")
    );
}

#[test]
fn every_unavailable_reason_stays_unknown_instead_of_inventing_a_health_status() {
    for reason in [
        DiskHealthUnavailable::MappingUnknown,
        DiskHealthUnavailable::MultipleDevices,
        DiskHealthUnavailable::UnsupportedDevice,
        DiskHealthUnavailable::ToolMissing,
        DiskHealthUnavailable::PermissionDenied,
        DiskHealthUnavailable::DeviceChanged,
        DiskHealthUnavailable::Busy,
        DiskHealthUnavailable::TimedOut,
        DiskHealthUnavailable::LowPower,
        DiskHealthUnavailable::QueryFailed,
        DiskHealthUnavailable::InvalidOutput,
        DiskHealthUnavailable::IncompleteOutput,
    ] {
        let shown = presentation(&DiskHealth::Unavailable { reason });
        assert_eq!(shown.state, "unavailable");
        assert_eq!(shown.status, "Disk health: unavailable / unknown");
        assert_eq!(shown.fields.len(), 1);
        assert!(!shown.fields[0].1.is_empty());
        assert!(!shown.fields.iter().any(|(_, value)| value.contains(" °C")));
    }
}

#[test]
fn response_boundary_rejects_health_attributed_to_unknown_or_multiple_devices() {
    for mapping in [DiskMapping::Multiple, DiskMapping::Unknown] {
        let mut record = card(smart(DiskHealthAssessment::Passed));
        record.mapping = mapping;
        assert!(matches!(validate(&record), Err(InvalidHealth::Mapping)));
        assert!(super::super::read_details("P01", json_codec::to_value(record).unwrap()).is_err());
    }
}

#[test]
fn response_boundary_rejects_invalid_health_time_device_and_missing_reports() {
    let source = json_codec::to_value(card(smart(DiskHealthAssessment::Passed))).unwrap();
    for (field, value) in [
        ("observed_at_unix_ms", json!(u64::MAX.to_string())),
        ("device_path", json!("")),
        ("device_path", json!("relative-device")),
        ("assessment", json!("unknown")),
    ] {
        let mut invalid = source.clone();
        invalid["health"][field] = value;
        assert!(
            super::super::read_details("P01", invalid).is_err(),
            "{field}"
        );
    }
    let mut missing = source;
    missing.as_object_mut().unwrap().remove("health");
    assert!(super::super::read_details("P01", missing).is_err());
}

#[test]
fn historical_smart_warnings_are_not_shown_as_passed_or_current_failure() {
    let shown = presentation(&smart(DiskHealthAssessment::Warning));
    assert_eq!(shown.state, "smart");
    assert_eq!(shown.status, "Disk health: SMART reports recorded warnings");
}
