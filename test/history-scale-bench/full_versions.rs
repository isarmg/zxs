use super::*;

#[test]
fn full_version_payload_is_fixed_size_and_uniquely_indexed() {
    for index in 1..=1024 {
        let bytes = body(index);
        assert_eq!(bytes.len(), 4096);
        assert_eq!(
            u64::from_le_bytes(bytes[..8].try_into().unwrap()),
            index as u64
        );
        assert!(bytes[8..].iter().all(|byte| *byte == CONTENT_FILL_BYTE));
        assert_ne!(bytes, body(index + 1));
    }
}

#[test]
fn workload_is_explicit_and_preserves_the_old_default() {
    assert_eq!(Workload::parse(None).unwrap(), Workload::FixedBodyBindings);
    assert_eq!(
        Workload::parse(Some("fixed-body-bindings")).unwrap(),
        Workload::FixedBodyBindings
    );
    assert_eq!(
        Workload::parse(Some("full-versions")).unwrap(),
        Workload::FullVersions
    );
    assert!(Workload::parse(Some("unknown")).is_err());
}
