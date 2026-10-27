use super::*;

fn object(kind: &'static str, value: json_codec::Value) -> PreparedMetadataObject {
    PreparedMetadataObject::new(kind, &value, MAX_METADATA).unwrap()
}

#[test]
fn first_occurrence_keeps_domains_order_and_collision_rejection() {
    let page = object("state-page", json!({}));
    let page_hash = page.hash.clone();
    let root = object("history-manifest", json!({"child": &page_hash}));
    let root_hash = root.hash.clone();
    let other_domain = object("metadata-index", json!({}));
    let other_hash = other_domain.hash.clone();
    let unique = unique_prepared_objects(vec![
        page,
        root,
        object("state-page", json!({})),
        other_domain,
    ])
    .unwrap();
    assert_eq!(
        unique.iter().map(|o| o.hash.as_str()).collect::<Vec<_>>(),
        vec![page_hash.as_str(), root_hash.as_str(), other_hash.as_str()]
    );
    let mut conflicting = object("state-page", json!({"different": true}));
    conflicting.hash = page_hash;
    let error = match unique_prepared_objects(vec![object("state-page", json!({})), conflicting]) {
        Ok(_) => panic!("different bytes at one address must be rejected"),
        Err(error) => error,
    };
    assert_eq!(error.code(), storage_contract::ErrorCode::ObjectCollision);
}

fn install_and_check_roots(objects: &[PreparedMetadataObject], members: &[DiskConfig]) {
    for object in objects {
        object.store(members).unwrap();
        if object.kind() == "history-manifest" {
            for member in members {
                // Each root is already complete when installed, not just at batch end.
                bound_history_graph_observed(
                    std::slice::from_ref(member),
                    &object.hash,
                    &mut |_| {},
                )
                .unwrap();
            }
        }
    }
}

#[test]
fn real_histories_share_one_empty_page_and_rebuild_exact_roots() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([
        temp.path().join("p"),
        temp.path().join("b"),
        temp.path().join("c"),
    ]);
    let mut engine = Engine::initialize(&temp.path().join("state"), config).unwrap();
    for name in ["first", "second", "third"] {
        engine
            .create_repository(
                &storage_contract::RepositoryName::parse(name).unwrap(),
                "P01",
                1,
                "dedup-test",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
    }
    let state = engine.state();
    let (roots, objects) = prepare_history_manifests(state, MAX_METADATA).unwrap();
    assert_eq!(state.histories.len(), 3);
    let mut logical_count = 0;
    for id in state.histories.keys() {
        let manifest = HistoryManifest::from_state(state, id).unwrap();
        let (hash, graph) = graph::prepare(&manifest, MAX_METADATA).unwrap();
        assert_eq!(hash, roots.catalog_binding(id).unwrap().hash);
        logical_count += graph.len();
    }
    assert!(objects.len() < logical_count);
    let empty = object("state-page", json!({})).hash;
    assert_eq!(objects.iter().filter(|o| o.hash == empty).count(), 1);
    assert_eq!(
        objects
            .iter()
            .map(|o| (o.kind(), &o.hash))
            .collect::<BTreeSet<_>>()
            .len(),
        objects.len()
    );
    let targets = Config::development([
        temp.path().join("fresh-one"),
        temp.path().join("fresh-two"),
        temp.path().join("fresh-three"),
    ]);
    install_and_check_roots(&objects, &targets.disks);
    let (reused_roots, reused) =
        prepare_history_manifests_incremental(state, state, &roots, &targets.disks, MAX_METADATA)
            .unwrap();
    assert_eq!(reused_roots, roots);
    assert_eq!(reused.len(), objects.len());
    let by_address = |objects: &[PreparedMetadataObject]| {
        objects
            .iter()
            .map(|o| ((o.kind(), o.hash.clone()), o.bytes().to_vec()))
            .collect::<BTreeMap<_, _>>()
    };
    assert_eq!(by_address(&objects), by_address(&reused));
    for member in &targets.disks {
        fs::remove_file(object_path(member, "state-page", &empty)).unwrap();
    }
    let (rebuilt_roots, rebuilt) =
        prepare_history_manifests_incremental(state, state, &roots, &targets.disks, MAX_METADATA)
            .unwrap();
    assert_eq!(rebuilt_roots, roots);
    assert_eq!(rebuilt.len(), objects.len());
    install_and_check_roots(&rebuilt, &targets.disks);
    assert!(prepare_history_manifests(state, 1).is_err());
    assert!(
        prepare_history_manifests_incremental(state, state, &roots, &targets.disks, 1).is_err()
    );
}

#[test]
fn failed_member_and_later_invocations_do_not_cache_installation() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([
        temp.path().join("one"),
        temp.path().join("two"),
        temp.path().join("three"),
    ]);
    let plan = || {
        unique_prepared_objects(vec![
            object("state-page", json!({})),
            object("state-page", json!({})),
        ])
        .unwrap()
    };
    let objects = plan();
    let hash = &objects[0].hash;
    let bad = object_path(&config.disks[1], "state-page", hash);
    write_atomic(&bad, b"bad", false).unwrap();
    assert_eq!(
        objects[0].store(&config.disks).unwrap_err().code(),
        storage_contract::ErrorCode::ObjectCollision
    );
    fs::remove_file(&bad).unwrap();
    let first = object_path(&config.disks[0], "state-page", hash);
    fs::remove_file(&first).unwrap();
    for object in plan() {
        object.store(&config.disks).unwrap();
    }
    for member in &config.disks {
        assert_eq!(
            fs::read(object_path(member, "state-page", hash)).unwrap(),
            b"{}"
        );
    }
    fs::remove_file(&first).unwrap();
    for object in plan() {
        object.store(&config.disks).unwrap();
    }
    assert_eq!(fs::read(first).unwrap(), b"{}");
}

#[test]
#[ignore = "requires immutable_io/run.sh isolated LD_PRELOAD children"]
fn existing_object_syncs_in_isolated_child() {
    let root = PathBuf::from(std::env::var_os("ZXS_HISTORY_IO_ROOT").unwrap());
    let log = PathBuf::from(std::env::var_os("ZXS_HISTORY_IO_LOG").unwrap());
    let stage = std::env::var("ZXS_HISTORY_IO_STAGE").unwrap();
    let member: usize = std::env::var("ZXS_HISTORY_IO_MEMBER")
        .unwrap()
        .parse()
        .unwrap();
    let config = Config::development([root.join("one"), root.join("two"), root.join("three")]);
    let plan = || {
        unique_prepared_objects(vec![
            object("state-page", json!({})),
            object("state-page", json!({})),
        ])
        .unwrap()
    };
    let prepared = plan();
    assert_eq!(prepared.len(), 1);
    // Preinstall through the real immutable API before enabling observation.
    prepared[0].store(&config.disks).unwrap();
    let mut targets = String::new();
    for disk in &config.disks {
        let path = object_path(disk, "state-page", &prepared[0].hash);
        assert_eq!(fs::read(&path).unwrap(), b"{}");
        targets.push_str(path.to_str().unwrap());
        targets.push('\n');
    }
    fs::write(root.join("targets"), targets).unwrap();
    fs::write(root.join("observe"), b"").unwrap();
    let result = prepared[0].store(&config.disks);
    let mut expected = sync_round(if stage == "success" {
        None
    } else {
        Some((member, &stage))
    });
    if stage == "success" {
        result.unwrap();
    } else {
        let error = result.unwrap_err();
        let mut cause: &dyn std::error::Error = &error;
        loop {
            if let Some(io) = cause.downcast_ref::<std::io::Error>() {
                assert_eq!(io.raw_os_error(), Some(5));
                break;
            }
            cause = cause.source().expect("injected EIO must retain its source");
        }
    }
    assert_eq!(fs::read_to_string(&log).unwrap(), expected);
    // Both retrying this plan and preparing a new one must perform all barriers.
    prepared[0].store(&config.disks).unwrap();
    expected.push_str(&sync_round(None));
    assert_eq!(fs::read_to_string(&log).unwrap(), expected);
    plan()[0].store(&config.disks).unwrap();
    expected.push_str(&sync_round(None));
    assert_eq!(fs::read_to_string(&log).unwrap(), expected);
    if stage == "success" {
        // The undeduplicated two-reference control performs twice the I/O.
        for _ in 0..2 {
            object("state-page", json!({}))
                .store(&config.disks)
                .unwrap();
            expected.push_str(&sync_round(None));
        }
        assert_eq!(fs::read_to_string(&log).unwrap(), expected);
    }
    fs::remove_file(root.join("observe")).unwrap();
    for disk in &config.disks {
        assert_eq!(
            fs::read(object_path(disk, "state-page", &prepared[0].hash)).unwrap(),
            b"{}"
        );
    }
    fs::write(root.join("completed"), format!("{stage}:{member}\n")).unwrap();
}

fn sync_round(failure: Option<(usize, &str)>) -> String {
    let mut expected = String::new();
    for member in 0..3 {
        for kind in ["file", "directory"] {
            expected.push_str(&format!("{member} {kind} attempt\n"));
            if failure == Some((member, kind)) {
                expected.push_str(&format!("{member} {kind} injected-EIO\n"));
                return expected;
            }
            expected.push_str(&format!("{member} {kind} success\n"));
        }
    }
    expected
}
