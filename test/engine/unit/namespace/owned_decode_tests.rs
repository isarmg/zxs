//! Consuming authenticated JSON preserves typed values and transfers strings.
use super::*;

#[test]
fn owned_decode_moves_page_and_index_strings_from_authenticated_values() {
    let (_, objects) = fixture_namespace(&wide_fixture());
    let mut pages = 0;
    let mut indexes = 0;
    for (hash, node) in &objects.decoded {
        let (kind, value) = match node {
            NamespaceNode::Page(page) => (
                NamespaceRefKind::Page,
                json_codec::to_value(page.as_ref()).unwrap(),
            ),
            NamespaceNode::Index(index) => (
                NamespaceRefKind::Index,
                json_codec::to_value(index.as_ref()).unwrap(),
            ),
        };
        let reference = NamespaceReference {
            kind,
            hash: hash.clone(),
        };
        let object = AuthenticatedMetadataObject::authenticate(kind.as_str(), hash, value).unwrap();
        let canonical_len = object.canonical_len();
        let field = match kind {
            NamespaceRefKind::Page => &object.value()["entries"][0]["entry_id"],
            NamespaceRefKind::Index => &object.value()["children"][0]["ref"]["hash"],
        };
        let original = field.as_str().unwrap().as_ptr();
        let decoded =
            NamespaceNode::decode(&reference, object.into_value(), canonical_len).unwrap();
        match (node, decoded) {
            (NamespaceNode::Page(expected), NamespaceNode::Page(actual)) => {
                assert_eq!(expected.as_ref(), actual.as_ref());
                let actual_id = match &actual.entries[0] {
                    NamespacePageEntry::File { entry_id, .. }
                    | NamespacePageEntry::Directory { entry_id, .. } => entry_id,
                };
                assert_eq!(original, actual_id.as_ptr());
                pages += 1;
            }
            (NamespaceNode::Index(expected), NamespaceNode::Index(actual)) => {
                assert_eq!(expected.as_ref(), actual.as_ref());
                assert_eq!(original, actual.children[0].reference.hash.as_ptr());
                indexes += 1;
            }
            _ => panic!("decoded node kind changed"),
        }
    }
    assert!(pages > 0 && indexes > 0);
}

#[test]
fn owned_decode_keeps_reference_and_shape_error_precedence() {
    let invalid = NamespaceReference {
        kind: NamespaceRefKind::Page,
        hash: "invalid-hash".into(),
    };
    let expected = Error::from(invalid.validate().unwrap_err());
    let actual = NamespaceNode::decode(&invalid, json_codec::Value::Null, 0)
        .err()
        .unwrap();
    assert_eq!(actual.code(), expected.code());
    assert_eq!(actual.to_string(), expected.to_string());

    let index = NamespaceIndex {
        format: Default::default(),
        children: Vec::new(),
    };
    let reference = NamespaceReference {
        kind: NamespaceRefKind::Index,
        hash: "a".repeat(64),
    };
    let expected = validate_index_shape(&index).unwrap_err();
    let actual = NamespaceNode::decode(&reference, json_codec::to_value(index).unwrap(), 0)
        .err()
        .unwrap();
    assert_eq!(actual.code(), expected.code());
    assert_eq!(actual.to_string(), expected.to_string());
}
