use super::*;

#[test]
fn namespace_record_noclobber_preserves_existing_evidence() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("operation.namespace.json");
    fs::write(&path, b"earlier recovery evidence").unwrap();
    assert!(write_namespace_record(&path, &json!({"replacement":true})).is_err());
    assert_eq!(fs::read(path).unwrap(), b"earlier recovery evidence");
}

#[test]
fn namespace_typed_paths_keep_lookup_spelling_but_require_nfc_for_creation() {
    let nfd = "Cafe\u{301}/FILE.txt";
    for kind in [NamespaceOperation::Unlink, NamespaceOperation::Rmdir] {
        let input = NamespaceInput::new(
            kind,
            name_policy::LookupPath::parse(nfd).unwrap(),
            None,
            false,
        )
        .unwrap();
        assert_eq!(input.source_path.as_str(), nfd);
        assert_eq!(input.source_path.key(), "café/file.txt");
        let storage_contract::NamespaceOperation::Delete { path } = input.change else {
            panic!("expected typed deletion")
        };
        assert_eq!(path, input.source_path);
    }
    assert!(
        NamespaceInput::new(
            NamespaceOperation::Mkdir,
            name_policy::LookupPath::parse(nfd).unwrap(),
            None,
            false,
        )
        .is_err()
    );
    let input = NamespaceInput::parse(
        NamespaceOperation::Rename,
        nfd.into(),
        Some("Café/New.txt".into()),
        true,
    )
    .unwrap();
    assert_eq!(input.source_path.as_str(), nfd);
    assert_eq!(
        input.destination_path.as_ref().unwrap().as_str(),
        "Café/New.txt"
    );
    let storage_contract::NamespaceOperation::Rename {
        source,
        destination,
    } = input.change
    else {
        panic!("expected typed rename")
    };
    assert_eq!(source, input.source_path);
    assert_eq!(Some(destination), input.destination_path);
    assert!(
        NamespaceInput::parse(
            NamespaceOperation::Rename,
            "file.txt".into(),
            Some(nfd.into()),
            false,
        )
        .is_err()
    );
}
