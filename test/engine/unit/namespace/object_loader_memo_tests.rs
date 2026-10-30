use super::*;

fn authenticate(
    objects: &NamespaceObjects,
    kind: &'static str,
    hash: &str,
    damaged: bool,
) -> Result<AuthenticatedMetadataObject> {
    let object = objects
        .objects()
        .iter()
        .find(|object| object.kind() == kind && object.hash == hash)
        .ok_or_else(|| corrupt("namespace fixture address is absent"))?;
    let value = if damaged {
        json_codec::json!({"format": "namespace-page", "entries": []})
    } else {
        json_codec::from_slice(object.bytes())?
    };
    AuthenticatedMetadataObject::authenticate(kind, hash, value)
}

#[test]
fn admission_node_memo_ends_with_its_source_and_does_not_cache_failures() {
    let (root, objects) = fixture_namespace(&[file("file.txt", 1)]);
    let reads = Cell::new(0);
    let damaged = Cell::new(false);
    {
        let source = ObjectLoaderSource::memoized(|kind, hash: &str| {
            reads.set(reads.get() + 1);
            authenticate(&objects, kind, hash, damaged.get())
        });
        source.node(&root.root).unwrap();
        source.node(&root.root).unwrap();
        assert_eq!(reads.get(), 1);
        // A cached node does not exempt a new graph walk from its entry budget.
        assert!(verify_namespace(&root, &source, 0).is_err());
        assert_eq!(reads.get(), 1);
    }
    damaged.set(true);
    let source = ObjectLoaderSource::memoized(|kind, hash: &str| {
        reads.set(reads.get() + 1);
        authenticate(&objects, kind, hash, damaged.get())
    });
    assert!(source.node(&root.root).is_err());
    assert!(source.node(&root.root).is_err());
    assert_eq!(reads.get(), 3);
    damaged.set(false);
    let source = ObjectLoaderSource::new(|kind, hash: &str| {
        reads.set(reads.get() + 1);
        authenticate(&objects, kind, hash, damaged.get())
    });
    source.node(&root.root).unwrap();
    source.node(&root.root).unwrap();
    assert_eq!(reads.get(), 5);
}

#[test]
fn admission_node_memo_keeps_domains_separate_and_rejects_invalid_references() {
    let (root, objects) = fixture_namespace(&[file("file.txt", 1)]);
    let reads = Cell::new(0);
    let source = ObjectLoaderSource::memoized(|kind, hash: &str| {
        reads.set(reads.get() + 1);
        authenticate(&objects, kind, hash, false)
    });
    source.node(&root.root).unwrap();
    let wrong_kind = NamespaceReference {
        kind: NamespaceRefKind::Index,
        hash: root.root.hash.clone(),
    };
    assert!(source.node(&wrong_kind).is_err());
    assert_eq!(reads.get(), 2);
    let invalid = NamespaceReference {
        kind: NamespaceRefKind::Page,
        hash: "g".repeat(64),
    };
    let cached = source.node(&root.root).unwrap();
    source.nodes.as_ref().unwrap().borrow_mut().insert(
        crate::segment_store::NamespaceCacheKey::Page([b'g'; 64]),
        cached,
        1,
    );
    assert!(source.node(&invalid).is_err());
    assert_eq!(reads.get(), 2);
    source.node(&root.root).unwrap();
    assert_eq!(reads.get(), 2);
}

#[test]
fn admission_node_memo_inserts_only_shape_valid_nodes_within_its_budget() {
    let mut objects = NamespaceObjects::new();
    let child = NamespacePageEntry::File {
        entry_id: "entry_000001".into(),
        name: name_policy::PortableName::parse("file.txt").unwrap(),
        file_id: "file_000001".into(),
        version_id: "ver_000001".into(),
        size: 1,
    };
    let bad = objects
        .page(NamespacePage {
            format: Default::default(),
            entries: vec![child.clone(), child.clone()],
        })
        .unwrap();
    let good = objects
        .page(NamespacePage {
            format: Default::default(),
            entries: vec![child],
        })
        .unwrap();
    let empty = objects
        .page(NamespacePage {
            format: Default::default(),
            entries: Vec::new(),
        })
        .unwrap();
    let good_bytes = objects
        .objects()
        .iter()
        .find(|object| object.hash == good.hash)
        .unwrap()
        .bytes()
        .len();
    let reads = Cell::new(0);
    let source = ObjectLoaderSource::memoized(|kind, hash: &str| {
        reads.set(reads.get() + 1);
        authenticate(&objects, kind, hash, false)
    });
    assert!(source.node(&bad).is_err());
    assert!(source.node(&bad).is_err());
    assert_eq!(reads.get(), 2);
    // Use the shared cache's existing oversize rule without a product setting.
    *source.nodes.as_ref().unwrap().borrow_mut() =
        crate::segment_store::BoundedCache::new(usize::MAX, 1);
    source.node(&good).unwrap();
    source.node(&good).unwrap();
    assert_eq!(reads.get(), 4);
    *source.nodes.as_ref().unwrap().borrow_mut() =
        crate::segment_store::BoundedCache::new(usize::MAX, good_bytes);
    source.node(&good).unwrap();
    source.node(&empty).unwrap();
    source.node(&good).unwrap();
    assert_eq!(reads.get(), 7);
}
