use super::*;
use crate::{State, canonical_json_value};

fn authenticate(objects: Vec<PreparedMetadataObject>) -> Vec<AuthenticatedMetadataObject> {
    objects
        .into_iter()
        .map(|object| {
            AuthenticatedMetadataObject::authenticate(
                object.kind(),
                &object.hash,
                crate::decode_strict(object.bytes()).unwrap(),
            )
            .unwrap()
        })
        .collect()
}

fn compare_created(prepared: &PreparedHistory, full: &PreparedHistory) {
    assert_eq!(prepared.binding.hash, full.binding.hash);
    assert_eq!(
        prepared.binding.maximum_object_bytes,
        full.binding.maximum_object_bytes
    );
    for object in &prepared.objects {
        let expected = full
            .objects
            .iter()
            .find(|expected| expected.kind() == object.kind() && expected.hash == object.hash)
            .unwrap();
        assert_eq!(object.bytes(), expected.bytes());
    }
}

#[test]
fn descriptor_multigeneration_growth_collapse_and_budget_match_full_encoder() {
    let mut manifest = manifest();
    let mut state = State::default();
    for number in 0..2048 {
        let key = format!("file-{number:06}");
        let value = "值".repeat(20);
        manifest.file_notes.insert(key.clone(), value.clone());
        state.file_notes.insert(key, value);
    }
    let mut prepared = prepare_encoded(&manifest, MAX_METADATA).unwrap();
    let first = Arc::clone(prepared.binding.encoder.as_ref().unwrap());
    for round in 0..24 {
        let replaced = format!("file-{:06}", round * 13);
        let removed = format!("file-{:06}", round * 13 + 1);
        let inserted = format!("新键-{round}");
        let value = if round % 2 == 0 {
            "x".repeat(70 * 1024)
        } else {
            "tiny".into()
        };
        state.file_notes.insert(replaced.clone(), value.clone());
        manifest.file_notes.insert(replaced.clone(), value.clone());
        state.file_notes.remove(&removed);
        manifest.file_notes.remove(&removed);
        state.file_notes.insert(inserted.clone(), "new".into());
        manifest.file_notes.insert(inserted.clone(), "new".into());
        let edits = HistoryEdits {
            maps: BTreeMap::from([(
                Field::FileNotes,
                BTreeMap::from([
                    (replaced, Some(Value::String(value))),
                    (removed, None),
                    (inserted, Some(Value::String("new".into()))),
                ]),
            )]),
            ..HistoryEdits::default()
        };
        let previous = Arc::clone(prepared.binding.encoder.as_ref().unwrap());
        prepared = update_encoded(&previous, edits, &state, MAX_METADATA).unwrap();
        let full = prepare_encoded(&manifest, MAX_METADATA).unwrap();
        compare_created(&prepared, &full);
        let next = prepared.binding.encoder.as_ref().unwrap();
        assert!(
            std::ptr::eq(
                previous.maps[&Field::Branches].reference(),
                next.maps[&Field::Branches].reference()
            ),
            "untouched field keeps its actual Arc node"
        );
        let mut old_nodes = BTreeSet::new();
        previous.maps[&Field::FileNotes]
            .visit(&mut |reference, _| {
                old_nodes.insert(reference as *const Reference as usize);
                Ok(())
            })
            .unwrap();
        let mut shared = 0;
        next.maps[&Field::FileNotes]
            .visit(&mut |reference, _| {
                shared +=
                    usize::from(old_nodes.contains(&(reference as *const Reference as usize)));
                Ok(())
            })
            .unwrap();
        assert!(shared > 0, "changed field shares unaffected radix nodes");
    }
    // Exact maximum must shrink when the final large rows are removed. A stale
    // old maximum would reject a valid smaller reader budget.
    let edits = HistoryEdits {
        maps: BTreeMap::from([(
            Field::FileNotes,
            state
                .file_notes
                .keys()
                .cloned()
                .map(|key| (key, None))
                .collect(),
        )]),
        ..HistoryEdits::default()
    };
    state.file_notes.clear();
    manifest.file_notes.clear();
    let full = prepare_encoded(&manifest, MAX_METADATA).unwrap();
    prepared = update_encoded(
        prepared.binding.encoder.as_ref().unwrap(),
        edits,
        &state,
        full.binding.maximum_object_bytes,
    )
    .unwrap();
    compare_created(&prepared, &full);
    assert_ne!(first.hash, prepared.binding.hash);
    assert!(
        first.maps[&Field::FileNotes].maximum_object_bytes()
            > prepared.binding.encoder.as_ref().unwrap().maps[&Field::FileNotes]
                .maximum_object_bytes()
    );
}

#[test]
fn untouched_large_field_budget_and_new_same_hash_root_remain_real_encoder_output() {
    let mut manifest = manifest();
    manifest
        .file_notes
        .insert("large".into(), "x".repeat(90 * 1024));
    let prepared = prepare_encoded(&manifest, MAX_METADATA).unwrap();
    let origin = prepared.binding.encoder.as_ref().unwrap();
    assert!(
        update_encoded(
            origin,
            HistoryEdits::default(),
            &State::default(),
            prepared.binding.maximum_object_bytes - 1
        )
        .is_err()
    );
    let next = update_encoded(
        origin,
        HistoryEdits::default(),
        &State::default(),
        MAX_METADATA,
    )
    .unwrap();
    assert_eq!(next.binding.hash, prepared.binding.hash);
    assert_eq!(next.objects.len(), 1);
    assert_eq!(next.objects[0].kind(), "history-manifest");
    assert_eq!(next.objects[0].hash, prepared.binding.hash);
    assert_eq!(
        canonical_json(&origin.root).unwrap(),
        next.objects[0].bytes()
    );
}

#[test]
fn raw_null_and_empty_rows_are_preserved_without_promoting_encoder_origin() {
    let mut manifest = manifest();
    // Use the actual branch enum encoding from the independent owner fixture;
    // explicit optional fields remain different raw canonical values.
    let fixture = crate::history_package::history_changes_oracle_tests::dependency_fixture();
    let mut raw_branch = json_codec::to_value(&fixture.branches["b1"]).unwrap();
    raw_branch["file_id"] = Value::Null;
    raw_branch["file_node_id"] = Value::Null;
    raw_branch["pending_file_versions"] = json!({});
    let typed: crate::Branch = json_codec::from_value(raw_branch.clone()).unwrap();
    assert_ne!(
        canonical_json_value(&raw_branch).unwrap(),
        canonical_json(&typed).unwrap()
    );
    manifest.branches.insert("b1".into(), typed);
    let encoded = prepare_encoded(&manifest, MAX_METADATA).unwrap();
    let mut root = json_codec::to_value(&encoded.binding.encoder.as_ref().unwrap().root).unwrap();
    let leaf =
        PreparedMetadataObject::new("state-page", &json!({"b1":raw_branch}), MAX_METADATA).unwrap();
    root["maps"]["branches"] = json!({"kind":"state-page", "hash":leaf.hash});
    let root = PreparedMetadataObject::new("history-manifest", &root, MAX_METADATA).unwrap();
    let mut objects = encoded
        .objects
        .into_iter()
        .filter(|object| object.kind() != "history-manifest")
        .collect::<Vec<_>>();
    objects.push(leaf);
    objects.push(root);
    let mut objects = authenticate(objects);
    let root = objects.pop().unwrap();
    let available: BTreeMap<_, _> = objects
        .into_iter()
        .map(|object| ((object.kind(), object.hash().to_owned()), object))
        .collect();
    let (resolved, objects) = super::super::resolve_with_objects(root, |kind, hash| {
        let object = &available[&(kind, hash.to_owned())];
        AuthenticatedMetadataObject::authenticate(kind, hash, object.value().clone())
    })
    .unwrap();
    assert_eq!(
        canonical_json(&resolved).unwrap(),
        canonical_json(&manifest).unwrap()
    );
    let update = prepare_update_with_origin(
        objects,
        None,
        HistoryEdits::default(),
        &State::default(),
        MAX_METADATA,
    )
    .unwrap();
    assert!(
        update.encoder.is_none(),
        "raw graph shape/semantic admission is not encoder origin"
    );
    assert_ne!(update.hash, encoded.binding.hash);
    assert!(update.retained.iter().any(|object| {
        let value: Value = crate::decode_strict(object.bytes()).unwrap();
        value
            .get("b1")
            .and_then(|branch| branch.get("pending_file_versions"))
            == Some(&json!({}))
    }));
}
