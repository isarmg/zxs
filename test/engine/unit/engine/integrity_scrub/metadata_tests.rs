use super::*;

#[test]
fn unique_object_budget_is_an_explicit_incomplete_scan_not_an_empty_success() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temp.path().join("system"),
        Config::development([temp.path().join("disk")]),
    )
    .unwrap();
    let cancellation = IntegrityScrubCancellation::new();
    let mut report = engine.scrub(&cancellation).unwrap();
    let disk = engine.config.disks[0].clone();
    let mut scan = MemberScan {
        engine: &engine,
        disk: &disk,
        cancellation: &cancellation,
        report: &mut report,
        verified: BTreeMap::new(),
        loaded_objects: MAX_VERIFIED_OBJECTS,
        unavailable: AcknowledgedUnavailable::default(),
    };
    let error = scan
        .graph_object("state-page", &"0".repeat(64))
        .err()
        .unwrap();
    assert_eq!(error.code(), ErrorCode::ResourceLimit);
    assert!(scan.verified.is_empty());
}

#[test]
fn one_page_is_read_once_even_if_multiple_closures_share_it() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temp.path().join("system"),
        Config::development([temp.path().join("disk")]),
    )
    .unwrap();
    let cancellation = IntegrityScrubCancellation::new();
    let mut report = engine.scrub(&cancellation).unwrap();
    let disk = engine.config.disks[0].clone();
    let hash = store_object(&disk, "state-page", &json_codec::json!({"one":"value"})).unwrap();
    let mut scan = MemberScan {
        engine: &engine,
        disk: &disk,
        cancellation: &cancellation,
        report: &mut report,
        verified: BTreeMap::new(),
        loaded_objects: 0,
        unavailable: AcknowledgedUnavailable::default(),
    };
    scan.page(&hash).unwrap().validate_at("").unwrap();
    scan.page(&hash).unwrap().validate_at("").unwrap();
    assert_eq!(scan.loaded_objects, 1);
    assert_eq!(scan.verified.len(), 1);
}
