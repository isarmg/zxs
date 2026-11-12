//! Fresh JSON admission and opaque retained encoding use the same codec authority.
use super::*;
use data_serialization::{Deserialize, Serialize};

fn signature(error: &crate::Error) -> (storage_contract::ErrorCode, Vec<String>) {
    let mut chain = Vec::new();
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(error) = source {
        chain.push(error.to_string());
        source = error.source();
    }
    (error.code(), chain)
}

// The reference performs the original strict-parse then value authentication,
// independently of the new raw-boundary path and its optional retained proof.
fn compare(
    read: RawMetadataRead<'_>,
    retained: Option<&RetainedMetadataEncoding>,
) -> Option<AuthenticatedMetadataObject> {
    let reference = storage_contract::strict_json(read.raw)
        .map_err(crate::Error::from)
        .and_then(|value| {
            AuthenticatedMetadataObject::authenticate(read.domain.0, read.expected.0, value)
        });
    let observed = read.authenticate(retained)
        .map(AuthenticatedMetadataRead::into_object);
    match (reference, observed) {
        (Ok(reference), Ok(observed)) => {
            assert_eq!(observed.kind(), reference.kind());
            assert_eq!(observed.hash(), reference.hash());
            assert_eq!(observed.bytes(), reference.bytes());
            assert_eq!(observed.value(), reference.value());
            Some(observed)
        }
        (Err(reference), Err(observed)) => {
            assert_eq!(signature(&observed), signature(&reference));
            None
        }
        _ => panic!("retained encoding changed admission success"),
    }
}

#[test]
fn exact_raw_and_domain_reuse_preserves_fresh_value_and_original_object() {
    let raw = br#"{"b":[null,true,7],"a":"fresh"}"#;
    let value = storage_contract::strict_json(raw).unwrap();
    let (_, expected) = storage_contract::encode_hashed_value("snapshot", &value).unwrap();
    let read = RawMetadataRead::parse("snapshot", &expected, raw).unwrap();
    let domain = read.domain();
    assert_eq!(read.raw(), raw);
    let fresh = read.authenticate(None).unwrap();
    let charge = fresh.retained_payload_bytes().unwrap();
    let retained = fresh.retain_encoding();
    assert_eq!(retained.payload_bytes(), Some(charge));
    let original = fresh.into_object();
    assert_eq!(charge, raw.len() + original.bytes().len() + original.hash().len());
    let again = RawMetadataRead::parse("snapshot", &expected, raw).unwrap();
    assert!(again.domain() == domain);
    assert!(retained.matches(&again));
    let clone = retained.clone();
    assert!(clone.matches(&again));
    let observed = compare(
        RawMetadataRead::parse("snapshot", &expected, raw).unwrap(),
        Some(&clone),
    ).unwrap();
    assert_eq!(observed.bytes(), original.bytes());
    assert_eq!(observed.value(), original.value());
    assert_eq!(observed.hash(), original.hash());
}

#[test]
fn mismatched_raw_or_domain_falls_back_to_original_encoding() {
    let raw = br#"{"value":1}"#;
    let value = storage_contract::strict_json(raw).unwrap();
    let (_, expected) = storage_contract::encode_hashed_value("snapshot", &value).unwrap();
    let retained = RawMetadataRead::parse("snapshot", &expected, raw).unwrap()
        .authenticate(None).unwrap().retain_encoding();
    let changed = br#"{"value":2}"#;
    let changed_read = RawMetadataRead::parse("snapshot", &expected, changed).unwrap();
    assert!(!retained.matches(&changed_read));
    assert!(compare(
        RawMetadataRead::parse("snapshot", &expected, changed).unwrap(),
        Some(&retained),
    ).is_none());
    let (_, changed_expected) = storage_contract::encode_hashed_value(
        "snapshot", &storage_contract::strict_json(changed).unwrap(),
    ).unwrap();
    assert!(compare(
        RawMetadataRead::parse("snapshot", &changed_expected, changed).unwrap(),
        Some(&retained),
    ).is_some());

    let other_domain = RawMetadataRead::parse("state-page", &expected, raw).unwrap();
    assert!(!retained.matches(&other_domain));
    assert!(compare(
        RawMetadataRead::parse("state-page", &expected, raw).unwrap(),
        Some(&retained),
    ).is_none());
    let (_, other_expected) = storage_contract::encode_hashed_value("state-page", &value).unwrap();
    assert!(compare(
        RawMetadataRead::parse("state-page", &other_expected, raw).unwrap(),
        Some(&retained),
    ).is_some());

    // Equivalent canonical JSON is not identical input to the retained proof.
    let spaced = b" { \"value\" : 1 } ";
    let equivalent = RawMetadataRead::parse("snapshot", &expected, spaced).unwrap();
    assert!(!retained.matches(&equivalent));
    let object = compare(
        RawMetadataRead::parse("snapshot", &expected, spaced).unwrap(),
        Some(&retained),
    ).unwrap();
    assert_eq!(object.value(), &value);
}

#[test]
fn fresh_parse_and_typed_projection_are_not_cached() {
    #[derive(Serialize, Deserialize)]
    #[serialization(deny_unknown_fields)]
    struct Projection {
        value: u64,
    }

    let raw = br#"{"value":1}"#;
    let (_, expected) = storage_contract::encode_hashed_value(
        "snapshot", &storage_contract::strict_json(raw).unwrap(),
    ).unwrap();
    let retained = RawMetadataRead::parse("snapshot", &expected, raw).unwrap()
        .authenticate(None).unwrap().retain_encoding();
    for invalid in [
        br#"{"value":1,"value":1}"#.as_slice(),
        br#"{"value":"#.as_slice(),
        &[0xff],
    ] {
        let reference = storage_contract::strict_json(invalid)
            .map_err(crate::Error::from).err().unwrap();
        let observed = RawMetadataRead::parse("snapshot", &expected, invalid)
            .err().unwrap();
        assert_eq!(signature(&observed), signature(&reference));
    }

    let unknown = br#"{"value":1,"unexpected":true}"#;
    let (_, unknown_hash) = storage_contract::encode_hashed_value(
        "snapshot", &storage_contract::strict_json(unknown).unwrap(),
    ).unwrap();
    let fresh = RawMetadataRead::parse("snapshot", &unknown_hash, unknown).unwrap()
        .authenticate(None).unwrap();
    let proof = fresh.retain_encoding();
    let object = compare(
        RawMetadataRead::parse("snapshot", &unknown_hash, unknown).unwrap(),
        Some(&proof),
    ).unwrap();
    assert!(json_codec::from_value::<Projection>(object.into_value()).is_err());

    // This API adds no address grammar check: the caller still owns that earlier gate.
    assert!(compare(
        RawMetadataRead::parse("snapshot", "not-a-digest", raw).unwrap(),
        Some(&retained),
    ).is_none());
}

#[test]
fn canonical_size_precedes_expected_digest_and_retention_counts_raw_bytes() {
    let raw = json_codec::to_vec(&"x".repeat(crate::journal::MAX_METADATA)).unwrap();
    let failure = RawMetadataRead::parse("snapshot", "wrong", &raw).unwrap()
        .authenticate(None).err().unwrap();
    assert_eq!(failure.code(), storage_contract::ErrorCode::ResourceLimit);
    assert!(compare(
        RawMetadataRead::parse("snapshot", "wrong", &raw).unwrap(),
        None,
    ).is_none());

    let padded = b"       {\"value\":1}       ";
    let (_, expected) = storage_contract::encode_hashed_value(
        "snapshot", &storage_contract::strict_json(padded).unwrap(),
    ).unwrap();
    let fresh = RawMetadataRead::parse("snapshot", &expected, padded).unwrap()
        .authenticate(None).unwrap();
    let charge = fresh.retained_payload_bytes().unwrap();
    let proof = fresh.retain_encoding();
    assert_eq!(proof.payload_bytes(), Some(charge));
    let object = fresh.into_object();
    assert!(padded.len() > object.bytes().len());
    assert_eq!(charge, padded.len() + object.bytes().len() + object.hash().len());
    assert_eq!(payload_bytes(usize::MAX, 1, 0), None);
    assert_eq!(payload_bytes(1, usize::MAX - 1, 1), None);
}
