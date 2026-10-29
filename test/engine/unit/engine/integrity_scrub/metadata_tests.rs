use super::*;

#[test]
fn shared_segment_shapes_are_read_once_but_check_each_directory_declaration() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temp.path().join("system"),
        Config::development([temp.path().join("disk")]),
    )
    .unwrap();
    engine
        .create_repository(
            &RepositoryName::parse("segment shapes").unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate(),
        )
        .unwrap();
    let cancellation = IntegrityScrubCancellation::new();
    let mut report = engine.scrub(&cancellation).unwrap();
    let disk = engine.config.disks[0].clone();
    let plan = engine.prepare_integrity_scrub().unwrap();
    let original: Snapshot =
        load_object(&disk, "snapshot", plan.snapshot.as_ref().unwrap()).unwrap();
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
    for (kind, field) in [
        (storage_contract::EVENT_SEGMENT_KIND, "first_sequence"),
        (storage_contract::OPERATION_SEGMENT_KIND, "first_sequence"),
        (storage_contract::OPERATION_SEGMENT_KIND, "records"),
    ] {
        let reference = if kind == storage_contract::EVENT_SEGMENT_KIND {
            &original.events
        } else {
            &original.maps["operation_log"]
        };
        let directory = crate::metadata_map::read_map(reference, |kind, hash| {
            crate::journal::load_authenticated_object(&disk, kind, hash)
        })
        .unwrap();
        let entry = directory.values().next().unwrap().clone();
        let hash = entry["hash"].as_str().unwrap();
        scan.segment_shape(kind, hash).unwrap();
        let loaded = scan.loaded_objects;
        let verified = scan.report.metadata_objects_verified;
        scan.segment_shape(kind, hash).unwrap();
        assert_eq!(scan.loaded_objects, loaded);
        assert_eq!(scan.report.metadata_objects_verified, verified);

        let mut different = entry.clone();
        let value: u64 = different[field].as_str().unwrap().parse().unwrap();
        different[field] = (value + 1).to_string().into();
        let reference = crate::journal::stored_map(
            std::slice::from_ref(&disk),
            [
                (format!("{:020}", 0), entry),
                (format!("{:020}", 1), different),
            ]
            .into_iter()
            .collect(),
        );
        let mut snapshot = original.clone();
        if kind == storage_contract::EVENT_SEGMENT_KIND {
            snapshot.events = reference;
        } else {
            snapshot.maps.insert("operation_log".into(), reference);
        }
        let error = crate::journal::visit_segment_objects_with(
            &snapshot,
            &mut scan,
            |scan, kind, hash| scan.read_graph_object(kind, hash),
            |scan, kind, hash| scan.segment_shape(kind, hash),
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::ObjectCorrupt);
        assert_eq!(
            error.message(),
            if kind == storage_contract::EVENT_SEGMENT_KIND {
                "event segment differs from its directory entry"
            } else {
                "operation segment differs from its directory entry"
            }
        );
    }
    cancellation.cancel();
    let address = scan
        .verified
        .iter()
        .find_map(|((kind, hash), shape)| {
            matches!(shape, Shape::SegmentBody(_)).then(|| (*kind, hash.clone()))
        })
        .unwrap();
    assert_eq!(
        scan.segment_shape(address.0, &address.1)
            .err()
            .unwrap()
            .code(),
        ErrorCode::OperationAborted
    );
}

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
