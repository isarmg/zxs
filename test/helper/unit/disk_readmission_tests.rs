//! Request, authority and uncertainty boundaries; source only, not executed.
use super::*;
use json_codec::json;
use storage_contract::storage_readmission::ReadmissionAuthority;
fn plan() -> Plan {
    let inode = json!({"device":"1","inode":"2"});
    json_codec::from_value(json!({"format":"zxs-helper-retained-terminal-readmission",
        "context":{"host":inode,"data":inode,"state":inode,"management":inode,"discovery":inode,
            "account":{"name":"zxs","uid":1001,"gid":1001,"source_device":"1","source_inode":"3","source_sha256":"a".repeat(64)}},
        "credential":{"principal":"admin","generation":"1","fingerprint":"b".repeat(64)},
        "readmission":{"request":{"operation_id":"op_11111111111111111111111111111111",
                "slot_id":"P01","expected_disk_id":"disk_original"},
            "target":{"pool_id":"pool_original","disk_id":"disk_original","slot_id":"P01",
                "binding_generation":"1","root_path":"/zxs/data/disks/original"},
            "principal":"admin","metadata_member":true,
            "configuration_digest":"c".repeat(64),"preview_digest":"d".repeat(64),
            "authority":{"kind":"terminal_management","terminal_authority_hash":"e".repeat(64),
                "terminal_operation_id":"op_22222222222222222222222222222222"},
            "registry_root_hash":"f".repeat(64),"registry_generation":"2",
            "source_sequence":"3","topology_generation":"1","snapshot_hash":"a".repeat(64)}})).unwrap()
}
#[test]
fn exact_root_epoch_confirmation_precedes_system_access() {
    let plan = plan();
    let input = Commit {
        input: Original {
            service_user: "zxs".into(),
            request_digest: plan_digest(&plan).unwrap(),
            external_writers_stopped: true,
        },
        plan,
        confirmation: "readmit-retained-terminal-metadata".into(),
        acknowledge_original_directory_and_new_epoch: false,
    };
    let error = commit(input).unwrap_err();
    assert!(error.to_string().contains("explicitly confirm"));
    assert!(stopped(false).is_err());
    assert!(request_digest("../untrusted").is_err());
    assert!(request_digest(&"A".repeat(64)).is_err());
}
#[test]
fn root_plan_is_not_foreign_or_after_retirement_admission_or_start_permission() {
    let plan = plan();
    assert!(supported(&plan.readmission).is_ok());
    let output = json_codec::to_value(response(plan, None).unwrap()).unwrap();
    for field in [
        "imports_foreign_history",
        "supports_after_terminal_retirement",
        "service_started",
        "complete_installation_ready",
    ] {
        assert_eq!(output[field], false);
    }
    assert_eq!(output["opens_original_directory_on_confirmation"], true);
    assert_eq!(output["restores_first_metadata_voter"], true);
    assert_eq!(output["original_status"], json_codec::Value::Null);
}
#[test]
fn active_or_data_only_source_is_rejected_not_coerced_into_metadata_role() {
    let mut plan = plan();
    plan.readmission.metadata_member = false;
    assert!(supported(&plan.readmission).is_err());
    plan.readmission.metadata_member = true;
    plan.readmission.authority = ReadmissionAuthority::ActiveMembership {
        membership_authority_hash: "a".repeat(64),
    };
    assert!(supported(&plan.readmission).is_err());
}
#[test]
fn retained_plan_digest_binds_root_epoch_authority_credential_and_operation() {
    let plan = plan();
    let digest = plan_digest(&plan).unwrap();
    let mut changed = plan.clone();
    changed.readmission.target.binding_generation += 1;
    assert_ne!(digest, plan_digest(&changed).unwrap());
    let mut changed = plan.clone();
    changed.readmission.target.root_path = "/zxs/data/disks/replacement".into();
    assert_ne!(digest, plan_digest(&changed).unwrap());
    let mut changed = plan.clone();
    changed.readmission.registry_generation += 1;
    assert_ne!(digest, plan_digest(&changed).unwrap());
    let mut wire = json_codec::to_value(&plan).unwrap();
    wire["credential"]["generation"] = "2".into();
    let changed: Plan = json_codec::from_value(wire).unwrap();
    assert_ne!(digest, plan_digest(&changed).unwrap());
    let mut changed = plan;
    changed.readmission.request.operation_id = OperationId::generate().unwrap();
    assert_ne!(digest, plan_digest(&changed).unwrap());
}
#[test]
fn request_cannot_choose_replacement_path_pool_or_force_old_authority() {
    let body = json!({"service_user":"zxs","operation_id":"op_11111111111111111111111111111111",
        "slot_id":"P01","expected_disk_id":"disk_original","external_writers_stopped":true});
    assert!(json_codec::from_value::<Input>(body.clone()).is_ok());
    for key in ["root_path", "system_root", "pool_id", "force", "action"] {
        let mut extra = body.clone();
        extra[key] = "untrusted".into();
        assert!(json_codec::from_value::<Input>(extra).is_err());
    }
}
#[test]
fn pending_error_retains_source_and_never_claims_rollback() {
    let error = Pending {
        source: std::io::Error::from_raw_os_error(5).into(),
    };
    assert!(error.to_string().contains("may be pending or published"));
    assert!(std::error::Error::source(&error).is_some());
}
