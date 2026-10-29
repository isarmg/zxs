//! Page-local editing work budgets and independent logical-tree oracles.
//!
//! Encoding work is measured before object-address deduplication. These tests
//! deliberately include directories wider than the namespace cache and do not
//! infer edit locality from retained hashes alone.
use super::*;
use std::cell::RefCell;

#[derive(Default, Debug)]
struct ReadWork {
    calls: usize,
    bytes: usize,
    addresses: BTreeSet<(NamespaceRefKind, String)>,
}

struct MeasuredSource<'a, S: ?Sized> {
    inner: &'a S,
    lengths: BTreeMap<&'a str, usize>,
    work: RefCell<ReadWork>,
}

impl<S: NamespaceSource + ?Sized> NamespaceSource for MeasuredSource<'_, S> {
    fn node(&self, reference: &NamespaceReference) -> Result<NamespaceNode> {
        let node = self.inner.node(reference)?;
        let mut work = self.work.borrow_mut();
        work.calls += 1;
        work.bytes += self.lengths[reference.hash.as_str()];
        work.addresses
            .insert((reference.kind, reference.hash.clone()));
        Ok(node)
    }
}

#[derive(Clone)]
enum Edit {
    Put(Entry),
    Remove(String),
    Rename { from: String, to: Entry },
}

fn edit(
    root: &CommitNamespace,
    objects: &NamespaceObjects,
    edits: &[Edit],
) -> (CommitNamespace, NamespaceObjects, ReadWork) {
    let missing = deny();
    let source = DraftSource {
        fresh: objects,
        base: &missing,
    };
    let measured = MeasuredSource {
        inner: &source,
        lengths: objects
            .objects()
            .iter()
            .map(|object| (object.hash.as_str(), object.bytes.len()))
            .collect(),
        work: RefCell::default(),
    };
    let mut editor = NamespaceEditor::open(root, &measured).unwrap();
    for edit in edits {
        match edit {
            Edit::Put(entry) => editor.upsert_file(entry).unwrap(),
            Edit::Remove(key) => editor.delete(key, false).unwrap(),
            Edit::Rename { from, to } => editor.rename(from, to).unwrap(),
        }
    }
    let mut fresh = NamespaceObjects::new();
    let root = editor.seal(&mut fresh).unwrap();
    (root, fresh, measured.work.into_inner())
}

fn oracle_apply(oracle: &mut BTreeMap<String, Entry>, edit: &Edit) {
    match edit {
        Edit::Put(entry) => {
            oracle.insert(entry.lookup_key().into(), entry.clone());
        }
        Edit::Remove(key) => {
            assert!(oracle.remove(key).is_some());
        }
        Edit::Rename { from, to } => {
            let mut entry = oracle.remove(from).unwrap();
            entry.relocate(to.path.clone());
            assert!(oracle.insert(entry.lookup_key().into(), entry).is_none());
        }
    }
}

fn oracle(entries: &[Entry]) -> BTreeMap<String, Entry> {
    entries
        .iter()
        .map(|entry| (entry.lookup_key().to_owned(), entry.clone()))
        .collect()
}

fn assert_logical_tree(
    root: &CommitNamespace,
    fresh: &NamespaceObjects,
    base: &NamespaceObjects,
    expected: &BTreeMap<String, Entry>,
) {
    let missing = deny();
    let base_source = DraftSource {
        fresh: base,
        base: &missing,
    };
    let source = DraftSource {
        fresh,
        base: &base_source,
    };
    let actual = NamespaceView::new(root, &source).collect_entries().unwrap();
    assert_eq!(actual, expected.values().cloned().collect::<Vec<_>>());
    assert_eq!(root.entries, expected.len() as u64);
    assert_eq!(
        verify_namespace(root, &source, expected.len() as u64).unwrap(),
        expected.len() as u64
    );
    assert_balanced_directory(&root.root, &source, true);
}

#[derive(Default)]
struct ShapeSummary {
    height: usize,
    first_key: Option<String>,
    entries: u64,
    files: u64,
}

/// Independently check structural balance, first-key routing, recorded totals
/// and wire bounds, not just whether the production verifier accepts the graph.
fn assert_balanced_directory(
    reference: &NamespaceReference,
    source: &(impl NamespaceSource + ?Sized),
    root: bool,
) -> usize {
    assert_shape(reference, source, root).height
}

fn assert_shape(
    reference: &NamespaceReference,
    source: &(impl NamespaceSource + ?Sized),
    root: bool,
) -> ShapeSummary {
    match source.node(reference).unwrap() {
        NamespaceNode::Page(page) => {
            assert!(page.entries.len() <= NAMESPACE_PAGE_ENTRIES);
            assert!(root || !page.entries.is_empty());
            assert!(
                page.entries.len() <= 1
                    || canonical_json(page.as_ref()).unwrap().len() <= NAMESPACE_PAGE_TARGET_BYTES
            );
            for pair in page.entries.windows(2) {
                assert!(pair[0].lookup_key() < pair[1].lookup_key());
            }
            let mut summary = ShapeSummary {
                height: 1,
                first_key: page
                    .entries
                    .first()
                    .map(|entry| entry.lookup_key().to_owned()),
                ..Default::default()
            };
            for entry in &page.entries {
                summary.entries += 1;
                match entry {
                    NamespacePageEntry::File { .. } => summary.files += 1,
                    NamespacePageEntry::Directory {
                        subtree,
                        entries,
                        files,
                        ..
                    } => {
                        let subtree = assert_shape(subtree, source, true);
                        assert_eq!(*entries, subtree.entries);
                        assert_eq!(*files, subtree.files);
                        summary.entries += subtree.entries;
                        summary.files += subtree.files;
                    }
                }
            }
            summary
        }
        NamespaceNode::Index(index) => {
            assert!(!index.children.is_empty());
            assert!(index.children.len() <= NAMESPACE_INDEX_FANOUT);
            assert!(!root || index.children.len() >= 2);
            assert!(root || index.children.len() >= NAMESPACE_INDEX_FANOUT / 2);
            for pair in index.children.windows(2) {
                assert!(pair[0].first_key < pair[1].first_key);
            }
            let mut summary = ShapeSummary::default();
            for child in &index.children {
                let verified = assert_shape(&child.reference, source, false);
                assert_eq!(
                    verified.first_key.as_deref(),
                    Some(child.first_key.as_str())
                );
                assert_eq!(verified.entries, child.entries);
                assert_eq!(verified.files, child.files);
                if summary.height == 0 {
                    summary.height = verified.height + 1;
                    summary.first_key = verified.first_key;
                } else {
                    assert_eq!(summary.height, verified.height + 1);
                }
                summary.entries += verified.entries;
                summary.files += verified.files;
            }
            assert!(summary.height <= storage_contract::NAMESPACE_INDEX_DEPTH);
            summary
        }
    }
}

fn assert_point_work(label: &str, objects: &NamespaceObjects, read: &ReadWork) {
    let (encoded_nodes, encoded_bytes) = objects.encoding_work();
    eprintln!(
        "namespace point {label}: reads={} read_bytes={} encoded_nodes={} encoded_bytes={} installed_nodes={}",
        read.calls,
        read.bytes,
        encoded_nodes,
        encoded_bytes,
        objects.objects().len()
    );
    assert!(read.calls <= 24, "{label}: {read:?}");
    assert!(read.bytes <= 24 * NAMESPACE_PAGE_TARGET_BYTES, "{label}");
    assert!(encoded_nodes <= 12, "{label}: encoded {encoded_nodes}");
    assert!(encoded_bytes <= 12 * NAMESPACE_PAGE_TARGET_BYTES, "{label}");
    assert!(objects.objects().len() <= encoded_nodes);
}

fn wide_entries(width: usize) -> Vec<Entry> {
    (0..width)
        .map(|n| file(&format!("f{:06}", n * 2), n))
        .collect()
}

#[test]
fn wide_directory_first_middle_last_edits_have_bounded_actual_work() {
    for width in [1_000, 10_000, 100_000] {
        let entries = wide_entries(width);
        let mut objects = NamespaceObjects::new();
        let root = build_from_entries(&entries, &mut objects).unwrap();
        for position in [0, width / 2, width - 1] {
            let insert_name = match position {
                0 => "a".to_owned(),
                position if position == width - 1 => "z".to_owned(),
                position => format!("f{:06}a", position * 2),
            };
            let mut replacement = entries[position].clone();
            replacement.version_id = format!("ver_replaced_{position:06}");
            replacement.size += 1;
            let edits = [
                Edit::Put(file(&insert_name, width + position)),
                Edit::Remove(entries[position].lookup_key().into()),
                Edit::Put(replacement),
            ];
            for (operation, change) in edits.into_iter().enumerate() {
                let label = format!("width={width}, position={position}, operation={operation}");
                let mut expected = oracle(&entries);
                oracle_apply(&mut expected, &change);
                let (edited, fresh, read) = edit(&root, &objects, &[change]);
                assert_point_work(&label, &fresh, &read);
                assert_logical_tree(&edited, &fresh, &objects, &expected);
            }
        }
    }
}

#[test]
fn repeated_edits_to_one_key_encode_its_final_route_only_once() {
    let entries = wide_entries(10_000);
    let mut objects = NamespaceObjects::new();
    let root = build_from_entries(&entries, &mut objects).unwrap();
    let changes: Vec<_> = (0..512)
        .map(|generation| {
            let mut updated = entries[5_000].clone();
            updated.version_id = format!("ver_generation_{generation}");
            updated.size = generation;
            Edit::Put(updated)
        })
        .collect();
    let (edited, fresh, read) = edit(&root, &objects, &changes);
    assert_point_work("512 updates of the same key", &fresh, &read);
    let (once, once_fresh, once_read) = edit(&root, &objects, &changes[511..]);
    assert_eq!(edited, once);
    assert_eq!(fresh.encoding_work(), once_fresh.encoding_work());
    assert_eq!(read.calls, once_read.calls);
    let mut expected = oracle(&entries);
    oracle_apply(&mut expected, &changes[511]);
    assert_logical_tree(&edited, &fresh, &objects, &expected);
}

#[test]
fn page_and_index_splits_and_merges_preserve_balance_and_the_oracle() {
    let entries = wide_entries(NAMESPACE_PAGE_ENTRIES * NAMESPACE_INDEX_FANOUT);
    let mut objects = NamespaceObjects::new();
    let root = build_from_entries(&entries, &mut objects).unwrap();
    let missing = deny();
    assert_eq!(
        assert_balanced_directory(
            &root.root,
            &DraftSource {
                fresh: &objects,
                base: &missing,
            },
            true,
        ),
        2
    );
    let inserted = Edit::Put(file("a", entries.len()));
    let mut expected = oracle(&entries);
    oracle_apply(&mut expected, &inserted);
    let (split, fresh, read) = edit(&root, &objects, &[inserted]);
    assert_point_work("leaf and root-index split", &fresh, &read);
    assert_logical_tree(&split, &fresh, &objects, &expected);
    objects.absorb(fresh);
    assert_eq!(
        assert_balanced_directory(
            &split.root,
            &DraftSource {
                fresh: &objects,
                base: &missing,
            },
            true,
        ),
        3
    );
    let removed = Edit::Remove("a".into());
    oracle_apply(&mut expected, &removed);
    let (merged, fresh, read) = edit(&split, &objects, &[removed]);
    assert_point_work("leaf and index merge", &fresh, &read);
    assert_logical_tree(&merged, &fresh, &objects, &expected);
    objects.absorb(fresh);
    assert_eq!(
        assert_balanced_directory(
            &merged.root,
            &DraftSource {
                fresh: &objects,
                base: &missing,
            },
            true,
        ),
        2
    );
}

#[test]
fn legal_variable_length_names_split_on_bytes_before_entry_count() {
    let mut entries = Vec::new();
    let mut page_entries = Vec::new();
    for tag in 0..NAMESPACE_PAGE_ENTRIES {
        let entry = file(&format!("f{tag:04}{}", "あ".repeat(78)), tag);
        let candidate = NamespacePageEntry::File {
            entry_id: entry.entry_id.clone(),
            name: entry.location().file_component(),
            file_id: entry.file_id.clone(),
            version_id: entry.version_id.clone(),
            size: entry.size,
        };
        page_entries.push(candidate);
        let bytes = canonical_json(&NamespacePage {
            format: Default::default(),
            entries: page_entries.clone(),
        })
        .unwrap()
        .len();
        if bytes > NAMESPACE_PAGE_TARGET_BYTES {
            break;
        }
        entries.push(entry);
    }
    assert!(entries.len() < NAMESPACE_PAGE_ENTRIES);
    let mut objects = NamespaceObjects::new();
    let root = build_from_entries(&entries, &mut objects).unwrap();
    assert_eq!(root.root.kind, NamespaceRefKind::Page);
    let inserted = Edit::Put(file(&format!("a00{}", "あ".repeat(79)), entries.len()));
    let mut expected = oracle(&entries);
    oracle_apply(&mut expected, &inserted);
    let (split, fresh, read) = edit(&root, &objects, &[inserted]);
    assert_point_work("byte-limited split", &fresh, &read);
    assert_eq!(split.root.kind, NamespaceRefKind::Index);
    assert_logical_tree(&split, &fresh, &objects, &expected);
}

#[test]
fn deep_wide_ancestors_only_encode_the_edited_path() {
    const DEPTH: usize = 12;
    let mut entries = Vec::new();
    let mut prefix = String::new();
    for depth in 0..DEPTH {
        for n in 0..1_000 {
            let path = if prefix.is_empty() {
                format!("f{n:04}")
            } else {
                format!("{prefix}/f{n:04}")
            };
            entries.push(file(&path, depth * 1_000 + n));
        }
        prefix = if prefix.is_empty() {
            "d".into()
        } else {
            format!("{prefix}/d")
        };
        entries.push(directory(&prefix, 100_000 + depth));
    }
    let target = file(&format!("{prefix}/target"), DEPTH * 1_000);
    entries.push(target.clone());
    let mut objects = NamespaceObjects::new();
    let root = build_from_entries(&entries, &mut objects).unwrap();
    let mut replaced = target;
    replaced.version_id = "ver_deep_replaced".into();
    let change = Edit::Put(replaced);
    let mut expected = oracle(&entries);
    oracle_apply(&mut expected, &change);
    let (edited, fresh, read) = edit(&root, &objects, &[change]);
    let (encoded_nodes, encoded_bytes) = fresh.encoding_work();
    assert!(read.calls <= 8 * (DEPTH + 1), "{read:?}");
    assert!(encoded_nodes <= 3 * (DEPTH + 1));
    assert!(encoded_bytes <= 3 * (DEPTH + 1) * NAMESPACE_PAGE_TARGET_BYTES);
    assert_logical_tree(&edited, &fresh, &objects, &expected);
}

#[test]
fn fixed_seed_mixed_point_edits_match_an_independent_flat_map() {
    let entries = wide_entries(1_000);
    let mut expected = oracle(&entries);
    let mut objects = NamespaceObjects::new();
    let mut root = build_from_entries(&entries, &mut objects).unwrap();
    let mut random = 0xa75d_31c8_294b_6e02u64;
    for round in 0..128 {
        let mut changes = Vec::new();
        for offset in 0..8 {
            random ^= random << 13;
            random ^= random >> 7;
            random ^= random << 17;
            let tag = (random % 2_000) as usize;
            let key = format!("f{tag:06}");
            let change = match (random % 3, expected.get(&key)) {
                (0, Some(_)) => Edit::Remove(key),
                (1, Some(entry)) => {
                    let destination = format!("m{round:04}-{offset}");
                    let mut to = entry.clone();
                    to.relocate(name_policy::PortablePath::parse(&destination).unwrap());
                    Edit::Rename { from: key, to }
                }
                _ => {
                    let mut entry = file(&key, 10_000 + round * 8 + offset);
                    entry.size = random;
                    Edit::Put(entry)
                }
            };
            oracle_apply(&mut expected, &change);
            changes.push(change);
        }
        let (next, fresh, read) = edit(&root, &objects, &changes);
        let (nodes, bytes) = fresh.encoding_work();
        assert!(read.calls <= changes.len() * 24, "round {round}: {read:?}");
        assert!(nodes <= changes.len() * 12, "round {round}");
        assert!(bytes <= changes.len() * 12 * NAMESPACE_PAGE_TARGET_BYTES);
        assert_logical_tree(&next, &fresh, &objects, &expected);
        objects.absorb(fresh);
        root = next;
    }
}

#[test]
fn directory_rename_reads_no_descendant_pages() {
    let mut entries = vec![directory("wide", 100_001)];
    for n in 0..10_000 {
        entries.push(file(&format!("wide/f{n:06}"), n));
    }
    let mut objects = NamespaceObjects::new();
    let root = build_from_entries(&entries, &mut objects).unwrap();
    let destination = directory("renamed", 100_001);
    let change = Edit::Rename {
        from: "wide".into(),
        to: destination.clone(),
    };
    let mut expected = BTreeMap::new();
    for mut entry in entries {
        if entry.path() == "wide" {
            entry.relocate(destination.path.clone());
        } else {
            entry.relocate(entry.location().rebase(1, &destination.path).unwrap());
        }
        expected.insert(entry.lookup_key().into(), entry);
    }
    let (renamed, fresh, read) = edit(&root, &objects, &[change]);
    assert!(read.calls <= 3, "rename read descendant pages: {read:?}");
    assert_eq!(
        read.addresses,
        BTreeSet::from([(root.root.kind, root.root.hash.clone())])
    );
    assert_eq!(fresh.encoding_work().0, 1);
    assert_logical_tree(&renamed, &fresh, &objects, &expected);
}

#[test]
fn encoding_work_counts_duplicate_encoding_before_address_deduplication() {
    let page = NamespacePage {
        format: Default::default(),
        entries: Vec::new(),
    };
    let mut objects = NamespaceObjects::new();
    let first = objects.page(page.clone()).unwrap();
    let second = objects.page(page).unwrap();
    assert_eq!(first, second);
    assert_eq!(objects.objects().len(), 1);
    assert_eq!(
        objects.encoding_work(),
        (2, objects.objects()[0].bytes.len() * 2)
    );
    let previous = objects.encoding_work();
    let mut combined = NamespaceObjects::new();
    combined.absorb(objects);
    assert_eq!(combined.encoding_work(), previous);
    assert_eq!(combined.take().len(), 1);
    assert_eq!(combined.encoding_work(), (0, 0));
}

#[test]
fn batched_deletions_collapse_to_one_page_and_then_an_empty_root() {
    let entries = wide_entries(256);
    let mut expected = oracle(&entries);
    let mut objects = NamespaceObjects::new();
    let root = build_from_entries(&entries, &mut objects).unwrap();
    let removed: Vec<_> = entries[..192]
        .iter()
        .map(|entry| Edit::Remove(entry.lookup_key().into()))
        .collect();
    for change in &removed {
        oracle_apply(&mut expected, change);
    }
    let (collapsed, fresh, _) = edit(&root, &objects, &removed);
    assert_eq!(collapsed.root.kind, NamespaceRefKind::Page);
    assert_logical_tree(&collapsed, &fresh, &objects, &expected);
    objects.absorb(fresh);
    let removed: Vec<_> = entries[192..]
        .iter()
        .map(|entry| Edit::Remove(entry.lookup_key().into()))
        .collect();
    let (empty, fresh, _) = edit(&collapsed, &objects, &removed);
    assert_eq!(empty.root.kind, NamespaceRefKind::Page);
    assert_eq!(empty.entries, 0);
    assert_logical_tree(&empty, &fresh, &objects, &BTreeMap::new());
}

#[test]
fn large_middle_replacements_split_locally_with_the_single_entry_exception() {
    let entries = wide_entries(NAMESPACE_PAGE_ENTRIES);
    let mut objects = NamespaceObjects::new();
    let root = build_from_entries(&entries, &mut objects).unwrap();
    assert_eq!(root.root.kind, NamespaceRefKind::Page);
    for size in [60 * 1024, NAMESPACE_PAGE_TARGET_BYTES] {
        // This is a layout-only fixture. Logical identity admission validates
        // real version IDs separately; arbitrary ID lengths exercise the
        // page format's explicit oversized-single-entry rule.
        let mut replacement = entries[NAMESPACE_PAGE_ENTRIES / 2].clone();
        replacement.version_id = "v".repeat(size);
        let change = Edit::Put(replacement);
        let mut expected = oracle(&entries);
        oracle_apply(&mut expected, &change);
        let (split, fresh, read) = edit(&root, &objects, &[change]);
        assert_point_work("three-way byte split", &fresh, &read);
        assert_logical_tree(&split, &fresh, &objects, &expected);
        let missing = deny();
        let base = DraftSource {
            fresh: &objects,
            base: &missing,
        };
        let source = DraftSource {
            fresh: &fresh,
            base: &base,
        };
        let NamespaceNode::Index(index) = source.node(&split.root).unwrap() else {
            panic!("large middle replacement must split the page");
        };
        assert_eq!(index.children.len(), 3);
        let NamespaceNode::Page(middle) = source.node(&index.children[1].reference).unwrap() else {
            panic!("the split root must reference leaf pages");
        };
        assert_eq!(middle.entries.len(), 1);
        let bytes = canonical_json(middle.as_ref()).unwrap().len();
        assert_eq!(
            bytes > NAMESPACE_PAGE_TARGET_BYTES,
            size == NAMESPACE_PAGE_TARGET_BYTES
        );
    }
}

#[test]
fn deleting_from_sparse_wire_indexes_contracts_promoted_sealed_roots() {
    let entries = [file("a", 1), file("b", 2)];
    let mut objects = NamespaceObjects::new();
    let mut children = Vec::new();
    for entry in &entries {
        let page = objects
            .page(NamespacePage {
                format: Default::default(),
                entries: vec![NamespacePageEntry::File {
                    entry_id: entry.entry_id.clone(),
                    name: entry.location().file_component(),
                    file_id: entry.file_id.clone(),
                    version_id: entry.version_id.clone(),
                    size: entry.size,
                }],
            })
            .unwrap();
        let index = objects
            .index(NamespaceIndex {
                format: Default::default(),
                children: vec![NamespaceIndexChild {
                    first_key: entry.lookup_key().into(),
                    entries: 1,
                    files: 1,
                    reference: page,
                }],
            })
            .unwrap();
        children.push(NamespaceIndexChild {
            first_key: entry.lookup_key().into(),
            entries: 1,
            files: 1,
            reference: index,
        });
    }
    // Wire admission permits sparse interior indexes; only the root needs
    // two children. Do not mistake builder occupancy for a wire requirement.
    let root = CommitNamespace {
        root: objects
            .index(NamespaceIndex {
                format: Default::default(),
                children,
            })
            .unwrap(),
        entries: 2,
    };
    let missing = deny();
    assert_eq!(
        verify_namespace(
            &root,
            &DraftSource {
                fresh: &objects,
                base: &missing,
            },
            2,
        )
        .unwrap(),
        2
    );
    let (collapsed, fresh, read) = edit(&root, &objects, &[Edit::Remove("a".into())]);
    assert_eq!(collapsed.root.kind, NamespaceRefKind::Page);
    assert_point_work("promoted sealed singleton index", &fresh, &read);
    assert_logical_tree(&collapsed, &fresh, &objects, &oracle(&entries[1..]));
}

fn rename_operation(
    root: &CommitNamespace,
    objects: &NamespaceObjects,
    operation: &NamespaceOperation,
) -> (Result<CommitNamespace>, NamespaceObjects, ReadWork) {
    let missing = deny();
    let source = DraftSource {
        fresh: objects,
        base: &missing,
    };
    let measured = MeasuredSource {
        inner: &source,
        lengths: objects
            .objects()
            .iter()
            .map(|object| (object.hash.as_str(), object.bytes.len()))
            .collect(),
        work: RefCell::default(),
    };
    let mut editor = NamespaceEditor::open(root, &measured).unwrap();
    let mut fresh = NamespaceObjects::new();
    let result = match editor.apply(operation) {
        Ok(_) => editor.seal(&mut fresh),
        Err(error) => Err(error),
    };
    (result, fresh, measured.work.into_inner())
}

#[test]
fn a_non_growing_directory_rename_operation_reuses_admitted_path_budgets() {
    let mut entries = vec![directory("long-directory", 100_001)];
    entries.extend((0..10_000).map(|n| file(&format!("long-directory/f{n:06}"), n)));
    let mut objects = NamespaceObjects::new();
    let root = build_from_entries(&entries, &mut objects).unwrap();
    let operation = NamespaceOperation::parse(
        storage_contract::NamespaceOperationKind::Rename,
        "long-directory",
        Some("x"),
    )
    .unwrap();
    let (result, fresh, read) = rename_operation(&root, &objects, &operation);
    let renamed = result.unwrap();
    assert_eq!(
        read.addresses,
        BTreeSet::from([(root.root.kind, root.root.hash.clone())])
    );
    assert!(read.calls <= 3);
    assert_eq!(fresh.encoding_work().0, 1);
    let mut expected = entries;
    let base = expected.clone();
    crate::business::apply_workspace_namespace(
        &mut expected,
        &base,
        std::slice::from_ref(&operation),
    )
    .unwrap();
    assert_logical_tree(&renamed, &fresh, &objects, &oracle(&expected));
}

#[test]
fn growing_rename_prefixes_still_check_descendants_using_stored_spelling() {
    let long_leaf = "x".repeat(193);
    let cases = [
        // Same key and byte length, but the actual stored spelling has one
        // fewer UTF-16 unit than the lookup spelling supplied by the caller.
        (
            "Straße",
            "STRASSE",
            "STRASSE",
            format!("Straße/{long_leaf}"),
        ),
        // A longer ordinary prefix must also reject a formerly valid path.
        ("a", "a", "longer", format!("a/{}", "x".repeat(198))),
    ];
    for (stored, lookup, target, file_path) in cases {
        let entries = vec![directory(stored, 100_001), file(&file_path, 1)];
        let mut objects = NamespaceObjects::new();
        let root = build_from_entries(&entries, &mut objects).unwrap();
        let operation = NamespaceOperation::parse(
            storage_contract::NamespaceOperationKind::Rename,
            lookup,
            Some(target),
        )
        .unwrap();
        let mut expected = entries;
        let base = expected.clone();
        let expected_error = crate::business::apply_workspace_namespace(
            &mut expected,
            &base,
            std::slice::from_ref(&operation),
        )
        .unwrap_err();
        let (result, fresh, read) = rename_operation(&root, &objects, &operation);
        let error = result.unwrap_err();
        assert_eq!(error.code(), expected_error.code());
        assert_eq!(error.to_string(), expected_error.to_string());
        assert!(
            read.addresses.len() > 1,
            "growing prefix skipped its subtree"
        );
        assert_eq!(fresh.encoding_work(), (0, 0));
    }
}

#[test]
fn deeper_rename_prefixes_preserve_the_descendant_depth_limit() {
    let mut entries = vec![directory("a", 100_001)];
    let mut path = "a".to_owned();
    for depth in 1..31 {
        path.push_str("/d");
        entries.push(directory(&path, 100_001 + depth));
    }
    entries.push(file(&format!("{path}/f"), 1));
    let mut objects = NamespaceObjects::new();
    let root = build_from_entries(&entries, &mut objects).unwrap();
    let operation = NamespaceOperation::parse(
        storage_contract::NamespaceOperationKind::Rename,
        "a",
        Some("x/y"),
    )
    .unwrap();
    let base = entries.clone();
    let expected_error = crate::business::apply_workspace_namespace(
        &mut entries,
        &base,
        std::slice::from_ref(&operation),
    )
    .unwrap_err();
    let (result, fresh, read) = rename_operation(&root, &objects, &operation);
    let error = result.unwrap_err();
    assert_eq!(error.code(), expected_error.code());
    assert_eq!(error.to_string(), expected_error.to_string());
    assert!(read.addresses.len() > 1);
    assert_eq!(fresh.encoding_work(), (0, 0));
}
