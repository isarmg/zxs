use super::*;
use json_codec::json;
fn plan() -> Plan {
    let inode = json!({"device":"1","inode":"2"});
    json_codec::from_value(json!({"format":"zxs-helper-active-disk-deregistration",
        "context":{"host":inode,"data":inode,"state":inode,"management":inode,"discovery":inode,
        "account":{"name":"zxs","uid":1001,"gid":1001,"source_device":"1","source_inode":"3","source_sha256":"a".repeat(64)}},
        "credential":{"principal":"admin","generation":"1","fingerprint":"b".repeat(64)},
        "exit":{"request":{"operation_id":"op_11111111111111111111111111111111",
            "slot_id":"P01","expected_disk_id":"disk_original","action":"deregister"},
            "target":{"pool_id":"pool_original","disk_id":"disk_original","slot_id":"P01",
            "binding_generation":"1","root_path":"/zxs/data/disks/target"},
            "authority":"active_membership","configuration_digest":"c".repeat(64),
            "preview_digest":"d".repeat(64),"source_sequence":"3","topology_generation":"1",
            "snapshot_hash":"e".repeat(64),"original_metadata_members":3,"target_metadata_members":2}})).unwrap()
}
#[test]
fn view_loss_acknowledgment_is_mandatory_before_any_system_access() {
    let p = plan();
    let input = Commit {
        input: Original {
            service_user: "zxs".into(),
            request_digest: plan_digest(&p).unwrap(),
            external_writers_stopped: true,
        },
        plan: p,
        confirmation: "deregister-member-preserve-bytes".into(),
        acknowledge_current_view_loss: false,
    };
    assert!(
        commit(input)
            .unwrap_err()
            .to_string()
            .contains("before system access")
    );
    assert!(stopped(false).is_err());
}
#[test]
fn no_planned_offline_or_final_voter_substitution() {
    let mut p = plan();
    assert!(supported(&p.exit).is_ok());
    p.exit.request.action = ExitAction::PlannedOffline;
    assert!(
        supported(&p.exit)
            .unwrap_err()
            .to_string()
            .contains("planned-offline")
    );
    p.exit.request.action = ExitAction::Deregister;
    p.exit.target_metadata_members = 0;
    p.exit.authority = ExitAuthority::TerminalManagement;
    assert!(
        supported(&p.exit)
            .unwrap_err()
            .to_string()
            .contains("final metadata voter")
    );
}
#[test]
fn native_worker_payload_cannot_supply_alternate_pool_or_root_account() {
    let mut body = json!({"service_user":"zxs","operation_id":"op_11111111111111111111111111111111",
        "slot_id":"P01","expected_disk_id":"disk_original","external_writers_stopped":true});
    assert!(json_codec::from_value::<Input>(body.clone()).is_ok());
    body["system_root"] = "/tmp/alternate".into();
    assert!(json_codec::from_value::<Input>(body).is_err());
}
#[test]
fn preview_neither_erases_media_nor_guarantees_remaining_content() {
    let p = plan();
    let first = plan_digest(&p).unwrap();
    let output = json_codec::to_value(preview_result(p.clone(), None).unwrap()).unwrap();
    for key in [
        "physical_bytes_erased",
        "remaining_content_preservation_verified",
        "complete_installation_ready",
        "service_started",
    ] {
        assert_eq!(output[key], false);
    }
    let mut changed = p;
    changed.exit.target.binding_generation += 1;
    assert_ne!(first, plan_digest(&changed).unwrap());
}
