//! Immutable expected images and role/identity lookups only. Matching an image
//! is not proof that a target installed it or that its current SQLite is sound.
use super::*;

fn registration() -> DiskRegistrationIdentity {
    DiskRegistrationIdentity {
        pool_id: "pool_00112233445566778899aabbccddeeff".into(),
        disk_id: "disk_00112233445566778899aabbccddeeff".into(),
        slot_id: "B01".into(),
        binding_generation: 1,
    }
}

#[test]
fn scoped_image_keeps_the_existing_descriptor_bytes_and_own_target_without_rehashing_inputs() {
    let own = registration();
    let mut scopes = Scopes::for_target(&own);
    scopes
        .add(
            Scope::SystemCurrent {
                pool_id: own.pool_id.clone(),
            },
            b"system".to_vec(),
        )
        .unwrap();
    scopes
        .add(
            Scope::DiskCurrent {
                identity: own.clone(),
            },
            b"current".to_vec(),
        )
        .unwrap();
    scopes
        .add(
            Scope::DiskHistory {
                identity: own.clone(),
            },
            b"history".to_vec(),
        )
        .unwrap();

    let [(_, current), (_, history)] = scopes.disk(&own).unwrap();
    let system = scopes.system().unwrap();
    for (file, bytes) in [
        (system, b"system".as_slice()),
        (current, b"current".as_slice()),
        (history, b"history".as_slice()),
    ] {
        assert_eq!(file.bytes(), bytes);
        assert!(file.reference().matches(bytes));
        assert!(!file.reference().matches(b"changed"));
    }
    let digest = blake3::hash(b"current").to_hex().to_string();
    let original = json_codec::json!({
        "scope": {"kind":"disk-current","identity":&own},
        "bytes": 7,
        "digest": &digest
    });
    assert_eq!(
        canonical_json(&current.descriptor()).unwrap(),
        canonical_json(&original).unwrap()
    );
    let local = json_codec::json!({"bytes":7, "digest":&digest});
    assert_eq!(
        canonical_json(current.reference()).unwrap(),
        canonical_json(&local).unwrap()
    );
    let report = scopes.report().unwrap();
    assert_eq!(report.target, own);
    assert_eq!(report.disk_current_hash, digest);
    assert_eq!(
        report.disk_history_hash,
        blake3::hash(b"history").to_hex().to_string()
    );
}

#[test]
fn shared_encoded_disk_path_does_not_authorize_another_registration_or_role() {
    let own = registration();
    let mut scopes = Scopes::for_target(&own);
    scopes
        .add(
            Scope::SystemCurrent {
                pool_id: own.pool_id.clone(),
            },
            b"system".to_vec(),
        )
        .unwrap();
    scopes
        .add(
            Scope::DiskCurrent {
                identity: own.clone(),
            },
            b"current".to_vec(),
        )
        .unwrap();
    assert!(scopes.disk(&own).is_err());
    let next = DiskRegistrationIdentity {
        binding_generation: 2,
        ..own.clone()
    };
    scopes
        .add(
            Scope::DiskHistory {
                identity: next.clone(),
            },
            b"foreign".to_vec(),
        )
        .unwrap();
    for id in [
        own.clone(),
        next,
        DiskRegistrationIdentity {
            slot_id: "B02".into(),
            ..own.clone()
        },
        DiskRegistrationIdentity {
            pool_id: "pool_ffeeddccbbaa99887766554433221100".into(),
            ..own.clone()
        },
    ] {
        assert!(scopes.disk(&id).is_err());
    }
    assert!(scopes.report().is_err());
    assert!(
        scopes
            .add(
                Scope::DiskHistory { identity: own },
                b"replacement".to_vec()
            )
            .is_err()
    );
    assert_eq!(scopes.files.len(), 3);
    assert_eq!(scopes.files[2].bytes(), b"foreign");
}
