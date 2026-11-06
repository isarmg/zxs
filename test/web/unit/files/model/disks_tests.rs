use super::*;
use json_codec::json;

fn record(slot: &str, level: u32, online: bool) -> Value {
    json!({
        "name":format!("directory-{slot}"),"slot_id":slot,"level":level,
        "online":online,"writable":level==1,"available_bytes":"0",
        "used_bytes":"0","simulation":true,"observation_failure":null
    })
}

#[test]
fn all_returned_levels_keep_server_order_identity_and_directory_names() {
    let disks = parse(json!({"disks":[
        record("third",3,false),record("primary",1,true),
        record("second",2,true),record("higher",7,true)
    ]}))
    .unwrap();
    assert_eq!(disks.len(), 4);
    assert_eq!(
        disks
            .iter()
            .map(|disk| disk.level.get())
            .collect::<Vec<_>>(),
        [3, 1, 2, 7]
    );
    for (disk, slot) in disks.iter().zip(["third", "primary", "second", "higher"]) {
        assert_eq!(disk.slot_id, slot);
        assert_eq!(disk.name, format!("directory-{slot}"));
        assert_eq!(disk.writable, disk.level.get() == 1);
    }
    assert!(!disks[0].online, "offline backup stays a visible record");
}

#[test]
fn extreme_level_is_one_returned_record_not_synthetic_ancestors() {
    let disks = parse(json!({"disks":[record("high",u32::MAX,true)]})).unwrap();
    assert_eq!(disks.len(), 1);
    assert_eq!(disks[0].level.get(), u32::MAX);
    assert!(!disks[0].writable);
}

#[test]
fn unknown_or_invalid_levels_reject_the_complete_listing() {
    let row = record("bad", 2, true);
    let mut missing = row.clone();
    missing.as_object_mut().unwrap().remove("level");
    assert!(parse(json!({"disks":[record("good",1,true),missing]})).is_err());
    for level in [
        json!(0),
        json!(-1),
        json!(4294967296_u64),
        json!(1.5),
        json!("2"),
        Value::Null,
    ] {
        let mut invalid = row.clone();
        invalid["level"] = level;
        assert!(parse(json!({"disks":[record("good",1,true),invalid]})).is_err());
    }
}

#[test]
fn writable_backup_is_rejected_not_silently_promoted_or_corrected() {
    for level in [2, 3, 7, u32::MAX] {
        for online in [true, false] {
            let mut row = record("backup", level, online);
            row["writable"] = true.into();
            assert!(parse(json!({"disks":[row]})).is_err());
        }
    }
}

#[test]
fn primary_read_only_is_not_upgraded_and_offline_is_separate_from_mode() {
    let mut primary = record("primary", 1, false);
    primary["writable"] = false.into();
    let disks = parse(json!({"disks":[primary]})).unwrap();
    assert_eq!(disks[0].level.get(), 1);
    assert!(!disks[0].writable);
    assert!(!disks[0].online);
    let disks = parse(json!({"disks":[record("primary",1,false)]})).unwrap();
    assert!(disks[0].writable, "the mode is not live write permission");
    assert!(!disks[0].online);
}

#[test]
fn current_wire_is_required_without_fallback_fields() {
    let row = record("primary", 1, true);
    for field in [
        "name",
        "slot_id",
        "level",
        "online",
        "writable",
        "available_bytes",
        "used_bytes",
        "simulation",
        "observation_failure",
    ] {
        let mut missing = row.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(
            parse(json!({"disks":[missing]})).is_err(),
            "missing {field}"
        );
    }
    let mut extra = row.clone();
    extra["physical_path"] = "/not-a-logical-path".into();
    assert!(parse(json!({"disks":[extra]})).is_err());
    assert!(parse(json!([row])).is_err());
    assert!(parse(json!({"disks":[]})).unwrap().is_empty());
}

#[test]
fn identity_observation_failure_cannot_claim_online() {
    let mut row = record("backup", 2, false);
    row["available_bytes"] = Value::Null;
    row["observation_failure"] = json!({"stage":"seal","code":"IO_ERROR"});
    let disks = parse(json!({"disks":[row.clone()]})).unwrap();
    assert_eq!(
        disks[0].observation_failure,
        Some(DiskObservationIssue::Seal(
            storage_contract::ErrorCode::IoError
        ))
    );
    assert!(!disks[0].online);
    row["online"] = true.into();
    assert!(parse(json!({"disks":[row]})).is_err());
}

#[test]
fn capacity_failure_retains_the_reported_reachability_and_specific_issue() {
    for online in [true, false] {
        let mut row = record("backup", 3, online);
        row["available_bytes"] = Value::Null;
        row["observation_failure"] = json!({"stage":"capacity","code":"DISK_UNAVAILABLE"});
        let disks = parse(json!({"disks":[row]})).unwrap();
        assert_eq!(disks[0].online, online);
        assert_eq!(disks[0].available_bytes, None);
        assert_eq!(
            disks[0].observation_failure,
            Some(DiskObservationIssue::Capacity(
                storage_contract::ErrorCode::DiskUnavailable
            ))
        );
    }
}

#[test]
fn real_server_fixture_preserves_zero_and_unavailable_backup_rows() {
    let value = json_codec::from_str(include_str!(
        "../../../../../test/server/fixtures/offline-file-manager-disks.json"
    ))
    .unwrap();
    let disks = parse(value).unwrap();
    assert_eq!(disks.len(), 2);
    assert_eq!(disks[0].level.get(), 1);
    assert_eq!(disks[0].available_bytes, Some(0));
    assert!(disks[0].online && disks[0].writable);
    assert_eq!(disks[1].level.get(), 2);
    assert_eq!(disks[1].available_bytes, None);
    assert!(!disks[1].online && !disks[1].writable);
}

#[test]
fn disk_strings_move_from_the_owned_response_without_path_rewriting() {
    let mut row = record("slot-identity", 3, true);
    row["name"] = "备份目录".into();
    let value = json!({"disks":[row]});
    let name_buffer = value["disks"][0]["name"].as_str().unwrap().as_ptr();
    let slot_buffer = value["disks"][0]["slot_id"].as_str().unwrap().as_ptr();
    let disks = parse(value).unwrap();
    assert_eq!(disks[0].name, "备份目录");
    assert_eq!(disks[0].name.as_ptr(), name_buffer);
    assert_eq!(disks[0].slot_id, "slot-identity");
    assert_eq!(disks[0].slot_id.as_ptr(), slot_buffer);
}

#[test]
fn display_names_are_portable_basenames_not_logical_backup_prefixes() {
    for name in ["", "../other", "parent/child", "parent\\child"] {
        let mut row = record("slot", 2, true);
        row["name"] = name.into();
        assert!(parse(json!({"disks":[row]})).is_err());
    }
}
