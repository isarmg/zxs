use super::*;
use json_codec::json;

type Objects = BTreeMap<(&'static str, String), AuthenticatedMetadataObject>;

fn object(kind: &'static str, value: Value) -> AuthenticatedMetadataObject {
    let hash = crate::object_hash(kind, &value).unwrap();
    AuthenticatedMetadataObject::authenticate(kind, &hash, value).unwrap()
}

fn reference(object: &AuthenticatedMetadataObject, kind: ReferenceKind) -> Reference {
    Reference {
        kind,
        hash: object.hash().to_owned(),
    }
}

#[test]
fn read_map_moves_nested_leaf_allocations_without_changing_values() {
    let expected = json!({"固定键": {"nested": ["owned payload", 17, null, true]}});
    let page = object("state-page", expected.clone());
    let key_pointer = page
        .value()
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .as_ptr();
    let payload_pointer = page.value()["固定键"]["nested"][0]
        .as_str()
        .unwrap()
        .as_ptr();
    let root = reference(&page, ReferenceKind::Page);
    let mut page = Some(page);
    let actual = read_map(&root, |_, _| Ok(page.take().unwrap())).unwrap();
    assert_eq!(actual.keys().next().unwrap().as_ptr(), key_pointer);
    assert_eq!(
        actual["固定键"]["nested"][0].as_str().unwrap().as_ptr(),
        payload_pointer
    );
    assert_eq!(Value::Object(actual), expected);
    assert!(page.is_none());
}

#[test]
fn read_map_preserves_empty_and_multilevel_maps() {
    for count in [0, 1, 4096] {
        let expected: Map<String, Value> = (0..count)
            .map(|n| (format!("实体-{n:08}"), json!({"items": [n, "value", null]})))
            .collect();
        let mut encoder = Encoder::new(MAX_METADATA);
        let root = encoder.map(expected.clone()).unwrap();
        let mut objects: Objects = encoder
            .finish()
            .into_iter()
            .map(|prepared| {
                let key = (prepared.kind(), prepared.hash.clone());
                let value = crate::decode_strict(prepared.bytes()).unwrap();
                let object =
                    AuthenticatedMetadataObject::authenticate(key.0, &key.1, value).unwrap();
                (key, object)
            })
            .collect();
        let actual = read_map(&root, |kind, hash| {
            Ok(objects.remove(&(kind, hash.to_owned())).unwrap())
        })
        .unwrap();
        assert_eq!(actual, expected);
        assert!(objects.is_empty());
    }
}

#[test]
fn authenticated_digest_does_not_admit_malformed_page_shape() {
    for value in [json!([]), json!(null), json!("not a map")] {
        let page = object("state-page", value);
        let root = reference(&page, ReferenceKind::Page);
        let mut page = Some(page);
        let error = read_map(&root, |_, _| Ok(page.take().unwrap())).unwrap_err();
        assert_eq!(
            error.code(),
            storage_contract::ErrorCode::HistoryPackageInvalid
        );
    }
    let oversized: Map<String, Value> = (0..=PAGE_ENTRIES)
        .map(|n| (format!("key-{n}"), Value::Null))
        .collect();
    let page = object("state-page", Value::Object(oversized));
    let root = reference(&page, ReferenceKind::Page);
    let mut page = Some(page);
    let error = read_map(&root, |_, _| Ok(page.take().unwrap())).unwrap_err();
    assert_eq!(
        error.code(),
        storage_contract::ErrorCode::HistoryPackageInvalid
    );
}

#[test]
fn read_map_rejects_duplicate_leaf_aliases_and_wrong_routes() {
    let leaf = json!({"stable-key": "retained"});
    let page = object("state-page", leaf.clone());
    let page_reference = reference(&page, ReferenceKind::Page);
    let digit = key_route("stable-key")[..1].to_owned();
    let wrong_digit = if digit == "0" { "1" } else { "0" }.to_owned();
    for children in [
        BTreeMap::from([(wrong_digit.clone(), page_reference.clone())]),
        BTreeMap::from([
            (digit.clone(), page_reference.clone()),
            (wrong_digit, page_reference.clone()),
        ]),
    ] {
        let index = object(
            "metadata-index",
            json_codec::to_value(MetadataIndex {
                prefix: String::new(),
                children,
            })
            .unwrap(),
        );
        let root = reference(&index, ReferenceKind::Index);
        let mut index = Some(index);
        let error = read_map(&root, |kind, _| {
            if kind == "metadata-index" {
                Ok(index.take().unwrap())
            } else {
                Ok(object("state-page", leaf.clone()))
            }
        })
        .unwrap_err();
        assert_eq!(
            error.code(),
            storage_contract::ErrorCode::HistoryPackageInvalid
        );
    }
}

#[test]
fn read_map_propagates_digest_refusal_before_consuming_entries() {
    let page = object("state-page", json!({"key": "original"}));
    let root = reference(&page, ReferenceKind::Page);
    let error = read_map(&root, |kind, hash| {
        AuthenticatedMetadataObject::authenticate(kind, hash, json!({"key": "corrupt"}))
    })
    .unwrap_err();
    assert_eq!(error.code(), storage_contract::ErrorCode::ObjectCorrupt);
}

#[test]
fn consuming_loaded_copy_cannot_mutate_retained_source() {
    let cached = object("state-page", json!({"key": {"nested": "original"}}));
    let root = reference(&cached, ReferenceKind::Page);
    let mut actual = read_map(&root, |kind, hash| {
        AuthenticatedMetadataObject::authenticate(kind, hash, cached.value().clone())
    })
    .unwrap();
    actual.get_mut("key").unwrap()["nested"] = json!("changed");
    assert_eq!(cached.value()["key"]["nested"], json!("original"));
    let reread = read_map(&root, |kind, hash| {
        AuthenticatedMetadataObject::authenticate(kind, hash, cached.value().clone())
    })
    .unwrap();
    assert_eq!(Value::Object(reread), *cached.value());
}

#[test]
fn read_map_rejects_wrong_index_prefix_and_object_domain() {
    let page = object("state-page", json!({"key": "original"}));
    let index = object(
        "metadata-index",
        json_codec::to_value(MetadataIndex {
            prefix: "0".into(),
            children: BTreeMap::from([("0".into(), reference(&page, ReferenceKind::Page))]),
        })
        .unwrap(),
    );
    let root = reference(&index, ReferenceKind::Index);
    let mut index = Some(index);
    let error = read_map(&root, |kind, _| {
        assert_eq!(
            kind, "metadata-index",
            "bad prefix must reject before loading children"
        );
        Ok(index.take().unwrap())
    })
    .unwrap_err();
    assert_eq!(
        error.code(),
        storage_contract::ErrorCode::HistoryPackageInvalid
    );

    let root = reference(&page, ReferenceKind::Page);
    let error = read_map(&root, |_, hash| {
        AuthenticatedMetadataObject::authenticate("metadata-index", hash, page.value().clone())
    })
    .unwrap_err();
    assert_eq!(error.code(), storage_contract::ErrorCode::ObjectCorrupt);
}
