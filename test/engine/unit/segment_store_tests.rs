use super::*;
use crate::disk::private_dir;
use crate::engine::ValidatedConfig;
use crate::journal::{object_path, store_object};
use storage_contract::{Config, Event, SegmentedEvents};

#[test]
fn cache_byte_limit_rejects_an_oversized_entry_without_evicting_valid_entries() {
    let mut cache = BoundedCache::<i32>::new(8, 32);
    cache.insert("oversized".into(), 1, 33);
    assert!(cache.entries.is_empty());
    assert_eq!(cache.bytes, 0);
    cache.insert("kept".into(), 2, 16);
    cache.insert("oversized".into(), 1, 33);
    assert_eq!(cache.get("kept"), Some(2));
    assert_eq!(cache.bytes, 16);
    cache.insert("next".into(), 3, 32);
    assert_eq!(cache.get("next"), Some(3));
    assert!(cache.get("kept").is_none());
    assert_eq!(cache.bytes, 32);
}

fn event(sequence: u64) -> Event {
    Event {
        sequence,
        operation_id: format!("op_{sequence:032x}").parse().unwrap(),
        kind: "MUTATION".into(),
        object_id: "paged".into(),
    }
}

/// Sparse publication slots with one duplicate pair inside segment 0 and one
/// duplicate straddling the second seal boundary.
fn sequences(count: usize) -> Vec<u64> {
    let mut sequences: Vec<u64> = (0..count as u64).map(|index| 2 * index).collect();
    sequences[11] = sequences[10];
    if count > 2 * EVENT_SEGMENT_SPAN {
        sequences[2 * EVENT_SEGMENT_SPAN] = sequences[2 * EVENT_SEGMENT_SPAN - 1];
    }
    sequences
}

fn fixture(
    temp: &std::path::Path,
    count: usize,
) -> (Vec<DiskConfig>, DiskResources, SegmentedEvents) {
    let config = Config::development([temp.join("a"), temp.join("b")]);
    for disk in &config.disks {
        private_dir(&disk.root).unwrap();
    }
    let admitted = ValidatedConfig::try_from(config.clone()).unwrap();
    let resources = DiskResources::open(&admitted, false).unwrap();
    let events =
        SegmentedEvents::from_events(sequences(count).into_iter().map(event).collect()).unwrap();
    for (ordinal, reference) in events.sealed().iter().enumerate() {
        let segment = EventSegment {
            format: Default::default(),
            events: sequences(count)
                [ordinal * EVENT_SEGMENT_SPAN..(ordinal + 1) * EVENT_SEGMENT_SPAN]
                .iter()
                .copied()
                .map(event)
                .collect(),
        };
        for disk in &config.disks {
            assert_eq!(
                store_object(disk, EVENT_SEGMENT_KIND, &segment).unwrap(),
                reference.hash
            );
        }
    }
    (config.disks, resources, events)
}

#[test]
fn event_pages_locate_segments_through_the_directory_and_never_expand_history() {
    let temp = tempfile::tempdir().unwrap();
    let count = 2 * EVENT_SEGMENT_SPAN + 10;
    let (members, resources, events) = fixture(temp.path(), count);
    let caches = SegmentCaches::new();
    let reader = SegmentReader {
        resources: &resources,
        members: &members,
        caches: &caches,
    };
    let all: Vec<Event> = sequences(count).into_iter().map(event).collect();
    let boundary = events.sealed()[1].first_sequence;
    let tail_first = events.tail()[0].sequence;
    // The paged read equals the sequence-filtered suffix for cursors on
    // gaps, duplicates, seal boundaries and the far end, at several limits.
    for after in [
        0,
        1,
        sequences(count)[10],
        sequences(count)[12],
        boundary - 2,
        boundary,
        tail_first - 2,
        tail_first,
        2 * (count as u64) + 10,
    ] {
        for limit in [0, 1, 3, EVENT_SEGMENT_SPAN, count + 1] {
            let expected: Vec<Event> = all
                .iter()
                .filter(|event| event.sequence > after)
                .take(limit)
                .cloned()
                .collect();
            assert_eq!(
                events_page(&events, &reader, after, limit).unwrap(),
                expected,
                "after {after} limit {limit}"
            );
        }
    }
    // Reads are located through the resident directory first: a cursor inside
    // segment 1 never touches segment 0's body.
    for disk in &members {
        std::fs::remove_file(object_path(
            disk,
            EVENT_SEGMENT_KIND,
            &events.sealed()[0].hash,
        ))
        .unwrap();
    }
    let fresh = SegmentCaches::new();
    let reader = SegmentReader {
        resources: &resources,
        members: &members,
        caches: &fresh,
    };
    let from_second = events_page(&events, &reader, boundary, count).unwrap();
    assert_eq!(
        from_second.len(),
        EVENT_SEGMENT_SPAN - 1 + events.tail().len()
    );
    // An older cursor needs that sealed body and fails closed instead of
    // skipping or guessing history.
    assert_eq!(
        events_page(&events, &reader, 0, count).unwrap_err().code(),
        ErrorCode::ObjectCorrupt
    );
    // Segment 1 is now cached: its body can disappear from every member and
    // the same page still serves; tail cursors never read bodies at all.
    for disk in &members {
        std::fs::remove_file(object_path(
            disk,
            EVENT_SEGMENT_KIND,
            &events.sealed()[1].hash,
        ))
        .unwrap();
    }
    assert_eq!(
        events_page(&events, &reader, boundary, count).unwrap(),
        from_second
    );
    assert_eq!(
        events_page(&events, &reader, tail_first, count)
            .unwrap()
            .len(),
        events.tail().len() - 1
    );
}

#[test]
fn namespace_cache_keys_preserve_kind_and_share_one_fifo_budget() {
    let mut cache = BoundedCache::new(8, 32);
    let page = NamespaceCacheKey::Page([b'a'; 64]);
    let index = NamespaceCacheKey::Index([b'a'; 64]);
    let next = NamespaceCacheKey::Page([b'b'; 64]);
    cache.insert(page, 1, 16);
    assert!(cache.get(&index).is_none());
    cache.insert(index, 2, 16);
    assert_eq!(cache.get(&page), Some(1));
    assert_eq!(cache.get(&index), Some(2));
    // Hits and duplicate insertion retain FIFO order and the original charge.
    cache.insert(page, 3, 8);
    assert_eq!(cache.bytes, 32);
    assert_eq!(cache.get(&page), Some(1));
    cache.insert(next, 4, 33);
    assert_eq!(cache.get(&page), Some(1));
    assert_eq!(cache.get(&index), Some(2));
    cache.insert(next, 4, 16);
    assert!(cache.get(&page).is_none());
    assert_eq!(cache.get(&index), Some(2));
    assert_eq!(cache.get(&next), Some(4));
    assert_eq!(cache.bytes, 32);
}

#[test]
fn namespace_cache_shares_authenticated_nodes_without_aliasing_kind_or_invalid_hashes() {
    use crate::namespace::NamespaceNode;
    use storage_contract::{NamespacePage, NamespaceRefKind, NamespaceReference};

    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([temp.path().join("a"), temp.path().join("b")]);
    for disk in &config.disks {
        private_dir(&disk.root).unwrap();
    }
    let admitted = ValidatedConfig::try_from(config.clone()).unwrap();
    let resources = DiskResources::open(&admitted, false).unwrap();
    let page = NamespacePage {
        format: Default::default(),
        entries: Vec::new(),
    };
    let hashes: Vec<_> = config
        .disks
        .iter()
        .map(|disk| store_object(disk, storage_contract::NAMESPACE_PAGE_KIND, &page).unwrap())
        .collect();
    assert_eq!(hashes[0], hashes[1]);
    let reference = NamespaceReference {
        kind: NamespaceRefKind::Page,
        hash: hashes[0].clone(),
    };
    let caches = SegmentCaches::new();
    let reader = SegmentReader {
        resources: &resources,
        members: &config.disks,
        caches: &caches,
    };
    let NamespaceNode::Page(first) = reader.namespace_node(&reference).unwrap() else {
        panic!("expected a namespace page");
    };
    for disk in &config.disks {
        std::fs::remove_file(object_path(
            disk,
            storage_contract::NAMESPACE_PAGE_KIND,
            &reference.hash,
        ))
        .unwrap();
    }
    let NamespaceNode::Page(cached) = reader.namespace_node(&reference).unwrap() else {
        panic!("expected the cached namespace page");
    };
    assert!(Arc::ptr_eq(&first, &cached));
    let other_kind = NamespaceReference {
        kind: NamespaceRefKind::Index,
        hash: reference.hash.clone(),
    };
    let Err(error) = reader.namespace_node(&other_kind) else {
        panic!("a page cache entry must not satisfy an index reference");
    };
    assert_eq!(error.code(), ErrorCode::ObjectCorrupt);
    for hash in [
        "a".repeat(63),
        "a".repeat(65),
        "A".repeat(64),
        "g".repeat(64),
    ] {
        let invalid = NamespaceReference {
            kind: NamespaceRefKind::Page,
            hash,
        };
        let Err(error) = reader.namespace_node(&invalid) else {
            panic!("an invalid namespace reference must be rejected");
        };
        assert_eq!(error.code(), ErrorCode::ObjectCorrupt);
        assert!(
            error
                .to_string()
                .contains("invalid namespace object reference")
        );
    }
}
