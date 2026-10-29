use super::*;

fn fixture_disk() -> (tempfile::TempDir, Config) {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([
        temp.path().join("a"),
        temp.path().join("b"),
        temp.path().join("c"),
    ]);
    (temp, config)
}
fn event_state(count: usize) -> State {
    State {
        sequence: count as u64,
        events: storage_contract::SegmentedEvents::from_events(
            (0..count)
                .map(|index| Event {
                    sequence: index as u64,
                    operation_id: format!("op_{index:032x}").parse().unwrap(),
                    kind: "MUTATION".into(),
                    object_id: "retained-history".into(),
                })
                .collect(),
        )
        .unwrap(),
        ..Default::default()
    }
}
/// Sealed event bodies live on the members since the publications that sealed
/// them; only the open tail and one fresh seal stay resident, so a fixture
/// state assembled in memory installs them before its first store.
fn install_sealed_event_bodies(config: &Config, state: &State) {
    for (ordinal, reference) in state.events.sealed().iter().enumerate() {
        let span = storage_contract::EVENT_SEGMENT_SPAN;
        let segment = storage_contract::EventSegment {
            format: Default::default(),
            events: (ordinal * span..(ordinal + 1) * span)
                .map(|index| Event {
                    sequence: index as u64,
                    operation_id: format!("op_{index:032x}").parse().unwrap(),
                    kind: "MUTATION".into(),
                    object_id: "retained-history".into(),
                })
                .collect(),
        };
        for disk in &config.disks {
            assert_eq!(
                store_object(disk, storage_contract::EVENT_SEGMENT_KIND, &segment).unwrap(),
                reference.hash
            );
        }
    }
}
fn object_bytes(objects: &[PreparedMetadataObject]) -> BTreeMap<String, String> {
    objects
        .iter()
        .map(|object| {
            (
                format!("{}/{}", object.kind, object.hash),
                String::from_utf8(object.bytes.clone()).unwrap(),
            )
        })
        .collect()
}
fn assert_reference(state: &State) {
    let actual = prepare_snapshot(state, MAX_METADATA).unwrap();
    let reference = prepare_snapshot_reference(state, MAX_METADATA).unwrap();
    assert_eq!(actual.hash, reference.hash);
    assert_eq!(
        object_bytes(&actual.objects),
        object_bytes(&reference.objects)
    );
}

#[test]
fn aggregate_snapshot_roots_are_fixed_schema_not_flat_page_lists() {
    let mut state = event_state(4096);
    state.file_notes = (0..4096)
        .map(|n| (format!("file-{n:08}"), "record".repeat(64)))
        .collect();
    let prepared = prepare_snapshot(&state, MAX_METADATA).unwrap();
    let empty = prepare_snapshot(&State::default(), MAX_METADATA).unwrap();
    let root = canonical_json(&prepared.descriptor).unwrap();
    assert!(root.len() < 8192);
    assert_eq!(prepared.descriptor.maps.len(), empty.descriptor.maps.len());
    assert_eq!(prepared.descriptor.maps.len(), 35);
    // 4096 events collapse to a four-entry segment directory: one bounded
    // page, never a flat per-event list.
    assert!(matches!(
        prepared.descriptor.events.kind(),
        crate::metadata_map::ReferenceKind::Page
    ));
    assert!(matches!(
        prepared.descriptor.maps["file_notes"].kind(),
        crate::metadata_map::ReferenceKind::Index
    ));
    assert!(prepared.objects.len() > 20);
    let (_temp, config) = fixture_disk();
    install_sealed_event_bodies(&config, &state);
    prepared.store(&config.disks, &state).unwrap();
    assert_eq!(
        load_raw_snapshot_fixture(&config.disks[0], &prepared.hash).unwrap(),
        state
    );
    assert_reference(&state);
}

#[test]
fn current_snapshot_and_candidate_reject_old_and_unknown_shapes_without_rewriting() {
    let (_temp, config) = fixture_disk();
    let disk = &config.disks[0];
    let prepared = prepare_snapshot(&State::default(), MAX_METADATA).unwrap();
    let mut value = json_codec::to_value(&prepared.descriptor).unwrap();
    assert_eq!(value["format"], "radix-map-roots");
    for mutation in [
        "missing_format",
        "unknown_format",
        "unknown_field",
        "old_flat",
    ] {
        let mut bad = value.clone();
        match mutation {
            "missing_format" => {
                bad.as_object_mut().unwrap().remove("format");
            }
            "unknown_format" => bad["format"] = json_codec::json!("future"),
            "unknown_field" => bad["format_version"] = json_codec::json!(2),
            _ => {
                bad = json_codec::json!({"sequence":"0","pages":{"events":[]},"scalars":{"sequence":"0","business_generation":"0","topology_generation":"0","control_auth":null}})
            }
        }
        let hash = store_object(disk, "snapshot", &bad).unwrap();
        let path = object_path(disk, "snapshot", &hash);
        let bytes = std::fs::read(&path).unwrap();
        assert!(
            load_raw_snapshot_fixture(disk, &hash).is_err(),
            "{mutation}"
        );
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
    value.as_object_mut().unwrap().remove("business_generation");
    assert!(json_codec::from_value::<Snapshot>(value).is_err());
    let candidate = Candidate {
        pool_id: new_id("pool"),
        sequence: 1,
        transaction_id: new_id("tx"),
        operation_id: storage_contract::OperationId::generate(),
        request_digest: "a".repeat(64),
        previous_snapshot: None,
        snapshot_hash: prepared.hash,
        versions: Vec::new(),
        metadata_disk_ids: Vec::new(),
        history_roots: stored_map(&[], json_codec::Map::new()),
    };
    let mut value = json_codec::to_value(&candidate).unwrap();
    value["additional"] = json_codec::json!(true);
    assert!(json_codec::from_value::<Candidate>(value).is_err());
    let mut old = json_codec::to_value(&candidate).unwrap();
    old.as_object_mut().unwrap().remove("history_roots");
    old["history_manifest_hashes"] = json_codec::json!({});
    let hash = store_object(disk, "candidate", &old).unwrap();
    let path = object_path(disk, "candidate", &hash);
    let bytes = std::fs::read(&path).unwrap();
    assert!(load_object::<Candidate>(disk, "candidate", &hash).is_err());
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[test]
fn current_snapshot_requires_every_map_even_when_empty_and_validates_addresses_before_io() {
    let prepared = prepare_snapshot(&State::default(), MAX_METADATA).unwrap();
    for field in prepared.descriptor.maps.keys() {
        let mut missing = prepared.descriptor.clone();
        missing.maps.remove(field);
        assert!(validate_snapshot_schema(&missing).is_err(), "{field}");
    }
    let mut bad = prepared.descriptor.clone();
    bad.maps.insert(
        "versions".into(),
        json_codec::from_value(json_codec::json!({"kind":"state-page","hash":"../../outside"}))
            .unwrap(),
    );
    assert!(resolve_raw_snapshot(bad, |_, _| panic!("address must fail before I/O")).is_err());
    let mut unknown = prepared.descriptor.clone();
    unknown
        .maps
        .insert("unknown".into(), unknown.events.clone());
    assert!(validate_snapshot_schema(&unknown).is_err());
}

#[test]
fn event_ordinal_map_rejects_missing_or_noncanonical_indexes() {
    let (_temp, config) = fixture_disk();
    let disk = &config.disks[0];
    let state = event_state(3);
    let prepared = prepare_snapshot(&state, MAX_METADATA).unwrap();
    prepared.store(&config.disks, &state).unwrap();
    // The single key of a three-event log is its open-tail directory entry.
    let tail = storage_contract::EventSegment {
        format: Default::default(),
        events: state.events.tail().to_vec(),
    };
    let entry = json_codec::to_value(storage_contract::EventDirectoryEntry {
        kind: Default::default(),
        hash: storage_contract::object_hash(storage_contract::EVENT_SEGMENT_KIND, &tail).unwrap(),
        first_sequence: 0,
    })
    .unwrap();
    for key in ["00000000000000000004", "2"] {
        let mut events = json_codec::Map::new();
        events.insert(key.into(), entry.clone());
        let mut altered = prepared.descriptor.clone();
        altered.events = stored_map(&config.disks, events);
        let hash = store_object(disk, "snapshot", &altered).unwrap();
        let bytes = std::fs::read(object_path(disk, "snapshot", &hash)).unwrap();
        assert_eq!(
            load_raw_snapshot_fixture(disk, &hash).unwrap_err().code(),
            ErrorCode::ObjectCorrupt
        );
        assert_eq!(
            std::fs::read(object_path(disk, "snapshot", &hash)).unwrap(),
            bytes
        );
    }
}

#[test]
fn opaque_auth_values_use_one_domain_object_not_snapshot_scalar_or_map_growth() {
    let (_temp, config) = fixture_disk();
    for auth in [
        None,
        Some(json_codec::json!({"nested":[true,null,"中文"]})),
        Some(json_codec::json!([1, 2, 3])),
        Some(json_codec::json!("opaque")),
    ] {
        let state = State {
            control_auth: auth,
            ..Default::default()
        };
        let prepared = prepare_snapshot(&state, MAX_METADATA).unwrap();
        prepared.store(&config.disks, &state).unwrap();
        assert_reference(&state);
        let value: json_codec::Value = load_object(
            &config.disks[0],
            "state-control-auth",
            prepared.descriptor.control_auth.hash(),
        )
        .unwrap();
        assert_eq!(value, json_codec::to_value(&state.control_auth).unwrap());
        assert_eq!(
            load_raw_snapshot_fixture(&config.disks[0], &prepared.hash).unwrap(),
            state
        );
    }
}

#[test]
fn typed_snapshot_preserves_decimal_counters_unicode_and_operation_keys() {
    let mut state = event_state(257);
    state.sequence = u64::MAX;
    state.business_generation = u64::MAX;
    state.topology_generation = u64::MAX;
    state
        .file_selection_generations
        .insert("中文".into(), u64::MAX);
    state
        .directory_selection_generations
        .insert("directory".into(), u64::MAX);
    state
        .file_notes
        .insert("quoted\"\\\n".into(), "value\n中文".into());
    // 4096 receipts seal sixteen 256-record segments; the locator map keeps
    // exactly one typed OperationId key per receipt with its decimal ordinal.
    let mut expected = json_codec::Map::new();
    let mut records = Vec::new();
    for index in 0..4096u64 {
        let key = format!("op_{index:032x}");
        records.push(storage_contract::OperationRecordEntry {
            principal: "owner".into(),
            result: OperationResult {
                operation_id: key.parse().unwrap(),
                kind: "file.save".into(),
                commit_id: None,
                history_tree_id: None,
                branch_id: None,
                commit_state: storage_contract::CommitState::Published,
                protection_at_publish: storage_contract::ProtectionAtPublish::NotApplicable,
                current_health: storage_contract::CurrentHealth::Healthy,
                request_digest: "digest".into(),
                sequence: index,
                result: json_codec::json!({}),
            },
        });
        expected.insert(key, json_codec::json!(index.to_string()));
    }
    // Push one receipt at a time, capturing every sealed body: only the open
    // tail and one fresh seal stay resident, while a store needs each sealed
    // body published on the members, exactly as the publications that sealed
    // them left it.
    let mut log = storage_contract::OperationLog::new();
    let mut bodies = Vec::new();
    for record in records {
        log.push(record).unwrap();
        if let Some((ordinal, segment)) = log.fresh_seal()
            && bodies.len() as u64 == ordinal
        {
            bodies.push(segment.clone());
        }
    }
    state.operations = log;
    assert_eq!(state.operations.sealed().len(), 16);
    assert_eq!(bodies.len(), 16);
    state.sequence = u64::MAX;
    let prepared = prepare_snapshot(&state, MAX_METADATA).unwrap();
    assert_reference(&state);
    assert_eq!(
        prepared.descriptor.maps["operation_locators"],
        stored_map(&[], expected)
    );
    let (_temp, config) = fixture_disk();
    for (segment, reference) in bodies.iter().zip(state.operations.sealed()) {
        for disk in &config.disks {
            assert_eq!(
                store_object(disk, storage_contract::OPERATION_SEGMENT_KIND, segment).unwrap(),
                reference.hash
            );
        }
    }
    prepared.store(&config.disks, &state).unwrap();
    assert_eq!(
        load_raw_snapshot_fixture(&config.disks[0], &prepared.hash).unwrap(),
        state
    );
}

#[test]
fn content_states_keep_exact_string_values_and_unknown_values_fail_closed() {
    let (_temp, config) = fixture_disk();
    let disk = &config.disks[0];
    let raw = json_codec::json!({"a":"AVAILABLE","m":"MIGRATED","p":"PURGED"});
    let state = State {
        content_states: json_codec::from_value(raw.clone()).unwrap(),
        ..Default::default()
    };
    let prepared = prepare_snapshot(&state, MAX_METADATA).unwrap();
    prepared.store(&config.disks, &state).unwrap();
    assert_eq!(
        prepared.descriptor.maps["content_states"],
        stored_map(&[], raw.as_object().unwrap().clone())
    );
    for invalid in [
        json_codec::json!("available"),
        json_codec::json!("UNKNOWN"),
        json_codec::json!({"AVAILABLE":null}),
        json_codec::json!(null),
    ] {
        let mut damaged = prepared.descriptor.clone();
        damaged.maps.insert(
            "content_states".into(),
            stored_map(
                &config.disks,
                json_codec::Map::from_iter([("a".into(), invalid)]),
            ),
        );
        let hash = store_object(disk, "snapshot", &damaged).unwrap();
        let path = object_path(disk, "snapshot", &hash);
        let bytes = std::fs::read(&path).unwrap();
        assert!(load_raw_snapshot_fixture(disk, &hash).is_err());
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}

// Frozen bytes from a real publication, not an independent encoder proof.
// Restore its exact namespace objects and compare their logical contents with
// the separately recorded flat entries; never repack entries into new roots.
#[test]
fn canonical_value_encoding_preserves_publication_bytes_and_hashes() {
    let fixture = crate::canonical_publication_fixture::CanonicalPublicationFixture::read();
    let state = fixture.restore(&[]);
    assert_eq!(state.commits.len(), 7);
    assert_eq!(state.versions.len(), 4);
    assert_eq!(
        canonical_json(&state).unwrap(),
        fixture.state_bytes.as_bytes()
    );
    let snapshot = prepare_snapshot(&state, MAX_METADATA).unwrap();
    assert_eq!(snapshot.hash, fixture.snapshot_hash);
    assert_eq!(object_bytes(&snapshot.objects), fixture.snapshot_objects);
    let (catalog, objects) = crate::history_package::prepare_history_manifests(
        &state,
        MAX_METADATA,
        &crate::namespace::fixture_store::FixtureSource,
    )
    .unwrap();
    assert_eq!(catalog.reference(), &fixture.history_roots);
    assert_eq!(object_bytes(&objects), fixture.history_objects);
}

/// One-time enrichment of the existing frozen vector. The old vector has
/// logical entries but no namespace object bytes. Reproduce those objects only
/// if every complete namespace identity matches the already recorded commit;
/// preserve all existing fields, IDs, times, bytes and hashes unchanged.
/// This is not the normal fixture loading path or an old-format fallback.
#[test]
#[ignore = "one-time fixture enrichment tool; adds only namespace_objects"]
fn enrich_canonical_publication_fixture_namespace_objects() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test/engine/unit/journal/fixtures/canonical_publication.json");
    let mut fixture: json_codec::Value =
        json_codec::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(
        fixture.get("namespace_objects").is_none(),
        "fixture is already enriched"
    );
    let state: State = json_codec::from_str(fixture["state_bytes"].as_str().unwrap()).unwrap();
    let commit_namespaces: BTreeMap<String, Vec<storage_contract::Entry>> =
        json_codec::from_value(fixture["commit_namespaces"].clone()).unwrap();
    assert_eq!(commit_namespaces.len(), state.commits.len());
    let mut namespace_objects = BTreeMap::new();
    for (commit_id, commit) in state.commits.iter() {
        let (namespace, objects) =
            crate::namespace::fixture_namespace(&commit_namespaces[commit_id]);
        assert_eq!(
            namespace, commit.namespace,
            "{commit_id}: recorded roots cannot be reproduced; retain the original fixture"
        );
        namespace_objects.extend(object_bytes(objects.objects()));
    }
    let object_count = namespace_objects.len();
    fixture["namespace_objects"] = json_codec::to_value(namespace_objects).unwrap();
    let enriched: crate::canonical_publication_fixture::CanonicalPublicationFixture =
        json_codec::from_value(fixture.clone()).unwrap();
    enriched.restore(&[]);
    std::fs::write(&path, json_codec::to_string_pretty(&fixture).unwrap()).unwrap();
    println!(
        "added {object_count} namespace objects to {}",
        path.display()
    );
}

/// Manual fixture capture after an intentional storage-format change. This
/// records production output for regression coverage, not independent proof of
/// the encoder. Preserve the exact published namespace closure alongside its
/// flat logical entry oracle: local editing may retain non-bulk page boundaries.
/// Run on the authorized GitHub verification branch with:
/// `cargo test -p storage-engine --lib journal::snapshot_partition_tests::regenerate_canonical_publication_fixture -- --exact --ignored --nocapture`
#[test]
#[ignore = "manual fixture regeneration tool; rewrites the committed fixture file"]
fn regenerate_canonical_publication_fixture() {
    let (temp, config) = fixture_disk();
    let mut engine = crate::Engine::initialize(&temp.path().join("state"), config).unwrap();
    let repo = engine
        .create_disk_root("P01", "owner", storage_contract::OperationId::generate())
        .unwrap();
    for round in ["base", "final"] {
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "owner")
            .unwrap();
        engine
            .stage_bytes(
                &workspace.workspace_id,
                "folder/a.txt",
                format!("a {round}").as_bytes(),
            )
            .unwrap();
        engine
            .stage_bytes(
                &workspace.workspace_id,
                "folder/报告.txt",
                format!("报 {round}").as_bytes(),
            )
            .unwrap();
        engine
            .commit_workspace(storage_contract::CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: storage_contract::OperationId::generate(),
                principal: "owner".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "legacy shared entries fixture".into(),
                request_digest: None,
            })
            .unwrap();
    }
    // Consumers decode state_bytes, so capture through the same decode: the
    // live state released its cold locator index at the published basis, while
    // a decoded tail-only log carries the derivable complete index.
    let state_bytes = String::from_utf8(canonical_json(engine.state()).unwrap()).unwrap();
    let state: State = json_codec::from_slice(state_bytes.as_bytes()).unwrap();
    assert_eq!(canonical_json(&state).unwrap(), state_bytes.as_bytes());
    // The oracle fixtures rely on genuinely populated coverage; a scenario
    // change that silently drops a field would weaken every consumer.
    assert_eq!(state.repositories.len(), 1);
    assert_eq!(state.histories.len(), 1);
    assert_eq!(
        state.branches.len(),
        3,
        "main plus two shared file branches"
    );
    assert_eq!(state.commits.len(), 7);
    assert_eq!(state.versions.len(), 4);
    for (name, empty) in [
        ("placements", state.placements.is_empty()),
        ("content_states", state.content_states.is_empty()),
        ("file_selections", state.file_selections.is_empty()),
        ("directory_branches", state.directory_branches.is_empty()),
        ("directory_trees", state.directory_trees.is_empty()),
        ("file_contexts", state.file_contexts.is_empty()),
        (
            "directory_tree_selections",
            state.directory_tree_selections.is_empty(),
        ),
        ("file_locations", state.file_locations.is_empty()),
        (
            "directory_selections",
            state.directory_selections.is_empty(),
        ),
        ("file_names", state.file_names.is_empty()),
        ("events", state.events.is_empty()),
        ("operations", state.operations.is_empty()),
    ] {
        assert!(!empty, "fixture scenario leaves {name} unpopulated");
    }
    let members = engine.members();
    let mut commit_namespaces = BTreeMap::new();
    let mut namespace_objects = BTreeMap::new();
    let mut visited = BTreeSet::new();
    for (commit_id, commit) in state.commits.iter() {
        let entries = engine
            .commit_namespace(commit_id)
            .unwrap()
            .entries()
            .unwrap();
        commit_namespaces.insert(commit_id.clone(), entries);
        crate::namespace::visit_namespace_objects(
            &commit.namespace.root,
            &mut |kind, hash| {
                let object =
                    crate::publication_evidence::verified_graph_object(&members, kind, hash)?;
                namespace_objects.insert(
                    format!("{kind}/{hash}"),
                    String::from_utf8(object.bytes().to_vec()).unwrap(),
                );
                Ok(object)
            },
            &mut visited,
        )
        .unwrap();
    }
    let snapshot = prepare_snapshot(&state, MAX_METADATA).unwrap();
    let (catalog, objects) = crate::history_package::prepare_history_manifests(
        &state,
        MAX_METADATA,
        &crate::history_package::fixture_member_source(&members),
    )
    .unwrap();
    let fixture = crate::canonical_publication_fixture::CanonicalPublicationFixture {
        commit_namespaces,
        namespace_objects,
        history_objects: object_bytes(&objects),
        history_roots: catalog.reference().clone(),
        snapshot_hash: snapshot.hash,
        snapshot_objects: object_bytes(&snapshot.objects),
        state_bytes,
    };
    fixture.restore(&[]);
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test/engine/unit/journal/fixtures/canonical_publication.json");
    std::fs::write(&path, json_codec::to_string_pretty(&fixture).unwrap()).unwrap();
    println!("wrote {}", path.display());
}
