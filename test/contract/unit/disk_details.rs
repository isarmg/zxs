use super::*;
use json_codec::json;

fn details() -> DiskDetailsRecord {
    DiskDetailsRecord {
        slot_id: "P01".into(),
        disk_id: "disk_00000000000000000000000000000001".into(),
        root_path: "/data/zxs".into(),
        mount_path: Some("/data".into()),
        model: None,
        serial: None,
        capacity_bytes: Some(u64::MAX),
        device_path: None,
        filesystem_uuid: None,
        mapping: DiskMapping::Unknown,
        health: DiskHealth::Unavailable {
            reason: DiskHealthUnavailable::MappingUnknown,
        },
        note: "自由备注\n第二行 <script>not executable</script>".into(),
    }
}

#[test]
fn card_roundtrip_preserves_large_decimal_capacity_multiline_notes_and_unknown_fields() {
    let source = details();
    let value = json_codec::to_value(&source).unwrap();
    assert_eq!(value["capacity_bytes"], u64::MAX.to_string());
    assert_eq!(value["filesystem_uuid"], json!(null));
    assert_eq!(value["mapping"], "unknown");
    assert_eq!(
        json_codec::from_value::<DiskDetailsRecord>(value).unwrap(),
        source
    );
}

#[test]
fn card_nullable_fields_are_explicit_and_capacity_is_not_a_json_number() {
    let value = json_codec::to_value(details()).unwrap();
    for field in [
        "mount_path",
        "model",
        "serial",
        "capacity_bytes",
        "device_path",
        "filesystem_uuid",
    ] {
        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(
            json_codec::from_value::<DiskDetailsRecord>(missing).is_err(),
            "{field}"
        );
    }
    let mut invalid = value.clone();
    invalid["capacity_bytes"] = json!(12);
    assert!(json_codec::from_value::<DiskDetailsRecord>(invalid).is_err());
    let mut invalid = value;
    invalid["physical_slot"] = json!("not part of this feature");
    assert!(json_codec::from_value::<DiskDetailsRecord>(invalid).is_err());
}

#[test]
fn mapping_is_a_closed_string_contract() {
    for (text, expected) in [
        ("single", DiskMapping::Single),
        ("multiple", DiskMapping::Multiple),
        ("unknown", DiskMapping::Unknown),
    ] {
        assert_eq!(
            json_codec::from_value::<DiskMapping>(json!(text)).unwrap(),
            expected
        );
    }
    for value in [
        json!("physical"),
        json!({"single":null}),
        json!(null),
        json!(1),
    ] {
        assert!(json_codec::from_value::<DiskMapping>(value).is_err());
    }
}

#[test]
fn note_command_requires_all_strings_and_allows_plain_multiline_text() {
    let value =
        json!({"expected_disk_id":"disk_id", "expected_note":"before", "note":"  after\n\t备忘  "});
    let command: DiskNoteRequest = json_codec::from_value(value.clone()).unwrap();
    assert_eq!(command.note, "  after\n\t备忘  ");
    for field in ["expected_disk_id", "expected_note", "note"] {
        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(json_codec::from_value::<DiskNoteRequest>(missing).is_err());
        let mut null = value.clone();
        null[field] = json!(null);
        assert!(json_codec::from_value::<DiskNoteRequest>(null).is_err());
    }
    let mut extra = value;
    extra["physical_slot"] = json!("5");
    assert!(json_codec::from_value::<DiskNoteRequest>(extra).is_err());
}

#[test]
fn smart_health_requires_observation_and_explicit_nullable_metrics() {
    let source = DiskHealth::Smart {
        observed_at_unix_ms: u64::MAX,
        device_path: "/dev/nvme0n1".into(),
        assessment: DiskHealthAssessment::Passed,
        temperature_celsius: None,
        power_on_hours: Some(u64::MAX),
    };
    let value = json_codec::to_value(&source).unwrap();
    assert_eq!(value["observed_at_unix_ms"], u64::MAX.to_string());
    assert_eq!(
        json_codec::from_value::<DiskHealth>(value.clone()).unwrap(),
        source
    );
    for field in [
        "state",
        "observed_at_unix_ms",
        "device_path",
        "assessment",
        "temperature_celsius",
        "power_on_hours",
    ] {
        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(
            json_codec::from_value::<DiskHealth>(missing).is_err(),
            "{field}"
        );
    }
    for (field, invalid) in [
        ("assessment", json!("unknown")),
        ("assessment", json!({"passed":null})),
        ("observed_at_unix_ms", json!(42)),
        ("power_on_hours", json!(42)),
        ("temperature_celsius", json!(2147483648_i64)),
        ("extra", json!(true)),
    ] {
        let mut bad = value.clone();
        bad[field] = invalid;
        assert!(
            json_codec::from_value::<DiskHealth>(bad).is_err(),
            "{field}"
        );
    }
}

#[test]
fn health_is_required_and_unavailable_is_not_a_smart_result() {
    let mut value = json_codec::to_value(details()).unwrap();
    value.as_object_mut().unwrap().remove("health");
    assert!(json_codec::from_value::<DiskDetailsRecord>(value).is_err());
    for value in [
        json!({"state":"unavailable"}),
        json!({"state":"unavailable","reason":"healthy"}),
        json!({"state":"unavailable","reason":"tool_missing","assessment":"passed"}),
    ] {
        assert!(json_codec::from_value::<DiskHealth>(value).is_err());
    }
}
