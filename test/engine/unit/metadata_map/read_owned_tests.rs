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

fn owned_index_decode(object: &AuthenticatedMetadataObject) -> Result<MetadataIndex> {
    let index: MetadataIndex = json_codec::from_value(object.value().clone())?;
    if index.prefix.len() >= RADIX_DEPTH
        || !hexadecimal(&index.prefix)
        || index.children.is_empty()
        || index.children.len() > 16
    {
        return Err(invalid("invalid metadata map index"));
    }
    for (digit, child) in &index.children {
        if digit.len() != 1 || !hexadecimal(digit) {
            return Err(invalid("invalid metadata map radix digit"));
        }
        child.validate()?;
    }
    Ok(index)
}

fn decoder_error_chain(error: &Error) -> (storage_contract::ErrorCode, Vec<String>) {
    let mut chain = Vec::new();
    let mut current: Option<&dyn std::error::Error> = Some(error);
    while let Some(error) = current {
        chain.push(error.to_string());
        current = error.source();
    }
    (error.code(), chain)
}

#[test]
fn index_borrowed_decode_matches_owned_admission_on_real_objects() {
    let temp = tempfile::tempdir().unwrap();
    let config = crate::Config::development([
        temp.path().join("a"),
        temp.path().join("b"),
        temp.path().join("c"),
    ]).unwrap();
    let disk = &config.disks[0];
    let values = (0..140)
        .map(|index| (format!("entry-{index}"), Value::String("payload".into())))
        .collect();
    let mut encoder = Encoder::new(MAX_METADATA);
    let reference = encoder.map(values).unwrap();
    assert_eq!(reference.kind(), ReferenceKind::Index);
    for object in encoder.finish() {
        object.store(std::slice::from_ref(disk)).unwrap();
    }
    let original = crate::journal::load_authenticated_object(
        disk, "metadata-index", reference.hash(),
    ).unwrap();
    let valid = original.value().clone();
    let mut cases = vec![valid.clone(), Value::Null, Value::Array(Vec::new())];
    let mut unknown = valid.clone();
    unknown["unknown"] = Value::Bool(true);
    cases.push(unknown);
    let mut missing = valid.clone();
    missing.as_object_mut().unwrap().remove("prefix");
    cases.push(missing);
    for prefix in [Value::Null, Value::Bool(false), Value::String("g".into()),
        Value::String("f".repeat(64))]
    {
        let mut value = valid.clone();
        value["prefix"] = prefix;
        cases.push(value);
    }
    let edge = valid["children"].as_object().unwrap().values().next().unwrap().clone();
    for children in [
        Value::Object(Map::new()),
        json_codec::json!({"ab": edge.clone()}),
        json_codec::json!({"g": edge.clone()}),
        json_codec::json!({"a": {"kind": "unknown", "hash": "0".repeat(64)}}),
        json_codec::json!({"a": {"kind": "state-page", "hash": "BAD"}}),
        json_codec::json!({"a": {"kind": "state-page"}}),
        json_codec::json!({"a": {"kind": "state-page", "hash": null}}),
        json_codec::json!({"a": {"kind": "state-page", "hash": "0".repeat(64), "x": 1}}),
    ] {
        let mut value = valid.clone();
        value["children"] = children;
        cases.push(value);
    }
    for value in cases {
        let prepared = PreparedMetadataObject::new("metadata-index", &value, MAX_METADATA).unwrap();
        prepared.store(std::slice::from_ref(disk)).unwrap();
        let path = crate::journal::object_path(disk, "metadata-index", &prepared.hash);
        let before = std::fs::read(&path).unwrap();
        let object = crate::journal::load_authenticated_object(
            disk, "metadata-index", &prepared.hash,
        ).unwrap();
        let expected = owned_index_decode(&object);
        let actual = decode_index(&object);
        match (expected, actual) {
            (Ok(expected), Ok(actual)) => {
                assert_eq!(canonical_json(&expected).unwrap(), canonical_json(&actual).unwrap());
            }
            (Err(expected), Err(actual)) => {
                assert_eq!(decoder_error_chain(&expected), decoder_error_chain(&actual));
            }
            (expected, actual) => {
                panic!("decode result differs: owned={}, borrowed={}", expected.is_ok(),
                    actual.is_ok());
            }
        }
        assert_eq!(std::fs::read(path).unwrap(), before);
    }
}
