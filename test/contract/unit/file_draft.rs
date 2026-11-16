use super::*;
use crate::canonical_json;

fn base() -> DraftBase {
    let version = VersionId::from_random([3; 16]);
    DraftBase::new(
        BranchId::from_random([2; 16]),
        FileNodeId::Version(version),
        version,
        Generation::new(7),
    )
}

fn payload() -> DraftPayload {
    DraftPayload::new(
        DraftPayloadId::from_random([4; 16]),
        ContentHash::from_digest(blake3::hash(b"working body")),
        12,
    )
}

#[test]
fn working_state_roundtrips_all_states_without_implicit_clean_defaults() {
    for value in [0, 1, (1 << 53) + 1, u64::MAX] {
        let revision = WorkingRevision::new(Generation::new(value));
        for state in [
            FileWorkingState::Automatic { revision },
            FileWorkingState::ManualClean { revision },
            FileWorkingState::ManualDirty {
                revision,
                base: base(),
                payload: payload(),
            },
        ] {
            let bytes = canonical_json(&state).unwrap();
            let decoded: FileWorkingState = json_codec::from_slice(&bytes).unwrap();
            assert_eq!(decoded, state);
            assert_eq!(decoded.revision(), revision);
            assert_eq!(canonical_json(&decoded).unwrap(), bytes);
            let value = json_codec::to_value(&decoded).unwrap();
            assert_eq!(value["revision"], revision.to_string());
        }
    }
    for wire in ["null", "{}", r#"{"state":"automatic"}"#] {
        assert!(json_codec::from_str::<FileWorkingState>(wire).is_err());
    }
}

#[test]
fn working_state_rejects_extra_dirty_fields_and_incomplete_manual_payloads() {
    let revision = WorkingRevision::new(Generation::new(1));
    let valid = FileWorkingState::ManualDirty {
        revision,
        base: base(),
        payload: payload(),
    };
    let value = json_codec::to_value(valid).unwrap();
    for state in ["automatic", "manual_clean", "unknown"] {
        let mut invalid = value.clone();
        invalid["state"] = state.into();
        assert!(json_codec::from_value::<FileWorkingState>(invalid).is_err());
    }
    for missing in ["state", "revision", "base", "payload"] {
        let mut invalid = value.clone();
        invalid.as_object_mut().unwrap().remove(missing);
        assert!(json_codec::from_value::<FileWorkingState>(invalid).is_err());
    }
    for invalid_revision in [
        json_codec::Value::Null,
        1.into(),
        "01".into(),
        "18446744073709551616".into(),
    ] {
        let mut invalid = value.clone();
        invalid["revision"] = invalid_revision;
        assert!(json_codec::from_value::<FileWorkingState>(invalid).is_err());
    }
    let mut extra = value.clone();
    extra["workspace_id"] = "not-a-container-draft".into();
    assert!(json_codec::from_value::<FileWorkingState>(extra).is_err());
    for wire in [
        r#"{"state":"automatic","revision":"1","revision":"2"}"#,
        r#"{"state":"manual_clean","state":"automatic","revision":"1"}"#,
    ] {
        assert!(json_codec::from_str::<FileWorkingState>(wire).is_err());
    }
    for extra_at in ["base", "payload"] {
        let mut invalid = value.clone();
        invalid[extra_at]["path"] = "/not/a/durable/draft/reference".into();
        assert!(json_codec::from_value::<FileWorkingState>(invalid).is_err());
    }
}

#[test]
fn draft_material_uses_its_own_identity_and_full_width_size() {
    let material = DraftPayload::new(
        payload().payload_id(),
        payload().content_hash(),
        u64::MAX,
    );
    let value = json_codec::to_value(material).unwrap();
    assert_eq!(value["logical_size"], u64::MAX.to_string());
    assert_eq!(
        json_codec::from_value::<DraftPayload>(value.clone()).unwrap(),
        material,
    );
    assert!(material.payload_id().to_string().parse::<VersionId>().is_err());
    let mut wrong_domain = value.clone();
    wrong_domain["payload_id"] = base().version_id().to_string().into();
    assert!(json_codec::from_value::<DraftPayload>(wrong_domain).is_err());
    let mut wrong_hash = value.clone();
    wrong_hash["content_hash"] = "ABCDEF".into();
    assert!(json_codec::from_value::<DraftPayload>(wrong_hash).is_err());
    for missing in ["payload_id", "content_hash", "logical_size"] {
        let mut invalid = value.clone();
        invalid.as_object_mut().unwrap().remove(missing);
        assert!(json_codec::from_value::<DraftPayload>(invalid).is_err());
    }
}
