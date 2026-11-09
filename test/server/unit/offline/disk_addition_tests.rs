use super::*;
use json_codec::json;
fn plan() -> Plan {
    let inode = json!({"device":"1","inode":"2"});
    json_codec::from_value(json!({"format":"zxs-helper-active-empty-directory-addition",
        "context":{"host":inode,"data":inode,"state":inode,"management":inode,"discovery":inode,
        "account":{"name":"zxs","uid":1001,"gid":1001,"source_device":"1","source_inode":"3","source_sha256":"a".repeat(64)}},
        "credential":{"principal":"admin","generation":"1","fingerprint":"b".repeat(64)},
        "registered_roots":["/zxs/data/disks/original"],
        "addition":{"authority":{"kind":"active_membership"},
            "request":{"operation_id":"op_11111111111111111111111111111111","additions":[]},
            "pool_id":"pool_original","configuration_digest":"c".repeat(64),"preview_digest":"d".repeat(64),
            "original_metadata_members":1,"target_metadata_members":1,"added_directories":0}})).unwrap()
}
#[test]
fn confirmation_and_stopped_acknowledgment_precede_disk_access() {
    let p = plan();
    let input = Commit {
        input: Original {
            service_user: "zxs".into(),
            request_digest: plan_digest(&p).unwrap(),
            external_writers_stopped: true,
        },
        plan: p,
        confirmation: "format-all-disks".into(),
    };
    assert!(
        commit(input)
            .unwrap_err()
            .to_string()
            .contains("before system access")
    );
    assert!(stopped(false).is_err());
    assert!(request_digest("../state").is_err());
    assert!(request_digest(&"A".repeat(64)).is_err());
}
#[test]
fn native_input_cannot_override_owner_or_claim_foreign_import() {
    let mut body = json!({"service_user":"zxs","operation_id":"op_11111111111111111111111111111111",
        "external_writers_stopped":true,"additions":[{"root_path":"/zxs/data/disks/new",
        "slot_id":"new","level":1,"upstream_slot_id":null,"metadata_member":true}]});
    assert!(json_codec::from_value::<Input>(body.clone()).is_ok());
    body["uid"] = 0.into();
    assert!(json_codec::from_value::<Input>(body).is_err());
    assert!(
        json_codec::from_value::<NewDirectory>(json!({"root_path":"/zxs/data/disks/new",
        "slot_id":"new","level":2,"upstream_slot_id":null,"metadata_member":true,
        "allow_missing_lower_chain":true}))
        .is_err()
    );
}
#[test]
fn unknown_discovery_roots_cannot_be_labeled_as_existing_members() {
    let mut p = plan();
    let original = p.registered_roots.clone();
    assert!(current_roots_match(&p, &original).is_ok());
    p.registered_roots
        .push("/zxs/data/disks/unregistered-foreign".into());
    assert!(current_roots_match(&p, &original).is_err());
}
#[test]
fn previews_do_not_claim_foreign_admission_or_complete_installation() {
    let output = json_codec::to_value(preview_result(plan(), None).unwrap()).unwrap();
    for key in [
        "imports_foreign_history",
        "formats_or_mounts_devices",
        "starts_service",
        "complete_installation_ready",
    ] {
        assert_eq!(output[key], false);
    }
    let mut p = plan();
    let first = plan_digest(&p).unwrap();
    p.registered_roots.push("/zxs/data/disks/another".into());
    assert_ne!(first, plan_digest(&p).unwrap());
}
