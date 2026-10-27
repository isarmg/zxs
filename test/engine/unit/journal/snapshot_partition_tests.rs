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
        events: (0..count)
            .map(|index| Event {
                sequence: index as u64,
                operation_id: format!("op_{index:032x}").parse().unwrap(),
                kind: "MUTATION".into(),
                object_id: "retained-history".into(),
            })
            .collect(),
        ..Default::default()
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
    assert!(matches!(
        prepared.descriptor.events.kind(),
        crate::metadata_map::ReferenceKind::Index
    ));
    assert!(matches!(
        prepared.descriptor.maps["file_notes"].kind(),
        crate::metadata_map::ReferenceKind::Index
    ));
    assert!(prepared.objects.len() > 20);
    let (_temp, config) = fixture_disk();
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
    for key in ["00000000000000000004", "2"] {
        let mut events = (0..3)
            .map(|i| {
                (
                    format!("{i:020}"),
                    json_codec::to_value(&state.events[i]).unwrap(),
                )
            })
            .collect::<json_codec::Map<_, _>>();
        let value = events.remove("00000000000000000002").unwrap();
        events.insert(key.into(), value);
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
    let mut expected = json_codec::Map::new();
    for index in 0..4096u64 {
        let key = format!("op_{index:032x}");
        state
            .operation_principals
            .insert(key.parse().unwrap(), "owner".into());
        expected.insert(key, json_codec::json!("owner"));
    }
    let prepared = prepare_snapshot(&state, MAX_METADATA).unwrap();
    assert_reference(&state);
    assert_eq!(
        prepared.descriptor.maps["operation_principals"],
        stored_map(&[], expected)
    );
    let (_temp, config) = fixture_disk();
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

// Independent fixed bytes from a real pool. The C fixture was transformed using
// a separate JSON/BLAKE3 tool, not generated by the encoder under test.
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct CanonicalPublicationFixture {
    state_bytes: String,
    snapshot_hash: String,
    snapshot_objects: BTreeMap<String, String>,
    history_roots: crate::metadata_map::Reference,
    history_objects: BTreeMap<String, String>,
}
#[test]
fn canonical_value_encoding_preserves_publication_bytes_and_hashes() {
    let fixture: CanonicalPublicationFixture =
        json_codec::from_slice(include_bytes!("fixtures/canonical_publication.json")).unwrap();
    let state: State = json_codec::from_slice(fixture.state_bytes.as_bytes()).unwrap();
    assert_eq!(state.commits.len(), 7);
    assert_eq!(state.versions.len(), 4);
    assert_eq!(
        canonical_json(&state).unwrap(),
        fixture.state_bytes.as_bytes()
    );
    let snapshot = prepare_snapshot(&state, MAX_METADATA).unwrap();
    assert_eq!(snapshot.hash, fixture.snapshot_hash);
    assert_eq!(object_bytes(&snapshot.objects), fixture.snapshot_objects);
    let (catalog, objects) =
        crate::history_package::prepare_history_manifests(&state, MAX_METADATA).unwrap();
    assert_eq!(catalog.reference(), &fixture.history_roots);
    assert_eq!(object_bytes(&objects), fixture.history_objects);
}
