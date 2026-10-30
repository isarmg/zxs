//! Traversal borrows immutable roots and node references without changing
//! owned lookup results, source failures, or delayed path-budget errors.
use super::*;
use std::cell::RefCell;
use std::sync::Weak;

struct BorrowedSource<'a> {
    objects: &'a NamespaceObjects,
    addresses: Vec<*const u8>,
    reads: Cell<usize>,
}

impl<'a> BorrowedSource<'a> {
    fn new(root: &'a CommitNamespace, objects: &'a NamespaceObjects) -> Self {
        let mut addresses = vec![root.root.hash.as_ptr()];
        for node in objects.decoded.values() {
            match node {
                NamespaceNode::Page(page) => {
                    for entry in &page.entries {
                        if let NamespacePageEntry::Directory { subtree, .. } = entry {
                            addresses.push(subtree.hash.as_ptr());
                        }
                    }
                }
                NamespaceNode::Index(index) => {
                    addresses.extend(
                        index
                            .children
                            .iter()
                            .map(|child| child.reference.hash.as_ptr()),
                    );
                }
            }
        }
        Self {
            objects,
            addresses,
            reads: Cell::new(0),
        }
    }
}

impl NamespaceSource for BorrowedSource<'_> {
    fn node(&self, reference: &NamespaceReference) -> Result<NamespaceNode> {
        assert!(
            self.addresses.contains(&reference.hash.as_ptr()),
            "traversal must borrow the root or a stored node reference"
        );
        self.reads.set(self.reads.get() + 1);
        reference.validate()?;
        self.objects
            .decoded
            .get(&reference.hash)
            .cloned()
            .ok_or_else(|| corrupt("test object is missing"))
    }
}

#[test]
fn lookup_borrows_root_and_index_subtree_references_but_returns_owned_entries() {
    let mut entries = wide_fixture();
    entries.push(file("Straße/A/B/C/Café.txt", 9000));
    let (root, objects) = fixture_namespace(&entries);
    let source = BorrowedSource::new(&root, &objects);
    let view = NamespaceView::new(&root, &source);
    assert!(std::ptr::eq(view.root, &root));
    for query in ["alpha/file-0299.txt", "STRASSE/a/b/c/CAFE\u{301}.TXT"] {
        let key = name_policy::path_key(query);
        let expected = entries
            .iter()
            .find(|entry| entry.lookup_key() == key)
            .unwrap();
        assert_eq!(view.resolve_key(&key).unwrap().as_ref(), Some(expected));
    }
    let before = source.reads.get();
    assert!(view.resolve_key("").unwrap().is_none());
    assert_eq!(source.reads.get(), before);
    for query in [
        "strasse/a/b/c/absent",
        "strasse/a/b/c/café.txt/absent",
        "alpha/file-0300.txt",
    ] {
        assert!(view.resolve_key(query).unwrap().is_none());
    }
    let owned = view.resolve_key("strasse/a/b/c/café.txt").unwrap().unwrap();
    drop(source);
    drop(objects);
    drop(root);
    assert_eq!(owned.path(), "Straße/A/B/C/Café.txt");
    assert_eq!(owned.file_id, "file_009000");
}

enum HeldNode {
    Page(Weak<NamespacePage>),
    Index(Weak<NamespaceIndex>),
}

impl HeldNode {
    fn alive(&self) -> bool {
        match self {
            Self::Page(page) => page.strong_count() > 0,
            Self::Index(index) => index.strong_count() > 0,
        }
    }
}

struct EvictingSource {
    nodes: RefCell<BTreeMap<String, NamespaceNode>>,
    previous: RefCell<Option<HeldNode>>,
    loads: Cell<usize>,
}

impl NamespaceSource for EvictingSource {
    fn node(&self, reference: &NamespaceReference) -> Result<NamespaceNode> {
        if let Some(previous) = self.previous.borrow().as_ref() {
            assert!(
                previous.alive(),
                "the borrowed reference owner must survive the next read"
            );
        }
        reference.validate()?;
        let node = self
            .nodes
            .borrow_mut()
            .remove(&reference.hash)
            .ok_or_else(|| corrupt("test object already evicted"))?;
        self.previous.replace(Some(match &node {
            NamespaceNode::Page(page) => HeldNode::Page(Arc::downgrade(page)),
            NamespaceNode::Index(index) => HeldNode::Index(Arc::downgrade(index)),
        }));
        self.loads.set(self.loads.get() + 1);
        Ok(node)
    }
}

#[test]
fn lookup_keeps_reference_owners_alive_during_reads_and_releases_the_final_leaf() {
    let (root, objects) = fixture_namespace(&wide_fixture());
    let source = EvictingSource {
        nodes: RefCell::new(objects.decoded),
        previous: RefCell::new(None),
        loads: Cell::new(0),
    };
    let entry = NamespaceView::new(&root, &source)
        .resolve_key("alpha/file-0299.txt")
        .unwrap()
        .unwrap();
    assert!(source.loads.get() >= 3, "root page, index, and leaf page");
    assert!(!source.previous.borrow().as_ref().unwrap().alive());
    drop(source);
    assert_eq!(entry.path(), "alpha/file-0299.txt");
}

#[test]
fn lookup_preserves_reference_failure_before_a_delayed_composed_path_error() {
    let parent = "a".repeat(100);
    let child = "b".repeat(100);
    let invalid = NamespaceReference {
        kind: NamespaceRefKind::Page,
        hash: "invalid-hash".into(),
    };
    let mut objects = NamespaceObjects::new();
    let middle = objects
        .page(NamespacePage {
            format: Default::default(),
            entries: vec![NamespacePageEntry::Directory {
                entry_id: "child".into(),
                name: name_policy::PortableName::parse(&child).unwrap(),
                entries: 1,
                files: 1,
                subtree: invalid.clone(),
            }],
        })
        .unwrap();
    let root = CommitNamespace {
        root: objects
            .page(NamespacePage {
                format: Default::default(),
                entries: vec![NamespacePageEntry::Directory {
                    entry_id: "parent".into(),
                    name: name_policy::PortableName::parse(&parent).unwrap(),
                    entries: 2,
                    files: 1,
                    subtree: middle,
                }],
            })
            .unwrap(),
        entries: 3,
    };
    let source = BorrowedSource::new(&root, &objects);
    let view = NamespaceView::new(&root, &source);
    let prefix = format!("{parent}/{child}");
    assert_eq!(
        view.resolve_key(&prefix).unwrap_err().code(),
        ErrorCode::ObjectCorrupt
    );
    let expected = Error::from(invalid.validate().unwrap_err());
    let actual = view.resolve_key(&format!("{prefix}/leaf")).unwrap_err();
    assert_eq!(actual.code(), expected.code());
    assert_eq!(actual.to_string(), expected.to_string());
    for key in [&prefix, &format!("{prefix}/leaf")] {
        let owned = view.resolve_key(key).unwrap_err();
        let projected = view
            .inspect_key(key, |_| {
                panic!("invalid path must not reach the projection")
            })
            .unwrap_err();
        assert_eq!(projected.code(), owned.code());
        assert_eq!(projected.to_string(), owned.to_string());
    }
    let invalid_root = CommitNamespace {
        root: invalid,
        entries: 1,
    };
    let invalid_source = BorrowedSource::new(&invalid_root, &objects);
    let actual = NamespaceView::new(&invalid_root, &invalid_source)
        .resolve_key("anything")
        .unwrap_err();
    assert_eq!(actual.to_string(), expected.to_string());
}

#[test]
fn projection_borrows_leaf_ids_until_callback_returns() {
    let (root, objects) = fixture_namespace(&wide_fixture());
    let source = EvictingSource {
        nodes: RefCell::new(objects.decoded),
        previous: RefCell::new(None),
        loads: Cell::new(0),
    };
    let view = NamespaceView::new(&root, &source);
    let matched = view
        .inspect_key("alpha/file-0299.txt", |entry| {
            let previous = source.previous.borrow();
            let HeldNode::Page(page) = previous.as_ref().unwrap() else {
                panic!("resolved entry must belong to a leaf page");
            };
            let page = page.upgrade().expect("leaf lives through projection");
            assert!(
                page.entries
                    .iter()
                    .any(|stored| std::ptr::eq(stored, entry))
            );
            matches!(entry, NamespacePageEntry::File { .. })
        })
        .unwrap();
    assert_eq!(matched, Some(true));
    assert!(!source.previous.borrow().as_ref().unwrap().alive());
}
