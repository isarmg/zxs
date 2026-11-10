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
        assert_eq!(input.kind, kind);
        assert_eq!(input.change.path(), nfd);
        assert_eq!(input.change.key(), "café/file.txt");
        assert!(input.change.destination().is_none());
        let storage_contract::NamespaceOperation::Delete { path } = input.change else {
            panic!("expected typed deletion")
        };
        assert_eq!(path.as_str(), nfd);
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
    assert_eq!(input.change.path(), nfd);
    assert_eq!(input.change.destination().unwrap().as_str(), "Café/New.txt");
    let storage_contract::NamespaceOperation::Rename {
        source,
        destination,
    } = input.change
    else {
        panic!("expected typed rename")
    };
    assert_eq!(source.as_str(), nfd);
    assert_eq!(destination.as_str(), "Café/New.txt");
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

#[test]
fn namespace_input_transfers_each_typed_path_into_one_staged_change() {
    for kind in [
        NamespaceOperation::Mkdir,
        NamespaceOperation::Unlink,
        NamespaceOperation::Rmdir,
        NamespaceOperation::Rename,
    ] {
        let source = name_policy::LookupPath::parse("Café/Existing.txt").unwrap();
        let source_display = source.as_str().as_ptr();
        let source_key = source.key().as_ptr();
        let destination = (kind == NamespaceOperation::Rename)
            .then(|| name_policy::PortablePath::parse("Café/New.txt").unwrap());
        let target_buffers = destination
            .as_ref()
            .map(|path| (path.as_str().as_ptr(), path.key().as_ptr()));
        let input = NamespaceInput::new(kind, source, destination, false).unwrap();
        assert_eq!(input.change.path().as_ptr(), source_display);
        assert_eq!(input.change.key().as_ptr(), source_key);
        assert_eq!(
            input
                .change
                .destination()
                .map(|path| (path.as_str().as_ptr(), path.key().as_ptr())),
            target_buffers
        );
    }
}
