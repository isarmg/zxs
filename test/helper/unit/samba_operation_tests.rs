use super::*;
use json_codec::json;
use std::{fs, os::unix::fs::PermissionsExt};

#[test]
fn one_shot_intent_admits_only_a_new_bundle_and_retains_the_original_plan() {
    let temporary = tempfile::tempdir().unwrap();
    let host = BundleDestination::open(temporary.path()).unwrap();
    let plan = json!({"operation_id":"test-original", "account":"share"});
    for prefix in [".zxs-helper-samba-reset-", ".zxs-helper-samba-enroll-"] {
        let name = format!("{prefix}{}", "a".repeat(64));
        admit(&host, &name, &plan).unwrap();
        let bytes = fs::read(temporary.path().join(&name).join("plan.json")).unwrap();
        assert_eq!(bytes, json_codec::to_vec_pretty(&plan).unwrap());
        let changed = json!({"operation_id":"different", "account":"other"});
        let error = admit(&host, &name, &changed).unwrap_err();
        assert!(error.to_string().contains("already admitted"));
        assert_eq!(
            fs::read(temporary.path().join(&name).join("plan.json")).unwrap(),
            bytes
        );
    }
}

#[test]
fn existing_empty_staging_may_finish_its_record_but_never_admits_a_command() {
    let temporary = tempfile::tempdir().unwrap();
    let host = BundleDestination::open(temporary.path()).unwrap();
    let name = "original";
    let pending = temporary.path().join("original.pending");
    fs::create_dir(&pending).unwrap();
    fs::set_permissions(&pending, fs::Permissions::from_mode(0o700)).unwrap();
    let plan = json!({"original":true});
    let error = admit(&host, name, &plan).unwrap_err();
    assert!(error.to_string().contains("staging"));
    assert!(!pending.exists());
    assert_eq!(
        host.read_operation(name, "plan.json", MAX_RECORD_BYTES)
            .unwrap()
            .unwrap(),
        json_codec::to_vec_pretty(&plan).unwrap()
    );
    assert!(admit(&host, name, &plan).is_err());
}

#[test]
fn unknown_staging_material_is_preserved_and_never_admitted() {
    let temporary = tempfile::tempdir().unwrap();
    let host = BundleDestination::open(temporary.path()).unwrap();
    let pending = temporary.path().join("original.pending");
    fs::create_dir(&pending).unwrap();
    fs::set_permissions(&pending, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(pending.join("unknown"), b"retained evidence").unwrap();
    assert!(admit(&host, "original", &json!({"original":true})).is_err());
    assert_eq!(
        fs::read(pending.join("unknown")).unwrap(),
        b"retained evidence"
    );
    assert!(!temporary.path().join("original").exists());
    assert!(!pending.join("plan.json").exists());
}

#[test]
fn historical_status_wire_shape_keeps_operation_specific_receipts() {
    let none: Status<json_codec::Value> = Status::NoCompletePlanReadable {
        request_digest: "original".into(),
    };
    assert_eq!(
        json_codec::to_value(none).unwrap(),
        json!({
            "state":"no_complete_plan_readable", "request_digest":"original"
        })
    );
    let unknown: Status<json_codec::Value> = Status::OutcomeUnknownDoNotReplay {
        request_digest: "original".into(),
        account: "share".into(),
    };
    assert_eq!(
        json_codec::to_value(unknown).unwrap(),
        json!({
            "state":"outcome_unknown_do_not_replay", "request_digest":"original", "account":"share"
        })
    );
    for receipt in [
        json!({"existing_account_read_back":true}),
        json!({"created_account_read_back":true}),
    ] {
        let status = Status::CommandCompletedHistoricalReceipt {
            receipt: Box::new(receipt.clone()),
            current_password_or_service_state_verified: false,
        };
        assert_eq!(
            json_codec::to_value(status).unwrap(),
            json!({
                "state":"command_completed_historical_receipt", "receipt":receipt,
                "current_password_or_service_state_verified":false
            })
        );
    }
}
