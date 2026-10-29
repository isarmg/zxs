use super::*;

fn file(name: &str) -> NamespacePageEntry {
    NamespacePageEntry::File {
        entry_id: format!("entry-{name}"),
        name: name_policy::PortableName::parse(name).unwrap(),
        file_id: format!("file-{name}"),
        version_id: format!("ver-{name}"),
        size: 7,
    }
}
fn directory(name: &str) -> NamespacePageEntry {
    NamespacePageEntry::Directory {
        entry_id: format!("entry-{name}"),
        name: name_policy::PortableName::parse(name).unwrap(),
        entries: 3,
        files: 2,
        subtree: NamespaceReference {
            kind: NamespaceRefKind::Page,
            hash: "a".repeat(64),
        },
    }
}

#[test]
fn page_entries_round_trip_with_derived_lookup_keys_and_component_names() {
    let page = NamespacePage {
        format: Default::default(),
        entries: vec![file("Café.txt"), directory("docs"), file("ß.txt")],
    };
    let value = json_codec::to_value(&page).unwrap();
    assert_eq!(value["format"], NAMESPACE_PAGE_KIND);
    assert_eq!(
        value["entries"][0]["lookup_key"],
        name_policy::lookup_key("Café.txt")
    );
    assert_eq!(value["entries"][1]["kind"], "directory");
    assert_eq!(value["entries"][1]["entries"], "3");
    assert_eq!(value["entries"][1]["files"], "2");
    assert!(value["entries"][0].get("subtree").is_none());
    assert!(value["entries"][1].get("file_id").is_none());
    let decoded: NamespacePage = json_codec::from_value(value).unwrap();
    assert_eq!(decoded, page);
    assert_eq!(decoded.entries[1].lookup_key(), "docs");
    assert!(decoded.entries[1].is_directory());
    assert_eq!(decoded.entries[0].entry_id(), "entry-Café.txt");
}

#[test]
fn malformed_page_entries_fail_closed() {
    let good = json_codec::to_value(&NamespacePage {
        format: Default::default(),
        entries: vec![file("a.txt"), directory("docs")],
    })
    .unwrap();
    // A recorded lookup key that differs from the display name's derived key.
    let mut forged = good.clone();
    forged["entries"][0]["lookup_key"] = json_codec::json!("other");
    assert!(json_codec::from_value::<NamespacePage>(forged).is_err());
    // Names must be single valid path components.
    for name in ["a/b.txt", "", "."] {
        let mut nested = good.clone();
        nested["entries"][0]["name"] = json_codec::json!(name);
        nested["entries"][0]["lookup_key"] = json_codec::json!(name_policy::lookup_key(name));
        assert!(
            json_codec::from_value::<NamespacePage>(nested).is_err(),
            "{name}"
        );
    }
    // Kind-specific fields are exact: a file never carries subtree totals and
    // a directory never carries version bindings.
    let mut mixed = good.clone();
    mixed["entries"][0]["entries"] = json_codec::json!("1");
    assert!(json_codec::from_value::<NamespacePage>(mixed).is_err());
    let mut mixed = good.clone();
    mixed["entries"][1]["version_id"] = json_codec::json!("ver");
    assert!(json_codec::from_value::<NamespacePage>(mixed).is_err());
    let mut missing = good.clone();
    missing["entries"][1]
        .as_object_mut()
        .unwrap()
        .remove("subtree");
    assert!(json_codec::from_value::<NamespacePage>(missing).is_err());
    let mut invalid = good.clone();
    invalid["entries"][1]["subtree"]["hash"] = json_codec::json!("short");
    assert!(json_codec::from_value::<NamespacePage>(invalid).is_err());
    let mut unknown = good;
    unknown["format"] = json_codec::json!("future-page");
    assert!(json_codec::from_value::<NamespacePage>(unknown).is_err());
}

#[test]
fn index_and_commit_namespace_round_trip_with_decimal_totals() {
    let index = NamespaceIndex {
        format: Default::default(),
        children: vec![
            NamespaceIndexChild {
                first_key: "aaa".into(),
                entries: u64::MAX,
                files: 9,
                reference: NamespaceReference {
                    kind: NamespaceRefKind::Index,
                    hash: "b".repeat(64),
                },
            },
            NamespaceIndexChild {
                first_key: "mmm".into(),
                entries: 1,
                files: 0,
                reference: NamespaceReference {
                    kind: NamespaceRefKind::Page,
                    hash: "c".repeat(64),
                },
            },
        ],
    };
    let value = json_codec::to_value(&index).unwrap();
    assert_eq!(value["format"], NAMESPACE_INDEX_KIND);
    assert_eq!(value["children"][0]["entries"], u64::MAX.to_string());
    assert_eq!(value["children"][0]["ref"]["kind"], NAMESPACE_INDEX_KIND);
    assert_eq!(
        json_codec::from_value::<NamespaceIndex>(value).unwrap(),
        index
    );
    let namespace = CommitNamespace {
        root: NamespaceReference {
            kind: NamespaceRefKind::Page,
            hash: "d".repeat(64),
        },
        entries: 42,
    };
    let value = json_codec::to_value(&namespace).unwrap();
    assert_eq!(value["entries"], "42");
    assert_eq!(
        json_codec::from_value::<CommitNamespace>(value).unwrap(),
        namespace
    );
    assert!(
        json_codec::from_value::<NamespaceReference>(json_codec::json!({
            "kind": "namespace-page", "hash": "A".repeat(64)
        }))
        .unwrap()
        .validate()
        .is_err(),
        "uppercase hex is not a canonical object hash"
    );
}

#[test]
fn retained_component_keys_preserve_canonical_wire_bytes() {
    let entry = file("Straße.txt");
    let expected = r#"{"entry_id":"entry-Straße.txt","file_id":"file-Straße.txt","kind":"file","lookup_key":"strasse.txt","name":"Straße.txt","size":"7","version_id":"ver-Straße.txt"}"#;
    assert_eq!(canonical_json(&entry).unwrap(), expected.as_bytes());
    let decoded: NamespacePageEntry = json_codec::from_str(expected).unwrap();
    assert_eq!(canonical_json(&decoded).unwrap(), expected.as_bytes());
    assert_eq!(decoded.name(), "Straße.txt");
    assert_eq!(decoded.lookup_key(), "strasse.txt");
    assert!(std::ptr::eq(
        decoded.lookup_key().as_ptr(),
        decoded.component().key().as_ptr()
    ));
    assert!(std::ptr::eq(
        decoded.lookup_key().as_ptr(),
        decoded.lookup_key().as_ptr()
    ));
    let mut nfd = json_codec::to_value(file("Café.txt")).unwrap();
    nfd["name"] = json_codec::json!("Cafe\u{301}.txt");
    assert!(json_codec::from_value::<NamespacePageEntry>(nfd).is_err());
}

#[test]
fn borrowed_page_entry_serialization_keeps_field_order_and_optional_omissions() {
    assert_eq!(
        json_codec::to_string(&file("a.txt")).unwrap(),
        r#"{"entry_id":"entry-a.txt","name":"a.txt","lookup_key":"a.txt","kind":"file","file_id":"file-a.txt","version_id":"ver-a.txt","size":"7"}"#
    );
    let expected = format!(
        r#"{{"entry_id":"entry-docs","name":"docs","lookup_key":"docs","kind":"directory","entries":"3","files":"2","subtree":{{"kind":"namespace-page","hash":"{}"}}}}"#,
        "a".repeat(64),
    );
    let entry = directory("docs");
    assert_eq!(json_codec::to_string(&entry).unwrap(), expected);
    assert_eq!(
        json_codec::from_str::<NamespacePageEntry>(&expected).unwrap(),
        entry
    );
}
