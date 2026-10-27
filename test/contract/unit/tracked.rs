use super::*;
fn base() -> TrackedMap<String> {
    [("a".into(), "first".into()), ("b".into(), "second".into())].into()
}
fn keys(changes: MapChanges) -> Vec<String> {
    match changes {
        MapChanges::Keys(keys) => keys.iter().cloned().collect(),
        MapChanges::All => panic!("unexpected whole-map invalidation"),
    }
}
#[test]
fn tracked_writes_clone_and_noop_access_are_conservative() {
    let before = base();
    let mut next = before.clone();
    next.begin_tracking();
    assert!(next.changes_since(&before).is_empty());
    assert!(next.get_mut("missing").is_none());
    assert!(next.remove("missing").is_none());
    assert!(next.changes_since(&before).is_empty());
    next.get_mut("a").unwrap().push('!');
    let mut branch = next.clone();
    branch.insert("c".into(), "third".into());
    assert_eq!(keys(next.changes_since(&before)), vec!["a"]);
    assert_eq!(keys(branch.changes_since(&before)), vec!["a", "c"]);
    assert_eq!(before["a"], "first");
    next.entry("b".into()).or_insert("unused".into());
    assert_eq!(keys(next.changes_since(&before)), vec!["a", "b"]);
}
#[test]
fn replacements_resets_and_deserialization_never_claim_old_lineage() {
    let before = base();
    let mut next = before.clone();
    next.begin_tracking();
    next.insert("a".into(), "changed".into());
    next.begin_tracking();
    assert_eq!(next.changes_since(&before), MapChanges::All);
    let replacement = base();
    assert_eq!(replacement.changes_since(&before), MapChanges::All);
    let encoded = crate::canonical_json(&before).unwrap();
    let decoded: TrackedMap<String> = json_codec::from_slice(&encoded).unwrap();
    assert_eq!(decoded.changes_since(&before), MapChanges::All);
    assert_eq!(encoded, crate::canonical_json(&*before).unwrap());
    assert_eq!(encoded, br#"{"a":"first","b":"second"}"#);
}
#[test]
fn mutable_escape_and_overflow_invalidate_whole_map() {
    let before = base();
    let mut next = before.clone();
    next.begin_tracking();
    let escaped: &mut BTreeMap<String, String> = &mut next;
    escaped.clear();
    assert_eq!(next.changes_since(&before), MapChanges::All);
    let mut next = before.clone();
    next.begin_tracking();
    for n in 0..=MAX_CHANGED_KEYS {
        next.insert(format!("{n}"), String::new());
    }
    assert_eq!(next.changes_since(&before), MapChanges::All);
}
#[test]
fn shared_record_tracking_preserves_value_isolation_and_wire_bytes() {
    let before: TrackedRecords<String> = [("a".into(), "first".into())].into_iter().collect();
    let mut next = before.clone();
    next.begin_tracking();
    next.get_mut("a").unwrap().push('!');
    assert_eq!(keys(next.changes_since(&before)), vec!["a"]);
    assert_eq!(before["a"], "first");
    assert_eq!(
        crate::canonical_json(&before).unwrap(),
        crate::canonical_json(&*before).unwrap()
    );
    assert!(next.remove_discard("a"));
    assert!(!next.remove_discard("missing"));
}

#[test]
fn oversized_keys_and_bulk_mutation_have_bounded_tracking() {
    let before = base();
    let mut next = before.clone();
    next.begin_tracking();
    next.insert("x".repeat(MAX_CHANGED_KEY_BYTES + 1), String::new());
    assert_eq!(next.changes_since(&before), MapChanges::All);
    let mut next = before.clone();
    next.begin_tracking();
    next.retain(|_, _| true);
    assert_eq!(next.changes_since(&before), MapChanges::All);
    let mut next = before.clone();
    next.begin_tracking();
    for (_, value) in &mut next {
        value.push('!');
    }
    assert_eq!(next.changes_since(&before), MapChanges::All);
}

#[test]
fn operation_map_insert_tracks_change_without_string_keys() {
    let id = crate::OperationId::generate();
    let before = TrackedOperations::new(BTreeMap::from([(id, "owner".to_owned())]));
    let mut next = before.clone();
    next.begin_tracking();
    assert!(next.changes_since(&before).is_empty());
    assert_eq!(
        next.insert(id, "replacement".to_owned()),
        Some("owner".to_owned())
    );
    assert_eq!(
        next.changes_since(&before),
        MapChanges::Keys(Arc::new(BTreeSet::from([id])))
    );
    assert_eq!(before.get(&id).unwrap(), "owner");
    assert_eq!(next.get(&id).unwrap(), "replacement");
    let expected = BTreeMap::from([(id.to_string(), "replacement")]);
    assert_eq!(
        crate::canonical_json(&next).unwrap(),
        crate::canonical_json(&expected).unwrap()
    );
}

#[test]
fn operation_write_sets_are_typed_bounded_and_branch_isolated() {
    let id = crate::OperationId::generate();
    let second = crate::OperationId::generate();
    let before = TrackedOperations::<String>::default();
    let mut next = before.clone();
    next.begin_tracking();
    next.insert(id, "first".into());
    next.insert(id, "updated".into());
    let mut branch = next.clone();
    branch.insert(second, "second".into());
    assert_eq!(
        next.changes_since(&before),
        MapChanges::Keys(Arc::new(BTreeSet::from([id])))
    );
    assert_eq!(
        branch.changes_since(&before),
        MapChanges::Keys(Arc::new(BTreeSet::from([id, second])))
    );
    assert_eq!(
        next.changed_key_bytes,
        std::mem::size_of::<crate::OperationId>()
    );
    let encoded = crate::canonical_json(&branch).unwrap();
    let decoded: TrackedOperations<String> = json_codec::from_slice(&encoded).unwrap();
    assert_eq!(decoded.changes_since(&before), MapChanges::All);
    let escaped: &mut BTreeMap<crate::OperationId, String> = &mut branch;
    escaped.retain(|_, _| false);
    assert_eq!(branch.changes_since(&before), MapChanges::All);
    next.begin_tracking();
    assert_eq!(next.changes_since(&before), MapChanges::All);
    let mut many = before.clone();
    many.begin_tracking();
    for index in 0..=MAX_CHANGED_KEYS {
        many.insert(format!("op_{index:032x}").parse().unwrap(), String::new());
    }
    assert_eq!(many.changes_since(&before), MapChanges::All);
}
