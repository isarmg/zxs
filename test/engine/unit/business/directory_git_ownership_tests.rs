//! Differential ownership checks use the previous copying algorithm only here.
use super::*;

fn entry(id: &str, path: &str, kind: NamespaceEntryKind, file: &str) -> Entry {
    Entry {
        entry_id: id.into(),
        path: name_policy::PortablePath::parse(path).unwrap(),
        kind,
        file_id: file.into(),
        version_id: format!("version_{id}"),
        size: 3,
    }
}

fn tree() -> DirectoryTree {
    let mut entries = vec![
        entry("child", "Root/Å", NamespaceEntryKind::Directory, ""),
        entry(
            "child_file",
            "Root/Å/file",
            NamespaceEntryKind::File,
            "shared",
        ),
        entry("sibling", "Root/Åland", NamespaceEntryKind::Directory, ""),
        entry(
            "owned",
            "Root/Åland/file",
            NamespaceEntryKind::File,
            "shared",
        ),
        entry(
            "ordinary",
            "Root/ordinary",
            NamespaceEntryKind::Directory,
            "",
        ),
        entry(
            "nested",
            "Root/ordinary/嵌套",
            NamespaceEntryKind::Directory,
            "",
        ),
        entry(
            "nested_file",
            "Root/ordinary/嵌套/file",
            NamespaceEntryKind::File,
            "nested_file",
        ),
        entry("plain", "Root/plain", NamespaceEntryKind::File, "plain"),
    ];
    entries.sort_by(|a, b| a.lookup_key().cmp(b.lookup_key()));
    let file_contexts = entries
        .iter()
        .filter(|entry| entry.kind == NamespaceEntryKind::File)
        .map(|entry| {
            (
                entry.lookup_key().into(),
                FileContext {
                    branch_id: format!("branch_{}", entry.entry_id),
                    node_id: Some(format!("node_{}", entry.entry_id)),
                    selection_generation: 7,
                    inherited_from: None,
                },
            )
        })
        .collect();
    DirectoryTree {
        tree_id: "tree".into(),
        directory_branch_id: "branch".into(),
        parent_tree_id: None,
        commit_id: "commit".into(),
        path: "Root".into(),
        entries,
        file_contexts,
        file_branches: BTreeMap::new(),
        inherited_files: BTreeMap::new(),
        directory_refs: BTreeMap::from([(
            "child".into(),
            DirectoryReference {
                directory_branch_id: "child_branch".into(),
                tree_id: "child_tree".into(),
            },
        )]),
        sequence: 8,
    }
}

#[derive(PartialEq)]
struct ReferenceEntry {
    entry: Entry,
    node: Option<String>,
    branch: Option<String>,
}

fn reference_owned(
    tree: &DirectoryTree,
    boundaries: &BTreeSet<&str>,
) -> BTreeMap<String, ReferenceEntry> {
    let blocked = tree
        .entries
        .iter()
        .filter(|entry| {
            entry.kind == NamespaceEntryKind::Directory
                && boundaries.contains(entry.entry_id.as_str())
        })
        .map(Entry::lookup_key)
        .collect::<Vec<_>>();
    tree.entries
        .iter()
        .filter(|entry| {
            !blocked.iter().any(|path| {
                entry.lookup_key() == *path || namespace::is_descendant(entry.lookup_key(), path)
            })
        })
        .map(|entry| {
            let context = tree.file_contexts.get(entry.lookup_key());
            let mut rebased = entry.clone();
            let path = directory::rebase_path(entry.path(), &tree.path, "").unwrap();
            rebased.relocate(name_policy::PortablePath::parse(&path).unwrap());
            (
                rebased.lookup_key().into(),
                ReferenceEntry {
                    entry: rebased,
                    node: context.and_then(|context| context.node_id.clone()),
                    branch: context.map(|context| context.branch_id.clone()),
                },
            )
        })
        .collect()
}

#[test]
fn borrowed_owned_comparison_matches_the_copying_oracle() {
    let before = tree();
    for mask in 0..4 {
        let boundaries = ["child", "nested"]
            .into_iter()
            .enumerate()
            .filter(|(index, _)| mask & (1 << index) != 0)
            .map(|(_, id)| id)
            .collect::<BTreeSet<_>>();
        for changed in ["owned", "child_file", "nested_file"] {
            for field in 0..11 {
                let mut after = before.clone();
                let entry = after
                    .entries
                    .iter_mut()
                    .find(|entry| entry.entry_id == changed)
                    .unwrap();
                let context = after.file_contexts.get_mut(entry.lookup_key()).unwrap();
                match field {
                    0 => entry.entry_id.push('x'),
                    1 => entry.file_id.push('x'),
                    2 => entry.version_id.push('x'),
                    3 => entry.size += 1,
                    4 => entry.kind = NamespaceEntryKind::Directory,
                    5 => context.branch_id.push('x'),
                    6 => context.node_id = Some("other_node".into()),
                    7 => context.node_id = None,
                    8 => context.selection_generation += 1,
                    9 => context.inherited_from = Some("other_origin".into()),
                    10 => entry.relocate(
                        name_policy::PortablePath::parse(&entry.path().replace("file", "FILE"))
                            .unwrap(),
                    ),
                    _ => unreachable!(),
                }
                after
                    .entries
                    .sort_by(|a, b| a.lookup_key().cmp(b.lookup_key()));
                let expected =
                    reference_owned(&before, &boundaries) == reference_owned(&after, &boundaries);
                let ignored = (changed == "child_file" && boundaries.contains("child"))
                    || (changed == "nested_file" && boundaries.contains("nested"))
                    || matches!(field, 8 | 9);
                assert_eq!(expected, ignored, "oracle {mask} {changed} {field}");
                assert_eq!(
                    owned(&before, &boundaries) == owned(&after, &boundaries),
                    expected,
                    "borrowed {mask} {changed} {field}"
                );
            }
        }
    }
}

#[test]
fn child_boundaries_follow_snapshot_identities_and_exact_components() {
    let tree = tree();
    let children = ChildBoundaries::new(&tree.entries, |id| {
        ["child", "nested", "plain"].contains(&id)
    });
    for path in [
        "Root/Å",
        "Root/Å/file",
        "Root/ordinary/嵌套",
        "Root/ordinary/嵌套/file",
    ] {
        assert!(children.contains(&name_policy::path_key(path)), "{path}");
    }
    for path in [
        "",
        "Root",
        "Root/Åland",
        "Root/Åland/file",
        "Root/ordinary",
        "Root/plain",
    ] {
        assert!(!children.contains(&name_policy::path_key(path)), "{path}");
    }
    let boundaries = BTreeMap::from([("child".into(), "a later unrelated working path".into())]);
    let retained = staged_owned_files(&tree, &boundaries)
        .map(|entry| entry.entry_id.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(retained, BTreeSet::from(["nested_file", "owned", "plain"]));
    assert!(rename::contains_owned_file(
        &tree,
        &BTreeSet::from(["shared"])
    ));
    let mut child_only = tree.clone();
    child_only.entries.retain(|entry| entry.entry_id != "owned");
    assert!(!rename::contains_owned_file(
        &child_only,
        &BTreeSet::from(["shared"])
    ));
}

#[test]
fn child_range_matches_strict_descendant_checks() {
    let tree = tree();
    let ids = ["child", "sibling", "nested"];
    for mask in 0..8 {
        let selected = ids
            .into_iter()
            .enumerate()
            .filter(|(index, _)| mask & (1 << index) != 0)
            .map(|(_, id)| id)
            .collect::<BTreeSet<_>>();
        let children = ChildBoundaries::new(&tree.entries, |id| selected.contains(id));
        let keys = tree.entries.iter().map(Entry::lookup_key).chain([
            "",
            "root",
            "root/missing",
            "root/åa",
        ]);
        for key in keys {
            let expected = tree.entries.iter().any(|entry| {
                entry.kind == NamespaceEntryKind::Directory
                    && selected.contains(entry.entry_id.as_str())
                    && namespace::is_descendant(entry.lookup_key(), key)
            });
            assert_eq!(children.has_descendant(key), expected, "{mask} {key}");
        }
    }
}

#[test]
fn owned_review_ignores_entry_order_and_ancestor_display_spelling() {
    let before = tree();
    let mut after = before.clone();
    after.entries.reverse();
    for entry in &mut after.entries {
        entry.relocate(
            name_policy::PortablePath::parse(&entry.path().replacen("Root/", "ROOT/", 1)).unwrap(),
        );
    }
    let boundaries = BTreeSet::from(["child"]);
    assert!(reference_owned(&before, &boundaries) == reference_owned(&after, &boundaries));
    assert!(owned(&before, &boundaries) == owned(&after, &boundaries));
}
