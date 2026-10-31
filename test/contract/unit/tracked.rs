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

#[test]
fn immutable_retention_tracks_only_removals_and_preserves_noop_lineage() {
    let before = base();
    let mut next = before.clone();
    next.begin_tracking();
    next.retain_keys(|_, _| true);
    assert!(next.changes_since(&before).is_empty());
    next.retain_keys(|key, _| key != "a");
    assert_eq!(keys(next.changes_since(&before)), vec!["a"]);
    assert_eq!(next["b"], "second");
    assert_eq!(before.len(), 2);
}

#[test]
fn tracked_shared_extension_keeps_record_handles_and_precise_key_union() {
    #[derive(Debug, PartialEq)]
    struct Immutable(u8);
    let before: TrackedRecords<Immutable> = [("a".into(), Immutable(1))].into_iter().collect();
    let source: SharedRecords<Immutable> = [("a".into(), Immutable(2)), ("b".into(), Immutable(3))]
        .into_iter()
        .collect();
    let retained = before.get_shared("a").unwrap();
    let mut next = before.clone();
    next.begin_tracking();
    next.extend_shared(&SharedRecords::new());
    assert!(next.changes_since(&before).is_empty());
    next.extend_shared(&source);
    assert_eq!(keys(next.changes_since(&before)), vec!["a", "b"]);
    assert!(Arc::ptr_eq(
        &next.get_shared("b").unwrap(),
        &source.get_shared("b").unwrap()
    ));
    assert_eq!(retained.0, 1);
    assert_eq!(next["a"].0, 2);
}

#[test]
fn field_snapshots_share_without_cloning_and_detach_only_on_mutation() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[derive(Debug)]
    struct Counted(Arc<AtomicUsize>);
    impl Clone for Counted {
        fn clone(&self) -> Self {
            self.0.fetch_add(1, Ordering::Relaxed);
            Self(Arc::clone(&self.0))
        }
    }
    let clones = Arc::new(AtomicUsize::new(0));
    let before: TrackedMap<Counted> = (0..1024)
        .map(|n| (n.to_string(), Counted(Arc::clone(&clones))))
        .collect();
    let mut next = before.clone();
    next.begin_tracking();
    assert!(Arc::ptr_eq(&before.value, &next.value));
    assert_eq!(clones.load(Ordering::Relaxed), 0);
    assert!(next.get_mut("absent").is_none());
    assert!(next.remove("absent").is_none());
    next.retain_keys(|_, _| true);
    assert!(Arc::ptr_eq(&before.value, &next.value));
    next.get_mut("0").unwrap();
    assert!(!Arc::ptr_eq(&before.value, &next.value));
    assert_eq!(clones.load(Ordering::Relaxed), 1024);
    next.get_mut("1").unwrap();
    assert_eq!(clones.load(Ordering::Relaxed), 1024);
    assert_eq!(keys(next.changes_since(&before)), ["0", "1"]);
}

#[test]
fn every_mutable_map_escape_detaches_and_preserves_retained_snapshot() {
    let before = base();
    for change in [
        |map: &mut TrackedMap<String>| {
            map.entry("a".into()).and_modify(|v| v.push('!'));
        },
        |map: &mut TrackedMap<String>| {
            map.insert("a".into(), "changed".into());
        },
        |map: &mut TrackedMap<String>| {
            map.remove("a");
        },
        |map: &mut TrackedMap<String>| {
            map.retain(|_, value| {
                value.push('!');
                true
            });
        },
        |map: &mut TrackedMap<String>| {
            map.retain_keys(|key, _| key != "a");
        },
        |map: &mut TrackedMap<String>| {
            map.clear();
        },
        |map: &mut TrackedMap<String>| {
            for (_, value) in map {
                value.push('!');
            }
        },
        |map: &mut TrackedMap<String>| {
            map.extend([("a".into(), "changed".into())]);
        },
    ] {
        let mut next = before.clone();
        next.begin_tracking();
        change(&mut next);
        assert!(!Arc::ptr_eq(&before.value, &next.value));
        assert!(!next.changes_since(&before).is_empty());
        assert_eq!(before, base());
        assert_ne!(next, before);
    }
    let owned: BTreeMap<_, _> = before.clone().into_iter().collect();
    assert_eq!(&owned, &*before);
}
