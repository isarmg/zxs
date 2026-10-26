use super::*;

fn entry(path: &str, kind: NamespaceEntryKind) -> Entry {
    Entry {
        entry_id: new_id("entry"),
        path: path.into(),
        name: path.rsplit('/').next().unwrap().into(),
        lookup_key: name_policy::path_key(path),
        kind,
        file_id: if kind == NamespaceEntryKind::File {
            new_id("file")
        } else {
            String::new()
        },
        version_id: if kind == NamespaceEntryKind::File {
            new_id("ver")
        } else {
            String::new()
        },
        size: 0,
    }
}

#[test]
fn relocating_entry_updates_path_fields_without_changing_identity_or_body() {
    let mut file = entry("Old/Document.txt", NamespaceEntryKind::File);
    file.size = 123;
    let original = file.clone();
    for path in ["Straße/文档.txt", "STRASSE/文档.txt", "STRASSE/文档.txt"] {
        relocate_entry(&mut file, path.into()).unwrap();
        assert_eq!(file.path, path);
        assert_eq!(file.name, "文档.txt");
        assert_eq!(file.lookup_key, name_policy::path_key(path));
        assert_eq!(file.entry_id, original.entry_id);
        assert_eq!(file.file_id, original.file_id);
        assert_eq!(file.version_id, original.version_id);
        assert_eq!(file.kind, original.kind);
        assert_eq!(file.size, original.size);
    }
    let before = json_codec::to_value(&file).unwrap();
    assert!(relocate_entry(&mut file, "../escape".into()).is_err());
    assert_eq!(json_codec::to_value(&file).unwrap(), before);
}

#[test]
fn rebasing_directory_preserves_identity_and_uses_component_depth() {
    let original = entry("STRASSE/文档.txt", NamespaceEntryKind::File);
    let rebased =
        super::super::directory::rebase_entries(std::slice::from_ref(&original), "Straße", "Moved")
            .unwrap();
    assert_eq!(rebased[0].path, "Moved/文档.txt");
    assert_eq!(rebased[0].name, "文档.txt");
    assert_eq!(
        rebased[0].lookup_key,
        name_policy::path_key("Moved/文档.txt")
    );
    assert_eq!(rebased[0].entry_id, original.entry_id);
    assert_eq!(rebased[0].file_id, original.file_id);
    assert_eq!(rebased[0].version_id, original.version_id);
}

#[test]
fn directory_rename_follows_normalized_ancestry_without_slicing_utf8_by_key_length() {
    for source in ["A", "Straße", "Σ"] {
        let alias = match source {
            "A" => "a",
            "Straße" => "STRASSE",
            _ => "ς",
        };
        let mut entries = vec![
            entry(source, NamespaceEntryKind::Directory),
            entry(&format!("{alias}/文档"), NamespaceEntryKind::Directory),
            entry(&format!("{alias}/文档/报告.txt"), NamespaceEntryKind::File),
            entry(&format!("{source}-other"), NamespaceEntryKind::Directory),
        ];
        validate_entries(&mut entries, 10).unwrap();
        let file_id = entries
            .iter()
            .find(|e| e.kind == NamespaceEntryKind::File)
            .unwrap()
            .file_id
            .clone();
        apply_namespace(
            &mut entries,
            &NamespaceOperation {
                kind: NamespaceOperationKind::Rename,
                path: alias.into(),
                destination: Some("Moved".into()),
            },
        )
        .unwrap();
        validate_entries(&mut entries, 10).unwrap();
        assert_eq!(
            find_unordered(&entries, "Moved/文档/报告.txt")
                .unwrap()
                .file_id,
            file_id
        );
        assert!(find_unordered(&entries, &format!("{source}-other")).is_some());
    }
}

#[test]
fn namespace_deletion_facts_follow_operations_not_identity_or_final_paths() {
    let first = entry("first.txt", NamespaceEntryKind::File);
    let mut second = first.clone();
    relocate_entry(&mut second, "second.txt".into()).unwrap();
    let base = vec![first, second];
    let mut entries = base.clone();
    let deleted = apply_workspace_namespace(
        &mut entries,
        &base,
        &[
            NamespaceOperation {
                kind: NamespaceOperationKind::Delete,
                path: "first.txt".into(),
                destination: None,
            },
            NamespaceOperation {
                kind: NamespaceOperationKind::Rename,
                path: "second.txt".into(),
                destination: Some("first.txt".into()),
            },
        ],
    )
    .unwrap();
    assert_eq!(deleted.len(), 1);
    assert_eq!(deleted[0].path, "first.txt");
    assert_eq!(deleted[0].kind, NamespaceEntryKind::File);
    assert_eq!(entries, vec![base[0].clone()]);
    let deleted = apply_namespace(
        &mut entries,
        &NamespaceOperation {
            kind: NamespaceOperationKind::Rename,
            path: "first.txt".into(),
            destination: Some("FIRST.txt".into()),
        },
    )
    .unwrap();
    assert!(deleted.is_empty());
}

#[test]
fn namespace_deletion_facts_preserve_repeated_intermediate_paths_and_tree_members() {
    let base = vec![
        entry("a", NamespaceEntryKind::File),
        entry("b", NamespaceEntryKind::File),
        entry("tree", NamespaceEntryKind::Directory),
        entry("tree/child", NamespaceEntryKind::File),
    ];
    let mut entries = base.clone();
    let deleted = apply_workspace_namespace(
        &mut entries,
        &base,
        &[
            NamespaceOperation {
                kind: NamespaceOperationKind::Rename,
                path: "a".into(),
                destination: Some("temporary".into()),
            },
            NamespaceOperation {
                kind: NamespaceOperationKind::Delete,
                path: "temporary".into(),
                destination: None,
            },
            NamespaceOperation {
                kind: NamespaceOperationKind::Rename,
                path: "b".into(),
                destination: Some("temporary".into()),
            },
            NamespaceOperation {
                kind: NamespaceOperationKind::Delete,
                path: "temporary".into(),
                destination: None,
            },
            NamespaceOperation {
                kind: NamespaceOperationKind::DeleteTree,
                path: "tree".into(),
                destination: None,
            },
        ],
    )
    .unwrap();
    assert_eq!(
        deleted
            .iter()
            .map(|fact| fact.path.as_str())
            .collect::<Vec<_>>(),
        vec!["temporary", "temporary", "tree", "tree/child"]
    );
    assert!(entries.is_empty());
}

#[test]
fn rejected_namespace_delete_does_not_remove_entries_or_return_facts() {
    let base = vec![
        entry("tree", NamespaceEntryKind::Directory),
        entry("tree/child", NamespaceEntryKind::File),
    ];
    for path in ["tree", "missing"] {
        let mut entries = base.clone();
        assert!(
            apply_namespace(
                &mut entries,
                &NamespaceOperation {
                    kind: NamespaceOperationKind::Delete,
                    path: path.into(),
                    destination: None,
                }
            )
            .is_err()
        );
        assert_eq!(entries, base);
    }
}

#[test]
fn canonical_equivalent_namespace_sources_preserve_identity_and_strict_destinations() {
    for (stored, query) in [
        ("é.txt", "e\u{301}.txt"),
        ("Å.txt", "A\u{30a}.txt"),
        ("가.txt", "\u{1100}\u{1161}.txt"),
    ] {
        let original = entry(stored, NamespaceEntryKind::File);
        let mut entries = vec![original.clone()];
        let before = json_codec::to_value(&entries).unwrap();
        for operation in [
            NamespaceOperation {
                kind: NamespaceOperationKind::Mkdir,
                path: query.into(),
                destination: None,
            },
            NamespaceOperation {
                kind: NamespaceOperationKind::Rename,
                path: stored.into(),
                destination: Some(query.into()),
            },
        ] {
            assert!(apply_namespace(&mut entries, &operation).is_err());
            assert_eq!(json_codec::to_value(&entries).unwrap(), before);
        }
        apply_namespace(
            &mut entries,
            &NamespaceOperation {
                kind: NamespaceOperationKind::Rename,
                path: query.into(),
                destination: Some("renamed.txt".into()),
            },
        )
        .unwrap();
        assert_eq!(entries[0].entry_id, original.entry_id);
        assert_eq!(entries[0].file_id, original.file_id);
        assert_eq!(entries[0].version_id, original.version_id);
        assert_eq!(entries[0].path, "renamed.txt");
        let mut entries = vec![original];
        apply_namespace(
            &mut entries,
            &NamespaceOperation {
                kind: NamespaceOperationKind::Delete,
                path: query.into(),
                destination: None,
            },
        )
        .unwrap();
        assert!(entries.is_empty());
    }
}
