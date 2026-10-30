use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

fn merge_records() -> BTreeMap<String, crate::FileBranchMerge> {
    [("merge-a", 0), ("merge-文件", u64::MAX)]
        .into_iter()
        .map(|(id, sequence)| {
            (
                id.into(),
                crate::FileBranchMerge {
                    merge_id: id.into(),
                    history_tree_id: "history".into(),
                    file_id: "file\n\"文件".into(),
                    input_branch_ids: vec!["left".into(), "right".into()],
                    input_node_ids: vec!["node-left".into(), "node-right".into()],
                    chosen_node_id: "node-left".into(),
                    result_node_id: "node-left".into(),
                    surviving_branch_id: "left".into(),
                    reused_node: true,
                    parent_node_ids: vec!["node-right".into()],
                    operation_id: "op_00000000000000000000000000000001".parse().unwrap(),
                    sequence,
                },
            )
        })
        .collect()
}

#[test]
fn merge_records_preserve_nonempty_map_bytes_hashes_and_sparse_state_wire() {
    let map = merge_records();
    let old: crate::TrackedMap<_> = map.clone().into();
    let records: crate::TrackedRecords<_> = map.clone().into_iter().collect();
    assert_eq!(
        json_codec::to_vec(&old).unwrap(),
        json_codec::to_vec(&records).unwrap()
    );
    assert_eq!(
        crate::canonical_json(&old).unwrap(),
        crate::canonical_json(&records).unwrap()
    );
    assert_eq!(
        crate::object_hash("file-branch-merges", &old).unwrap(),
        crate::object_hash("file-branch-merges", &records).unwrap()
    );
    let state = crate::State {
        file_branch_merges: records,
        ..Default::default()
    };
    let mut old_wire = json_codec::to_value(crate::State::default()).unwrap();
    assert!(old_wire.get("file_branch_merges").is_none());
    old_wire["file_branch_merges"] = json_codec::to_value(&map).unwrap();
    let bytes = crate::canonical_json(&old_wire).unwrap();
    assert_eq!(crate::canonical_json(&state).unwrap(), bytes);
    assert_eq!(
        crate::object_hash("state", &state).unwrap(),
        crate::object_hash("state", &old_wire).unwrap()
    );
    let decoded: crate::State = json_codec::from_slice(&bytes).unwrap();
    assert_eq!(decoded, state);
    assert!(
        !decoded
            .file_branch_merges
            .shares_storage_with(&state.file_branch_merges)
    );
    assert_eq!(
        decoded
            .file_branch_merges
            .changes_since(&state.file_branch_merges),
        crate::MapChanges::All
    );
    old_wire["file_branch_merges"] = json_codec::json!({});
    let empty: crate::State = json_codec::from_value(old_wire).unwrap();
    assert_eq!(empty, crate::State::default());
    assert!(
        json_codec::to_value(empty)
            .unwrap()
            .get("file_branch_merges")
            .is_none()
    );
}

#[test]
fn merge_state_clones_share_payloads_but_keep_exact_writes_and_old_snapshots() {
    let before = crate::State {
        file_branch_merges: merge_records().into_iter().collect(),
        ..Default::default()
    };
    let held = before.file_branch_merges.get_shared("merge-a").unwrap();
    let untouched = before.file_branch_merges.get_shared("merge-文件").unwrap();
    let bytes = crate::canonical_json(&before).unwrap();
    let mut next = before.clone();
    next.begin_tracking();
    assert!(
        next.file_branch_merges
            .shares_storage_with(&before.file_branch_merges)
    );
    assert!(Arc::ptr_eq(
        &held,
        &next.file_branch_merges.get_shared("merge-a").unwrap()
    ));
    assert!(next.file_branch_merges.get_mut("absent").is_none());
    assert!(next.file_branch_merges.remove("absent").is_none());
    assert!(
        next.file_branch_merges
            .changes_since(&before.file_branch_merges)
            .is_empty()
    );
    next.file_branch_merges
        .get_mut("merge-a")
        .unwrap()
        .parent_node_ids
        .push("new-parent".into());
    next.file_branch_merges
        .insert("temporary".into(), (*held).clone());
    next.file_branch_merges.remove("temporary").unwrap();
    next.file_branch_merges
        .extend([("imported".into(), (*held).clone())]);
    let crate::MapChanges::Keys(keys) = next
        .file_branch_merges
        .changes_since(&before.file_branch_merges)
    else {
        panic!("precise merge writes became All")
    };
    assert_eq!(
        keys.iter().map(String::as_str).collect::<Vec<_>>(),
        ["imported", "merge-a", "temporary"]
    );
    assert_eq!(crate::canonical_json(&before).unwrap(), bytes);
    assert_eq!(held.parent_node_ids, ["node-right"]);
    assert!(Arc::ptr_eq(
        &untouched,
        &next.file_branch_merges.get_shared("merge-文件").unwrap()
    ));
    assert!(!Arc::ptr_eq(
        &held,
        &next.file_branch_merges.get_shared("merge-a").unwrap()
    ));
    let mut same = before.clone();
    same.begin_tracking();
    same.file_branch_merges
        .insert("merge-a".into(), (*held).clone());
    let crate::MapChanges::Keys(keys) = same
        .file_branch_merges
        .changes_since(&before.file_branch_merges)
    else {
        panic!("same-value write lost provenance")
    };
    assert_eq!(
        keys.iter().map(String::as_str).collect::<Vec<_>>(),
        ["merge-a"]
    );
    assert_eq!(same, before);
    same.file_branch_merges = merge_records().into_iter().collect();
    assert_eq!(
        same.file_branch_merges
            .changes_since(&before.file_branch_merges),
        crate::MapChanges::All
    );
}

fn path_records() -> BTreeMap<String, crate::FilePathNode> {
    [("path-a", 0), ("path-文件", u64::MAX)]
        .into_iter()
        .map(|(id, sequence)| {
            (
                id.into(),
                crate::FilePathNode {
                    node_id: id.into(),
                    history_tree_id: "history".into(),
                    file_id: "file".into(),
                    version_id: "version".into(),
                    parent_node_ids: vec!["parent-a".into(), "父".into()],
                    branch_id: "branch".into(),
                    path: "目录/新📁.txt".into(),
                    previous_path: "目录/旧.txt".into(),
                    operation_id: "op_00000000000000000000000000000001".parse().unwrap(),
                    author: "作者\n\"\\".into(),
                    sequence,
                    created_at: sequence,
                },
            )
        })
        .collect()
}

#[test]
fn path_records_preserve_exact_map_bytes_hashes_and_sparse_state_roundtrip() {
    const EXPECTED: &str = concat!(
        r#"{"path-a":{"author":"作者\n\"\\","branch_id":"branch","created_at":"0","#,
        r#""file_id":"file","history_tree_id":"history","node_id":"path-a","#,
        r#""operation_id":"op_00000000000000000000000000000001","#,
        r#""parent_node_ids":["parent-a","父"],"path":"目录/新📁.txt","#,
        r#""previous_path":"目录/旧.txt","sequence":"0","version_id":"version"},"#,
        r#""path-文件":{"author":"作者\n\"\\","branch_id":"branch","#,
        r#""created_at":"18446744073709551615","file_id":"file","#,
        r#""history_tree_id":"history","node_id":"path-文件","#,
        r#""operation_id":"op_00000000000000000000000000000001","#,
        r#""parent_node_ids":["parent-a","父"],"path":"目录/新📁.txt","#,
        r#""previous_path":"目录/旧.txt","sequence":"18446744073709551615","#,
        r#""version_id":"version"}}"#,
    );
    let map = path_records();
    let old: crate::TrackedMap<_> = map.clone().into();
    let records: crate::TrackedRecords<_> = map.clone().into_iter().collect();
    assert_eq!(
        json_codec::to_vec(&old).unwrap(),
        json_codec::to_vec(&records).unwrap()
    );
    let (old_bytes, old_hash) = crate::encode_hashed_object("state-page", &old).unwrap();
    let (new_bytes, new_hash) = crate::encode_hashed_object("state-page", &records).unwrap();
    assert_eq!(old_bytes, EXPECTED.as_bytes());
    assert_eq!(new_bytes, old_bytes);
    assert_eq!(new_hash, old_hash);
    let mut digest = blake3::Hasher::new();
    digest.update(b"zxs:state-page\0");
    digest.update(EXPECTED.as_bytes());
    assert_eq!(old_hash, digest.finalize().to_hex().as_str());

    let state = crate::State {
        file_path_nodes: records,
        ..Default::default()
    };
    let mut old_wire = json_codec::to_value(crate::State::default()).unwrap();
    assert!(old_wire.get("file_path_nodes").is_none());
    old_wire["file_path_nodes"] = json_codec::to_value(&map).unwrap();
    let (old_state_bytes, old_state_hash) =
        crate::encode_hashed_object("state", &old_wire).unwrap();
    let (new_state_bytes, new_state_hash) = crate::encode_hashed_object("state", &state).unwrap();
    assert_eq!(new_state_bytes, old_state_bytes);
    assert_eq!(new_state_hash, old_state_hash);
    println!("file_path_nodes state-page hash before/after: {old_hash}");
    println!("file_path_nodes State hash before/after: {old_state_hash}");
    let decoded: crate::State = json_codec::from_slice(&old_state_bytes).unwrap();
    assert_eq!(decoded, state);
    assert!(
        !decoded
            .file_path_nodes
            .shares_storage_with(&state.file_path_nodes)
    );
    assert_eq!(
        decoded
            .file_path_nodes
            .changes_since(&state.file_path_nodes),
        crate::MapChanges::All
    );
    old_wire["file_path_nodes"] = json_codec::json!({});
    let empty: crate::State = json_codec::from_value(old_wire).unwrap();
    assert_eq!(empty, crate::State::default());
    assert!(
        json_codec::to_value(empty)
            .unwrap()
            .get("file_path_nodes")
            .is_none()
    );
}

#[test]
fn path_state_clones_share_unchanged_records_and_track_isolated_writes() {
    let before = crate::State {
        file_path_nodes: path_records().into_iter().collect(),
        ..Default::default()
    };
    let held = before.file_path_nodes.get_shared("path-a").unwrap();
    let untouched = before.file_path_nodes.get_shared("path-文件").unwrap();
    let bytes = crate::canonical_json(&before).unwrap();
    let mut next = before.clone();
    next.begin_tracking();
    assert!(
        next.file_path_nodes
            .shares_storage_with(&before.file_path_nodes)
    );
    assert!(Arc::ptr_eq(
        &held,
        &next.file_path_nodes.get_shared("path-a").unwrap()
    ));
    assert!(next.file_path_nodes.get_mut("absent").is_none());
    assert!(next.file_path_nodes.remove("absent").is_none());
    assert!(!next.file_path_nodes.remove_discard("absent"));
    assert!(
        next.file_path_nodes
            .changes_since(&before.file_path_nodes)
            .is_empty()
    );
    assert!(
        next.file_path_nodes
            .shares_storage_with(&before.file_path_nodes)
    );

    next.file_path_nodes
        .get_mut("path-a")
        .unwrap()
        .parent_node_ids
        .push("new-parent".into());
    assert!(!Arc::ptr_eq(
        &held,
        &next.file_path_nodes.get_shared("path-a").unwrap()
    ));
    assert!(Arc::ptr_eq(
        &untouched,
        &next.file_path_nodes.get_shared("path-文件").unwrap()
    ));
    next.file_path_nodes
        .insert("temporary".into(), (*held).clone());
    assert_eq!(next.file_path_nodes.remove("temporary").unwrap(), *held);
    // History import uses Extend over owned records from its BTreeMap.
    next.file_path_nodes
        .extend([("imported".into(), (*held).clone())]);
    let crate::MapChanges::Keys(keys) = next.file_path_nodes.changes_since(&before.file_path_nodes)
    else {
        panic!("precise path record writes became All")
    };
    assert_eq!(
        keys.iter().map(String::as_str).collect::<Vec<_>>(),
        ["imported", "path-a", "temporary"]
    );
    assert_eq!(crate::canonical_json(&before).unwrap(), bytes);
    assert_eq!(held.parent_node_ids, ["parent-a", "父"]);

    let mut sibling = before.clone();
    sibling.begin_tracking();
    sibling
        .file_path_nodes
        .get_mut("path-a")
        .unwrap()
        .path
        .push_str(".sibling");
    assert_eq!(next.file_path_nodes["path-a"].path, held.path);
    assert_eq!(before.file_path_nodes["path-a"].path, held.path);
    drop(next);
    assert_eq!(crate::canonical_json(&before).unwrap(), bytes);

    let removed = sibling.file_path_nodes.remove("path-a").unwrap();
    assert!(removed.path.ends_with(".sibling"));
    assert!(sibling.file_path_nodes.remove_discard("path-文件"));
    let crate::MapChanges::Keys(keys) = sibling
        .file_path_nodes
        .changes_since(&before.file_path_nodes)
    else {
        panic!("path record removals lost their write set")
    };
    assert_eq!(
        keys.iter().map(String::as_str).collect::<Vec<_>>(),
        ["path-a", "path-文件"]
    );
    assert!(sibling.file_path_nodes.is_empty());
    assert_eq!(crate::canonical_json(&before).unwrap(), bytes);
    assert_eq!(untouched.parent_node_ids, ["parent-a", "父"]);
}

#[derive(Debug)]
struct Counted {
    value: String,
    clones: Arc<AtomicUsize>,
}
impl Clone for Counted {
    fn clone(&self) -> Self {
        self.clones.fetch_add(1, Ordering::SeqCst);
        Self {
            value: self.value.clone(),
            clones: Arc::clone(&self.clones),
        }
    }
}
fn counted(value: &str, clones: &Arc<AtomicUsize>) -> Counted {
    Counted {
        value: value.into(),
        clones: Arc::clone(clones),
    }
}
fn clone_count(clones: &Arc<AtomicUsize>) -> usize {
    clones.load(Ordering::SeqCst)
}

#[test]
fn cloning_and_reads_do_not_clone_payloads_and_writes_clone_only_touched_records() {
    let clones = Arc::new(AtomicUsize::new(0));
    let mut base: SharedRecords<_> = (0..128)
        .map(|n| (format!("key-{n:03}"), counted("original", &clones)))
        .collect();
    let frozen = base.clone();
    let mut candidate = base.clone();
    assert_eq!(candidate.len(), 128);
    assert!(candidate.contains_key("key-000"));
    assert_eq!(candidate.get("key-127").unwrap().value, "original");
    let (key, record) = candidate.get_key_value("key-127").unwrap();
    assert_eq!(key, "key-127");
    assert!(std::ptr::eq(record, candidate.get("key-127").unwrap()));
    assert!(candidate.get_key_value("missing").is_none());
    assert_eq!(candidate.iter().count(), 128);
    assert_eq!(candidate.values().count(), 128);
    assert_eq!(clone_count(&clones), 0);
    candidate
        .get_mut("key-000")
        .unwrap()
        .value
        .push_str(" candidate");
    assert_eq!(clone_count(&clones), 1);
    candidate.get_mut("key-000").unwrap().value.push('!');
    assert_eq!(clone_count(&clones), 1);
    base.get_mut("key-001").unwrap().value.push_str(" base");
    assert_eq!(clone_count(&clones), 2);
    assert_eq!(base["key-000"].value, "original");
    assert_eq!(candidate["key-001"].value, "original");
    assert!(frozen.values().all(|value| value.value == "original"));
    assert!(candidate.get_mut("absent").is_none());
    assert_eq!(clone_count(&clones), 2);
    drop(candidate); // Abandoning a private candidate must not affect either base.
    assert_eq!(base["key-001"].value, "original base");
    assert!(frozen.values().all(|value| value.value == "original"));
}

#[test]
fn insert_extend_and_discard_do_not_clone_unused_payloads() {
    let clones = Arc::new(AtomicUsize::new(0));
    let base: SharedRecords<_> = [
        ("replace".into(), counted("old", &clones)),
        ("discard".into(), counted("old", &clones)),
    ]
    .into_iter()
    .collect();
    let mut candidate = base.clone();
    assert!(
        candidate
            .insert("new".into(), counted("new", &clones))
            .is_none()
    );
    candidate.extend([
        ("replace".into(), counted("replacement", &clones)),
        ("added".into(), counted("added", &clones)),
    ]);
    assert!(candidate.remove_discard("discard"));
    assert!(!candidate.remove_discard("missing"));
    assert_eq!(clone_count(&clones), 0);
    assert_eq!(base.len(), 2);
    assert_eq!(base["replace"].value, "old");
    assert!(base.contains_key("discard"));
    assert_eq!(candidate["replace"].value, "replacement");
    assert_eq!(candidate.len(), 3);
}

#[test]
fn owned_removal_and_replacement_clone_only_if_returned_payload_is_shared() {
    let clones = Arc::new(AtomicUsize::new(0));
    let base: SharedRecords<_> = [
        ("remove".into(), counted("removed", &clones)),
        ("replace".into(), counted("replaced", &clones)),
    ]
    .into_iter()
    .collect();
    let mut candidate = base.clone();
    let mut removed = candidate.remove("remove").unwrap();
    assert_eq!(clone_count(&clones), 1);
    removed.value.push('!');
    assert_eq!(base["remove"].value, "removed");
    let replaced = candidate
        .insert("replace".into(), counted("new", &clones))
        .unwrap();
    assert_eq!(replaced.value, "replaced");
    assert_eq!(clone_count(&clones), 2);
    assert_eq!(base["replace"].value, "replaced");
    assert!(candidate.remove("missing").is_none());
    // The replacement is uniquely owned and is moved out, not cloned.
    assert_eq!(candidate.remove("replace").unwrap().value, "new");
    assert_eq!(clone_count(&clones), 2);
    drop(candidate);
    let mut unique = base;
    assert_eq!(unique.remove("remove").unwrap().value, "removed");
    assert_eq!(
        unique
            .insert("replace".into(), counted("last", &clones))
            .unwrap()
            .value,
        "replaced"
    );
    assert_eq!(clone_count(&clones), 2);
}

#[test]
fn non_clone_payloads_support_sharing_reads_extension_and_discard() {
    #[derive(Debug, PartialEq)]
    struct NonClone(u32);
    let mut records: SharedRecords<_> = [("a".into(), NonClone(1))].into_iter().collect();
    let frozen = records.clone();
    records.extend([("a".into(), NonClone(2)), ("b".into(), NonClone(3))]);
    assert_eq!(records["a"], NonClone(2));
    assert_eq!(frozen["a"], NonClone(1));
    assert!(records.remove_discard("b"));
}

#[test]
fn ordered_reads_and_wire_bytes_match_btree_maps_for_empty_and_unicode_records() {
    for map in [
        BTreeMap::<String, String>::new(),
        BTreeMap::from([
            ("中文/📁".into(), "含有\n换行".into()),
            ("z".into(), "quotes: \" and slash: \\".into()),
            ("a".into(), "".into()),
            ("é".into(), "e\u{301}".into()),
        ]),
    ] {
        let records: SharedRecords<_> = map.clone().into_iter().collect();
        assert_eq!(records.len(), map.len());
        assert_eq!(records.is_empty(), map.is_empty());
        assert_eq!(
            records.keys().collect::<Vec<_>>(),
            map.keys().collect::<Vec<_>>()
        );
        assert_eq!(
            records.iter().collect::<Vec<_>>(),
            map.iter().collect::<Vec<_>>()
        );
        assert_eq!(
            records.iter().rev().collect::<Vec<_>>(),
            map.iter().rev().collect::<Vec<_>>()
        );
        assert_eq!(
            records.values().rev().collect::<Vec<_>>(),
            map.values().rev().collect::<Vec<_>>()
        );
        assert_eq!(records.iter().len(), map.len());
        assert_eq!(records.values().len(), map.len());
        let bytes = json_codec::to_vec(&map).unwrap();
        assert_eq!(json_codec::to_vec(&records).unwrap(), bytes);
        assert_eq!(
            crate::canonical_json(&records).unwrap(),
            crate::canonical_json(&map).unwrap()
        );
        assert_eq!(
            crate::object_hash("records", &records).unwrap(),
            crate::object_hash("records", &map).unwrap()
        );
        let decoded: SharedRecords<String> = json_codec::from_slice(&bytes).unwrap();
        assert_eq!(decoded, records);
        assert_eq!(
            json_codec::from_slice::<BTreeMap<String, String>>(
                &json_codec::to_vec(&decoded).unwrap()
            )
            .unwrap(),
            map
        );
    }
    assert!(SharedRecords::<String>::new().is_empty());
    assert!(SharedRecords::<String>::default().is_empty());
}

#[test]
fn deserialization_supports_borrowed_non_clone_payloads() {
    #[derive(Debug, PartialEq, Deserialize)]
    struct Borrowed<'a> {
        text: &'a str,
    }
    let bytes = br#"{"record":{"text":"borrowed payload"}}"#.to_vec();
    let records: SharedRecords<Borrowed<'_>> = json_codec::from_slice(&bytes).unwrap();
    assert_eq!(records["record"].text, "borrowed payload");
    let start = bytes.as_ptr() as usize;
    let borrowed = records["record"].text.as_ptr() as usize;
    assert!((start..start + bytes.len()).contains(&borrowed));
    let frozen = records.clone();
    assert_eq!(records, frozen);
}

#[test]
fn state_preserves_required_record_maps_and_sparse_directory_tree_wire_semantics() {
    let state = crate::State::default();
    let value = json_codec::to_value(&state).unwrap();
    assert_eq!(value.get("commits"), Some(&json_codec::json!({})));
    assert_eq!(value.get("versions"), Some(&json_codec::json!({})));
    assert!(value.get("directory_trees").is_none());
    let decoded: crate::State = json_codec::from_value(value.clone()).unwrap();
    assert!(decoded.commits.is_empty());
    assert!(decoded.versions.is_empty());
    assert!(decoded.directory_trees.is_empty());

    let mut explicit_empty = value.clone();
    explicit_empty["directory_trees"] = json_codec::json!({});
    assert_eq!(
        json_codec::from_value::<crate::State>(explicit_empty).unwrap(),
        state
    );
    for field in ["commits", "versions"] {
        let mut missing = value.clone();
        assert!(missing.as_object_mut().unwrap().remove(field).is_some());
        assert!(json_codec::from_value::<crate::State>(missing).is_err());
    }
}

#[test]
fn file_version_records_preserve_map_bytes_hashes_and_state_roundtrip() {
    let map: BTreeMap<_, _> = [("version-文件", 0), ("version-z", u64::MAX)]
        .into_iter()
        .map(|(id, sequence)| {
            (
                id.to_owned(),
                crate::FileVersion {
                    history_tree_id: "history".into(),
                    version_id: id.into(),
                    file_id: "file".into(),
                    creation_primary_slot_id: "P01".into(),
                    logical_size: u64::MAX,
                    version_sequence: sequence,
                    content_hash: "a".repeat(64).parse().unwrap(),
                    checksum_chunk_bytes: 4096,
                    checksum_manifest_hash: "b".repeat(64),
                    creation_name: "文件\n\".txt".into(),
                    storage_name: "stored.txt".into(),
                    created_at: "2026-10-25T00:00:00Z".into(),
                    source_version_ids: vec!["source-a".into(), "来源".into()],
                    created_as_full_checkpoint: true,
                },
            )
        })
        .collect();
    let bytes = json_codec::to_vec(&map).unwrap();
    let records: SharedRecords<crate::FileVersion> = json_codec::from_slice(&bytes).unwrap();
    assert_eq!(json_codec::to_vec(&records).unwrap(), bytes);
    assert_eq!(
        crate::canonical_json(&records).unwrap(),
        crate::canonical_json(&map).unwrap()
    );
    assert_eq!(
        crate::object_hash("versions", &records).unwrap(),
        crate::object_hash("versions", &map).unwrap()
    );
    let state = crate::State {
        versions: records.into(),
        ..Default::default()
    };
    let mut old_state = json_codec::to_value(crate::State::default()).unwrap();
    old_state["versions"] = json_codec::to_value(&map).unwrap();
    assert_eq!(
        crate::canonical_json(&state).unwrap(),
        crate::canonical_json(&old_state).unwrap()
    );
    let decoded: crate::State =
        json_codec::from_slice(&crate::canonical_json(&old_state).unwrap()).unwrap();
    assert_eq!(decoded, state);
    assert!(!decoded.versions.shares_storage_with(&state.versions));
}

#[test]
fn storage_identity_is_one_way_and_mutations_detach_shared_maps() {
    let original: SharedRecords<_> = [("a".into(), "original".to_owned())].into_iter().collect();
    let mut candidate = original.clone();
    assert!(candidate.shares_storage_with(&original));
    let equal: SharedRecords<_> = original
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    assert_eq!(candidate, equal);
    assert!(!candidate.shares_storage_with(&equal));
    // Even mutable access without a value change must stop claiming sameness.
    assert_eq!(candidate.get_mut("a").unwrap(), "original");
    assert_eq!(candidate, original);
    assert!(!candidate.shares_storage_with(&original));
    candidate.get_mut("a").unwrap().push_str(" changed");
    assert_eq!(original["a"], "original");
    let mut inserted = original.clone();
    inserted.insert("b".into(), "new".into());
    assert!(!inserted.shares_storage_with(&original));
    assert!(inserted.remove_discard("b"));
    assert_eq!(inserted, original);
    assert!(!inserted.shares_storage_with(&original));
}

#[test]
fn no_op_mutations_preserve_shared_map_identity() {
    let clones = Arc::new(AtomicUsize::new(0));
    let base: SharedRecords<_> = [("present".into(), counted("original", &clones))]
        .into_iter()
        .collect();
    let mut candidate = base.clone();
    assert!(candidate.shares_storage_with(&base));
    assert!(candidate.get_mut("missing").is_none());
    assert!(candidate.shares_storage_with(&base));
    assert!(candidate.remove("missing").is_none());
    assert!(candidate.shares_storage_with(&base));
    assert!(!candidate.remove_discard("missing"));
    assert!(candidate.shares_storage_with(&base));
    candidate.extend(std::iter::empty());
    assert!(candidate.shares_storage_with(&base));
    assert_eq!(clone_count(&clones), 0);
}

#[test]
fn subsets_and_shared_extension_clone_no_payloads_and_isolate_writes() {
    let clones = Arc::new(AtomicUsize::new(0));
    let mut source: SharedRecords<_> = [
        ("a".into(), counted("a", &clones)),
        ("b".into(), counted("b", &clones)),
        ("c".into(), counted("c", &clones)),
    ]
    .into_iter()
    .collect();
    let mut subset = source.subset(|key, _| key != "c");
    assert_eq!(
        subset.keys().map(String::as_str).collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert!(std::ptr::eq(&source["a"], &subset["a"]));
    let mut destination = source.clone();
    destination.extend_shared(&subset); // Overwrites shared values without cloning them.
    assert_eq!(clone_count(&clones), 0);
    let frozen = destination.clone();
    source.get_mut("a").unwrap().value.push('!');
    subset.get_mut("b").unwrap().value.push('?');
    destination.get_mut("c").unwrap().value.push('.');
    assert_eq!(clone_count(&clones), 3);
    assert_eq!(subset["a"].value, "a");
    assert_eq!(source["b"].value, "b");
    assert_eq!(frozen["c"].value, "c");
    drop(destination);
    assert_eq!(frozen["a"].value, "a");
    let empty = source.subset(|_, _| false);
    assert!(empty.is_empty());
    let mut no_op = frozen.clone();
    no_op.extend_shared(&empty);
    assert!(Arc::ptr_eq(&no_op.records, &frozen.records));
    assert_eq!(clone_count(&clones), 3);
}

#[test]
fn shared_projection_and_merge_support_non_clone_payloads_and_replace_values() {
    struct NonClone(u32);
    let source: SharedRecords<_> = [("a".into(), NonClone(1)), ("b".into(), NonClone(2))]
        .into_iter()
        .collect();
    let selected = source.subset(|_, value| value.0 == 2);
    let mut destination: SharedRecords<_> = [("b".into(), NonClone(3)), ("c".into(), NonClone(4))]
        .into_iter()
        .collect();
    let previous = destination.clone();
    destination.extend_shared(&selected);
    assert_eq!(destination["b"].0, 2);
    assert_eq!(destination["c"].0, 4);
    assert_eq!(previous["b"].0, 3);
    assert!(std::ptr::eq(&destination["b"], &source["b"]));
}

#[test]
fn shared_record_handles_borrow_payloads_and_isolate_writes_in_both_directions() {
    let clones = Arc::new(AtomicUsize::new(0));
    let mut records: SharedRecords<_> = [
        ("map-write".into(), counted("map original", &clones)),
        ("handle-write".into(), counted("handle original", &clones)),
    ]
    .into_iter()
    .collect();
    let from_map = records.get_shared("map-write").unwrap();
    let mut from_handle = records.get_shared("handle-write").unwrap();
    assert!(std::ptr::eq(from_map.as_ref(), &records["map-write"]));
    assert!(std::ptr::eq(from_handle.as_ref(), &records["handle-write"]));
    assert!(records.get_shared("missing").is_none());
    assert_eq!(clone_count(&clones), 0);
    records.get_mut("map-write").unwrap().value.push('!');
    assert_eq!(from_map.value, "map original");
    assert_eq!(records["map-write"].value, "map original!");
    assert_eq!(clone_count(&clones), 1);
    Arc::make_mut(&mut from_handle).value.push('?');
    assert_eq!(records["handle-write"].value, "handle original");
    assert_eq!(from_handle.value, "handle original?");
    assert_eq!(clone_count(&clones), 2);
    drop(records);
    assert_eq!(from_map.value, "map original");
    assert_eq!(from_handle.value, "handle original?");
    assert_eq!(clone_count(&clones), 2);
}

#[test]
fn shared_record_handle_supports_non_clone_values_and_outlives_the_map() {
    struct NonClone(String);
    let records: SharedRecords<_> = [("key".into(), NonClone("retained".into()))]
        .into_iter()
        .collect();
    let first = records.get_shared("key").unwrap();
    let second = records.get_shared("key").unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    assert!(std::ptr::eq(first.as_ref(), &records["key"]));
    drop(records);
    drop(first);
    assert_eq!(second.0, "retained");
}

#[test]
fn shared_value_iteration_filters_before_cloning_and_retains_cow_isolation() {
    let clones = Arc::new(AtomicUsize::new(0));
    let mut records: SharedRecords<_> = [
        ("a".into(), counted("selected", &clones)),
        ("b".into(), counted("excluded", &clones)),
    ]
    .into_iter()
    .collect();
    let selected = records
        .shared_values()
        .filter(|value| value.value == "selected")
        .map(Arc::clone)
        .collect::<Vec<_>>();
    assert_eq!(records.shared_values().len(), 2);
    assert_eq!(
        records.shared_values().next_back().unwrap().value,
        "excluded"
    );
    assert!(std::ptr::eq(selected[0].as_ref(), &records["a"]));
    assert_eq!(
        Arc::strong_count(records.shared_values().next_back().unwrap()),
        1
    );
    assert_eq!(clone_count(&clones), 0);
    records.get_mut("a").unwrap().value.push('!');
    assert_eq!(clone_count(&clones), 1);
    assert_eq!(selected[0].value, "selected");
    drop(records);
    assert_eq!(selected[0].value, "selected");
}

#[test]
fn shared_value_iteration_accepts_non_clone_payloads() {
    struct NonClone(u64);
    let records: SharedRecords<_> = [("b".into(), NonClone(2)), ("a".into(), NonClone(1))]
        .into_iter()
        .collect();
    let handles = records.shared_values().map(Arc::clone).collect::<Vec<_>>();
    drop(records);
    assert_eq!(
        handles.iter().map(|value| value.0).collect::<Vec<_>>(),
        [1, 2]
    );
}

#[test]
fn subset_calls_predicate_once_in_order_for_every_selection() {
    let source: SharedRecords<_> = ["a", "b", "c", "d"]
        .into_iter()
        .enumerate()
        .map(|(value, key)| (key.to_owned(), value))
        .collect();
    for mask in 0u8..16 {
        let mut visits = Vec::new();
        let selected = source.subset(|key, value| {
            visits.push((key.to_owned(), *value));
            mask & (1 << *value) != 0
        });
        assert_eq!(
            visits,
            source
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect::<Vec<_>>()
        );
        let expected: BTreeMap<_, _> = source
            .iter()
            .filter(|(_, value)| mask & (1 << **value) != 0)
            .map(|(key, value)| (key.clone(), *value))
            .collect();
        assert_eq!(
            selected
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect::<BTreeMap<_, _>>(),
            expected
        );
        assert_eq!(selected.shares_storage_with(&source), mask == 15);
    }
}

#[test]
fn full_subset_shares_storage_and_isolates_writes_in_both_directions() {
    let clones = Arc::new(AtomicUsize::new(0));
    let mut source: SharedRecords<_> = [
        ("a".into(), counted("a", &clones)),
        ("b".into(), counted("b", &clones)),
    ]
    .into_iter()
    .collect();
    let mut selected = source.subset(|_, _| true);
    assert!(selected.shares_storage_with(&source));
    assert_eq!(clone_count(&clones), 0);
    source.get_mut("a").unwrap().value.push('!');
    selected.get_mut("b").unwrap().value.push('?');
    assert!(!selected.shares_storage_with(&source));
    assert_eq!(source["a"].value, "a!");
    assert_eq!(selected["a"].value, "a");
    assert_eq!(source["b"].value, "b");
    assert_eq!(selected["b"].value, "b?");
    assert_eq!(clone_count(&clones), 2);
}

#[test]
fn empty_subset_does_not_call_predicate_and_shares_empty_storage() {
    let source = SharedRecords::<String>::new();
    let selected = source.subset(|_, _| panic!("empty source has no records"));
    assert!(selected.is_empty());
    assert!(selected.shares_storage_with(&source));
}

#[test]
fn full_subset_supports_non_clone_and_preserves_interior_mutability_contract() {
    use std::cell::Cell;
    struct NonClone(Cell<u32>);
    let source: SharedRecords<_> = [("a".into(), NonClone(Cell::new(1)))].into_iter().collect();
    let selected = source.subset(|_, value| {
        value.0.set(2);
        true
    });
    assert!(selected.shares_storage_with(&source));
    selected["a"].0.set(3);
    assert_eq!(source["a"].0.get(), 3);
}
