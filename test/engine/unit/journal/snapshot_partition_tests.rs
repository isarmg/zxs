use super::*;
use std::collections::BTreeSet;

fn partition(
    map: &json_codec::Map<String, json_codec::Value>,
) -> Vec<json_codec::Map<String, json_codec::Value>> {
    state_pages(map).collect()
}

fn synthetic_map() -> json_codec::Map<String, json_codec::Value> {
    (0u32..4096)
        .map(|n| {
            let key = blake3::hash(&n.to_le_bytes()).to_hex().to_string();
            (key, json_codec::json!({"data": "immutable".repeat(64)}))
        })
        .collect()
}
fn hashes(pages: &[json_codec::Map<String, json_codec::Value>]) -> BTreeSet<String> {
    pages
        .iter()
        .map(|page| object_hash("state-page", page).unwrap())
        .collect()
}

#[test]
fn deterministic_random_key_insertion_and_update_reuse_unaffected_pages() {
    let original = synthetic_map();
    let pages = partition(&original);
    assert!(pages.len() > 50);
    assert!(
        pages
            .iter()
            .all(|page| !page.is_empty() && page.len() <= STATE_PAGE_ENTRIES)
    );
    assert_eq!(
        pages
            .iter()
            .flat_map(|p| p.clone())
            .collect::<json_codec::Map<_, _>>(),
        original
    );
    let before = hashes(&pages);
    // Choose a non-boundary new key inside a page that cannot overflow.
    let key = (4096u32..8192)
        .map(|n| blake3::hash(&n.to_le_bytes()).to_hex().to_string())
        .find(|key| {
            blake3::hash(key.as_bytes()).as_bytes()[0] & 31 != 0
                && pages.iter().any(|page| {
                    page.len() < STATE_PAGE_ENTRIES
                        && page.keys().next().unwrap() < key
                        && key < page.keys().next_back().unwrap()
                })
        })
        .unwrap();
    let mut changed = original.clone();
    changed.insert(key.clone(), json_codec::json!("new"));
    let after = hashes(&partition(&changed));
    assert_eq!(before.difference(&after).count(), 1);
    assert_eq!(after.difference(&before).count(), 1);
    // Values cannot influence boundaries, even when their size changes.
    changed.insert(key, json_codec::json!("updated".repeat(1024)));
    let updated = hashes(&partition(&changed));
    assert_eq!(after.difference(&updated).count(), 1);
    assert_eq!(updated.difference(&after).count(), 1);
    // Insertion order has no influence on the content-addressed pages.
    let reverse = changed.into_iter().rev().collect();
    assert_eq!(updated, hashes(&partition(&reverse)));
}

#[test]
fn adversarial_boundary_free_keys_remain_bounded_and_lossless() {
    let map = (0..10000)
        .map(|n| format!("key-{n:05}"))
        .filter(|key| blake3::hash(key.as_bytes()).as_bytes()[0] & 31 != 0)
        .take(STATE_PAGE_ENTRIES * 3 + 1)
        .map(|key| (key, json_codec::json!("value")))
        .collect::<json_codec::Map<_, _>>();
    let pages = partition(&map);
    assert_eq!(
        pages.iter().map(|p| p.len()).collect::<Vec<_>>(),
        vec![128, 128, 128, 1]
    );
    assert_eq!(
        pages
            .into_iter()
            .flatten()
            .collect::<json_codec::Map<_, _>>(),
        map
    );
    assert!(partition(&json_codec::Map::new()).is_empty());
}

fn event_state(count: usize) -> State {
    State {
        events: (0..count)
            .map(|index| Event {
                sequence: index as u64,
                operation_id: format!("op-{index}"),
                kind: "MUTATION".into(),
                object_id: "retained-history".into(),
            })
            .collect(),
        ..Default::default()
    }
}

#[test]
fn event_pages_remove_scalar_cap_preserve_order_and_append_locality() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([
        temp.path().join("one"),
        temp.path().join("two"),
        temp.path().join("three"),
    ]);
    let state = event_state(2000);
    let maximum = 20_000;
    assert!(encode_metadata(&state.events, maximum).is_err());
    let prepared = prepare_snapshot(&state, maximum).unwrap();
    assert!(!prepared.descriptor.scalars.contains_key("events"));
    assert!(prepared.descriptor.pages["events"].len() > 1);
    let hash = prepared.store(&config.disks, &state).unwrap();
    for disk in &config.disks {
        assert_eq!(load_snapshot(disk, &hash).unwrap(), state);
        assert_eq!(
            read_bytes(&object_path(disk, "snapshot", &hash), MAX_METADATA).unwrap(),
            encode_metadata(&prepared.descriptor, maximum).unwrap()
        );
    }
    let next = prepare_snapshot(&event_state(2001), maximum).unwrap();
    let old = &prepared.descriptor.pages["events"];
    let new = &next.descriptor.pages["events"];
    assert_eq!(&old[..old.len() - 1], &new[..old.len() - 1]);
    // Prepared objects remain immutable, and installation still refuses
    // conflicting disk bytes rather than overwriting or trusting a cache.
    let page = &old[0];
    let path = object_path(&config.disks[0], "state-page", page);
    std::fs::write(&path, b"corrupt").unwrap();
    assert_eq!(
        prepared.store(&config.disks, &state).unwrap_err().code(),
        storage_contract::ErrorCode::ObjectCollision
    );
    assert_eq!(std::fs::read(path).unwrap(), b"corrupt");
}

#[test]
fn current_snapshot_and_candidate_are_strict_unversioned_structures() {
    let prepared = prepare_snapshot(&State::default(), MAX_METADATA).unwrap();
    let mut snapshot = json_codec::to_value(&prepared.descriptor).unwrap();
    assert!(snapshot.get("format_version").is_none());
    snapshot["format_version"] = json_codec::json!(2);
    assert!(json_codec::from_value::<Snapshot>(snapshot).is_err());
    let candidate = Candidate {
        pool_id: new_id("pool"),
        sequence: 1,
        transaction_id: new_id("tx"),
        operation_id: new_id("op"),
        request_digest: "a".repeat(64),
        previous_snapshot: None,
        snapshot_hash: prepared.hash,
        versions: Vec::new(),
        metadata_disk_ids: Vec::new(),
        history_manifest_hashes: BTreeMap::new(),
    };
    let mut candidate = json_codec::to_value(candidate).unwrap();
    assert!(candidate.get("format_version").is_none());
    candidate["format_version"] = json_codec::json!(2);
    assert!(json_codec::from_value::<Candidate>(candidate).is_err());
    let config = Config::development(["a".into(), "b".into(), "c".into()]);
    let mut config = json_codec::to_value(config).unwrap();
    assert!(config.get("format_version").is_none());
    config["format_version"] = json_codec::json!(2);
    assert!(json_codec::from_value::<Config>(config).is_err());
}

#[test]
fn current_snapshot_requires_nonsparse_state_fields() {
    let state = State::default();
    let snapshot = prepare_snapshot(&state, MAX_METADATA).unwrap().descriptor;
    assert_eq!(
        resolve_snapshot(snapshot, |_| panic!("empty maps have no pages")).unwrap(),
        state
    );
    for field in [
        "business_generation",
        "history_boundaries",
        "placement_rules",
        "protection_policies",
        "operation_principals",
    ] {
        let mut snapshot = prepare_snapshot(&state, MAX_METADATA).unwrap().descriptor;
        if field == "business_generation" {
            assert!(snapshot.scalars.remove(field).is_some());
        } else {
            assert!(snapshot.pages.remove(field).is_some());
        }
        let error = resolve_snapshot(snapshot, |_| panic!("empty maps have no pages")).unwrap_err();
        assert_eq!(
            error.code(),
            storage_contract::ErrorCode::InvalidJson,
            "{field}"
        );
    }
}

#[test]
fn scalar_event_snapshots_are_rejected_without_rewriting_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([
        temp.path().join("one"),
        temp.path().join("two"),
        temp.path().join("three"),
    ]);
    let disk = &config.disks[0];
    let state = event_state(1);
    let mut descriptor = prepare_snapshot(&state, MAX_METADATA).unwrap().descriptor;
    descriptor.pages.remove("events");
    descriptor.scalars.insert(
        "events".into(),
        json_codec::to_value(&state.events).unwrap(),
    );
    let hash = store_object(disk, "snapshot", &descriptor).unwrap();
    let path = object_path(disk, "snapshot", &hash);
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(
        load_snapshot(disk, &hash).unwrap_err().code(),
        storage_contract::ErrorCode::FormatUnsupported
    );
    assert_eq!(std::fs::read(path).unwrap(), bytes);
    descriptor.scalars.remove("events");
    let absent_hash = store_object(disk, "snapshot", &descriptor).unwrap();
    assert_eq!(
        load_snapshot(disk, &absent_hash).unwrap_err().code(),
        storage_contract::ErrorCode::FormatUnsupported
    );
}

#[test]
fn event_snapshot_rejects_gaps_and_ambiguous_scalar_page_fields() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([
        temp.path().join("one"),
        temp.path().join("two"),
        temp.path().join("three"),
    ]);
    let disk = &config.disks[0];
    let prepared = prepare_snapshot(&event_state(1), MAX_METADATA).unwrap();
    prepared
        .store(std::slice::from_ref(disk), &event_state(1))
        .unwrap();
    let mut descriptor = prepared.descriptor;
    let original_pages = descriptor.pages["events"].clone();
    descriptor
        .pages
        .get_mut("events")
        .unwrap()
        .extend(original_pages);
    let duplicate_hash = store_object(disk, "snapshot", &descriptor).unwrap();
    assert_eq!(
        load_snapshot(disk, &duplicate_hash).unwrap_err().code(),
        storage_contract::ErrorCode::ObjectCorrupt
    );
    let bad_page = json_codec::json!({"00000000000000000001": event_state(1).events[0]});
    descriptor.pages.insert(
        "events".into(),
        vec![store_object(disk, "state-page", &bad_page).unwrap()],
    );
    let hash = store_object(disk, "snapshot", &descriptor).unwrap();
    assert_eq!(
        load_snapshot(disk, &hash).unwrap_err().code(),
        storage_contract::ErrorCode::ObjectCorrupt
    );
    descriptor
        .scalars
        .insert("events".into(), json_codec::json!([]));
    let hash = store_object(disk, "snapshot", &descriptor).unwrap();
    assert_eq!(
        load_snapshot(disk, &hash).unwrap_err().code(),
        storage_contract::ErrorCode::ObjectCorrupt
    );
}

fn assert_prepared_snapshot_identity(state: &State) {
    let reference = prepare_snapshot_reference(state, MAX_METADATA).unwrap();
    let actual = prepare_snapshot(state, MAX_METADATA).unwrap();
    assert_eq!(actual.hash, reference.hash);
    assert_eq!(
        canonical_json(&actual.descriptor).unwrap(),
        canonical_json(&reference.descriptor).unwrap()
    );
    let objects = |snapshot: PreparedSnapshot| {
        let mut objects = snapshot
            .objects
            .into_iter()
            .map(|object| ((object.kind, object.hash), object.bytes))
            .collect::<Vec<_>>();
        objects.sort();
        objects
    };
    assert_eq!(objects(actual), objects(reference));
}

#[test]
fn typed_snapshot_matches_full_value_encoder_exactly() {
    // Empty required maps and omitted optional maps have different wire rules.
    assert_prepared_snapshot_identity(&State::default());
    let mut state = event_state(2000);
    state.sequence = u64::MAX;
    state.business_generation = u64::MAX - 1;
    state.topology_generation = u64::MAX - 2;
    state
        .file_selection_generations
        .insert("unicode-文件".into(), u64::MAX);
    state
        .directory_selection_generations
        .insert("directory".into(), 9_007_199_254_740_992);
    state.file_notes = synthetic_map()
        .into_iter()
        .map(|(key, _)| (key, "note".into()))
        .collect();
    state
        .file_names
        .insert("\u{10000}".into(), "supplementary".into());
    state.file_names.insert("\u{e000}".into(), "bmp".into());
    for auth in [
        None,
        Some(json_codec::json!({})),
        Some(json_codec::json!({"nested": {"roles": ["reader"]}, "name": "用户"})),
        Some(json_codec::json!([{"nested": true}, null, "scalar"])),
        Some(json_codec::json!("legacy scalar")),
        Some(json_codec::json!(null)),
    ] {
        state.control_auth = auth;
        assert_prepared_snapshot_identity(&state);
    }
}

#[test]
fn typed_snapshot_only_buffers_one_page_of_borrowed_entries() {
    let state = State {
        content_states: (0..20_000)
            .map(|i| {
                (
                    format!("version-{i:020}"),
                    storage_contract::ContentState::Available,
                )
            })
            .collect(),
        ..event_state(20_000)
    };
    assert_prepared_snapshot_identity(&state);
    let peak = snapshot::SNAPSHOT_PEAK_PAGE_ENTRIES.with(|peak| peak.get());
    assert!(
        peak > 0 && peak <= STATE_PAGE_ENTRIES,
        "peak buffered entries: {peak}"
    );
    // The previous encoder first materialized both whole collections as JSON,
    // before page clones. This is structural evidence, not an RSS/timing claim.
    let old_value = json_codec::to_value(&state).unwrap();
    assert_eq!(
        old_value["content_states"].as_object().unwrap().len(),
        20_000
    );
    assert_eq!(old_value["events"].as_array().unwrap().len(), 20_000);
}

#[test]
fn alternate_page_partitions_preserve_exact_bound_current_snapshots() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([
        temp.path().join("one"),
        temp.path().join("two"),
        temp.path().join("three"),
    ]);
    let disk = &config.disks[0];
    let state = State {
        sequence: 123,
        business_generation: 19,
        events: event_state(300).events,
        content_states: synthetic_map()
            .into_iter()
            .map(|(key, _)| key)
            .enumerate()
            .map(|(n, key)| {
                (
                    key,
                    if n % 3 == 0 {
                        storage_contract::ContentState::Purged
                    } else {
                        storage_contract::ContentState::Available
                    },
                )
            })
            .collect(),
        file_notes: [("文件".into(), "preserved note".into())].into(),
        control_auth: Some(json_codec::json!({"nested": {"roles": ["reader"]}})),
        ..Default::default()
    };
    let value = json_codec::to_value(&state).unwrap();
    let mut alternate = Snapshot {
        sequence: state.sequence,
        pages: BTreeMap::new(),
        scalars: json_codec::Map::new(),
    };
    for (field, value) in value.as_object().unwrap() {
        if let Some(map) = value.as_object() {
            let entries = map.iter().collect::<Vec<_>>();
            let pages = entries
                .chunks(STATE_PAGE_ENTRIES)
                .map(|chunk| {
                    let page = chunk
                        .iter()
                        .map(|(k, v)| ((*k).clone(), (*v).clone()))
                        .collect::<json_codec::Map<_, _>>();
                    store_object(disk, "state-page", &page).unwrap()
                })
                .collect();
            alternate.pages.insert(field.clone(), pages);
        } else {
            alternate.scalars.insert(field.clone(), value.clone());
        }
    }
    // Different map partitions remain valid current-format evidence, but
    // scalar event arrays are no longer an accepted representation.
    alternate.scalars.remove("events");
    let modern = prepare_snapshot(&state, MAX_METADATA).unwrap();
    alternate
        .pages
        .insert("events".into(), modern.descriptor.pages["events"].clone());
    let modern_hash = modern.store(std::slice::from_ref(disk), &state).unwrap();
    let alternate_hash = store_object(disk, "snapshot", &alternate).unwrap();
    assert_ne!(alternate_hash, modern_hash);
    assert_eq!(load_snapshot(disk, &alternate_hash).unwrap(), state);
    assert_eq!(load_snapshot(disk, &modern_hash).unwrap(), state);
    // Writing a later state must not change either retained snapshot.
    let mut next = state.clone();
    next.sequence += 1;
    next.content_states.insert(
        "new-version".into(),
        storage_contract::ContentState::Migrated,
    );
    let next_hash = store_snapshot(std::slice::from_ref(disk), &next).unwrap();
    assert_eq!(load_snapshot(disk, &next_hash).unwrap(), next);
    assert_eq!(load_snapshot(disk, &alternate_hash).unwrap(), state);
    assert_eq!(load_snapshot(disk, &modern_hash).unwrap(), state);
}

// Frozen output from a real three-disk engine pool, captured before the
// canonical Value encoding fast path. Two saves of nested files (including a
// Unicode filename) create seven commits, directory branches and four versions.
// The single-administrator contract deliberately removed the two repository
// ACL fields. Its fixture was independently re-encoded and rehashed once; all
// unrelated objects and the original graph topology remain unchanged. This
// fixed byte oracle pins the current publication bytes and hashes.
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct CanonicalPublicationFixture {
    state_bytes: String,
    snapshot_hash: String,
    snapshot_objects: BTreeMap<String, String>,
    history_manifest_hashes: BTreeMap<String, String>,
    history_objects: BTreeMap<String, String>,
}

fn publication_object_bytes(objects: &[PreparedMetadataObject]) -> BTreeMap<String, String> {
    let mut result = BTreeMap::new();
    for object in objects {
        let key = format!("{}/{}", object.kind, object.hash);
        let bytes = String::from_utf8(object.bytes.clone()).unwrap();
        if let Some(previous) = result.insert(key, bytes.clone()) {
            // Empty history maps can share one content-addressed page.
            assert_eq!(previous, bytes);
        }
    }
    result
}

#[test]
fn canonical_value_encoding_preserves_publication_bytes_and_hashes() {
    let fixture: CanonicalPublicationFixture =
        json_codec::from_slice(include_bytes!("fixtures/canonical_publication.json")).unwrap();
    let state: State = json_codec::from_slice(fixture.state_bytes.as_bytes()).unwrap();
    assert_eq!(state.commits.len(), 7);
    assert_eq!(
        state
            .commits
            .values()
            .filter(|commit| !commit.entries.is_empty())
            .count(),
        6
    );
    assert_eq!(
        state
            .commits
            .values()
            .map(|commit| commit.entries.len())
            .sum::<usize>(),
        14
    );
    assert_eq!(state.branches.len(), 3);
    assert_eq!(state.versions.len(), 4);
    assert_eq!(
        canonical_json(&state).unwrap(),
        fixture.state_bytes.as_bytes()
    );

    let snapshot = prepare_snapshot(&state, MAX_METADATA).unwrap();
    assert_eq!(snapshot.hash, fixture.snapshot_hash);
    assert_eq!(fixture.snapshot_objects.len(), 23);
    assert_eq!(
        publication_object_bytes(&snapshot.objects),
        fixture.snapshot_objects
    );

    let (history_hashes, history_objects) =
        crate::history_package::prepare_history_manifests(&state, MAX_METADATA).unwrap();
    assert_eq!(history_hashes, fixture.history_manifest_hashes);
    assert_eq!(fixture.history_objects.len(), 15);
    assert_eq!(
        publication_object_bytes(&history_objects),
        fixture.history_objects
    );
}

#[test]
fn content_state_snapshot_pages_keep_string_bytes_and_reject_unknown_values() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([temp.path().join("disk")]);
    let disk = &config.disks[0];
    let raw = json_codec::json!({"a":"AVAILABLE","m":"MIGRATED","p":"PURGED"});
    let state = State {
        content_states: json_codec::from_value(raw.clone()).unwrap(),
        ..State::default()
    };
    let prepared = prepare_snapshot(&state, MAX_METADATA).unwrap();
    let expected_pages = state_pages(raw.as_object().unwrap()).collect::<Vec<_>>();
    let expected_hashes = expected_pages
        .iter()
        .map(|page| object_hash("state-page", page).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(prepared.descriptor.pages["content_states"], expected_hashes);
    let descriptor = prepared.descriptor.clone();
    let hash = prepared.store(std::slice::from_ref(disk), &state).unwrap();
    for (page, hash) in expected_pages.iter().zip(expected_hashes) {
        assert_eq!(
            std::fs::read(object_path(disk, "state-page", &hash)).unwrap(),
            canonical_json(page).unwrap()
        );
    }
    assert_eq!(load_snapshot(disk, &hash).unwrap(), state);
    for invalid in [
        json_codec::json!("available"),
        json_codec::json!("UNKNOWN"),
        json_codec::json!({"AVAILABLE":null}),
        json_codec::json!(null),
    ] {
        let page = json_codec::json!({"a":invalid});
        let page_hash = store_object(disk, "state-page", &page).unwrap();
        let bytes = std::fs::read(object_path(disk, "state-page", &page_hash)).unwrap();
        let mut damaged = descriptor.clone();
        damaged
            .pages
            .insert("content_states".into(), vec![page_hash.clone()]);
        let bad_hash = store_object(disk, "snapshot", &damaged).unwrap();
        assert!(load_snapshot(disk, &bad_hash).is_err());
        assert_eq!(
            std::fs::read(object_path(disk, "state-page", &page_hash)).unwrap(),
            bytes
        );
    }
}
