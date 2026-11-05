use super::*;
use json_codec::json;

#[test]
fn registration_modes_are_required_and_unknown_never_defaults_to_enabled() {
    let entry = json!({
        "slot_id": "P01",
        "disk_id": "disk_00000000000000000000000000000001",
        "binding_generation": "18446744073709551615",
        "mode": "disabled",
        "metadata_member": false,
        "readmission": "metadata_member",
    });
    let registration: RegistrationEntry = json_codec::from_value(entry.clone()).unwrap();
    assert_eq!(registration.binding_generation, u64::MAX);
    assert_eq!(registration.mode, RegistrationMode::Disabled);
    let mut missing = entry.clone();
    missing.as_object_mut().unwrap().remove("mode");
    assert!(json_codec::from_value::<RegistrationEntry>(missing).is_err());
    let mut unsupported = entry;
    unsupported["mode"] = json!("device-offline");
    assert!(json_codec::from_value::<RegistrationEntry>(unsupported).is_err());
    let mut missing_role = json_codec::to_value(registration).unwrap();
    missing_role.as_object_mut().unwrap().remove("readmission");
    assert!(json_codec::from_value::<RegistrationEntry>(missing_role).is_err());
    assert_eq!(
        json_codec::from_value::<RegistrationSnapshot>(json!({"authority":"unadmitted"})).unwrap(),
        RegistrationSnapshot::Unadmitted
    );
    assert!(
        json_codec::from_value::<RegistrationSnapshot>(
            json!({"authority":"unadmitted", "registrations":[]})
        )
        .is_err()
    );
}

#[test]
fn readmission_authorities_are_closed_and_terminal_hash_is_explicit() {
    let terminal = json!({
        "kind": "terminal_management",
        "terminal_authority_hash": "a".repeat(64),
        "terminal_operation_id": "op_00000000000000000000000000000001",
    });
    let authority: ReadmissionAuthority = json_codec::from_value(terminal.clone()).unwrap();
    assert!(authority.is_terminal());
    assert_eq!(authority.hash(), "a".repeat(64));
    let mut missing_hash = terminal.clone();
    missing_hash
        .as_object_mut()
        .unwrap()
        .remove("terminal_authority_hash");
    assert!(json_codec::from_value::<ReadmissionAuthority>(missing_hash).is_err());
    let mut wrong_kind = terminal.clone();
    wrong_kind["kind"] = json!("active_membership");
    assert!(json_codec::from_value::<ReadmissionAuthority>(wrong_kind).is_err());
    let mut unknown = terminal;
    unknown["kind"] = json!("unadmitted");
    assert!(json_codec::from_value::<ReadmissionAuthority>(unknown).is_err());
    assert!(
        json_codec::from_value::<ReadmissionAuthority>(json!({
            "kind": "active_membership", "membership_authority_hash": "b".repeat(64),
        }))
        .is_ok()
    );
}

#[test]
fn readmission_request_cannot_replace_a_registered_root_or_choose_an_actor() {
    let request = json!({
        "operation_id": "op_00000000000000000000000000000001",
        "slot_id": "P01",
        "expected_disk_id": "disk_00000000000000000000000000000001",
    });
    assert!(json_codec::from_value::<ReadmissionRequest>(request.clone()).is_ok());
    for key in ["root_path", "principal", "metadata_member", "force"] {
        let mut extra = request.clone();
        extra[key] = json!("untrusted");
        assert!(json_codec::from_value::<ReadmissionRequest>(extra).is_err());
    }
}

#[test]
fn readmission_status_requires_explicit_nullable_fields() {
    assert!(json_codec::from_value::<ReadmissionStatusResponse>(json!({})).is_err());
    assert_eq!(
        json_codec::from_value::<ReadmissionStatusResponse>(json!({"operation":null})).unwrap(),
        ReadmissionStatusResponse { operation: None }
    );
    let vectors: json_codec::Value =
        json_codec::from_str(include_str!("../../../docs/contracts/vectors.json")).unwrap();
    let status = vectors["positive"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == "readmission-running")
        .unwrap()["value"]
        .clone();
    assert!(json_codec::from_value::<ReadmissionStatus>(status.clone()).is_ok());
    for field in ["failure", "result"] {
        let mut incomplete = status.clone();
        incomplete.as_object_mut().unwrap().remove(field);
        assert!(json_codec::from_value::<ReadmissionStatus>(incomplete).is_err());
    }
}
