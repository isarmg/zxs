use super::*;

#[test]
fn retained_enrollment_plan_keeps_its_exact_wire_order_and_absence_proof() {
    let bytes = include_str!("../fixtures/samba-enrollment-plan.json").trim_end();
    let plan: Plan = json_codec::from_str(bytes).unwrap();
    assert_eq!(json_codec::to_vec(&plan).unwrap(), bytes.as_bytes());
    assert_eq!(
        digest(&plan).unwrap(),
        "ad40f40037c7f10f1c7aceb87b88b2ba1fb3e310ae29d6a41efc907810c175a8"
    );
    let reset = include_str!("../fixtures/samba-reset-plan.json");
    assert!(json_codec::from_str::<Plan>(reset).is_err());
    let mut invalid: json_codec::Value = json_codec::from_str(bytes).unwrap();
    invalid["database"]["unproved"] = json_codec::json!(true);
    assert!(json_codec::from_value::<Plan>(invalid).is_err());
}

#[test]
fn enrollment_keeps_the_confirmation_gate_before_any_system_target_access() {
    let request: Create = json_codec::from_value(json_codec::json!({
        "input": {"deployment": {"service_user":"zxs", "service_group":"zxs",
            "listen":"0.0.0.0:7400", "smb":{"account":"share", "peer_uid":0}},
            "operation_id":"00112233445566778899aabbccddeeff", "external_writers_stopped":false},
        "request_digest":"0".repeat(64), "confirmation":"create-missing-local-samba-account",
        "new_password":"test-password-only", "repeat_password":"test-password-only"
    }))
    .unwrap();
    let error = create(request).err().unwrap();
    assert!(error.to_string().contains("acknowledgments"));
    assert!(!error.to_string().contains("test-password-only"));
}
