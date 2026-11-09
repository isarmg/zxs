use super::*;

fn operation() -> OperationId {
    "op_11111111111111111111111111111111".parse().unwrap()
}
fn original() -> wire::StartRequest {
    json_codec::from_value(json_codec::json!({
        "operation_id":operation(),"source_slot_id":"P01","expected_sequence":"17",
        "confirmation":"prepare_independent_complete_copies",
        "targets":[{
            "selection":{
                "selection_id":"observed","discovery_root":"/fresh",
                "discovery_identity":{"device":"1","inode":"1","ctime_seconds":"0","ctime_nanoseconds":0},
                "mount_path":"/fresh","root_path":"/fresh/a",
                "mount_identity":{"mount_id":"1","device_major":1,"device_minor":1,"root_inode":"1","filesystem":"ext4","filesystem_root":"/"},
                "root_identity":{"device":"1","inode":"2","ctime_seconds":"0","ctime_nanoseconds":0}
            },"disk_id":"disk_a","slot_id":"B01","upstream_slot_id":"P01"
        }]
    })).unwrap()
}

#[test]
fn operation_record_has_no_credentials_or_implicit_continuation_action() {
    let record = retain(None, operation(), None).unwrap();
    let encoded = json_codec::to_string(&record).unwrap();
    assert_eq!(decode_record(&encoded).unwrap(), record);
    let value: json_codec::Value = json_codec::from_str(&encoded).unwrap();
    assert_eq!(value.as_object().unwrap().len(), 3);
    assert!(value["original_start"].is_null());
    assert!(value.get("action").is_none());
    assert!(value.get("token").is_none());
}

#[test]
fn first_start_retains_the_entire_identical_input_before_any_http_result() {
    let request = original();
    let record = retain(None, operation(), Some(&request)).unwrap();
    let decoded = decode_record(&json_codec::to_string(&record).unwrap()).unwrap();
    assert_eq!(decoded.original_start, Some(request));
    assert_eq!(
        retain(Some(decoded.clone()), operation(), None).unwrap(),
        decoded
    );
}

#[test]
fn retained_operation_and_original_input_cannot_be_rebound() {
    let request = original();
    let record = retain(None, operation(), Some(&request)).unwrap();
    let other = "op_22222222222222222222222222222222".parse().unwrap();
    assert!(retain(Some(record.clone()), other, None).is_err());
    let mut changed = request;
    changed.targets[0].selection.root_path = "/fresh/replacement".into();
    assert!(retain(Some(record.clone()), operation(), Some(&changed)).is_err());
    changed.operation_id = other;
    assert!(retain(None, operation(), Some(&changed)).is_err());
    assert!(retain(Some(record), operation(), None).is_ok());
}

#[test]
fn corrupt_unknown_oversized_and_duplicate_record_fields_are_not_ignored() {
    for bytes in [
        "{".to_owned(),
        "x".repeat(MAX_RECORD_BYTES + 1),
        r#"{"format":"old","operation_id":"op_11111111111111111111111111111111","original_start":null}"#.into(),
        r#"{"format":"original-backup-transaction","operation_id":"op_11111111111111111111111111111111","original_start":null,"action":"activate"}"#.into(),
        r#"{"format":"original-backup-transaction","operation_id":"op_11111111111111111111111111111111","operation_id":"op_22222222222222222222222222222222","original_start":null}"#.into(),
    ] { assert!(decode_record(&bytes).is_err()); }
    let invalid = "{";
    assert!(matches!(
        decode_record(invalid).unwrap_err().source.as_deref(),
        Some(api::ErrorSource::BackupInput(_))
    ));
}

#[test]
fn browser_keys_are_account_scoped_not_service_credentials() {
    assert_ne!(key("a"), key("b"));
    assert_ne!(key("a/b"), key("a%2Fb"));
    assert!(key("administrator").starts_with("zxs-backup-transaction:"));
}
