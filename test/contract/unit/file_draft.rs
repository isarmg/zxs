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
            FileWorkingState::enabled_automatic(revision, crate::file_draft::automatic::AutomaticPolicy::TwentySavesInSixtySeconds),
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
        payload().content_hash().unwrap(),
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

#[test]
fn unborn_batch_shrink_reextend_preserves_zero_tail_in_one_save_generation() {
    use crate::file_container::ContainerEntity;
    use crate::file_draft::manifest::{
        ExtentHash, SaveGeneration, WorkingSave, WorkingExtent, WorkingExtentId,
    };
    let file = crate::FileId::from_random([21; 16]);
    let operation = crate::OperationId::from_random([22; 16]);
    // Codec/geometry facts only. These inode numbers are not an engine source.
    let first = WorkingExtent::from_observation(
        WorkingExtentId::from_random([23; 16]), ContainerEntity::new(7, 8),
        8, ExtentHash::of_bytes(b"abcdefgh"),
    ).unwrap();
    let mut batch = WorkingSave::first(file, operation, 128);
    batch.write_blocks(0, 8, &[(0, Some(first))]).unwrap();
    batch.resize(3).unwrap();
    batch.resize(8).unwrap();
    assert_eq!(batch.view().generation(), SaveGeneration::first());
    assert_eq!(batch.view().logical_size(), 8);
    assert_eq!(batch.view().block(0).unwrap().retained_bytes(), 3);
    let first_save = batch.finish().unwrap();
    let wire = canonical_json(&first_save).unwrap();
    let reopened: manifest::WorkingManifest = json_codec::from_slice(&wire).unwrap();
    assert_eq!(reopened, first_save);
    assert_eq!(reopened.block(0).unwrap().retained_bytes(), 3);

    let retained = reopened.begin_save(crate::OperationId::from_random([26; 16]), 128)
        .unwrap().finish().unwrap();
    retained.require_successor_of(&reopened, 128).unwrap();
    let mut resurrected = json_codec::to_value(&retained).unwrap();
    resurrected["blocks"][0]["retained_bytes"] = "8".into();
    let resurrected: manifest::WorkingManifest = json_codec::from_value(resurrected).unwrap();
    assert!(resurrected.require_successor_of(&reopened, 128).is_err());
    let mut renamed_extent = json_codec::to_value(&retained).unwrap();
    renamed_extent["blocks"][0]["extent"]["id"] =
        WorkingExtentId::from_random([27; 16]).to_string().into();
    let renamed_extent: manifest::WorkingManifest =
        json_codec::from_value(renamed_extent).unwrap();
    assert!(renamed_extent.require_successor_of(&reopened, 128).is_err());

    let uncut = manifest::WorkingManifest::first(
        file, crate::OperationId::from_random([28; 16]), 8,
        vec![manifest::WorkingBlock::new(0, first)], 128,
    ).unwrap();
    let mut two_cuts = uncut.begin_save(crate::OperationId::from_random([29; 16]), 128)
        .unwrap();
    two_cuts.resize(3).unwrap();
    two_cuts.resize(8).unwrap();
    two_cuts.resize(6).unwrap();
    let two_cuts = two_cuts.finish().unwrap();
    two_cuts.require_successor_of(&uncut, 128).unwrap();
    let mut restored_tail = json_codec::to_value(&two_cuts).unwrap();
    assert_eq!(restored_tail["truncation"]["retained_prefix"], "3");
    assert_eq!(restored_tail["truncation"]["old_visible_length"], "8");
    restored_tail["blocks"][0]["retained_bytes"] = "6".into();
    let restored_tail: manifest::WorkingManifest =
        json_codec::from_value(restored_tail).unwrap();
    assert!(restored_tail.require_successor_of(&uncut, 128).is_err());

    let mut successor = reopened.begin_save(crate::OperationId::from_random([24; 16]), 128)
        .unwrap();
    let before = successor.view().clone();
    assert!(successor.write_blocks(8, 1, &[]).is_err());
    assert_eq!(successor.view(), &before);
    assert!(successor.resize(129).is_err());
    assert_eq!(successor.view(), &before);
    let replacement = WorkingExtent::from_observation(
        WorkingExtentId::from_random([25; 16]), ContainerEntity::new(7, 9),
        9, ExtentHash::of_bytes(b"abc\0\0\0\0\0x"),
    ).unwrap();
    successor.write_blocks(8, 1, &[(0, Some(replacement))]).unwrap();
    successor.resize(10).unwrap();
    let second_save = successor.finish().unwrap();
    assert_eq!(second_save.generation(), SaveGeneration::first().next().unwrap());
    assert_eq!(second_save.block(0).unwrap().retained_bytes(), 9);
    assert_ne!(second_save.commitment(), first_save.commitment());
    let working = UnbornWorking::birth().saved(file, first_save, 128).unwrap();
    assert_eq!(working.mode(), CommitMode::Manual);
    assert!(working.saved(file, second_save.clone(), 128).is_ok());
    assert!(UnbornWorking::birth().saved(file, second_save, 128).is_err());
}

#[test]
fn unborn_manifest_rejects_missing_unknown_duplicate_and_cross_generation_bytes() {
    use crate::file_draft::manifest::{WorkingManifest, WorkingSave};
    let file = crate::FileId::from_random([31; 16]);
    let operation = crate::OperationId::from_random([32; 16]);
    let first = WorkingSave::first(file, operation, u64::MAX).finish().unwrap();
    let value = json_codec::to_value(&first).unwrap();
    for field in ["file", "operation", "generation", "previous", "logical_size", "truncation",
        "blocks"]
    {
        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(json_codec::from_value::<WorkingManifest>(missing).is_err());
    }
    let mut unknown = value.clone();
    unknown["base_version"] = VersionId::from_random([33; 16]).to_string().into();
    assert!(json_codec::from_value::<WorkingManifest>(unknown).is_err());
    let mut wrong_generation = value.clone();
    wrong_generation["generation"] = "2".into();
    assert!(json_codec::from_value::<WorkingManifest>(wrong_generation).is_err());
    let encoded = String::from_utf8(canonical_json(&first).unwrap()).unwrap();
    let duplicate = encoded.replacen("\"blocks\":[]", "\"blocks\":[],\"blocks\":[]", 1);
    assert_ne!(duplicate, encoded);
    assert!(json_codec::from_str::<WorkingManifest>(&duplicate).is_err());
    let working = FileWorkingState::Unborn { working: UnbornWorking::birth() };
    assert_eq!(working.mode(), CommitMode::Manual);
    assert_eq!(working.material_bytes(), 0);
    assert!(working.draft().is_none());
    assert!(json_codec::to_value(&working).unwrap().get("base").is_none());
    let golden = concat!(
        r#"{"state":"unborn","working":{"body":{"kind":"empty"},"#,
        r#""mode":"manual","revision":"1"}}"#,
    );
    assert_eq!(canonical_json(&working).unwrap(), golden.as_bytes());
    assert_eq!(json_codec::from_str::<FileWorkingState>(golden).unwrap(), working);
    for wire in [
        golden.replace(r#""kind":"empty""#, r#""kind":"empty","version":"fake""#),
        golden.replace(r#""kind":"empty""#, r#""kind":"empty","kind":"empty""#),
        golden.replace(r#""kind":"empty""#, r#""kind":"empty","manifest":null"#),
    ] {
        assert!(json_codec::from_str::<FileWorkingState>(&wire).is_err());
    }
    let mut unknown_birth = value.clone();
    unknown_birth["previous"]["version"] = "fake".into();
    assert!(json_codec::from_value::<WorkingManifest>(unknown_birth).is_err());
    let duplicate_birth = encoded.replacen(
        r#""kind":"birth""#, r#""kind":"birth","kind":"birth""#, 1,
    );
    assert_ne!(duplicate_birth, encoded);
    assert!(json_codec::from_str::<WorkingManifest>(&duplicate_birth).is_err());
}

#[test]
fn incremental_manifest_retains_exact_base_prefix_across_saves_and_roundtrips() {
    use manifest::WorkingSave;
    let file = crate::FileId::from_random([41; 16]);
    let first = WorkingSave::first_with_length(
        file, crate::OperationId::from_random([42; 16]), 32, 128,
    ).unwrap().finish().unwrap();
    let original = IncrementalManifest::new(first, 32).unwrap();
    original.require_previous(None, 32, 128).unwrap();
    let mut second = original.material().begin_save(
        crate::OperationId::from_random([43; 16]), 128,
    ).unwrap();
    second.resize(8).unwrap();
    second.resize(32).unwrap();
    let saved = IncrementalManifest::new(second.finish().unwrap(), 8).unwrap();
    saved.require_previous(Some(&original), 32, 128).unwrap();
    let wrong = IncrementalManifest::new(saved.material().clone(), 32).unwrap();
    assert!(wrong.require_previous(Some(&original), 32, 128).is_err());
    let state = FileWorkingState::ManualIncremental {
        revision: WorkingRevision::new(Generation::new(7)),
        base: base(),
        payload: DraftPayload::ranges(payload().payload_id(), saved.material().commitment(), 32),
        manifest: saved,
    };
    let bytes = canonical_json(&state).unwrap();
    let decoded: FileWorkingState = json_codec::from_slice(&bytes).unwrap();
    assert_eq!(decoded, state);
    assert_eq!(canonical_json(&decoded).unwrap(), bytes);
    assert_eq!(decoded.mode(), CommitMode::Manual);
    assert_eq!(decoded.material_bytes(), 0);
    assert_eq!(decoded.draft().unwrap().1.content_hash(), None);
    let encoded = json_codec::to_value(&decoded).unwrap();
    assert!(encoded["payload"].get("content_hash").is_none());
    assert_eq!(encoded["payload"]["manifest_commitment"],
        decoded.incremental().unwrap().material().commitment().to_string());
    assert_eq!(decoded.draft().unwrap().0, &base());
    assert_eq!(decoded.incremental().unwrap().retained_base_bytes(), 8);
    let mut invalid = json_codec::to_value(&state).unwrap();
    invalid["manifest"]["retained_base_bytes"] = "33".into();
    assert!(json_codec::from_value::<FileWorkingState>(invalid).is_err());
}
