use super::*;

#[test]
fn full_path_walk_matches_flat_order_with_interleaved_aliases_and_pages() {
    let mut entries = vec![
        directory("A", 1),
        file("A-b", 7),
        file("A.b", 7),
        file("A/c", 7),
        directory("A/d", 2),
        file("A/d-e", 8),
        file("A/d/f", 9),
        directory("A.e", 3),
        file("A.e/z", 10),
        directory("Straße", 4),
        file("Straße-a", 11),
        file("Straße/x", 12),
        file("STRASSZ", 13),
    ];
    entries[1].version_id = "ver_first_alias".into();
    entries[2].version_id = "ver_middle_alias".into();
    entries[3].version_id = "ver_last_alias".into();
    for n in 0..300 {
        entries.push(file(&format!("a-zz-{n:04}"), 1000 + n));
        entries.push(file(&format!("A/d-{n:04}"), 2000 + n));
    }
    let oracle = sorted_oracle(entries.clone());
    let mut objects = NamespaceObjects::new();
    let namespace = build_from_entries(&entries, &mut objects).unwrap();
    let base = deny();
    let source = DraftSource {
        fresh: &objects,
        base: &base,
    };
    let view = NamespaceView::new(&namespace, &source);
    let mut walked = Vec::new();
    let mut last_alias_versions = BTreeMap::new();
    view.walk(|location, entry| {
        let entry = materialized_entry(location.clone(), entry);
        if entry.kind == NamespaceEntryKind::File {
            last_alias_versions.insert(entry.file_id.clone(), entry.version_id.clone());
        }
        walked.push(entry);
        Ok(true)
    })
    .unwrap();
    assert_eq!(walked, oracle);
    assert_eq!(view.collect_entries().unwrap(), oracle);
    let first = oracle.iter().find(|entry| entry.file_id == "file_000007");
    assert_eq!(view.find_file("file_000007").unwrap().as_ref(), first);
    assert_eq!(first.unwrap().path(), "A-b");
    let legacy_last: BTreeMap<_, _> = oracle
        .iter()
        .filter(|entry| entry.kind == NamespaceEntryKind::File)
        .map(|entry| (entry.file_id.clone(), entry.version_id.clone()))
        .collect();
    assert_eq!(last_alias_versions, legacy_last);
    assert_eq!(last_alias_versions["file_000007"], "ver_last_alias");
    assert!(view.find_file("file_absent").unwrap().is_none());
    for key in ["a", "a/d", "strasse", "a-b", "missing"] {
        let mut subtree = Vec::new();
        view.walk_subtree(key, |_, entry| {
            subtree.push(entry.clone());
            Ok(true)
        })
        .unwrap();
        let prefix = format!("{key}/");
        let expected: Vec<_> = oracle
            .iter()
            .filter(|entry| entry.lookup_key() == key || entry.lookup_key().starts_with(&prefix))
            .cloned()
            .collect();
        assert_eq!(subtree, expected);
    }
}

#[test]
fn subtree_alias_search_keeps_absolute_display_path_and_stops_before_later_reads() {
    let entries = vec![
        directory("Parent", 1),
        directory("Parent/A", 2),
        file("Parent/A-b", 7),
        file("Parent/A/c", 7),
        directory("unrelated", 3),
        file("unrelated/file", 8),
    ];
    let mut objects = NamespaceObjects::new();
    let namespace = build_from_entries(&entries, &mut objects).unwrap();
    let base = deny();
    let source = DraftSource {
        fresh: &objects,
        base: &base,
    };
    let view = NamespaceView::new(&namespace, &source);
    let mut forbidden = BTreeSet::new();
    for key in ["parent/a", "unrelated"] {
        let NamespacePageEntry::Directory { subtree, .. } = view.find_key(key).unwrap().unwrap()
        else {
            panic!("fixture key must be a directory");
        };
        forbidden.insert(subtree.hash);
    }
    let restricted = LoaderSource(|reference: &NamespaceReference| {
        assert!(!forbidden.contains(&reference.hash));
        source.node(reference)
    });
    let mut first = None;
    NamespaceView::new(&namespace, &restricted)
        .walk_subtree("parent", |_, entry| {
            if entry.file_id == "file_000007" {
                first = Some(entry.clone());
                return Ok(false);
            }
            Ok(true)
        })
        .unwrap();
    assert_eq!(first.unwrap().path(), "Parent/A-b");
}

#[test]
fn first_punctuation_alias_does_not_read_later_subtrees_or_directory_width() {
    for width in [128, 1024, 10_000] {
        let mut entries = vec![directory("a", 1), file("a-b", 7), file("a/c", 7)];
        for n in 0..width {
            entries.push(file(&format!("z-{n:05}"), 1000 + n));
        }
        let mut objects = NamespaceObjects::new();
        let namespace = build_from_entries(&entries, &mut objects).unwrap();
        let base = deny();
        let source = DraftSource {
            fresh: &objects,
            base: &base,
        };
        let NamespacePageEntry::Directory { subtree, .. } = NamespaceView::new(&namespace, &source)
            .find_key("a")
            .unwrap()
            .unwrap()
        else {
            panic!("a must be a directory");
        };
        let reads = Cell::new(0);
        let restricted = LoaderSource(|reference: &NamespaceReference| {
            reads.set(reads.get() + 1);
            assert_ne!(reference, &subtree, "a-b precedes every a/ descendant");
            source.node(reference)
        });
        let found = NamespaceView::new(&namespace, &restricted)
            .find_file("file_000007")
            .unwrap()
            .unwrap();
        assert_eq!(found.path(), "a-b");
        assert!(
            reads.get() <= 5,
            "one root index route plus first page, not {width} siblings: {} reads",
            reads.get()
        );
    }
}

#[test]
fn next_index_child_is_not_read_before_an_earlier_subtree_hit() {
    // Explicit page boundaries isolate cursor scheduling from the packing
    // policy: `a` is the LAST entry of the first page, and `z` is the first
    // entry of the next page. Advancing the parent eagerly would read z.
    let mut objects = NamespaceObjects::new();
    let subtree = build_from_entries(&[file("c", 7)], &mut objects).unwrap();
    let later = build_from_entries(&[file("z", 8)], &mut objects).unwrap();
    let first_page = objects
        .page(NamespacePage {
            format: Default::default(),
            entries: vec![NamespacePageEntry::Directory {
                entry_id: "entry_directory".into(),
                name: name_policy::PortableName::parse("a").unwrap(),
                entries: 1,
                files: 1,
                subtree: subtree.root,
            }],
        })
        .unwrap();
    let root = objects
        .index(NamespaceIndex {
            format: Default::default(),
            children: vec![
                NamespaceIndexChild {
                    first_key: "a".into(),
                    entries: 2,
                    files: 1,
                    reference: first_page,
                },
                NamespaceIndexChild {
                    first_key: "z".into(),
                    entries: 1,
                    files: 1,
                    reference: later.root.clone(),
                },
            ],
        })
        .unwrap();
    let namespace = CommitNamespace { root, entries: 3 };
    let base = deny();
    let source = DraftSource {
        fresh: &objects,
        base: &base,
    };
    let restricted = LoaderSource(|reference: &NamespaceReference| {
        assert_ne!(
            reference, &later.root,
            "the later index child's first_key must stay a lazy frontier bound"
        );
        source.node(reference)
    });
    let found = NamespaceView::new(&namespace, &restricted)
        .find_file("file_000007")
        .unwrap()
        .unwrap();
    assert_eq!(found.path(), "a/c");
}

#[test]
fn ordered_walk_preserves_reader_and_callback_errors_without_read_ahead() {
    let entries = vec![directory("a", 1), file("a-b", 7), file("a/c", 7)];
    let mut objects = NamespaceObjects::new();
    let namespace = build_from_entries(&entries, &mut objects).unwrap();
    let base = deny();
    let source = DraftSource {
        fresh: &objects,
        base: &base,
    };
    let restricted = LoaderSource(|reference: &NamespaceReference| {
        if reference != &namespace.root {
            return Err(Error::new(ErrorCode::ObjectCorrupt, "deferred read failed"));
        }
        source.node(reference)
    });
    let view = NamespaceView::new(&namespace, &restricted);
    assert_eq!(
        view.find_file("file_000007").unwrap().unwrap().path(),
        "a-b"
    );
    let error = view.walk(|_, _| Ok(true)).unwrap_err();
    assert_eq!(error.code(), ErrorCode::ObjectCorrupt);
    assert!(error.to_string().contains("deferred read failed"));
    let error = view
        .walk(|_, _| Err(Error::new(ErrorCode::StateInvalid, "visitor failed")))
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::StateInvalid);
    assert!(error.to_string().contains("visitor failed"));
}
