use super::*;

#[test]
fn retained_component_roundtrips_fixed_prefix_and_complete_original_name() {
    for original in ["report.txt".to_owned(), "界".repeat(79), "x".repeat(239)] {
        let encoded = format!("00126bh125abcde-{original}");
        let parsed = RetainedName::parse(&encoded).unwrap();
        assert_eq!(parsed.number().value(), 0);
        assert_eq!(parsed.kind(), RetainedKind::File);
        assert_eq!(parsed.utc_stamp(), "26bh125");
        assert_eq!(parsed.original_name(), original);
        assert_eq!(
            parsed.as_str().len(),
            RETAINED_PREFIX_BYTES + original.len()
        );
        let bytes = crate::canonical_json(&parsed).unwrap();
        let decoded = crate::decode_strict::<RetainedName>(&bytes).unwrap();
        assert_eq!(parsed, decoded);
        assert_eq!(crate::canonical_json(&decoded).unwrap(), bytes);
    }
    assert_eq!(RetainedDiskNumber::new(1295).unwrap().to_string(), "zz");
    assert!(RetainedDiskNumber::new(1296).is_none());
    let parsed = RetainedName::parse("zz026bh125abcde-archive").unwrap();
    assert_eq!(parsed.number().value(), 1295);
    assert_eq!(parsed.kind(), RetainedKind::Repository);
    for malformed in [
        "00126bh125abcde-../report.txt",
        "00126bh125abcde-",
        "00126bh125ABcde-report.txt",
        "00126bh165abcde-report.txt",
        "00126dh125abcde-report.txt",
        "00126b0125abcde-report.txt",
        "00126bhp25abcde-report.txt",
        "00226bh125abcde-report.txt",
        "0A126bh125abcde-report.txt",
    ] {
        assert!(RetainedName::parse(malformed).is_err());
    }
}

#[test]
fn original_allocation_retains_full_time_and_pool_disk_registration() {
    let number = RetainedDiskNumber::new(35).unwrap();
    let original = Name::parse("same.txt").unwrap();
    let allocation = RetainedAllocation {
        registration: RetainedDiskRegistration {
            pool: PoolId::from_random([1; 16]),
            disk: DiskId::from_random([2; 16]),
            number,
            operation: OperationId::from_random([3; 16]),
        },
        created_at: Timestamp::from_unix_seconds(1_794_878_700),
        name: RetainedName::sample(number, RetainedKind::File, "26bh125", &original).unwrap(),
    };
    assert_eq!(&allocation.name.as_str()[..3], "0z1");
    let bytes = crate::canonical_json(&allocation).unwrap();
    assert_eq!(
        crate::decode_strict::<RetainedAllocation>(&bytes).unwrap(),
        allocation
    );
    assert_eq!(allocation.created_at.unix_seconds(), 1_794_878_700);
}
