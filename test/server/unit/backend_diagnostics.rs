use super::*;
use json_codec::json;

#[test]
fn diagnostics_wire_preserves_disk_fields_decimal_bytes_and_numeric_counts() {
    let disks = vec![storage_contract::DiskStatus {
        slot_id: "P01".into(),
        disk_id: "disk-primary".into(),
        level: 1,
        upstream_slot_id: None,
        online: false,
        metadata_member: true,
        physical_device: None,
        available_bytes: u64::MAX,
        used_bytes: 7,
        simulation: true,
    }];
    let old_disks = json!({"disks":disks});
    let actual = DisksResponse::from(disks);
    assert_eq!(json_codec::to_value(&actual).unwrap(), old_disks);
    assert_eq!(storage_contract::canonical_json(&actual).unwrap(), br#"{"disks":[{"available_bytes":"18446744073709551615","disk_id":"disk-primary","level":1,"metadata_member":true,"online":false,"physical_device":null,"simulation":true,"slot_id":"P01","upstream_slot_id":null,"used_bytes":"7"}]}"#);
    let report = storage_contract::CheckReport {
        checked_versions: 17,
        healthy_replicas: 23,
        tombstoned_versions: 7,
        issues: vec!["P01: missing".into(), "B01: damaged".into()],
    };
    let old_report = json_codec::to_value(&report).unwrap();
    let actual = CheckResponse::from(report);
    assert_eq!(json_codec::to_value(&actual).unwrap(), old_report);
    assert_eq!(storage_contract::canonical_json(&actual).unwrap(),br#"{"checked_versions":17,"healthy_replicas":23,"issues":["P01: missing","B01: damaged"],"tombstoned_versions":7}"#);
}

#[test]
fn file_manager_disk_wire_is_only_the_existing_seven_field_projection() {
    let disks: Vec<_> = (1..=2)
        .map(|level| storage_contract::DiskStatus {
            slot_id: format!("slot-{level}"),
            disk_id: "not-public".into(),
            level,
            upstream_slot_id: Some("not-public".into()),
            online: false,
            metadata_member: true,
            physical_device: Some("not-public".into()),
            available_bytes: u64::MAX,
            used_bytes: 7,
            simulation: true,
        })
        .collect();
    let response = FileManagerDisksResponse::from(disks);
    let expected = json!({"disks":[
        {"slot_id":"slot-1","level":1,"online":false,"writable":true,"available_bytes":"18446744073709551615","used_bytes":"7","simulation":true},
        {"slot_id":"slot-2","level":2,"online":false,"writable":false,"available_bytes":"18446744073709551615","used_bytes":"7","simulation":true}
    ]});
    assert_eq!(json_codec::to_value(&response).unwrap(), expected);
    assert_eq!(
        storage_contract::canonical_json(&response).unwrap(),
        storage_contract::canonical_json(&expected).unwrap()
    );
}
