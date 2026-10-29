//! Exercise the maximum index height without constructing millions of entries.
use super::*;

/// Only the leftmost edit route is full. Every other branch is a unary chain
/// at the same height, which is valid wire shape even though the bulk builder
/// normally produces denser interior nodes.
fn full_left_route(
    height: usize,
    full: bool,
    next: &mut usize,
    objects: &mut NamespaceObjects,
) -> NamespaceIndexChild {
    if height == 1 {
        let count = if full { NAMESPACE_PAGE_ENTRIES } else { 1 };
        let mut entries = Vec::new();
        for _ in 0..count {
            let entry = file(&format!("k{:06}", *next), *next);
            *next += 1;
            entries.push(NamespacePageEntry::File {
                name: entry.name().into(),
                entry_id: entry.entry_id,
                file_id: entry.file_id,
                version_id: entry.version_id,
                size: entry.size,
            });
        }
        let first_key = entries[0].lookup_key();
        return NamespaceIndexChild {
            first_key,
            entries: count as u64,
            files: count as u64,
            reference: objects
                .page(NamespacePage {
                    format: Default::default(),
                    entries,
                })
                .unwrap(),
        };
    }
    let count = if full { NAMESPACE_INDEX_FANOUT } else { 1 };
    let children: Vec<_> = (0..count)
        .map(|position| full_left_route(height - 1, full && position == 0, next, objects))
        .collect();
    let first_key = children[0].first_key.clone();
    let entries = children.iter().map(|child| child.entries).sum();
    let files = children.iter().map(|child| child.files).sum();
    NamespaceIndexChild {
        first_key,
        entries,
        files,
        reference: objects
            .index(NamespaceIndex {
                format: Default::default(),
                children,
            })
            .unwrap(),
    }
}

fn balanced_height(
    reference: &NamespaceReference,
    source: &(impl NamespaceSource + ?Sized),
) -> usize {
    match source.node(reference).unwrap() {
        NamespaceNode::Page(_) => 1,
        NamespaceNode::Index(index) => {
            let heights: Vec<_> = index
                .children
                .iter()
                .map(|child| balanced_height(&child.reference, source))
                .collect();
            assert!(heights.iter().all(|height| *height == heights[0]));
            heights[0] + 1
        }
    }
}

#[test]
fn maximum_height_replacement_succeeds_but_root_growth_rejects_before_encoding() {
    let mut objects = NamespaceObjects::new();
    let mut next = 0;
    let root = full_left_route(
        storage_contract::NAMESPACE_INDEX_DEPTH,
        true,
        &mut next,
        &mut objects,
    );
    let namespace = CommitNamespace {
        root: root.reference,
        entries: root.entries,
    };
    // 128 entries in the target page plus 127 singleton leaves at each
    // of the three index levels: a 509-entry, four-object-height graph.
    assert_eq!(next, 509);
    let missing = deny();
    let source = DraftSource {
        fresh: &objects,
        base: &missing,
    };
    assert_eq!(
        balanced_height(&namespace.root, &source),
        storage_contract::NAMESPACE_INDEX_DEPTH
    );
    assert_eq!(
        verify_namespace(&namespace, &source, next as u64).unwrap(),
        next as u64
    );

    let original = file("k000000", 0);
    let mut replacement = original.clone();
    replacement.version_id = "ver_replaced_at_maximum_height".into();
    let mut editor = NamespaceEditor::open(&namespace, &source).unwrap();
    editor.upsert_file(&replacement).unwrap();
    let mut changed = NamespaceObjects::new();
    let replaced = editor.seal(&mut changed).unwrap();
    let changed_source = DraftSource {
        fresh: &changed,
        base: &source,
    };
    assert_eq!(
        balanced_height(&replaced.root, &changed_source),
        storage_contract::NAMESPACE_INDEX_DEPTH
    );
    assert_eq!(
        verify_namespace(&replaced, &changed_source, next as u64).unwrap(),
        next as u64
    );
    assert_eq!(
        NamespaceView::new(&replaced, &changed_source)
            .resolve_key(replacement.lookup_key())
            .unwrap(),
        Some(replacement)
    );

    // Every node on the first route is full, so one new minimum key splits
    // the leaf and all three indexes, requiring a fifth root-to-page object.
    let mut editor = NamespaceEditor::open(&namespace, &source).unwrap();
    editor.upsert_file(&file("a", next)).unwrap();
    let mut rejected = NamespaceObjects::new();
    let error = editor.seal(&mut rejected).unwrap_err();
    assert_eq!(error.code(), ErrorCode::StateInvalid);
    assert!(error.to_string().contains("index depth"));
    assert_eq!(rejected.encoding_work(), (0, 0));
    assert!(rejected.objects().is_empty());
    assert_eq!(
        NamespaceView::new(&namespace, &source)
            .resolve_key(original.lookup_key())
            .unwrap(),
        Some(original)
    );
    assert!(
        NamespaceView::new(&namespace, &source)
            .find_key("a")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        verify_namespace(&namespace, &source, next as u64).unwrap(),
        next as u64
    );
}
