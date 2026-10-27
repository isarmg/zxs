//! Independent schema/byte oracle for typed-key radix snapshot updates.
//!
//! Fixtures are decoded through State's serde schema; the reference encoder
//! materializes State independently of the incremental field classification.
use super::*;

const REQUIRED_FIELDS: &[&str] = &[
    "repositories",
    "histories",
    "branches",
    "commits",
    "versions",
    "placements",
    "merge_links",
    "history_boundaries",
    "content_states",
    "operations",
    "plans",
    "placement_rules",
    "protection_policies",
    "operation_principals",
];
const OPTIONAL_FIELDS: &[&str] = &[
    "file_selections",
    "directory_branches",
    "directory_trees",
    "file_write_origins",
    "file_contexts",
    "file_path_nodes",
    "file_metadata_nodes",
    "file_branch_merges",
    "file_branch_aliases",
    "file_node_parents",
    "file_path_notes",
    "directory_tree_selections",
    "file_locations",
    "file_backup_paths",
    "file_backup_policies",
    "directory_selections",
    "file_notes",
    "version_notes",
    "file_names",
    "directory_selection_generations",
    "file_selection_generations",
];

macro_rules! every_tracked_field {
    ($apply:ident) => {
        $apply!(repositories);
        $apply!(histories);
        $apply!(branches);
        $apply!(commits);
        $apply!(versions);
        $apply!(placements);
        $apply!(merge_links);
        $apply!(history_boundaries);
        $apply!(content_states);
        $apply!(operations);
        $apply!(plans);
        $apply!(placement_rules);
        $apply!(protection_policies);
        $apply!(operation_principals);
        $apply!(file_selections);
        $apply!(directory_branches);
        $apply!(directory_trees);
        $apply!(file_write_origins);
        $apply!(file_contexts);
        $apply!(file_path_nodes);
        $apply!(file_metadata_nodes);
        $apply!(file_branch_merges);
        $apply!(file_branch_aliases);
        $apply!(file_node_parents);
        $apply!(file_path_notes);
        $apply!(directory_tree_selections);
        $apply!(file_locations);
        $apply!(file_backup_paths);
        $apply!(file_backup_policies);
        $apply!(directory_selections);
        $apply!(file_notes);
        $apply!(version_notes);
        $apply!(file_names);
        $apply!(directory_selection_generations);
        $apply!(file_selection_generations);
    };
}

fn fixture() -> json_codec::Value {
    json_codec::from_slice(include_bytes!("fixtures/canonical_publication.json")).unwrap()
}

fn fixture_state() -> State {
    json_codec::from_str(fixture()["state_bytes"].as_str().unwrap()).unwrap()
}

fn populated_state() -> State {
    let mut value: json_codec::Value =
        json_codec::from_str(fixture()["state_bytes"].as_str().unwrap()).unwrap();
    // Supplement the real publication fixture, so every field must demonstrate
    // actual serialization on a cold pass and zero serialization on reuse.
    for field in [
        "file_write_origins",
        "file_branch_aliases",
        "file_path_notes",
        "file_backup_policies",
        "file_notes",
        "version_notes",
    ] {
        value[field] = json_codec::json!({"文件/\u{10000}": "值/\u{e000}\n\"\\"});
    }
    value["file_node_parents"] = json_codec::json!({"node": ["parent-一", "parent-二"]});
    value["file_backup_paths"] = json_codec::json!({"file": {"B01": "目录/备份"}});
    value["directory_selection_generations"] =
        json_codec::json!({"directory": "18446744073709551615"});
    value["file_selection_generations"] = json_codec::json!({"file": "9007199254740992"});
    value["merge_links"] = json_codec::json!({"merge": {
        "merge_link_id": "merge", "history_tree_id": "history", "source_branch_id": "source",
        "source_commit_id": "source-commit", "target_branch_id": "target",
        "target_commit_id": "target-commit", "author": "作者", "operation_id": "op_00000000000000000000000000000000"
    }});
    value["history_boundaries"] = json_codec::json!({"boundary": {
        "boundary_id": "boundary", "history_tree_id": "history", "local_commit_id": "commit",
        "local_branch_id": null, "relation": "split", "external_history_tree_id": "external",
        "external_commit_id": "external-commit", "source_merge_link_id": null
    }});
    value["plans"] = json_codec::json!({"plan": {
        "plan_id": "plan", "kind": "maintenance", "principal": "作者",
        "expected_sequence": "18446744073709551615", "digest": "digest",
        "input": {"nested": [null, false, {"\u{10000}": "supplementary", "\u{e000}": "BMP"}]},
        "state": "ready"
    }});
    value["placement_rules"] = json_codec::json!({"rule": {
        "rule_id": "rule", "repository_id": "repository", "path_prefix": "目录",
        "primary_slot_id": "P01", "generation": "9007199254740992"
    }});
    value["protection_policies"] = json_codec::json!({"policy": {
        "policy_id": "policy", "repository_id": "repository", "path_prefix": "目录",
        "target_copies": 2, "inheritable": true, "scope_file_id": "file",
        "backup_prefixes": {"B01": "备份"}, "generation": "18446744073709551615"
    }});
    value["file_path_nodes"] = json_codec::json!({"path-node": {
        "node_id": "path-node", "history_tree_id": "history", "file_id": "file", "version_id": "version",
        "parent_node_ids": ["parent"], "branch_id": "branch", "path": "目录/新", "previous_path": "目录/旧",
        "operation_id": "op_00000000000000000000000000000000", "author": "作者", "sequence": "9007199254740992", "created_at": "18446744073709551615"
    }});
    value["file_metadata_nodes"] = json_codec::json!({"metadata-node": {
        "node_id": "metadata-node", "history_tree_id": "history", "file_id": "file", "version_id": "version",
        "parent_node_ids": ["parent"], "branch_id": "branch", "kind": "tags", "tags": ["标签", "\u{10000}"],
        "operation_id": "op_00000000000000000000000000000000", "author": "作者", "sequence": "9007199254740992", "created_at": "18446744073709551615"
    }});
    value["file_branch_merges"] = json_codec::json!({"branch-merge": {
        "merge_id": "branch-merge", "history_tree_id": "history", "file_id": "file",
        "input_branch_ids": ["source", "target"], "input_node_ids": ["first", "second"],
        "chosen_node_id": "first", "result_node_id": "result", "surviving_branch_id": "target",
        "reused_node": false, "parent_node_ids": ["first", "second"], "operation_id": "op_00000000000000000000000000000000",
        "sequence": "18446744073709551615"
    }});
    let mut state: State = json_codec::from_value(value).unwrap();
    state
        .file_names
        .insert("\u{10000}".into(), "supplementary".into());
    state.file_names.insert("\u{e000}".into(), "BMP".into());
    state.control_auth =
        Some(json_codec::json!({"\u{10000}": {"roles": ["读者"]}, "\u{e000}": true}));
    let encoded = json_codec::to_value(&state).unwrap();
    assert_eq!(REQUIRED_FIELDS.len() + OPTIONAL_FIELDS.len(), 35);
    for field in REQUIRED_FIELDS.iter().chain(OPTIONAL_FIELDS) {
        assert!(!encoded[*field].as_object().unwrap().is_empty(), "{field}");
    }
    state
}

fn config(root: &Path) -> Config {
    Config::development([root.join("a"), root.join("b"), root.join("c")])
}

fn raw_hash(kind: &str, bytes: &[u8]) -> String {
    let mut hash = blake3::Hasher::new();
    hash.update(format!("zxs:{kind}\0").as_bytes());
    hash.update(bytes);
    hash.finalize().to_hex().to_string()
}

fn assert_complete_byte_oracle(actual: &PreparedSnapshot, state: &State, members: &[DiskConfig]) {
    let reference = prepare_snapshot_reference(state, MAX_METADATA).unwrap();
    let full = prepare_snapshot(state, MAX_METADATA).unwrap();
    assert_eq!(actual.hash, reference.hash);
    assert_eq!(full.hash, reference.hash);
    assert_eq!(full.objects.len(), reference.objects.len());
    for prepared in [actual, &full] {
        for object in &prepared.objects {
            let expected = reference
                .objects
                .iter()
                .find(|entry| entry.kind == object.kind && entry.hash == object.hash)
                .unwrap();
            assert_eq!(object.bytes, expected.bytes);
        }
    }
    assert_eq!(
        canonical_json(&actual.descriptor).unwrap(),
        canonical_json(&reference.descriptor).unwrap()
    );
    assert_eq!(actual.store(members, state).unwrap(), reference.hash);
    // Read raw bytes, not load_object's semantic digest: whitespace and key
    // ordering must be identical to the complete canonical encoder too.
    for object in &reference.objects {
        assert_eq!(raw_hash(object.kind, &object.bytes), object.hash);
        for member in members {
            assert_eq!(
                std::fs::read(object_path(member, object.kind, &object.hash)).unwrap(),
                object.bytes,
                "{}/{} on {}",
                object.kind,
                object.hash,
                member.disk_id
            );
        }
    }
    for member in members {
        // Option<Value>::Some(Null) and None intentionally share wire null.
        assert_eq!(
            canonical_json(&load_raw_snapshot_fixture(member, &actual.hash).unwrap()).unwrap(),
            canonical_json(state).unwrap(),
        );
    }
}

fn assert_explicit_empty_roots(snapshot: &Snapshot) {
    let expected_hash = raw_hash("state-page", b"{}");
    assert_eq!(
        snapshot.maps.len(),
        REQUIRED_FIELDS.len() + OPTIONAL_FIELDS.len()
    );
    for field in REQUIRED_FIELDS.iter().chain(OPTIONAL_FIELDS) {
        let reference = &snapshot.maps[*field];
        assert_eq!(reference.kind(), crate::metadata_map::ReferenceKind::Page);
        assert_eq!(reference.hash(), expected_hash, "{field}");
    }
    assert_eq!(
        snapshot.events.kind(),
        crate::metadata_map::ReferenceKind::Page
    );
    assert_eq!(snapshot.events.hash(), expected_hash);
    let encoded = json_codec::to_value(snapshot).unwrap();
    assert_eq!(encoded["format"], "radix-map-roots");
    assert!(encoded.get("pages").is_none());
    assert!(encoded.get("scalars").is_none());
}

#[test]
fn incremental_oracle_fixture_covers_every_tracked_field_and_current_wire_categories() {
    let state = populated_state();
    let temp = tempfile::tempdir().unwrap();
    let config = config(temp.path());
    let actual = prepare_snapshot(&state, MAX_METADATA).unwrap();
    assert_complete_byte_oracle(&actual, &state, &config.disks);
    let empty_state = State::default();
    let empty = prepare_snapshot(&empty_state, MAX_METADATA).unwrap();
    assert_explicit_empty_roots(&empty.descriptor);
    assert_complete_byte_oracle(&empty, &empty_state, &config.disks);
}

fn baseline(state: &State, members: &[DiskConfig]) -> SnapshotReuse {
    let snapshot = prepare_snapshot(state, MAX_METADATA).unwrap();
    snapshot.store(members, state).unwrap();
    snapshot.reuse_basis()
}

fn incremental(
    before: &State,
    after: &State,
    basis: &SnapshotReuse,
    members: &[DiskConfig],
) -> (PreparedSnapshot, BTreeMap<String, usize>) {
    snapshot::take_encoding_counts();
    let actual = prepare_snapshot_incremental(before, after, basis, MAX_METADATA).unwrap();
    actual.store(members, after).unwrap();
    let counts = snapshot::take_encoding_counts();
    (actual, counts)
}

fn assert_only_field_encoded(counts: &BTreeMap<String, usize>, changed: Option<&str>) {
    for field in REQUIRED_FIELDS.iter().chain(OPTIONAL_FIELDS) {
        let count = counts.get(*field).copied().unwrap_or(0);
        if changed == Some(*field) {
            assert!(
                count > 0,
                "changed field {field} was not serialized: {counts:?}"
            );
        } else {
            assert_eq!(
                count, 0,
                "unchanged field {field} was serialized: {counts:?}"
            );
        }
    }
}

#[test]
fn incremental_snapshot_retains_fixed_publication_root_and_every_raw_object_byte() {
    let before = fixture_state();
    let temp = tempfile::tempdir().unwrap();
    let config = config(temp.path());
    let basis = baseline(&before, &config.disks);
    let mut after = before.clone();
    after.begin_tracking();
    let (actual, counts) = incremental(&before, &after, &basis, &config.disks);
    assert_only_field_encoded(&counts, None);
    let fixture = fixture();
    assert_eq!(actual.hash, fixture["snapshot_hash"].as_str().unwrap());
    let mut addresses = snapshot_addresses(&config.disks[0], &actual.descriptor)
        .into_iter()
        .map(|(kind, hash)| format!("{kind}/{hash}"))
        .collect::<BTreeSet<_>>();
    addresses.insert(format!("snapshot/{}", actual.hash));
    assert_eq!(
        addresses,
        fixture["snapshot_objects"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect()
    );
    for (address, bytes) in fixture["snapshot_objects"].as_object().unwrap() {
        let (kind, hash) = address.split_once('/').unwrap();
        for disk in &config.disks {
            assert_eq!(
                std::fs::read(object_path(disk, kind, hash)).unwrap(),
                bytes.as_str().unwrap().as_bytes()
            );
        }
    }
    assert_complete_byte_oracle(&actual, &after, &config.disks);
}

#[test]
fn incremental_snapshot_unchanged_maps_have_zero_record_serialization_with_all_auth_shapes() {
    let before = populated_state();
    let temp = tempfile::tempdir().unwrap();
    let config = config(temp.path());
    let basis = baseline(&before, &config.disks);
    for auth in [
        None,
        Some(json_codec::json!(null)),
        Some(json_codec::json!({})),
        Some(json_codec::json!({"\u{e000}": true, "\u{10000}": {"nested": ["用户", 3]}})),
        Some(json_codec::json!([{"nested": true}, null, "scalar"])),
        Some(json_codec::json!("not an object")),
        Some(json_codec::json!(false)),
        Some(json_codec::json!(9_007_199_254_740_991u64)),
    ] {
        let mut after = before.clone();
        after.begin_tracking();
        after.sequence = u64::MAX;
        after.business_generation = u64::MAX - 1;
        after.topology_generation = u64::MAX - 2;
        after.control_auth = auth;
        after.events.push(Event {
            sequence: u64::MAX,
            operation_id: "op_00000000000000000000000000000000".parse().unwrap(),
            kind: "MUTATION".into(),
            object_id: "\u{10000}".into(),
        });
        let (actual, counts) = incremental(&before, &after, &basis, &config.disks);
        assert_only_field_encoded(&counts, None);
        assert_complete_byte_oracle(&actual, &after, &config.disks);
    }
}

#[test]
fn incremental_snapshot_each_tracked_field_mutation_reencodes_only_that_field() {
    let before = populated_state();
    let temp = tempfile::tempdir().unwrap();
    let config = config(temp.path());
    let basis = baseline(&before, &config.disks);
    let descriptor = prepare_snapshot(&before, MAX_METADATA).unwrap().descriptor;
    macro_rules! new_key {
        (operations) => {
            "op_ffffffffffffffffffffffffffffffff".parse().unwrap()
        };
        (operation_principals) => {
            "op_ffffffffffffffffffffffffffffffff".parse().unwrap()
        };
        ($other:ident) => {
            "new-字段-\u{10000}".into()
        };
    }
    macro_rules! check_field {
        ($field:ident) => {{
            let mut after = before.clone();
            after.begin_tracking();
            let value = before.$field.iter().next().unwrap().1.clone();
            // Adding an entry changes bytes for every field type; mutating only
            // its tracker would not catch accidental reuse of dirty contents.
            assert!(after.$field.insert(new_key!($field), value).is_none());
            let (actual, counts) = incremental(&before, &after, &basis, &config.disks);
            assert_only_field_encoded(&counts, Some(stringify!($field)));
            assert_ne!(
                actual.descriptor.maps[stringify!($field)],
                descriptor.maps[stringify!($field)]
            );
            for field in REQUIRED_FIELDS.iter().chain(OPTIONAL_FIELDS) {
                if *field != stringify!($field) {
                    assert_eq!(
                        actual.descriptor.maps[*field], descriptor.maps[*field],
                        "{field}"
                    );
                }
            }
            assert_complete_byte_oracle(&actual, &after, &config.disks);
        }};
    }
    every_tracked_field!(check_field);
}

#[test]
fn incremental_snapshot_all_and_foreign_map_replacements_never_claim_empty_deltas() {
    let before = populated_state();
    let temp = tempfile::tempdir().unwrap();
    let config = config(temp.path());
    let basis = baseline(&before, &config.disks);
    macro_rules! check_field {
        ($field:ident) => {{
            for replace in [false, true] {
                let mut after = before.clone();
                after.begin_tracking();
                if replace {
                    after.$field = before
                        .$field
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect();
                } else {
                    // The DerefMut escape hatch conservatively marks All even
                    // when the caller ultimately leaves its values unchanged.
                    let _ = &mut *after.$field;
                }
                assert!(matches!(
                    after.$field.changes_since(&before.$field),
                    MapChanges::All
                ));
                let (actual, counts) = incremental(&before, &after, &basis, &config.disks);
                assert_only_field_encoded(&counts, Some(stringify!($field)));
                assert_eq!(counts[stringify!($field)], after.$field.len());
                assert_complete_byte_oracle(&actual, &after, &config.disks);
            }
        }};
    }
    every_tracked_field!(check_field);
}

#[test]
fn incremental_snapshot_empty_required_optional_removal_and_reintroduction_match_full_schema() {
    let before = populated_state();
    let temp = tempfile::tempdir().unwrap();
    let config = config(temp.path());
    let basis = baseline(&before, &config.disks);
    let mut empty = before.clone();
    empty.begin_tracking();
    macro_rules! clear_field {
        ($field:ident) => {
            empty.$field = Default::default();
        };
    }
    every_tracked_field!(clear_field);
    empty.events.clear();
    empty.control_auth = None;
    let (actual, counts) = incremental(&before, &empty, &basis, &config.disks);
    assert_only_field_encoded(&counts, None); // No values exist to serialize.
    assert_explicit_empty_roots(&actual.descriptor);
    assert_complete_byte_oracle(&actual, &empty, &config.disks);
    let empty_basis = actual.reuse_basis();
    let mut still_empty = empty.clone();
    still_empty.begin_tracking();
    let (actual, counts) = incremental(&empty, &still_empty, &empty_basis, &config.disks);
    assert_only_field_encoded(&counts, None);
    assert_complete_byte_oracle(&actual, &still_empty, &config.disks);
    let (actual, counts) = incremental(&empty, &before, &empty_basis, &config.disks);
    for field in REQUIRED_FIELDS.iter().chain(OPTIONAL_FIELDS) {
        assert!(
            counts.get(*field).copied().unwrap_or(0) > 0,
            "introduced field {field}"
        );
    }
    assert_complete_byte_oracle(&actual, &before, &config.disks);
}

#[test]
fn incremental_snapshot_deserialized_maps_are_cold_and_u64_maps_stay_decimal_strings() {
    let before = populated_state();
    let temp = tempfile::tempdir().unwrap();
    let config = config(temp.path());
    let basis = baseline(&before, &config.disks);
    // Equal values reconstructed through serde do not carry transaction lineage.
    let mut after: State = json_codec::from_slice(&canonical_json(&before).unwrap()).unwrap();
    after.begin_tracking();
    let (actual, counts) = incremental(&before, &after, &basis, &config.disks);
    for field in REQUIRED_FIELDS.iter().chain(OPTIONAL_FIELDS) {
        assert!(
            counts.get(*field).copied().unwrap_or(0) > 0,
            "cold field {field}"
        );
    }
    assert_complete_byte_oracle(&actual, &after, &config.disks);
    for (field, value) in [
        ("directory_selection_generations", u64::MAX),
        ("file_selection_generations", 9_007_199_254_740_992),
    ] {
        let mut seen = Vec::new();
        for hash in map_pages(&config.disks[0], &actual.descriptor.maps[field]) {
            let bytes = std::fs::read(object_path(&config.disks[0], "state-page", &hash)).unwrap();
            let page: BTreeMap<String, String> = json_codec::from_slice(&bytes).unwrap();
            seen.extend(page.into_values());
        }
        assert_eq!(seen, vec![value.to_string()]);
    }
}

#[test]
fn incremental_snapshot_many_radix_nodes_reuse_exact_roots_without_value_serialization() {
    let before = State {
        content_states: (0u32..4096)
            .map(|number| {
                (
                    blake3::hash(&number.to_le_bytes()).to_hex().to_string(),
                    storage_contract::ContentState::Available,
                )
            })
            .collect(),
        file_notes: (0..4096)
            .map(|number| (format!("note-{number:020}"), "文件备注".repeat(4)))
            .collect(),
        ..Default::default()
    };
    let temp = tempfile::tempdir().unwrap();
    let config = config(temp.path());
    let original = prepare_snapshot(&before, MAX_METADATA).unwrap();
    original.store(&config.disks, &before).unwrap();
    for field in ["content_states", "file_notes"] {
        assert!(map_pages(&config.disks[0], &original.descriptor.maps[field]).len() > 32);
    }
    let basis = original.reuse_basis();
    let mut after = before.clone();
    after.begin_tracking();
    after.sequence = 1;
    let (actual, counts) = incremental(&before, &after, &basis, &config.disks);
    assert_only_field_encoded(&counts, None);
    assert_eq!(actual.descriptor.maps, original.descriptor.maps);
    assert_eq!(
        actual.objects.len(),
        1,
        "unchanged maps prepare no dependency objects"
    );
    assert_eq!(
        snapshot_addresses(&config.disks[0], &actual.descriptor),
        snapshot_addresses(&config.disks[0], &original.descriptor)
    );
    assert_complete_byte_oracle(&actual, &after, &config.disks);
}

#[test]
fn incremental_snapshot_baseline_sequence_mismatch_falls_back_to_complete_encoding() {
    let state = populated_state();
    let temp = tempfile::tempdir().unwrap();
    let config = config(temp.path());
    let basis = baseline(&state, &config.disks);
    let mut before = state;
    before.sequence += 1;
    let mut after = before.clone();
    after.begin_tracking();
    let (actual, counts) = incremental(&before, &after, &basis, &config.disks);
    for field in REQUIRED_FIELDS.iter().chain(OPTIONAL_FIELDS) {
        assert!(
            counts.get(*field).copied().unwrap_or(0) > 0,
            "mismatched baseline field {field}"
        );
    }
    assert_complete_byte_oracle(&actual, &after, &config.disks);
}

#[test]
fn incremental_snapshot_missing_special_map_pages_reconstruct_exact_wire_bytes() {
    let mut before = populated_state();
    before
        .file_selection_generations
        .extend((0..512).map(|number| {
            (
                format!("selection-\u{10000}-{number:020}"),
                u64::MAX - number,
            )
        }));
    let temp = tempfile::tempdir().unwrap();
    let config = config(temp.path());
    let original = prepare_snapshot(&before, MAX_METADATA).unwrap();
    original.store(&config.disks, &before).unwrap();
    let basis = original.reuse_basis();
    let missing = [
        "directory_selection_generations",
        "file_selection_generations",
        "file_names",
    ];
    assert!(
        map_pages(
            &config.disks[0],
            &original.descriptor.maps["file_selection_generations"]
        )
        .len()
            > 1
    );
    let addresses = missing
        .iter()
        .flat_map(|field| map_addresses(&config.disks[0], &original.descriptor.maps[*field]))
        .collect::<BTreeSet<_>>();
    for (kind, hash) in addresses {
        for disk in &config.disks {
            std::fs::remove_file(object_path(disk, kind, &hash)).unwrap();
        }
    }
    let mut after = before.clone();
    after.begin_tracking();
    after.sequence += 1;
    let (actual, counts) = incremental(&before, &after, &basis, &config.disks);
    for field in REQUIRED_FIELDS.iter().chain(OPTIONAL_FIELDS) {
        assert_eq!(
            counts.get(*field).copied().unwrap_or(0) > 0,
            missing.contains(field),
            "{field}: {counts:?}"
        );
    }
    assert_eq!(actual.descriptor.maps, original.descriptor.maps);
    assert_complete_byte_oracle(&actual, &after, &config.disks);
}

#[test]
fn incremental_snapshot_middle_and_final_missing_pages_require_the_exact_reconstructed_root() {
    let before = State {
        file_notes: (0..2048)
            .map(|number| (format!("note-{number:020}"), "value".into()))
            .collect(),
        ..Default::default()
    };
    let temp = tempfile::tempdir().unwrap();
    let config = config(temp.path());
    let original = prepare_snapshot(&before, MAX_METADATA).unwrap();
    original.store(&config.disks, &before).unwrap();
    let basis = original.reuse_basis();
    let expected = map_addresses(&config.disks[0], &original.descriptor.maps["file_notes"]);
    let pages = map_pages(&config.disks[0], &original.descriptor.maps["file_notes"]);
    assert!(pages.len() > 16);
    let first: BTreeMap<String, String> =
        load_object(&config.disks[0], "state-page", &pages[0]).unwrap();
    let first_key = first.keys().next().unwrap();
    let original_bytes = expected
        .iter()
        .map(|(kind, hash)| {
            (
                (*kind, hash.clone()),
                std::fs::read(object_path(&config.disks[0], kind, hash)).unwrap(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut after = before.clone();
    after.begin_tracking();
    for missing_index in [pages.len() / 2, pages.len() - 1] {
        after.sequence += 1;
        let missing = &pages[missing_index];
        for disk in &config.disks {
            std::fs::remove_file(object_path(disk, "state-page", missing)).unwrap();
        }
        let actual = prepare_snapshot_incremental(&before, &after, &basis, MAX_METADATA).unwrap();
        // A different value in an already-read radix prefix must invalidate the
        // reconstruction even when the missing object is later in the traversal.
        let mut wrong = after.clone();
        wrong
            .file_notes
            .insert(first_key.clone(), "changed prefix".into());
        snapshot::take_encoding_counts();
        assert_eq!(
            actual.store(&config.disks, &wrong).unwrap_err().code(),
            storage_contract::ErrorCode::ObjectCorrupt
        );
        assert_eq!(
            snapshot::take_encoding_counts(),
            BTreeMap::from([("file_notes".into(), before.file_notes.len())])
        );
        for disk in &config.disks {
            assert!(!object_path(disk, "snapshot", &actual.hash).exists());
            assert!(!object_path(disk, "state-page", missing).exists());
            for ((kind, hash), bytes) in &original_bytes {
                if *kind != "state-page" || hash != missing {
                    assert_eq!(
                        std::fs::read(object_path(disk, kind, hash)).unwrap(),
                        *bytes
                    );
                }
            }
        }
        snapshot::take_encoding_counts();
        actual.store(&config.disks, &after).unwrap();
        assert_eq!(
            snapshot::take_encoding_counts(),
            BTreeMap::from([("file_notes".into(), before.file_notes.len())])
        );
        assert_eq!(
            map_addresses(&config.disks[0], &actual.descriptor.maps["file_notes"]),
            expected
        );
        assert_complete_byte_oracle(&actual, &after, &config.disks);
    }
}

#[test]
fn incremental_snapshot_installation_errors_are_not_retried_as_source_failures() {
    for missing in [false, true] {
        let before = State {
            file_notes: (0..512)
                .map(|number| (format!("note-{number:020}"), "value".into()))
                .collect(),
            ..Default::default()
        };
        let temp = tempfile::tempdir().unwrap();
        let config = config(temp.path());
        let original = prepare_snapshot(&before, MAX_METADATA).unwrap();
        original.store(&config.disks, &before).unwrap();
        let pages = map_pages(&config.disks[0], &original.descriptor.maps["file_notes"]);
        assert!(pages.len() > 2);
        let mut after = before.clone();
        after.begin_tracking();
        after.sequence += 1;
        let actual =
            prepare_snapshot_incremental(&before, &after, &original.reuse_basis(), MAX_METADATA)
                .unwrap();
        if missing {
            for disk in &config.disks {
                std::fs::remove_file(object_path(disk, "state-page", &pages[0])).unwrap();
            }
        }
        let path = object_path(&config.disks[0], "state-page", &pages[1]);
        let mut corrupt = std::fs::read(&path).unwrap();
        corrupt[0] ^= 1;
        std::fs::write(&path, &corrupt).unwrap();
        snapshot::take_encoding_counts();
        assert_eq!(
            actual.store(&config.disks, &after).unwrap_err().code(),
            storage_contract::ErrorCode::ObjectCollision
        );
        let counts = snapshot::take_encoding_counts();
        assert_eq!(
            counts.get("file_notes").copied().unwrap_or(0),
            if missing { before.file_notes.len() } else { 0 },
            "a destination failure must not cause another logical reconstruction"
        );
        assert_eq!(std::fs::read(path).unwrap(), corrupt);
        for disk in &config.disks {
            assert!(!object_path(disk, "snapshot", &actual.hash).exists());
        }
    }
}

#[test]
fn operation_identity_writes_remain_typed_through_snapshot_preparation() {
    let before = populated_state();
    let temp = tempfile::tempdir().unwrap();
    let config = config(temp.path());
    let basis = baseline(&before, &config.disks);
    let mut after = before.clone();
    after.begin_tracking();
    let id = "op_ffffffffffffffffffffffffffffffff".parse().unwrap();
    let mut receipt = before.operations.values().next().unwrap().clone();
    receipt.operation_id = id;
    after.operations.insert(id, receipt);
    after.operation_principals.insert(id, "new owner".into());
    for changes in [
        after.operations.changes_since(&before.operations),
        after
            .operation_principals
            .changes_since(&before.operation_principals),
    ] {
        let MapChanges::Keys(keys) = changes else {
            panic!("normal receipt insertion lost its typed write set");
        };
        assert_eq!(keys.iter().copied().collect::<Vec<_>>(), vec![id]);
    }
    let (actual, counts) = incremental(&before, &after, &basis, &config.disks);
    assert!(counts["operations"] > 0);
    assert!(counts["operation_principals"] > 0);
    assert_complete_byte_oracle(&actual, &after, &config.disks);
}

#[test]
fn incremental_snapshot_typed_point_changes_stay_leaf_bounded_across_radix_transitions() {
    for count in [0, 1, 127, 128, 129, 4096] {
        let before = State {
            file_notes: (0..count)
                .map(|number| (format!("note-{number:020}"), "值/\u{10000}".into()))
                .collect(),
            ..Default::default()
        };
        let temp = tempfile::tempdir().unwrap();
        let config = config(temp.path());
        let original = prepare_snapshot(&before, MAX_METADATA).unwrap();
        original.store(&config.disks, &before).unwrap();
        let basis = original.reuse_basis();
        let mut cases = Vec::new();
        let mut inserted = before.clone();
        inserted.begin_tracking();
        inserted
            .file_notes
            .insert("new-字段-\u{e000}".into(), "new value".into());
        cases.push((inserted, STATE_PAGE_ENTRIES + 1));
        if let Some(key) = before.file_notes.keys().next() {
            let mut changed = before.clone();
            changed.begin_tracking();
            changed.file_notes.insert(key.clone(), "changed".into());
            cases.push((changed, STATE_PAGE_ENTRIES));
            let mut removed = before.clone();
            removed.begin_tracking();
            removed.file_notes.remove(key);
            cases.push((removed, STATE_PAGE_ENTRIES));
        }
        for (mut after, maximum) in cases {
            after.sequence += 1;
            let MapChanges::Keys(keys) = after.file_notes.changes_since(&before.file_notes) else {
                panic!("point operations must retain their typed key set");
            };
            assert_eq!(keys.len(), 1);
            let (actual, counts) = incremental(&before, &after, &basis, &config.disks);
            let encoded = counts.get("file_notes").copied().unwrap_or(0);
            assert!(encoded <= maximum, "{count} records: {counts:?}");
            assert!(encoded <= after.file_notes.len());
            if !after.file_notes.is_empty() {
                assert!(encoded > 0);
            }
            assert_only_field_encoded(
                &counts,
                (!after.file_notes.is_empty()).then_some("file_notes"),
            );
            assert_complete_byte_oracle(&actual, &after, &config.disks);
        }
    }
}

#[test]
fn incremental_snapshot_value_replacement_preserves_every_unaffected_radix_path() {
    let before = State {
        file_notes: (0..4096)
            .map(|number| (format!("note-{number:020}"), "value".into()))
            .collect(),
        ..Default::default()
    };
    let temp = tempfile::tempdir().unwrap();
    let config = config(temp.path());
    let original = prepare_snapshot(&before, MAX_METADATA).unwrap();
    original.store(&config.disks, &before).unwrap();
    let mut after = before.clone();
    after.begin_tracking();
    let key = before.file_notes.keys().next().unwrap();
    after.file_notes.insert(key.clone(), "other".into());
    after.sequence += 1;
    let (actual, counts) = incremental(&before, &after, &original.reuse_basis(), &config.disks);
    assert!(counts["file_notes"] <= STATE_PAGE_ENTRIES);
    let route = blake3::hash(key.as_bytes()).to_hex().to_string();
    let mut paths = BTreeMap::new();
    let snapshot::FieldRoot::Text(actual_notes) = &actual.fields["file_notes"] else {
        panic!("note provenance must retain text keys")
    };
    actual_notes
        .visit_nodes("", &mut |root, prefix| {
            paths.insert(prefix.to_owned(), root.reference().clone());
            Ok(())
        })
        .unwrap();
    let mut reused = 0;
    let snapshot::FieldRoot::Text(original_notes) = &original.fields["file_notes"] else {
        panic!("note provenance must retain text keys")
    };
    original_notes
        .visit_nodes("", &mut |root, prefix| {
            if !route.starts_with(prefix) {
                assert_eq!(paths.get(prefix), Some(root.reference()), "{prefix}");
                assert!(!actual.objects.iter().any(|object| {
                    object.kind == root.reference().kind().domain()
                        && object.hash == root.reference().hash()
                }));
                reused += 1;
            }
            Ok(())
        })
        .unwrap();
    assert!(reused > 32);
    assert_complete_byte_oracle(&actual, &after, &config.disks);
}
