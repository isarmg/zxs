use super::*;
use crate::{
    disk::private_dir,
    engine::ValidatedConfig,
    journal::{object_path, store_object},
};
use storage_contract::{Config, EVENT_SEGMENT_KIND, EVENT_SEGMENT_SPAN, EventSegment};

fn event(sequence: u64) -> Event {
    Event {
        sequence,
        operation_id: format!("op_{sequence:032x}").parse().unwrap(),
        kind: "test".into(),
        object_id: "object".into(),
    }
}

fn fixture(
    root: &std::path::Path,
    all: &[Event],
) -> (Vec<DiskConfig>, DiskResources, SegmentedEvents) {
    let config = Config::development([root.join("disk")]);
    private_dir(&config.disks[0].root).unwrap();
    let admitted = ValidatedConfig::try_from(config.clone()).unwrap();
    let resources = DiskResources::open(&admitted, false).unwrap();
    let events = SegmentedEvents::from_events(all.to_vec()).unwrap();
    for (reference, chunk) in events
        .sealed()
        .iter()
        .zip(all.as_chunks::<EVENT_SEGMENT_SPAN>().0)
    {
        let segment = EventSegment {
            format: Default::default(),
            events: chunk.to_vec(),
        };
        assert_eq!(
            store_object(&config.disks[0], EVENT_SEGMENT_KIND, &segment).unwrap(),
            reference.hash
        );
    }
    (config.disks, resources, events)
}

#[test]
fn detached_event_selection_matches_the_sequence_suffix_with_bounded_capture() {
    let temp = tempfile::tempdir().unwrap();
    let mut all: Vec<_> = (0..3 * EVENT_SEGMENT_SPAN + 7)
        .map(|n| event(2 * n as u64))
        .collect();
    all[11].sequence = all[10].sequence;
    all[2 * EVENT_SEGMENT_SPAN].sequence = all[2 * EVENT_SEGMENT_SPAN - 1].sequence;
    let (members, resources, events) = fixture(temp.path(), &all);
    let caches = SegmentCaches::new();
    let reader = SegmentReader {
        resources: &resources,
        members: &members,
        caches: &caches,
        observe: None,
    };
    for after in [0, 1, 20, 2046, 2048, 4094, 4096, 6142, 6144, u64::MAX] {
        let rows = EventRows::capture(&events, after);
        assert!(rows.sealed.len() <= 2);
        assert!(rows.tail.len() <= PAGE_SIZE);
        if rows.sealed.len() == 2 {
            assert!(rows.tail.is_empty());
        }
        let expected: Vec<_> = all
            .iter()
            .filter(|event| event.sequence > after)
            .take(PAGE_SIZE)
            .cloned()
            .collect();
        assert_eq!(
            rows.read(&reader, after).unwrap(),
            expected,
            "after {after}"
        );
    }
}

#[test]
fn detached_event_read_does_not_touch_an_unneeded_second_segment() {
    let temp = tempfile::tempdir().unwrap();
    let all: Vec<_> = (1..=2 * EVENT_SEGMENT_SPAN + 5)
        .map(|n| event(n as u64))
        .collect();
    let (members, resources, events) = fixture(temp.path(), &all);
    std::fs::remove_file(object_path(
        &members[0],
        EVENT_SEGMENT_KIND,
        &events.sealed()[1].hash,
    ))
    .unwrap();
    let caches = SegmentCaches::new();
    let reader = SegmentReader {
        resources: &resources,
        members: &members,
        caches: &caches,
        observe: None,
    };
    let rows = EventRows::capture(&events, 0);
    assert!(rows.tail.is_empty());
    assert_eq!(rows.read(&reader, 0).unwrap(), all[..PAGE_SIZE]);
    assert!(
        EventRows::capture(&events, 1000)
            .read(&reader, 1000)
            .is_err()
    );
    let after = events.tail()[0].sequence;
    assert_eq!(
        EventRows::capture(&events, after)
            .read(&reader, after)
            .unwrap(),
        events.tail()[1..]
    );
}

#[test]
fn detached_event_selection_skips_repeated_first_sequences_without_changing_cursor_semantics() {
    let temp = tempfile::tempdir().unwrap();
    let mut all = vec![event(1); 2 * EVENT_SEGMENT_SPAN];
    all.extend((2..=EVENT_SEGMENT_SPAN + 1).map(|n| event(n as u64)));
    let (members, resources, events) = fixture(temp.path(), &all);
    let caches = SegmentCaches::new();
    let reader = SegmentReader {
        resources: &resources,
        members: &members,
        caches: &caches,
        observe: None,
    };
    let rows = EventRows::capture(&events, 1);
    assert_eq!(rows.sealed.len(), 2);
    assert_eq!(
        rows.read(&reader, 1).unwrap(),
        all[2 * EVENT_SEGMENT_SPAN..2 * EVENT_SEGMENT_SPAN + PAGE_SIZE]
    );
    assert!(
        EventRows::capture(&events, u64::MAX)
            .read(&reader, u64::MAX)
            .unwrap()
            .is_empty()
    );
    std::fs::remove_file(object_path(
        &members[0],
        EVENT_SEGMENT_KIND,
        &events.sealed()[2].hash,
    ))
    .unwrap();
    let fresh = SegmentCaches::new();
    let reader = SegmentReader {
        resources: &resources,
        members: &members,
        caches: &fresh,
        observe: None,
    };
    assert!(
        EventRows::capture(&events, u64::MAX)
            .read(&reader, u64::MAX)
            .is_err()
    );
}

#[test]
fn detached_event_rows_survive_a_new_tail_seal() {
    let temp = tempfile::tempdir().unwrap();
    let all: Vec<_> = (1..2 * EVENT_SEGMENT_SPAN)
        .map(|n| event(n as u64))
        .collect();
    let (members, resources, mut events) = fixture(temp.path(), &all);
    let after = EVENT_SEGMENT_SPAN as u64;
    let rows = EventRows::capture(&events, after);
    events.push(event(2 * EVENT_SEGMENT_SPAN as u64)).unwrap();
    assert!(events.tail().is_empty());
    let caches = SegmentCaches::new();
    let reader = SegmentReader {
        resources: &resources,
        members: &members,
        caches: &caches,
        observe: None,
    };
    assert_eq!(
        rows.read(&reader, after).unwrap(),
        all[EVENT_SEGMENT_SPAN..EVENT_SEGMENT_SPAN + PAGE_SIZE]
    );
}

#[test]
fn detached_event_plan_owns_resources_after_engine_drop() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temp.path().join("system"),
        Config::development([temp.path().join("disk")]),
    )
    .unwrap();
    engine
        .create_repository(
            &storage_contract::RepositoryName::parse("retained").unwrap(),
            "P01",
            1,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let expected = engine.events_page(0, PAGE_SIZE).unwrap();
    assert!(!expected.is_empty());
    let plan = engine.prepare_event_page(0);
    drop(engine);
    assert_eq!(plan.read().unwrap(), expected);
}

#[test]
fn detached_event_plan_reads_cold_segments_after_the_source_log_is_dropped() {
    let temp = tempfile::tempdir().unwrap();
    let all: Vec<_> = (1..=2 * EVENT_SEGMENT_SPAN)
        .map(|n| event(n as u64))
        .collect();
    let (members, resources, events) = fixture(temp.path(), &all);
    let plan = EventPageReadPlan {
        resources: Arc::new(resources),
        members,
        caches: Arc::new(SegmentCaches::new()),
        rows: EventRows::capture(&events, 1000),
        after: 1000,
    };
    drop(events);
    assert_eq!(plan.read().unwrap(), all[1000..2000]);
}
