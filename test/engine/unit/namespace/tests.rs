//! Paged namespace unit coverage: deterministic node encoding, path-copy edit
//! locality, index-first point/page reads, full admission re-verification,
//! and editor semantics mirroring the legacy flat-list operations.
use super::*;
use std::cell::Cell;
use storage_contract::NamespaceOperation;

#[path = "borrowed_lookup_tests.rs"]
mod borrowed_lookup_tests;
#[path = "composition_path_tests.rs"]
mod composition_path_tests;
#[path = "depth_bound_tests.rs"]
mod depth_bound_tests;
#[path = "object_loader_memo_tests.rs"]
mod object_loader_memo_tests;
#[path = "ordered_walk_tests.rs"]
mod ordered_walk_tests;
#[path = "owned_decode_tests.rs"]
mod owned_decode_tests;
#[path = "page_local_tests.rs"]
mod page_local_tests;

fn missing(_: &NamespaceReference) -> Result<NamespaceNode> {
    Err(Error::new(
        ErrorCode::ObjectCorrupt,
        "namespace object is absent from the test object set",
    ))
}

fn deny() -> LoaderSource<fn(&NamespaceReference) -> Result<NamespaceNode>> {
    LoaderSource(missing)
}

struct CountingSource<'a, S: ?Sized> {
    inner: &'a S,
    reads: Cell<usize>,
}
impl<S: NamespaceSource + ?Sized> NamespaceSource for CountingSource<'_, S> {
    fn node(&self, reference: &NamespaceReference) -> Result<NamespaceNode> {
        self.reads.set(self.reads.get() + 1);
        self.inner.node(reference)
    }
}

fn file(path: &str, tag: usize) -> Entry {
    Entry {
        entry_id: format!("entry_{tag:06}"),
        path: name_policy::PortablePath::parse(path).unwrap(),
        kind: NamespaceEntryKind::File,
        file_id: format!("file_{tag:06}"),
        version_id: format!("ver_{tag:06}"),
        size: tag as u64,
    }
}
fn directory(path: &str, tag: usize) -> Entry {
    Entry {
        entry_id: format!("entry_d{tag:05}"),
        path: name_policy::PortablePath::parse(path).unwrap(),
        kind: NamespaceEntryKind::Directory,
        file_id: String::new(),
        version_id: String::new(),
        size: 0,
    }
}

fn sorted_oracle(mut entries: Vec<Entry>) -> Vec<Entry> {
    entries.sort_by(|a, b| a.lookup_key().as_bytes().cmp(b.lookup_key().as_bytes()));
    entries
}

/// Two directories of three pages each plus root files: enough to force an
/// index level without exceeding unit-test budgets.
fn wide_fixture() -> Vec<Entry> {
    let mut entries = vec![directory("alpha", 1), directory("beta", 2)];
    for n in 0..300 {
        entries.push(file(&format!("alpha/file-{n:04}.txt"), 1000 + n));
        entries.push(file(&format!("beta/file-{n:04}.txt"), 2000 + n));
    }
    for n in 0..5 {
        entries.push(file(&format!("root-{n}.txt"), 3000 + n));
    }
    entries
}

#[test]
fn full_builds_are_canonical_and_verify_like_the_flat_oracle() {
    let entries = wide_fixture();
    let mut shuffled = entries.clone();
    shuffled.reverse();

    let mut out = NamespaceObjects::new();
    let base = deny();
    let namespace = build_from_entries(&entries, &mut out).unwrap();
    let source = DraftSource {
        fresh: &out,
        base: &base,
    };

    // Unsorted input packs to the identical root: partitions are a pure
    // function of the sorted child lists, never of insertion order.
    let mut shuffled_out = NamespaceObjects::new();
    let reversed = build_from_entries(&shuffled, &mut shuffled_out).unwrap();
    assert_eq!(namespace.root, reversed.root);
    assert_eq!(namespace.entries, entries.len() as u64);

    // Full admission re-verification accepts the build and counts every entry.
    assert_eq!(
        verify_namespace(&namespace, &source, 1_000_000).unwrap(),
        entries.len() as u64
    );
    // A tighter budget than the stored total fails closed.
    assert!(
        verify_namespace(&namespace, &source, 10)
            .unwrap_err()
            .to_string()
            .contains("entry limit")
    );

    // The complete canonical enumeration equals the flat sorted oracle.
    let view = NamespaceView::new(&namespace, &source);
    let collected = view.collect_entries().unwrap();
    let oracle = sorted_oracle(entries.clone());
    assert_eq!(collected.len(), oracle.len());
    for (collected, oracle) in collected.iter().zip(&oracle) {
        assert_eq!(collected.path(), oracle.path());
        assert_eq!(collected.entry_id, oracle.entry_id);
        assert_eq!(collected.version_id, oracle.version_id);
        assert_eq!(collected.kind, oracle.kind);
    }

    // Every stored page respects the entry bound; a 300-child directory
    // cannot be one page, so an index level exists.
    let mut pages = 0usize;
    let mut indexes = 0usize;
    for object in out.objects() {
        assert!(object.bytes.len() <= NAMESPACE_PAGE_TARGET_BYTES + 4096);
        match object.kind {
            NAMESPACE_PAGE_KIND => {
                let page: NamespacePage = json_codec::from_slice(&object.bytes).unwrap();
                assert!(page.entries.len() <= NAMESPACE_PAGE_ENTRIES);
                pages += 1;
            }
            NAMESPACE_INDEX_KIND => indexes += 1,
            other => panic!("unexpected namespace object kind {other}"),
        }
    }
    assert!(pages >= 7, "two 300-entry directories need several pages");
    assert!(indexes >= 2, "both wide directories need an index level");
}

#[test]
fn path_copy_edits_reencode_only_the_touched_route() {
    let entries = wide_fixture();
    let mut base_objects = NamespaceObjects::new();
    let base_root = build_from_entries(&entries, &mut base_objects).unwrap();
    let deny = deny();
    let base_source = DraftSource {
        fresh: &base_objects,
        base: &deny,
    };

    // Replace one file's version through the editor.
    let mut replaced = file("alpha/file-0042.txt", 1042);
    replaced.version_id = "ver_rewritten".into();
    replaced.size = 777;
    let mut fresh = NamespaceObjects::new();
    let edited = {
        let mut editor = NamespaceEditor::open(&base_root, &base_source).unwrap();
        editor.upsert_file(&replaced).unwrap();
        editor.seal(&mut fresh).unwrap()
    };

    // A flat rebuild is a logical oracle; only node bytes, not global packing,
    // are canonical across edit histories.
    let mut oracle_entries = entries.clone();
    let slot = oracle_entries
        .iter_mut()
        .find(|e| e.path() == "alpha/file-0042.txt")
        .unwrap();
    *slot = replaced.clone();
    let mut oracle_objects = NamespaceObjects::new();
    let oracle_root = build_from_entries(&oracle_entries, &mut oracle_objects).unwrap();
    assert_eq!(edited.entries, oracle_root.entries);
    let edited_source = DraftSource {
        fresh: &fresh,
        base: &base_source,
    };
    assert_eq!(
        NamespaceView::new(&edited, &edited_source)
            .collect_entries()
            .unwrap(),
        sorted_oracle(oracle_entries)
    );

    // Locality: the edit re-encodes the touched page and its ancestor route,
    // never the untouched sibling subtree or the bulk of the base objects.
    let base_addresses: BTreeSet<&str> = base_objects
        .objects()
        .iter()
        .map(|o| o.hash.as_str())
        .collect();
    let fresh_new = fresh
        .objects()
        .iter()
        .filter(|o| !base_addresses.contains(o.hash.as_str()))
        .count();
    assert!(fresh_new >= 1, "the touched page must re-encode");
    assert!(
        fresh.objects().len() <= 4,
        "one file edit re-encodes at most page+index+root, got {}",
        fresh.objects().len()
    );
    // The untouched sibling directory's subtree reference is shared.
    let merged = {
        let mut all = NamespaceObjects::new();
        all.absorb(fresh);
        all.absorb(base_objects);
        all
    };
    let merged_source = DraftSource {
        fresh: &merged,
        base: &deny,
    };
    let before = NamespaceView::new(&base_root, &merged_source)
        .find_key("beta")
        .unwrap()
        .unwrap();
    let after = NamespaceView::new(&edited, &merged_source)
        .find_key("beta")
        .unwrap()
        .unwrap();
    match (before, after) {
        (
            NamespacePageEntry::Directory { subtree: b, .. },
            NamespacePageEntry::Directory { subtree: a, .. },
        ) => assert_eq!(b, a, "untouched subtree reference must be shared"),
        _ => panic!("beta must stay a directory"),
    }
}

#[test]
fn renames_share_the_subtree_without_reencoding_descendants() {
    let entries = wide_fixture();
    let mut base_objects = NamespaceObjects::new();
    let base_root = build_from_entries(&entries, &mut base_objects).unwrap();
    let deny = deny();
    let base_source = DraftSource {
        fresh: &base_objects,
        base: &deny,
    };
    let original = match NamespaceView::new(&base_root, &base_source)
        .find_key("alpha")
        .unwrap()
        .unwrap()
    {
        NamespacePageEntry::Directory { subtree, .. } => subtree,
        NamespacePageEntry::File { .. } => panic!("alpha must be a directory"),
    };

    let mut fresh = NamespaceObjects::new();
    let renamed = {
        let mut editor = NamespaceEditor::open(&base_root, &base_source).unwrap();
        editor.rename("alpha", &directory("omega", 90001)).unwrap();
        editor.seal(&mut fresh).unwrap()
    };
    assert!(
        fresh.objects().len() <= 2,
        "a directory rename re-encodes only its ancestor route, got {}",
        fresh.objects().len()
    );

    let merged = {
        let mut all = NamespaceObjects::new();
        all.absorb(fresh);
        all.absorb(base_objects);
        all
    };
    let merged_source = DraftSource {
        fresh: &merged,
        base: &deny,
    };
    let view = NamespaceView::new(&renamed, &merged_source);
    let moved = match view.find_key("omega").unwrap().unwrap() {
        NamespacePageEntry::Directory { subtree, .. } => subtree,
        NamespacePageEntry::File { .. } => panic!("omega must be a directory"),
    };
    assert_eq!(original, moved, "a rename must share the subtree reference");
    // Descendant absolute paths re-spell under the new prefix without any
    // descendant re-encoding.
    let entry = view.resolve_key("omega/file-0007.txt").unwrap().unwrap();
    assert_eq!(entry.path(), "omega/file-0007.txt");
    assert_eq!(entry.version_id, format!("ver_{:06}", 1007));
    assert!(view.resolve_key("alpha/file-0007.txt").unwrap().is_none());
}

#[test]
fn point_and_page_reads_locate_through_the_index_without_expansion() {
    let mut entries = vec![directory("big", 1)];
    for n in 0..1000 {
        entries.push(file(&format!("big/file-{n:04}.txt"), 1000 + n));
    }
    let mut out = NamespaceObjects::new();
    let namespace = build_from_entries(&entries, &mut out).unwrap();
    let total_objects = out.objects().len();
    assert!(total_objects >= 9, "1000 children need several pages");
    let deny = deny();
    let inner = DraftSource {
        fresh: &out,
        base: &deny,
    };

    // A point read descends root -> index route -> one page.
    let counting = CountingSource {
        inner: &inner,
        reads: Cell::new(0),
    };
    let view = NamespaceView::new(&namespace, &counting);
    let entry = view.resolve_key("big/file-0777.txt").unwrap().unwrap();
    assert_eq!(entry.version_id, format!("ver_{:06}", 1777));
    let point_reads = counting.reads.get();
    assert!(
        point_reads <= 1 + storage_contract::NAMESPACE_INDEX_DEPTH + 1,
        "a point read is bounded by the descent, got {point_reads}"
    );

    // A page read locates its start through the index and reads forward only
    // as far as the limit, never expanding the whole directory first.
    let counting = CountingSource {
        inner: &inner,
        reads: Cell::new(0),
    };
    let view = NamespaceView::new(&namespace, &counting);
    let page = view
        .directory_page_entries("big", Some("file-0499.txt"), 50)
        .unwrap()
        .unwrap();
    assert_eq!(page.len(), 50);
    assert_eq!(page[0].lookup_key(), "big/file-0500.txt");
    assert_eq!(page[49].lookup_key(), "big/file-0549.txt");
    let page_reads = counting.reads.get();
    assert!(
        page_reads < total_objects / 2,
        "a bounded page never reads most of the directory ({page_reads} of {total_objects})"
    );

    // The first page needs no cursor, and a trailing cursor yields an empty page.
    let first = view
        .directory_page_entries("big", None, 3)
        .unwrap()
        .unwrap();
    assert_eq!(first.len(), 3);
    assert_eq!(first[0].lookup_key(), "big/file-0000.txt");
    let tail = view
        .directory_page_entries("big", Some("file-0999.txt"), 3)
        .unwrap()
        .unwrap();
    assert!(tail.is_empty());
    // A file key is not a directory listing.
    assert!(
        view.directory_page_entries("big/file-0000.txt", None, 3)
            .unwrap()
            .is_none()
    );
}

#[test]
fn verification_rejects_misordered_pages_and_lying_totals() {
    // Serve hand-crafted nodes from a map: hashes are addresses only here,
    // because verification re-checks structure, order and totals.
    fn reference(kind: NamespaceRefKind, tag: u8) -> NamespaceReference {
        NamespaceReference {
            kind,
            hash: format!("{:064x}", tag),
        }
    }
    let child = |name: &str, tag: usize| NamespacePageEntry::File {
        entry_id: format!("entry_{tag:06}"),
        name: name_policy::PortableName::parse(name).unwrap(),
        file_id: format!("file_{tag:06}"),
        version_id: format!("ver_{tag:06}"),
        size: 1,
    };

    // Misordered sibling keys in one page.
    let bad_page = NamespacePage {
        format: storage_contract::NamespacePageFormat,
        entries: vec![child("zeta.txt", 1), child("alpha.txt", 2)],
    };
    let mut nodes = BTreeMap::new();
    nodes.insert(
        reference(NamespaceRefKind::Page, 1).hash,
        NamespaceNode::Page(Arc::new(bad_page)),
    );
    let source = LoaderSource(|r: &NamespaceReference| {
        nodes
            .get(&r.hash)
            .cloned()
            .ok_or_else(|| Error::new(ErrorCode::ObjectCorrupt, "absent"))
    });
    let root = CommitNamespace {
        root: reference(NamespaceRefKind::Page, 1),
        entries: 2,
    };
    let error = verify_namespace(&root, &source, 1_000).unwrap_err();
    assert!(
        error.to_string().contains("unique and sorted"),
        "misordered page must fail closed: {error}"
    );

    // A directory entry lying about its subtree totals.
    let leaf = NamespacePage {
        format: storage_contract::NamespacePageFormat,
        entries: vec![child("one.txt", 3)],
    };
    let lying_root = NamespacePage {
        format: storage_contract::NamespacePageFormat,
        entries: vec![NamespacePageEntry::Directory {
            entry_id: "entry_d1".into(),
            name: name_policy::PortableName::parse("dir").unwrap(),
            entries: 5,
            files: 5,
            subtree: reference(NamespaceRefKind::Page, 3),
        }],
    };
    let mut nodes = BTreeMap::new();
    nodes.insert(
        reference(NamespaceRefKind::Page, 2).hash,
        NamespaceNode::Page(Arc::new(lying_root)),
    );
    nodes.insert(
        reference(NamespaceRefKind::Page, 3).hash,
        NamespaceNode::Page(Arc::new(leaf)),
    );
    let source = LoaderSource(|r: &NamespaceReference| {
        nodes
            .get(&r.hash)
            .cloned()
            .ok_or_else(|| Error::new(ErrorCode::ObjectCorrupt, "absent"))
    });
    let root = CommitNamespace {
        root: reference(NamespaceRefKind::Page, 2),
        entries: 6,
    };
    let error = verify_namespace(&root, &source, 1_000).unwrap_err();
    assert!(
        error.to_string().contains("recorded totals differ"),
        "lying subtree totals must fail closed: {error}"
    );

    // A root whose recorded grand total disagrees with the verified graph.
    let honest_root = CommitNamespace {
        root: reference(NamespaceRefKind::Page, 3),
        entries: 2,
    };
    let error = verify_namespace(&honest_root, &source, 1_000).unwrap_err();
    assert!(
        error.to_string().contains("entry total differs"),
        "lying root total must fail closed: {error}"
    );
}

#[test]
fn the_editor_mirrors_the_legacy_namespace_operation_semantics() {
    let entries = vec![
        directory("docs", 1),
        file("docs/a.txt", 10),
        file("docs/b.txt", 11),
        directory("empty", 2),
        file("top.txt", 12),
    ];
    let mut out = NamespaceObjects::new();
    let namespace = build_from_entries(&entries, &mut out).unwrap();
    let deny = deny();
    let source = DraftSource {
        fresh: &out,
        base: &deny,
    };
    let op = |kind, path: &str, destination: Option<&str>| {
        NamespaceOperation::parse(kind, path, destination).unwrap()
    };
    use storage_contract::NamespaceOperationKind as Kind;

    let failures = [
        (op(Kind::Mkdir, "docs", None), "directory target exists"),
        (op(Kind::Delete, "ghost.txt", None), "delete target missing"),
        (
            op(Kind::Delete, "docs", None),
            "directory contains children",
        ),
        (
            op(Kind::Rename, "ghost.txt", Some("other.txt")),
            "rename source missing",
        ),
        (
            op(Kind::Rename, "docs", Some("docs/inner")),
            "cannot move a directory into itself",
        ),
        (
            op(Kind::Rename, "docs/a.txt", Some("docs/b.txt")),
            "rename destination exists",
        ),
    ];
    for (operation, message) in failures {
        let mut editor = NamespaceEditor::open(&namespace, &source).unwrap();
        let error = editor.apply(&operation).unwrap_err();
        assert!(
            error.to_string().contains(message),
            "{operation:?} must fail with {message}, got {error}"
        );
    }

    // Success path: delete-tree lists the target first, then descendants in
    // normalized full-key order, and the result matches a flat rebuild.
    let mut editor = NamespaceEditor::open(&namespace, &source).unwrap();
    let deleted = editor.apply(&op(Kind::DeleteTree, "docs", None)).unwrap();
    assert_eq!(
        deleted.iter().map(|d| d.path.as_str()).collect::<Vec<_>>(),
        vec!["docs", "docs/a.txt", "docs/b.txt"]
    );
    let deleted = editor.apply(&op(Kind::Delete, "empty", None)).unwrap();
    assert_eq!(
        deleted.iter().map(|d| d.path.as_str()).collect::<Vec<_>>(),
        vec!["empty"]
    );
    let mut fresh = NamespaceObjects::new();
    let sealed = editor.seal(&mut fresh).unwrap();
    let mut oracle_objects = NamespaceObjects::new();
    let oracle = build_from_entries(&[file("top.txt", 12)], &mut oracle_objects).unwrap();
    assert_eq!(sealed.root, oracle.root);
    assert_eq!(sealed.entries, 1);
}

#[test]
fn path_closure_and_file_search_stop_early() {
    let mut entries = vec![
        directory("a", 1),
        directory("a/b", 2),
        file("a/b/c.txt", 10),
    ];
    for n in 0..200 {
        entries.push(file(&format!("zz/file-{n:04}.txt"), 5000 + n));
    }
    entries.push(directory("zz", 3));
    let mut out = NamespaceObjects::new();
    let namespace = build_from_entries(&entries, &mut out).unwrap();
    let deny = deny();
    let source = DraftSource {
        fresh: &out,
        base: &deny,
    };
    let view = NamespaceView::new(&namespace, &source);

    let closure = view.path_closure("a/b/c.txt").unwrap();
    assert_eq!(
        closure.iter().map(Entry::path).collect::<Vec<_>>(),
        vec!["a", "a/b", "a/b/c.txt"]
    );
    assert!(view.path_closure("a/ghost/c.txt").unwrap().is_empty());

    // find_file walks in canonical order and stops at the first hit: a file
    // under "a/" never touches the large "zz" subtree.
    let counting = CountingSource {
        inner: &source,
        reads: Cell::new(0),
    };
    let view = NamespaceView::new(&namespace, &counting);
    let found = view.find_file("file_000010").unwrap().unwrap();
    assert_eq!(found.path(), "a/b/c.txt");
    assert!(
        counting.reads.get() <= 4,
        "an early hit must not expand later subtrees, got {}",
        counting.reads.get()
    );
}

#[test]
fn admission_rejects_forged_index_totals_at_every_level() {
    fn child(first_key: &str, reference: NamespaceReference, count: u64) -> NamespaceIndexChild {
        NamespaceIndexChild {
            first_key: first_key.into(),
            entries: count,
            files: count,
            reference,
        }
    }
    let mut objects = NamespaceObjects::new();
    let mut pages = Vec::new();
    for (tag, name) in ["a", "b", "c", "d"].into_iter().enumerate() {
        let entry = file(name, tag);
        pages.push(
            objects
                .page(NamespacePage {
                    format: Default::default(),
                    entries: vec![NamespacePageEntry::File {
                        entry_id: entry.entry_id,
                        name: name_policy::PortableName::parse(name).unwrap(),
                        file_id: entry.file_id,
                        version_id: entry.version_id,
                        size: entry.size,
                    }],
                })
                .unwrap(),
        );
    }
    let lower = NamespaceIndex {
        format: Default::default(),
        children: vec![
            child("a", pages[0].clone(), 1),
            child("b", pages[1].clone(), 1),
        ],
    };
    let left = objects.index(lower.clone()).unwrap();
    let right = objects
        .index(NamespaceIndex {
            format: Default::default(),
            children: vec![
                child("c", pages[2].clone(), 1),
                child("d", pages[3].clone(), 1),
            ],
        })
        .unwrap();
    let top = NamespaceIndex {
        format: Default::default(),
        children: vec![child("a", left, 2), child("c", right, 2)],
    };
    let root = CommitNamespace {
        root: objects.index(top.clone()).unwrap(),
        entries: 4,
    };
    let base = deny();
    assert_eq!(
        verify_namespace(
            &root,
            &DraftSource {
                fresh: &objects,
                base: &base
            },
            4
        )
        .unwrap(),
        4
    );

    for nested in [false, true] {
        for forge_files in [false, true] {
            let mut changed = if nested { lower.clone() } else { top.clone() };
            if forge_files {
                changed.children[0].files = u64::MAX;
            } else {
                changed.children[0].entries = u64::MAX;
            }
            let reference = objects.index(changed).unwrap();
            let reference = if nested {
                let mut outer = top.clone();
                outer.children[0].reference = reference;
                objects.index(outer).unwrap()
            } else {
                reference
            };
            let forged = CommitNamespace {
                root: reference,
                entries: 4,
            };
            let error = verify_namespace(
                &forged,
                &DraftSource {
                    fresh: &objects,
                    base: &base,
                },
                4,
            )
            .unwrap_err();
            assert_eq!(error.code(), ErrorCode::ObjectCorrupt);
            assert!(error.to_string().contains("index totals differ"));
        }
    }
}

#[test]
fn admission_stops_at_the_entry_budget_before_reading_later_pages() {
    let entries: Vec<_> = (0..1000)
        .map(|n| file(&format!("file-{n:04}"), n))
        .collect();
    let mut objects = NamespaceObjects::new();
    let namespace = build_from_entries(&entries, &mut objects).unwrap();
    let base = deny();
    let source = DraftSource {
        fresh: &objects,
        base: &base,
    };
    for budget in [0, 1, NAMESPACE_PAGE_ENTRIES as u64] {
        let counting = CountingSource {
            inner: &source,
            reads: Cell::new(0),
        };
        let error = verify_namespace(&namespace, &counting, budget).unwrap_err();
        assert!(error.to_string().contains("entry limit"));
        assert!(
            counting.reads.get() <= 2,
            "read past the first bounded page"
        );
    }
    let empty = build_from_entries(&[], &mut objects).unwrap();
    assert_eq!(
        verify_namespace(
            &empty,
            &DraftSource {
                fresh: &objects,
                base: &base
            },
            0
        )
        .unwrap(),
        0
    );
}

#[test]
fn page_decode_enforces_canonical_bytes_with_the_single_entry_exception() {
    let mut objects = NamespaceObjects::new();
    let entries: Vec<_> = (0..NAMESPACE_PAGE_ENTRIES)
        .map(|n| {
            let entry = file(&format!("{n:04}{}", "a".repeat(196)), n);
            NamespacePageEntry::File {
                name: entry.location().file_component(),
                entry_id: entry.entry_id,
                file_id: entry.file_id,
                version_id: entry.version_id,
                size: entry.size,
            }
        })
        .collect();
    let page = NamespacePage {
        format: Default::default(),
        entries,
    };
    let reference = objects.page(page).unwrap();
    let object = &objects.objects()[0];
    assert!(object.bytes.len() > NAMESPACE_PAGE_TARGET_BYTES);
    let value = json_codec::from_slice(&object.bytes).unwrap();
    let error = match NamespaceNode::decode(&reference, value, object.bytes.len()) {
        Ok(_) => panic!("oversized multi-entry page admitted"),
        Err(error) => error,
    };
    assert_eq!(error.code(), ErrorCode::ObjectCorrupt);
    assert!(error.to_string().contains("byte bound"));

    // Field identities have their own manifest admission. This tests only the
    // layout's explicit oversized-single-entry rule at the decoding boundary.
    let large = NamespacePage {
        format: Default::default(),
        entries: vec![NamespacePageEntry::File {
            entry_id: "entry_large".into(),
            name: name_policy::PortableName::parse("large").unwrap(),
            file_id: "file_large".into(),
            version_id: "v".repeat(NAMESPACE_PAGE_TARGET_BYTES),
            size: 0,
        }],
    };
    let reference = objects.page(large).unwrap();
    let object = objects.objects().last().unwrap();
    assert!(object.bytes.len() > NAMESPACE_PAGE_TARGET_BYTES);
    assert!(
        NamespaceNode::decode(
            &reference,
            json_codec::from_slice(&object.bytes).unwrap(),
            object.bytes.len(),
        )
        .is_ok()
    );
}

#[test]
fn packing_counts_the_page_envelope_in_the_byte_bound() {
    let mut children = vec![
        NamespacePageEntry::File {
            entry_id: "entry_a".into(),
            name: name_policy::PortableName::parse("a").unwrap(),
            file_id: "file_a".into(),
            version_id: "ver_a".into(),
            size: 0,
        },
        NamespacePageEntry::File {
            entry_id: "entry_b".into(),
            name: name_policy::PortableName::parse("b").unwrap(),
            file_id: "file_b".into(),
            version_id: "ver_b".into(),
            size: 0,
        },
    ];
    let entry_bytes: usize = children
        .iter()
        .map(|entry| canonical_json(entry).unwrap().len())
        .sum();
    if let NamespacePageEntry::File { version_id, .. } = &mut children[1] {
        // The old entry-only counter fits exactly, but the page wrapper does
        // not. Both resulting single-entry pages remain below the byte target.
        version_id.push_str(&"v".repeat(NAMESPACE_PAGE_TARGET_BYTES - entry_bytes - 2));
    }
    let mut objects = NamespaceObjects::new();
    let (root, entries, _) = pack_directory(&children, &mut objects).unwrap();
    assert_eq!(entries, 2);
    assert_eq!(root.kind, NamespaceRefKind::Index);
    for object in objects
        .objects()
        .iter()
        .filter(|object| object.kind == NAMESPACE_PAGE_KIND)
    {
        assert!(object.bytes.len() <= NAMESPACE_PAGE_TARGET_BYTES);
        let reference = NamespaceReference {
            kind: NamespaceRefKind::Page,
            hash: object.hash.clone(),
        };
        assert!(
            NamespaceNode::decode(
                &reference,
                json_codec::from_slice(&object.bytes).unwrap(),
                object.bytes.len(),
            )
            .is_ok()
        );
    }
}

#[test]
fn admission_rejects_mixed_index_heights_but_accepts_sparse_balanced_trees() {
    fn child(name: &str, reference: NamespaceReference) -> NamespaceIndexChild {
        NamespaceIndexChild {
            first_key: name.into(),
            entries: 1,
            files: 1,
            reference,
        }
    }
    let mut objects = NamespaceObjects::new();
    let mut pages = Vec::new();
    for name in ["a", "b"] {
        pages.push(
            objects
                .page(NamespacePage {
                    format: Default::default(),
                    entries: vec![NamespacePageEntry::File {
                        entry_id: format!("entry_{name}"),
                        name: name_policy::PortableName::parse(name).unwrap(),
                        file_id: format!("file_{name}"),
                        version_id: format!("ver_{name}"),
                        size: 0,
                    }],
                })
                .unwrap(),
        );
    }
    // Interior fanout one is part of the wire contract. Builder occupancy
    // choices must not turn into a stricter external admission requirement.
    let left = objects
        .index(NamespaceIndex {
            format: Default::default(),
            children: vec![child("a", pages[0].clone())],
        })
        .unwrap();
    let right = objects
        .index(NamespaceIndex {
            format: Default::default(),
            children: vec![child("b", pages[1].clone())],
        })
        .unwrap();
    let balanced = CommitNamespace {
        root: objects
            .index(NamespaceIndex {
                format: Default::default(),
                children: vec![child("a", left), child("b", right.clone())],
            })
            .unwrap(),
        entries: 2,
    };
    let base = deny();
    assert_eq!(
        verify_namespace(
            &balanced,
            &DraftSource {
                fresh: &objects,
                base: &base
            },
            2
        )
        .unwrap(),
        2
    );

    let mixed = CommitNamespace {
        root: objects
            .index(NamespaceIndex {
                format: Default::default(),
                children: vec![child("a", pages[0].clone()), child("b", right)],
            })
            .unwrap(),
        entries: 2,
    };
    let error = verify_namespace(
        &mixed,
        &DraftSource {
            fresh: &objects,
            base: &base,
        },
        2,
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::ObjectCorrupt);
    assert!(error.to_string().contains("different heights"));
}

#[test]
fn admission_allows_each_directory_its_own_index_height() {
    let mut entries = vec![
        directory("wide", 1),
        directory("narrow", 2),
        file("narrow/only", 3),
    ];
    entries.extend((0..130).map(|n| file(&format!("wide/file-{n:04}"), 1000 + n)));
    let mut objects = NamespaceObjects::new();
    let namespace = build_from_entries(&entries, &mut objects).unwrap();
    let base = deny();
    assert_eq!(
        verify_namespace(
            &namespace,
            &DraftSource {
                fresh: &objects,
                base: &base
            },
            entries.len() as u64
        )
        .unwrap(),
        entries.len() as u64,
    );
}

#[test]
fn point_cursor_borrows_the_admitted_page_and_directory_listing_descends_once() {
    let entries = [directory("Straße", 1), file("Straße/Café.txt", 2)];
    let (root, objects) = fixture_namespace(&entries);
    let missing = deny();
    let source = DraftSource {
        fresh: &objects,
        base: &missing,
    };
    let child = locate_child_ref(&root.root, "strasse", &source)
        .unwrap()
        .unwrap();
    let NamespaceNode::Page(page) = source.node(&root.root).unwrap() else {
        panic!("fixture root is a page");
    };
    assert!(Arc::ptr_eq(&child.page, &page));
    assert_eq!(child.entry().name(), "Straße");
    let counting = CountingSource {
        inner: &source,
        reads: Cell::new(0),
    };
    let view = NamespaceView::new(&root, &counting);
    let page = view
        .directory_page_entries("strasse", None, 1)
        .unwrap()
        .unwrap();
    assert_eq!(page, vec![entries[1].clone()]);
    assert_eq!(
        counting.reads.get(),
        2,
        "one root lookup and one child page"
    );
    assert_eq!(
        view.resolve_key("strasse/café.txt").unwrap(),
        Some(entries[1].clone())
    );
    assert!(
        view.directory_page_entries("strasse/café.txt", None, 1)
            .unwrap()
            .is_none()
    );
}

#[test]
fn point_path_budget_failure_is_reported_only_for_a_resolved_path() {
    let parent = "a".repeat(100);
    let child = "b".repeat(100);
    let mut objects = NamespaceObjects::new();
    let leaf = objects
        .page(NamespacePage {
            format: Default::default(),
            entries: vec![NamespacePageEntry::File {
                entry_id: "entry-leaf".into(),
                name: name_policy::PortableName::parse("leaf").unwrap(),
                file_id: "file-leaf".into(),
                version_id: "version-leaf".into(),
                size: 0,
            }],
        })
        .unwrap();
    let middle = objects
        .page(NamespacePage {
            format: Default::default(),
            entries: vec![NamespacePageEntry::Directory {
                entry_id: "entry-child".into(),
                name: name_policy::PortableName::parse(&child).unwrap(),
                entries: 1,
                files: 1,
                subtree: leaf,
            }],
        })
        .unwrap();
    let root = CommitNamespace {
        root: objects
            .page(NamespacePage {
                format: Default::default(),
                entries: vec![NamespacePageEntry::Directory {
                    entry_id: "entry-parent".into(),
                    name: name_policy::PortableName::parse(&parent).unwrap(),
                    entries: 2,
                    files: 1,
                    subtree: middle,
                }],
            })
            .unwrap(),
        entries: 3,
    };
    let missing = deny();
    let source = DraftSource {
        fresh: &objects,
        base: &missing,
    };
    let view = NamespaceView::new(&root, &source);
    let key = format!("{parent}/{child}");
    assert!(view.find_key(&key).unwrap().is_some());
    assert_eq!(
        view.resolve_key(&key).unwrap_err().code(),
        ErrorCode::ObjectCorrupt
    );
    assert_eq!(
        view.resolve_key(&format!("{key}/leaf")).unwrap_err().code(),
        ErrorCode::ObjectCorrupt
    );
    assert!(
        view.resolve_key(&format!("{key}/absent"))
            .unwrap()
            .is_none()
    );
    assert!(
        view.resolve_key(&format!("{key}/leaf/absent"))
            .unwrap()
            .is_none()
    );
}
