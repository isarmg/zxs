//! Explicit, isolated-process acceptance measurements; never ordinary CI work.
use super::*;
use crate::namespace::{
    DraftSource, NamespaceEditor, NamespaceNode, NamespaceObjects, NamespaceSource, NamespaceView,
    build_from_entries,
};
use std::cell::Cell;
use std::time::Instant;
use storage_contract::{
    Entry, NAMESPACE_PAGE_KIND, NamespaceEntryKind, NamespacePage, NamespacePageEntry,
    NamespaceRefKind, NamespaceReference,
};

struct ObservedReader<'a> {
    reader: SegmentReader<'a>,
    calls: Cell<usize>,
    misses: Cell<usize>,
    canonical_bytes: Cell<usize>,
}

impl NamespaceSource for ObservedReader<'_> {
    fn node(&self, reference: &NamespaceReference) -> Result<NamespaceNode> {
        let address = format!("{}/{}", reference.kind.as_str(), reference.hash);
        let hit = SegmentCaches::lock(&self.reader.caches.namespace)
            .entries
            .contains_key(&address);
        let node = self.reader.namespace_node(reference)?;
        self.calls.set(self.calls.get() + 1);
        if !hit {
            self.misses.set(self.misses.get() + 1);
            let cache = SegmentCaches::lock(&self.reader.caches.namespace);
            self.canonical_bytes
                .set(self.canonical_bytes.get() + cache.entries[&address].1);
        }
        Ok(node)
    }
}

impl ObservedReader<'_> {
    fn reset(&self) {
        self.calls.set(0);
        self.misses.set(0);
        self.canonical_bytes.set(0);
    }

    fn report(&self, width: usize, phase: &str, started: Instant, encoding: (usize, usize)) {
        let elapsed = started.elapsed();
        let status = std::fs::read_to_string("/proc/self/status").unwrap();
        let kib = |key: &str| -> u64 {
            status
                .lines()
                .find_map(|line| line.strip_prefix(key))
                .expect("Linux exposes VmRSS and VmHWM")
                .split_whitespace()
                .next()
                .unwrap()
                .parse()
                .unwrap()
        };
        let cache = SegmentCaches::lock(&self.reader.caches.namespace);
        assert!(cache.bytes <= MAX_CACHED_NAMESPACE_BYTES);
        println!(
            "NAMESPACE_ADOPTION {}",
            json_codec::json!({
                "width": width, "phase": phase,
                "elapsed_ns": elapsed.as_nanos().to_string(),
                "source_calls": self.calls.get(), "cache_misses": self.misses.get(),
                "miss_canonical_bytes": self.canonical_bytes.get(),
                "cache_canonical_bytes": cache.bytes, "cache_objects": cache.entries.len(),
                "encoded_nodes": encoding.0, "encoded_bytes": encoding.1,
                "process_rss_kib": kib("VmRSS:"), "process_hwm_kib": kib("VmHWM:")
            })
        );
    }
}

fn entry(number: usize) -> Entry {
    Entry {
        entry_id: format!("entry_{number:06}"),
        path: name_policy::PortablePath::parse(&format!("f{:06}", number * 2)).unwrap(),
        kind: NamespaceEntryKind::File,
        file_id: format!("file_{number:06}"),
        version_id: format!("ver_{number:06}"),
        size: number as u64,
    }
}

fn assert_page(node: NamespaceNode, expected: &NamespacePage) {
    match node {
        NamespaceNode::Page(page) => assert_eq!(page.as_ref(), expected),
        NamespaceNode::Index(_) => panic!("fixture is a page"),
    }
}

fn exercise_eviction(source: &ObservedReader<'_>, root: &NamespaceReference, width: usize) {
    let member = &source.reader.members[0];
    source.node(root).unwrap();
    let path = object_path(member, root.kind.as_str(), &root.hash);
    let withheld = path.with_extension("withheld");
    std::fs::rename(&path, &withheld).unwrap();
    source.reset();
    let started = Instant::now();
    source.node(root).unwrap();
    assert_eq!(source.misses.get(), 0, "warm root needs no member object");
    source.report(width, "warm-withheld-root", started, (0, 0));

    let address = format!("{}/{}", root.kind.as_str(), root.hash);
    let initial_bytes = SegmentCaches::lock(&source.reader.caches.namespace).bytes;
    let mut admitted = 0;
    for batch in 0..1024 {
        let page = NamespacePage {
            format: Default::default(),
            entries: (0..128)
                .map(|item| NamespacePageEntry::File {
                    entry_id: format!("evict_{batch}_{item}"),
                    name: format!("page{batch:04}-item{item:04}-{}", "x".repeat(120)),
                    file_id: format!("file_{batch}_{item}"),
                    version_id: format!("ver_{batch}_{item}"),
                    size: item,
                })
                .collect(),
        };
        let hash = store_object(member, NAMESPACE_PAGE_KIND, &page).unwrap();
        let reference = NamespaceReference {
            kind: NamespaceRefKind::Page,
            hash,
        };
        assert_page(source.node(&reference).unwrap(), &page);
        admitted += storage_contract::canonical_json(&page).unwrap().len();
        let cache = SegmentCaches::lock(&source.reader.caches.namespace);
        assert!(cache.bytes <= MAX_CACHED_NAMESPACE_BYTES);
        if !cache.entries.contains_key(&address) {
            break;
        }
    }
    assert!(admitted + initial_bytes > MAX_CACHED_NAMESPACE_BYTES);
    assert!(
        !SegmentCaches::lock(&source.reader.caches.namespace)
            .entries
            .contains_key(&address)
    );
    assert_eq!(
        source.node(root).err().unwrap().code(),
        ErrorCode::ObjectCorrupt
    );
    std::fs::rename(&withheld, &path).unwrap();
    source.reset();
    let started = Instant::now();
    source.node(root).unwrap();
    assert_eq!(
        source.misses.get(),
        1,
        "evicted root re-enters through authenticated disk read"
    );
    source.report(width, "evicted-restored-root", started, (0, 0));
}

#[derive(Clone, Copy, Debug)]
enum Edit {
    Insert,
    Delete,
    Replace,
}

fn measure(width: usize) {
    let temporary = tempfile::tempdir().unwrap();
    let config = Config::development([temporary.path().join("member")]);
    private_dir(&config.disks[0].root).unwrap();
    let admitted = ValidatedConfig::try_from(config.clone()).unwrap();
    let resources = DiskResources::open(&admitted, false).unwrap();
    let entries: Vec<_> = (0..width).map(entry).collect();
    let mut objects = NamespaceObjects::new();
    let root = build_from_entries(&entries, &mut objects).unwrap();
    for object in objects.objects() {
        object.store_at(&resources, &config.disks).unwrap();
    }
    // Fixture construction remains outside point-edit timing; profiling retains
    // its separate call stacks, and process high-water memory still includes it.
    drop(objects);
    for position in [0, width / 2, width - 1] {
        let caches = SegmentCaches::new();
        let source = ObservedReader {
            reader: SegmentReader {
                resources: &resources,
                members: &config.disks,
                caches: &caches,
            },
            calls: Cell::new(0),
            misses: Cell::new(0),
            canonical_bytes: Cell::new(0),
        };
        let original = &entries[position];
        for phase in ["cold-point", "warm-point"] {
            source.reset();
            let started = Instant::now();
            let actual = NamespaceView::new(&root, &source)
                .resolve_key(original.lookup_key())
                .unwrap();
            assert_eq!(actual.as_ref(), Some(original));
            if phase == "warm-point" {
                assert_eq!(source.misses.get(), 0);
            } else {
                assert!(source.misses.get() > 0);
            }
            source.report(width, &format!("{phase}-{position}"), started, (0, 0));
        }
        for operation in [Edit::Insert, Edit::Delete, Edit::Replace] {
            let mut cold_root = None;
            for temperature in ["cold", "warm"] {
                if temperature == "cold" {
                    *SegmentCaches::lock(&caches.namespace) =
                        BoundedCache::new(usize::MAX, MAX_CACHED_NAMESPACE_BYTES);
                }
                source.reset();
                let started = Instant::now();
                let mut editor = NamespaceEditor::open(&root, &source).unwrap();
                let mut replacement = original.clone();
                match operation {
                    Edit::Insert => {
                        replacement.entry_id = format!("entry_added_{position}");
                        replacement.file_id = format!("file_added_{position}");
                        replacement.version_id = format!("ver_added_{position}");
                        replacement.path =
                            name_policy::PortablePath::parse(&format!("{}a", original.path()))
                                .unwrap();
                        editor.upsert_file(&replacement).unwrap();
                    }
                    Edit::Delete => editor.delete(original.lookup_key(), false).unwrap(),
                    Edit::Replace => {
                        replacement.version_id = "ver_replaced".into();
                        replacement.size += 1;
                        editor.upsert_file(&replacement).unwrap();
                    }
                }
                let mut fresh = NamespaceObjects::new();
                let changed = editor.seal(&mut fresh).unwrap();
                let encoding = fresh.encoding_work();
                if temperature == "warm" {
                    assert_eq!(source.misses.get(), 0);
                }
                assert!(source.calls.get() <= 24);
                assert!(encoding.0 <= 12);
                assert!(encoding.1 <= 12 * storage_contract::NAMESPACE_PAGE_TARGET_BYTES);
                assert!(
                    source.canonical_bytes.get()
                        <= 24 * storage_contract::NAMESPACE_PAGE_TARGET_BYTES
                );
                source.report(
                    width,
                    &format!("{temperature}-{operation:?}-{position}"),
                    started,
                    encoding,
                );
                if temperature == "cold" {
                    cold_root = Some(changed);
                    continue;
                }
                assert_eq!(Some(&changed), cold_root.as_ref());
                let view_source = DraftSource {
                    fresh: &fresh,
                    base: &source,
                };
                let actual = NamespaceView::new(&changed, &view_source)
                    .collect_entries()
                    .unwrap();
                let mut expected = entries.clone();
                match operation {
                    Edit::Insert => expected.insert(position + 1, replacement),
                    Edit::Delete => {
                        expected.remove(position);
                    }
                    Edit::Replace => expected[position] = replacement,
                }
                assert_eq!(actual, expected);
            }
        }
    }
    let caches = SegmentCaches::new();
    let source = ObservedReader {
        reader: SegmentReader {
            resources: &resources,
            members: &config.disks,
            caches: &caches,
        },
        calls: Cell::new(0),
        misses: Cell::new(0),
        canonical_bytes: Cell::new(0),
    };
    exercise_eviction(&source, &root.root, width);
}

#[test]
#[ignore = "isolated acceptance measurement; run explicitly in release mode"]
fn namespace_cache_adoption_1000() {
    measure(1_000);
}

#[test]
#[ignore = "isolated acceptance measurement; run explicitly in release mode"]
fn namespace_cache_adoption_10000() {
    measure(10_000);
}

#[test]
#[ignore = "isolated acceptance measurement; run explicitly in release mode"]
fn namespace_cache_adoption_100000() {
    measure(100_000);
}
