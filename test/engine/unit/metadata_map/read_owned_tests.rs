use super::*;

#[test]
fn streamed_leaf_transfers_nested_values_without_cloning_allocations() {
    let value = json_codec::json!({"entry": {"nested": ["owned payload ".repeat(2000)]}});
    let payload = value["entry"]["nested"][0].as_str().unwrap();
    let original_pointer = payload.as_ptr();
    let expected = payload.to_owned();
    let prepared = PreparedMetadataObject::new("state-page", &value, MAX_METADATA).unwrap();
    let reference = Reference {
        kind: ReferenceKind::Page,
        hash: prepared.hash.clone(),
    };
    let mut object = Some(
        AuthenticatedMetadataObject::authenticate("state-page", &reference.hash, value).unwrap(),
    );
    let mut loads = 0;
    let result = read_map(&reference, |kind, hash| {
        assert_eq!(kind, "state-page");
        assert_eq!(hash, reference.hash);
        loads += 1;
        Ok(object.take().unwrap())
    })
    .unwrap();
    let actual = result["entry"]["nested"][0].as_str().unwrap();
    assert_eq!(actual, expected);
    assert_eq!(actual.as_ptr(), original_pointer);
    assert_eq!(loads, 1);
}

#[test]
fn streamed_move_still_rejects_nonmap_leaf_and_wrong_route_before_returning_values() {
    for value in [
        json_codec::json!(["not a map"]),
        json_codec::json!({"entry": "body"}),
    ] {
        let prepared = PreparedMetadataObject::new("state-page", &value, MAX_METADATA).unwrap();
        let leaf = Reference {
            kind: ReferenceKind::Page,
            hash: prepared.hash.clone(),
        };
        let mut leaf_object = Some(
            AuthenticatedMetadataObject::authenticate("state-page", &leaf.hash, value.clone())
                .unwrap(),
        );
        if value.is_array() {
            assert!(read_map(&leaf, |_, _| Ok(leaf_object.take().unwrap())).is_err());
        } else {
            let wrong = if key_route("entry").starts_with('0') {
                "1"
            } else {
                "0"
            };
            let index = MetadataIndex {
                prefix: String::new(),
                children: BTreeMap::from([(wrong.into(), leaf.clone())]),
            };
            let encoded =
                PreparedMetadataObject::new("metadata-index", &index, MAX_METADATA).unwrap();
            let root = Reference {
                kind: ReferenceKind::Index,
                hash: encoded.hash.clone(),
            };
            let mut index_object = Some(
                AuthenticatedMetadataObject::authenticate(
                    "metadata-index",
                    &root.hash,
                    json_codec::to_value(index).unwrap(),
                )
                .unwrap(),
            );
            assert!(
                read_map(&root, |kind, _| Ok(if kind == "state-page" {
                    leaf_object.take().unwrap()
                } else {
                    index_object.take().unwrap()
                }))
                .is_err()
            );
        }
    }
}
