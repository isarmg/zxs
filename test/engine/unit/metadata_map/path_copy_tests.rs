use super::*;

fn map(count: usize) -> Map<String, Value> {
    (0..count)
        .map(|n| (format!("key-{n:08}"), Value::String("retained".repeat(24))))
        .collect()
}
fn authenticated(
    objects: Vec<PreparedMetadataObject>,
) -> BTreeMap<(String, String), AuthenticatedMetadataObject> {
    objects
        .into_iter()
        .map(|object| {
            let key = (object.kind().to_owned(), object.hash.clone());
            let value = json_codec::from_slice(object.bytes()).unwrap();
            (
                key,
                AuthenticatedMetadataObject::authenticate(object.kind(), &object.hash, value)
                    .unwrap(),
            )
        })
        .collect()
}
fn proof(
    root: &MapRoot,
    key: &str,
    objects: &BTreeMap<(String, String), AuthenticatedMetadataObject>,
) -> (Option<Value>, InclusionProof) {
    read_proof(root.reference(), key, |kind, hash| {
        let object = &objects[&(kind.into(), hash.into())];
        AuthenticatedMetadataObject::authenticate(kind, hash, object.value().clone())
    })
    .unwrap()
}

#[test]
fn dirty_insert_remove_growth_and_collapse_match_full_bytes_without_global_lookup() {
    let mut logical = map(4096);
    let mut encoder = Encoder::new(MAX_METADATA);
    let mut root = encoder
        .map_root(logical.clone(), ProofMeasure::Plain)
        .unwrap();
    for round in 0..48 {
        let replace = format!("key-{:08}", round * 31);
        let removed = format!("key-{:08}", round * 31 + 1);
        let inserted = format!("新键-{round:08}");
        logical.insert(
            replace.clone(),
            Value::String(if round % 2 == 0 {
                "x".repeat(70 * 1024)
            } else {
                "tiny".into()
            }),
        );
        logical.remove(&removed);
        logical.insert(inserted.clone(), Value::String("added".into()));
        let changed = BTreeSet::from([replace, removed, inserted]);
        let mut lookups = 0;
        let mut delta = Encoder::new(MAX_METADATA);
        let next = delta
            .update_map(&root, changed, ProofMeasure::Plain, |key| {
                lookups += 1;
                Ok(logical.get(key).cloned())
            })
            .unwrap();
        let mut full = Encoder::new(MAX_METADATA);
        let expected = full.map_root(logical.clone(), ProofMeasure::Plain).unwrap();
        assert_eq!(next.reference(), expected.reference());
        assert_eq!(next.maximum_object_bytes(), expected.maximum_object_bytes());
        assert!(lookups <= 128 * 4, "round {round}, lookups={lookups}");
        let full = full
            .finish()
            .into_iter()
            .map(|o| ((o.kind(), o.hash.clone()), o.bytes().to_vec()))
            .collect::<BTreeMap<_, _>>();
        for object in delta.finish() {
            assert_eq!(object.bytes(), full[&(object.kind(), object.hash.clone())]);
        }
        root = next;
    }
}

#[test]
fn empty_write_set_shares_entire_descriptor_without_encoding_or_value_lookup() {
    let mut encoder = Encoder::new(MAX_METADATA);
    let root = encoder.map_root(map(4096), ProofMeasure::Plain).unwrap();
    let mut delta = Encoder::new(MAX_METADATA);
    let same = delta
        .update_map(&root, BTreeSet::new(), ProofMeasure::Plain, |_| {
            panic!("unchanged keys must not be looked up")
        })
        .unwrap();
    assert!(std::sync::Arc::ptr_eq(&root.0, &same.0));
    assert!(delta.finish().is_empty());
}

#[test]
fn proof_paths_are_exact_bounded_and_reject_wrong_root_key_truncation_or_extra_nodes() {
    let logical = map(500);
    let mut encoder = Encoder::new(MAX_METADATA);
    let root = encoder
        .map_root(logical.clone(), ProofMeasure::Plain)
        .unwrap();
    let objects = authenticated(encoder.finish());
    let key = logical.keys().next().unwrap();
    let (value, path) = proof(&root, key, &objects);
    assert_eq!(value, logical.get(key).cloned());
    assert_eq!(path.verify(root.reference(), key).unwrap(), value);
    let raw = json_codec::to_value(&path).unwrap();
    let nodes = raw["nodes"].as_array().unwrap();
    assert!(nodes.len() <= 65 && nodes.len() > 1);
    for damaged in [
        json_codec::json!({"nodes":&nodes[..nodes.len()-1]}),
        json_codec::json!({"nodes":nodes.iter().cloned().chain(nodes.last().cloned()).collect::<Vec<_>>()}),
        json_codec::json!({"nodes":[]}),
    ] {
        let proof: InclusionProof = json_codec::from_value(damaged).unwrap();
        assert!(proof.verify(root.reference(), key).is_err());
    }
    let mut other = Encoder::new(MAX_METADATA);
    let different = other.map_root(map(501), ProofMeasure::Plain).unwrap();
    assert!(path.verify(different.reference(), key).is_err());
    let (absent, negative) = proof(&root, "absent", &objects);
    assert!(absent.is_none());
    assert_eq!(negative.verify(root.reference(), "absent").unwrap(), None);
    let wrong = logical
        .keys()
        .find(|other| key_route(other)[..1] != key_route(key)[..1])
        .unwrap();
    assert!(path.verify(root.reference(), wrong).is_err());
}

#[test]
fn exact_tombstone_tail_pairs_each_real_status_and_key_with_its_own_path() {
    let mut logical = (0..500)
        .map(|n| {
            (
                format!("version-{}-{n:08}", "x".repeat(n % 17)),
                Value::String(
                    if n % 3 == 0 {
                        "AVAILABLE"
                    } else if n % 3 == 1 {
                        "PURGED"
                    } else {
                        "MIGRATED"
                    }
                    .into(),
                ),
            )
        })
        .collect::<Map<_, _>>();
    let mut encoder = Encoder::new(MAX_METADATA);
    let mut root = encoder
        .map_root(logical.clone(), ProofMeasure::ContentStates)
        .unwrap();
    let mut objects = authenticated(encoder.finish());
    for _ in 0..12 {
        let (worst, size) = logical
            .iter()
            .filter(|(_, v)| v.as_str() != Some("AVAILABLE"))
            .map(|(key, value)| {
                let (_, path) = proof(&root, key, &objects);
                let encoded = json_codec::to_value(&path).unwrap();
                (
                    key.clone(),
                    canonical_json_value(&encoded["nodes"]).unwrap().len() - 2
                        + canonical_json(key).unwrap().len()
                        + canonical_json_value(value).unwrap().len(),
                )
            })
            .max_by_key(|(_, size)| *size)
            .unwrap();
        assert_eq!(root.maximum_tombstone_tail(), Some(size));
        logical.remove(&worst);
        let mut encoder = Encoder::new(MAX_METADATA);
        root = encoder
            .update_map(
                &root,
                BTreeSet::from([worst]),
                ProofMeasure::ContentStates,
                |key| Ok(logical.get(key).cloned()),
            )
            .unwrap();
        objects.extend(authenticated(encoder.finish()));
    }
}

#[test]
fn catalog_reuses_bounded_typed_bindings_and_removes_obsolete_largest_dependency() {
    let mut bindings = (0..2048)
        .map(|n| {
            (
                format!("history_{n:032x}"),
                HistoryBinding {
                    hash: format!("{n:064x}"),
                    maximum_object_bytes: 1024 * 1024 + n,
                    encoder: None,
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut encoder = Encoder::new(MAX_METADATA);
    let root = encoder.map_catalog(&bindings).unwrap();
    assert_eq!(root.maximum_object_bytes(), 1024 * 1024 + 2047);
    let largest = bindings.keys().next_back().unwrap().clone();
    bindings.remove(&largest);
    let mut encoder = Encoder::new(MAX_METADATA);
    let changed = encoder
        .update_catalog(&root, BTreeSet::from([largest.clone()]), |key| {
            Ok(bindings.get(key).cloned())
        })
        .unwrap();
    assert!(changed.catalog_binding(&largest).is_none());
    assert!(root.catalog_binding(&largest).is_some());
    assert_eq!(changed.maximum_object_bytes(), 1024 * 1024 + 2046);
    let mut full = Encoder::new(MAX_METADATA);
    assert_eq!(
        changed.reference(),
        full.map_catalog(&bindings).unwrap().reference()
    );
    assert!(encoder.finish().len() < full.finish().len() / 4);
}

#[test]
fn inclusion_budget_is_an_actual_key_path_and_empty_catalog_has_none() {
    for count in [0, 1, 2048] {
        let bindings = (0..count)
            .map(|n| {
                (
                    format!("history_{n:032x}"),
                    HistoryBinding {
                        hash: format!("{n:064x}"),
                        maximum_object_bytes: 100,
                        encoder: None,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut encoder = Encoder::new(MAX_METADATA);
        let root = encoder.map_catalog(&bindings).unwrap();
        let objects = authenticated(encoder.finish());
        let actual = bindings
            .keys()
            .map(|key| {
                let (found, proof) = proof(&root, key, &objects);
                assert!(found.is_some());
                canonical_json_value(&json_codec::to_value(proof).unwrap()["nodes"])
                    .unwrap()
                    .len()
            })
            .max();
        assert_eq!(root.maximum_inclusion_proof_bytes(), actual);
        if count == 0 {
            let (found, absent) = proof(&root, "history_absent", &objects);
            assert!(found.is_none());
            assert!(
                absent
                    .verify(root.reference(), "history_absent")
                    .unwrap()
                    .is_none()
            );
        }
    }
}

// A resident key need not have any parser, Display or serialization implementation.
// Only the real encoding boundary supplies its canonical wire spelling.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct NativeKey(u32);

fn native_text(key: &NativeKey) -> Cow<'_, str> {
    Cow::Owned(format!("key-{:08}", key.0))
}

#[test]
fn typed_resident_keys_need_no_parser_and_updates_match_the_text_wire_oracle() {
    let mut logical = (0..4096)
        .map(|n| (NativeKey(n), Value::String("old".repeat(80))))
        .collect::<BTreeMap<_, _>>();
    let mut encoder = Encoder::new(MAX_METADATA);
    let mut root = encoder
        .map_typed(logical.clone(), ProofMeasure::Plain, &native_text)
        .unwrap();
    for round in 0..12 {
        let changed = BTreeSet::from([
            NativeKey(round),
            NativeKey(100 + round),
            NativeKey(5000 + round),
        ]);
        logical.remove(&NativeKey(round));
        logical.insert(NativeKey(100 + round), Value::String("new".repeat(200)));
        logical.insert(NativeKey(5000 + round), Value::String("inserted".into()));
        let formats = std::cell::Cell::new(0);
        let present = std::cell::Cell::new(0);
        let lookups = std::cell::Cell::new(0);
        let mut encoder = Encoder::new(MAX_METADATA);
        root = encoder
            .update_typed(
                &root,
                changed.clone(),
                ProofMeasure::Plain,
                &|key: &NativeKey| {
                    formats.set(formats.get() + 1);
                    native_text(key)
                },
                |key| {
                    lookups.set(lookups.get() + 1);
                    let found = logical.get(key);
                    present.set(present.get() + usize::from(found.is_some()));
                    Ok(found.cloned())
                },
            )
            .unwrap();
        assert_eq!(
            formats.get(),
            changed.len() + present.get(),
            "one route spelling per change and one actual leaf encoding per present lookup"
        );
        assert!(lookups.get() <= 6 * PAGE_ENTRIES);
        let mut full = Encoder::new(MAX_METADATA);
        let expected = full
            .map_root(
                logical
                    .iter()
                    .map(|(key, value)| (native_text(key).into_owned(), value.clone()))
                    .collect(),
                ProofMeasure::Plain,
            )
            .unwrap();
        assert_eq!(root.reference(), expected.reference());
        let all = full.finish();
        for object in encoder.finish() {
            let expected = all
                .iter()
                .find(|candidate| {
                    candidate.kind() == object.kind() && candidate.hash == object.hash
                })
                .unwrap();
            assert_eq!(object.bytes(), expected.bytes());
        }
    }
    let mut encoder = Encoder::new(MAX_METADATA);
    let unchanged = encoder
        .update_typed(
            &root,
            BTreeSet::new(),
            ProofMeasure::Plain,
            &|_: &NativeKey| panic!("unchanged typed roots do not format keys"),
            |_| panic!("unchanged typed roots do not perform lookups"),
        )
        .unwrap();
    assert!(Arc::ptr_eq(&root.0, &unchanged.0));
    assert!(encoder.finish().is_empty());
}

#[test]
fn text_entry_encoding_does_not_own_a_second_copy_of_resident_key_text() {
    let entries = encode_entries(map(128), &|key: &String| Cow::Borrowed(key.as_str())).unwrap();
    assert!(entries.iter().all(|entry| entry.text.is_none()));
}
