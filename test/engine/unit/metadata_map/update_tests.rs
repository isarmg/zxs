use super::*;
use crate::{canonical_json, decode_strict};
use json_codec::json;

type Objects = BTreeMap<(&'static str, String), AuthenticatedMetadataObject>;

fn prepared(map: Map<String, Value>, maximum: usize) -> (Reference, Vec<PreparedMetadataObject>) {
    let mut encoder = Encoder::new(maximum);
    let reference = encoder.map(map).unwrap();
    (reference, encoder.finish())
}

fn authenticate(objects: Vec<PreparedMetadataObject>) -> Objects {
    objects
        .into_iter()
        .map(|object| {
            let value = decode_strict(object.bytes()).unwrap();
            let key = (object.kind(), object.hash.clone());
            let authenticated =
                AuthenticatedMetadataObject::authenticate(key.0, &key.1, value).unwrap();
            (key, authenticated)
        })
        .collect()
}

fn compare_update(
    before: Map<String, Value>,
    changes: Map<String, Value>,
    maximum: usize,
) -> (usize, usize) {
    compare_edits(
        before,
        changes
            .into_iter()
            .map(|(key, value)| (key, Some(value)))
            .collect(),
        maximum,
    )
}

fn compare_edits(
    before: Map<String, Value>,
    changes: BTreeMap<String, Option<Value>>,
    maximum: usize,
) -> (usize, usize) {
    let (reference, objects) = prepared(before.clone(), MAX_METADATA);
    let mut previous = authenticate(objects);
    let mut expected = before;
    for (key, value) in &changes {
        match value {
            Some(value) => {
                expected.insert(key.clone(), value.clone());
            }
            None => {
                expected.remove(key);
            }
        }
    }
    let (expected_root, expected_objects) = prepared(expected.clone(), maximum);
    let mut encoder = Encoder::new(maximum);
    let updated = encoder
        .edit_map_values(&reference, changes, |reference| {
            Ok(&previous[&(reference.kind.domain(), reference.hash.clone())])
        })
        .unwrap();
    assert_eq!(
        canonical_json(updated.root.reference()).unwrap(),
        canonical_json(&expected_root).unwrap()
    );
    let encoded = encoder.finish();
    let encoded_count = encoded.len();
    let full_count = expected_objects.len();
    let expected_objects = expected_objects
        .into_iter()
        .map(|object| ((object.kind(), object.hash.clone()), object))
        .collect::<BTreeMap<_, _>>();
    for object in &encoded {
        let key = (object.kind(), object.hash.clone());
        assert_eq!(object.bytes(), expected_objects[&key].bytes());
    }
    for reference in &updated.reused_roots {
        assert!(previous.contains_key(&(reference.kind.domain(), reference.hash.clone())));
    }
    previous.extend(authenticate(encoded));
    let mut resolver = Resolver::new(|kind, hash: &str| {
        let object = &previous[&(kind, hash.to_owned())];
        AuthenticatedMetadataObject::authenticate(kind, hash, object.value().clone())
    });
    let resolved = resolver.map(updated.root.reference()).unwrap();
    assert_eq!(resolved, expected);
    let reachable = resolver.finish();
    assert_eq!(reachable.len(), expected_objects.len());
    for object in reachable {
        assert_eq!(
            object.bytes(),
            expected_objects[&(object.kind(), object.hash().to_owned())].bytes()
        );
    }
    (encoded_count, full_count)
}

#[test]
fn one_replacement_encodes_only_its_leaf_and_ancestors() {
    let before = (0..4000)
        .map(|index| (format!("key-{index:06}"), json!(format!("value-{index}"))))
        .collect::<Map<_, _>>();
    let (encoded, full) = compare_update(
        before,
        Map::from_iter([("key-001234".into(), json!("updated / 文 / 📁"))]),
        MAX_METADATA,
    );
    assert!(encoded < full / 4, "{encoded} new objects out of {full}");
}

#[test]
fn byte_boundary_growth_splits_and_shrinkage_collapses_exactly_like_full_encoder() {
    let small = Map::from_iter([("first".into(), json!("a")), ("second".into(), json!("b"))]);
    let large = Map::from_iter([
        ("first".into(), json!("x".repeat(70 * 1024))),
        ("second".into(), json!("b")),
    ]);
    compare_update(
        small,
        Map::from_iter([("first".into(), large["first"].clone())]),
        MAX_METADATA,
    );
    // Replaced oversized old objects are not subject to the new candidate's
    // smaller ceiling: the exact new reachable map is a small single leaf.
    compare_update(
        large,
        Map::from_iter([("first".into(), json!("a"))]),
        PAGE_TARGET_BYTES,
    );
}

#[test]
fn multiple_dirty_paths_same_values_and_unicode_keys_match_full_bytes() {
    let mut before = (0..128)
        .map(|index| (format!("entry-{index:03}"), json!("x".repeat(600))))
        .collect::<Map<_, _>>();
    before.insert("\u{e000}".into(), json!("private"));
    before.insert("\u{10000}".into(), json!("astral"));
    let changes = before
        .iter()
        .enumerate()
        .filter(|(index, _)| index % 3 == 0)
        .map(|(index, (key, value))| {
            (
                key.clone(),
                if index % 2 == 0 {
                    json!("short")
                } else {
                    value.clone()
                },
            )
        })
        .collect();
    compare_update(before, changes, MAX_METADATA);
}

#[test]
fn empty_write_set_reuses_the_complete_root_without_encoding_objects() {
    let before = (0..500)
        .map(|index| (format!("key-{index}"), json!(index)))
        .collect();
    let (encoded, full) = compare_update(before, Map::new(), MAX_METADATA);
    assert_eq!(encoded, 0);
    assert!(full > 1);
}

#[test]
fn replacement_cannot_add_an_unproven_key() {
    let (reference, objects) = prepared(
        Map::from_iter([("existing".into(), json!(1))]),
        MAX_METADATA,
    );
    let objects = authenticate(objects);
    let mut encoder = Encoder::new(MAX_METADATA);
    assert!(
        encoder
            .replace_map_values(
                &reference,
                Map::from_iter([("absent".into(), json!(2))]),
                |reference| { Ok(&objects[&(reference.kind.domain(), reference.hash.clone())]) }
            )
            .is_err()
    );
}

#[test]
fn deterministic_replacement_batches_match_independent_complete_encoding() {
    let mut map = (0..400)
        .map(|index| (format!("key-{index:04}"), json!(format!("initial-{index}"))))
        .collect::<Map<_, _>>();
    let mut random = 0x91a2_u64;
    for _ in 0..20 {
        let mut changes = Map::new();
        for _ in 0..12 {
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            let index = random % 400;
            let length = usize::try_from((random >> 32) % 1400).unwrap();
            changes.insert(format!("key-{index:04}"), json!("文".repeat(length)));
        }
        compare_update(map.clone(), changes.clone(), MAX_METADATA);
        map.extend(changes);
    }
}

#[test]
fn inserted_and_deleted_rows_cover_empty_roots_new_paths_and_collapse() {
    compare_edits(
        Map::new(),
        BTreeMap::from_iter([("absent".into(), None)]),
        MAX_METADATA,
    );
    compare_edits(
        Map::new(),
        BTreeMap::from_iter([("new".into(), Some(json!(1)))]),
        MAX_METADATA,
    );
    let before = (0..1000)
        .map(|index| (format!("row-{index:04}"), json!(index)))
        .collect::<Map<_, _>>();
    let mut changes = before
        .keys()
        .take(998)
        .map(|key| (key.clone(), None))
        .collect::<BTreeMap<_, _>>();
    changes.insert("new-radix-route".into(), Some(json!("文")));
    compare_edits(before.clone(), changes, MAX_METADATA);
    compare_edits(
        before.clone(),
        before.keys().map(|key| (key.clone(), None)).collect(),
        MAX_METADATA,
    );
}

#[test]
fn mixed_insert_update_delete_batches_match_independent_complete_bytes() {
    let mut before = Map::new();
    let mut random = 0xa5f1_u64;
    for _ in 0..20 {
        let mut changes = BTreeMap::new();
        for _ in 0..24 {
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            let key = format!("row-{:04}", random % 160);
            let value = (!random.is_multiple_of(3))
                .then(|| json!("文".repeat(((random >> 16) % 1500) as usize)));
            changes.insert(key, value);
        }
        compare_edits(before.clone(), changes.clone(), MAX_METADATA);
        for (key, value) in changes {
            match value {
                Some(value) => {
                    before.insert(key, value);
                }
                None => {
                    before.remove(&key);
                }
            }
        }
    }
}
