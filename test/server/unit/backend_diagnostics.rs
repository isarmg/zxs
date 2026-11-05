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
        available_bytes: Some(u64::MAX),
        observation_failure: None,
        used_bytes: 7,
        simulation: true,
    }];
    let old_disks = json!({"disks":disks});
    let actual = DisksResponse::from(disks);
    assert_eq!(json_codec::to_value(&actual).unwrap(), old_disks);
    assert_eq!(storage_contract::canonical_json(&actual).unwrap(), br#"{"disks":[{"available_bytes":"18446744073709551615","disk_id":"disk-primary","level":1,"metadata_member":true,"observation_failure":null,"online":false,"physical_device":null,"simulation":true,"slot_id":"P01","upstream_slot_id":null,"used_bytes":"7"}]}"#);
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
fn file_manager_disk_wire_keeps_listing_fields_and_safe_failure_classification() {
    let disks: Vec<_> = (1..=2)
        .map(|level| storage_contract::DiskStatus {
            slot_id: format!("slot-{level}"),
            disk_id: "not-public".into(),
            level,
            upstream_slot_id: Some("not-public".into()),
            online: false,
            metadata_member: true,
            physical_device: Some("not-public".into()),
            available_bytes: Some(u64::MAX),
            observation_failure: None,
            used_bytes: 7,
            simulation: true,
        })
        .collect();
    let response = file_response(disks);
    let expected = json!({"disks":[
        {"name":"directory-slot-1","slot_id":"slot-1","level":1,"online":false,"writable":true,"available_bytes":"18446744073709551615","observation_failure":null,"used_bytes":"7","simulation":true},
        {"name":"directory-slot-2","slot_id":"slot-2","level":2,"online":false,"writable":false,"available_bytes":"18446744073709551615","observation_failure":null,"used_bytes":"7","simulation":true}
    ]});
    assert_eq!(json_codec::to_value(&response).unwrap(), expected);
    assert_eq!(
        storage_contract::canonical_json(&response).unwrap(),
        storage_contract::canonical_json(&expected).unwrap()
    );
}

#[test]
fn offline_diagnostic_vectors_keep_unknown_distinct_from_successful_zero() {
    let admin: json_codec::Value =
        json_codec::from_str(include_str!("../fixtures/offline-admin-disks.json")).unwrap();
    let disks: Vec<storage_contract::DiskStatus> =
        json_codec::from_value(admin["disks"].clone()).unwrap();
    assert_eq!(disks[0].available_bytes, Some(0));
    assert_eq!(disks[1].available_bytes, None);
    assert_eq!(
        json_codec::to_value(DisksResponse::from(disks.clone())).unwrap(),
        admin
    );
    let files: json_codec::Value =
        json_codec::from_str(include_str!("../fixtures/offline-file-manager-disks.json")).unwrap();
    assert_eq!(json_codec::to_value(file_response(disks)).unwrap(), files);
}

fn topology(disks: &[storage_contract::DiskStatus]) -> crate::TopologySnapshot {
    crate::TopologySnapshot {
        generation: 1,
        slots: disks
            .iter()
            .map(|disk| crate::TopologySlot {
                name: Some(
                    name_policy::PortablePath::parse(&format!("directory-{}", disk.slot_id))
                        .unwrap(),
                ),
                slot_id: disk.slot_id.clone(),
                disk_id: disk.disk_id.clone(),
                level: disk.level,
                upstream_slot_id: disk.upstream_slot_id.clone(),
                metadata_member: disk.metadata_member,
                binding_generation: 1,
            })
            .collect(),
    }
}
fn file_response(disks: Vec<storage_contract::DiskStatus>) -> FileManagerDisksResponse {
    let topology = topology(&disks);
    FileManagerDisksResponse::from_snapshot(disks, topology).unwrap()
}
#[test]
fn file_manager_rejects_changed_membership_and_missing_names() {
    let admin: json_codec::Value =
        json_codec::from_str(include_str!("../fixtures/offline-admin-disks.json")).unwrap();
    let disks: Vec<storage_contract::DiskStatus> =
        json_codec::from_value(admin["disks"].clone()).unwrap();
    let mut changed = topology(&disks);
    changed.slots[0].disk_id = "replacement".into();
    assert!(FileManagerDisksResponse::from_snapshot(disks.clone(), changed).is_err());
    let mut missing = topology(&disks);
    missing.slots[0].name = None;
    assert!(FileManagerDisksResponse::from_snapshot(disks, missing).is_err());
}
