use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

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
fn state_preserves_required_commits_and_sparse_directory_tree_wire_semantics() {
    let state = crate::State::default();
    let value = json_codec::to_value(&state).unwrap();
    assert_eq!(value.get("commits"), Some(&json_codec::json!({})));
    assert!(value.get("directory_trees").is_none());
    let decoded: crate::State = json_codec::from_value(value.clone()).unwrap();
    assert!(decoded.commits.is_empty());
    assert!(decoded.directory_trees.is_empty());

    let mut explicit_empty = value.clone();
    explicit_empty["directory_trees"] = json_codec::json!({});
    assert_eq!(
        json_codec::from_value::<crate::State>(explicit_empty).unwrap(),
        state
    );
    let mut missing_commits = value;
    assert!(
        missing_commits
            .as_object_mut()
            .unwrap()
            .remove("commits")
            .is_some()
    );
    assert!(json_codec::from_value::<crate::State>(missing_commits).is_err());
}

#[test]
fn no_op_mutations_preserve_shared_map_identity() {
    let clones = Arc::new(AtomicUsize::new(0));
    let base: SharedRecords<_> = [("present".into(), counted("original", &clones))]
        .into_iter()
        .collect();
    let mut candidate = base.clone();
    assert!(Arc::ptr_eq(&candidate.records, &base.records));
    assert!(candidate.get_mut("missing").is_none());
    assert!(Arc::ptr_eq(&candidate.records, &base.records));
    assert!(candidate.remove("missing").is_none());
    assert!(Arc::ptr_eq(&candidate.records, &base.records));
    assert!(!candidate.remove_discard("missing"));
    assert!(Arc::ptr_eq(&candidate.records, &base.records));
    candidate.extend(std::iter::empty());
    assert!(Arc::ptr_eq(&candidate.records, &base.records));
    assert_eq!(clone_count(&clones), 0);
}
