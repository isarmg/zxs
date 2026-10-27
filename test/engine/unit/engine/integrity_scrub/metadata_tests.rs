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
    let plan = engine.prepare_integrity_scrub().unwrap();
    let mut observe = |_| cancellation.checkpoint();
    let mut progress = |_: &IntegrityScrubReport| {};
    let mut scan = MemberScan {
        plan: &plan,
        disk: &disk,
        root: &plan.disk_resources.get(&disk).unwrap().root,
        observe: &mut observe,
        progress: &mut progress,
        report: &mut report,
        verified: BTreeMap::new(),
        loaded_objects: MAX_VERIFIED_OBJECTS,
        catalog_bindings: 0,
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
    let plan = engine.prepare_integrity_scrub().unwrap();
    let mut observe = |_| cancellation.checkpoint();
    let mut progress = |_: &IntegrityScrubReport| {};
    let mut scan = MemberScan {
        plan: &plan,
        disk: &disk,
        root: &plan.disk_resources.get(&disk).unwrap().root,
        observe: &mut observe,
        progress: &mut progress,
        report: &mut report,
        verified: BTreeMap::new(),
        loaded_objects: 0,
        catalog_bindings: 0,
        unavailable: AcknowledgedUnavailable::default(),
    };
    scan.page(&hash).unwrap().validate_at("").unwrap();
    scan.page(&hash).unwrap().validate_at("").unwrap();
    assert_eq!(scan.loaded_objects, 1);
    assert_eq!(scan.verified.len(), 1);
}

#[test]
fn page_role_classification_never_reloads_a_catalog_leaf() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temp.path().join("system"),
        Config::development([temp.path().join("disk")]),
    )
    .unwrap();
    let cancellation = IntegrityScrubCancellation::new();
    let mut report = engine.scrub(&cancellation).unwrap();
    let disk = engine.config.disks[0].clone();
    for value in [
        json_codec::json!({}),
        json_codec::json!({"business":"not a catalog"}),
        json_codec::json!({new_id("history"):"0".repeat(64)}),
    ] {
        let reference = crate::journal::stored_map(
            std::slice::from_ref(&disk),
            value.as_object().unwrap().clone(),
        );
        let plan = engine.prepare_integrity_scrub().unwrap();
        let mut observe = |_| cancellation.checkpoint();
        let mut progress = |_: &IntegrityScrubReport| {};
        let mut scan = MemberScan {
            plan: &plan,
            disk: &disk,
            root: &plan.disk_resources.get(&disk).unwrap().root,
            observe: &mut observe,
            progress: &mut progress,
            report: &mut report,
            verified: BTreeMap::new(),
            loaded_objects: 0,
            catalog_bindings: 0,
            unavailable: AcknowledgedUnavailable::default(),
        };
        scan.page(reference.hash()).unwrap();
        let result = scan.catalog_node(&reference, "", false);
        if value.as_object().unwrap().is_empty() {
            assert_eq!(result.unwrap(), 0);
            assert_eq!(scan.loaded_objects, 1);
        } else if value.get("business").is_some() {
            assert_eq!(result.unwrap_err().code(), ErrorCode::ObjectCorrupt);
            assert_eq!(scan.loaded_objects, 1);
        } else {
            assert!(result.is_err());
            assert_eq!(
                scan.loaded_objects, 2,
                "one page read plus one missing manifest attempt, no page reload"
            );
        }
    }
}
