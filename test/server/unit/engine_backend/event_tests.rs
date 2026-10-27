use super::*;
use storage_contract::{Config, Event, TrackedEvents};

fn event(sequence: u64) -> Event {
    Event {
        sequence,
        operation_id: format!("op_{sequence:032x}").parse().unwrap(),
        kind: "test".into(),
        object_id: "object".into(),
    }
}

// Test fixtures supply the monotone premise directly; production cursor
// selection is exposed only through the admitted Engine view.
fn events_after(events: &TrackedEvents, after: u64) -> impl ExactSizeIterator<Item = &Event> {
    let rank = events.partition_point(|event| event.sequence <= after);
    events[rank..].iter()
}

#[test]
fn event_cursor_seeks_past_boundaries_and_gaps_without_copying() {
    assert_eq!(events_after(&TrackedEvents::default(), 0).len(), 0);
    let events: TrackedEvents = [1, 3, 3, 8, u64::MAX].map(event).into_iter().collect();
    for after in [0, 1, 2, 3, 4, 8, 9, u64::MAX] {
        let expected = events
            .iter()
            .filter(|event| event.sequence > after)
            .collect::<Vec<_>>();
        let actual = events_after(&events, after).collect::<Vec<_>>();
        assert_eq!(actual, expected);
        assert!(
            actual
                .iter()
                .zip(expected)
                .all(|(actual, expected)| std::ptr::eq(*actual, expected))
        );
    }
}

#[test]
fn event_cursor_at_large_history_tail_returns_only_unseen_events() {
    let events: TrackedEvents = (1..=100_000).map(event).collect();
    assert_eq!(
        events_after(&events, 99_999).collect::<Vec<_>>(),
        vec![&events[99_999]]
    );
    assert_eq!(events_after(&events, 100_000).len(), 0);
}

fn poll(backend: &EngineBackend, identity: &Identity, after: u64) -> Result<Value, ApiError> {
    backend.event_page(identity, after).map(|page| {
        json_codec::to_value(crate::backend_events::EventPageResponse::from(page)).unwrap()
    })
}

#[test]
fn event_cursor_preserves_singleton_visibility_progress_and_recovery_order() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("system");
    let mut engine =
        Engine::initialize(&root, Config::development([temporary.path().join("disk")])).unwrap();
    let identity = install_test_administrator(&mut engine, "owner");
    let baseline = engine.state().sequence;
    let backend = EngineBackend::new(engine);
    assert_eq!(
        poll(&backend, &identity, baseline).unwrap(),
        json!({"events":[],"next_cursor":baseline.to_string()})
    );
    assert_eq!(
        poll(&backend, &identity, baseline + 1).unwrap_err().code,
        storage_contract::ErrorCode::InvalidCursor
    );
    let (first, last) = {
        let mut engine = backend.engine.lock().unwrap();
        engine
            .create_repository(
                &storage_contract::RepositoryName::parse("First").unwrap(),
                "P01",
                1,
                "owner",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
        let first = engine.state().sequence;
        engine
            .create_repository(
                &storage_contract::RepositoryName::parse("Second").unwrap(),
                "P01",
                1,
                "owner",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
        (first, engine.state().sequence)
    };
    let expected = poll(&backend, &identity, baseline).unwrap();
    assert_eq!(expected["events"].as_array().unwrap().len(), 2);
    assert_eq!(expected["events"][0]["sequence"], first.to_string());
    assert_eq!(expected["events"][1]["sequence"], last.to_string());
    assert_eq!(expected["next_cursor"], last.to_string());
    assert_eq!(
        poll(&backend, &identity, last).unwrap(),
        json!({"events":[],"next_cursor":last.to_string()})
    );
    assert_eq!(
        poll(&backend, &identity, last + 1).unwrap_err().code,
        storage_contract::ErrorCode::InvalidCursor
    );
    let stranger = Identity {
        principal: "other".into(),
        ..identity.clone()
    };
    assert_eq!(
        poll(&backend, &stranger, 0).unwrap_err().code,
        storage_contract::ErrorCode::Unauthorized
    );
    drop(backend);
    let backend = EngineBackend::new(Engine::open(&root).unwrap());
    assert_eq!(poll(&backend, &identity, baseline).unwrap(), expected);
    assert_eq!(
        poll(&backend, &identity, first).unwrap()["events"][0]["sequence"],
        last.to_string()
    );
}

#[test]
fn typed_event_page_copies_only_bounded_suffix_and_keeps_cursor_gaps() {
    let events: TrackedEvents = (1..=100_000).map(event).collect();
    let page = capture_event_page(events_after(&events, 98_500), 100_005, 98_500).unwrap();
    assert_eq!(page.events.len(), 1000);
    assert_eq!(page.next_cursor, 99_500);
    assert_eq!(
        page.events,
        events[98_500..]
            .iter()
            .take(1000)
            .cloned()
            .collect::<Vec<_>>()
    );
    assert_ne!(page.events[0].kind.as_ptr(), events[98_500].kind.as_ptr());
    let last = capture_event_page(
        events_after(&events, page.next_cursor),
        100_005,
        page.next_cursor,
    )
    .unwrap();
    assert_eq!(last.events.len(), 500);
    assert_eq!(last.next_cursor, 100_000);
    let empty = capture_event_page(events_after(&events, 100_004), 100_005, 100_004).unwrap();
    assert!(empty.events.is_empty());
    assert_eq!(empty.next_cursor, 100_004);
    assert_eq!(
        capture_event_page(events_after(&events, 100_006), 100_005, 100_006)
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::InvalidCursor
    );
    assert_eq!(
        capture_event_page(std::iter::empty(), u64::MAX, u64::MAX)
            .unwrap()
            .next_cursor,
        u64::MAX
    );
}

#[test]
fn typed_event_page_is_frozen_and_serializes_without_the_engine_lock() {
    use data_serialization::{Serialize, Serializer, ser::Error as _};
    let root = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &root.path().join("system"),
        Config::development([root.path().join("disk")]),
    )
    .unwrap();
    let identity = install_test_administrator(&mut engine, "owner");
    engine
        .create_repository(
            &storage_contract::RepositoryName::parse("first").unwrap(),
            "P01",
            1,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let expected = json!({"events":engine.state().events,"next_cursor":engine.state().events.iter().next_back().unwrap().sequence.to_string()});
    let backend = EngineBackend::new(engine);
    let snapshot = backend.event_page(&identity, 0).unwrap();
    backend
        .engine
        .lock()
        .unwrap()
        .create_repository(
            &storage_contract::RepositoryName::parse("second").unwrap(),
            "P01",
            1,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    struct Probe<'a>(
        &'a EngineBackend,
        &'a crate::backend_events::EventPageResponse,
    );
    impl Serialize for Probe<'_> {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let unlocked = std::thread::scope(|scope| {
                scope
                    .spawn(|| self.0.engine.try_lock().is_ok())
                    .join()
                    .unwrap()
            });
            if !unlocked {
                return Err(S::Error::custom(
                    "event serialization retained engine mutex",
                ));
            }
            self.1.serialize(serializer)
        }
    }
    let response = crate::backend_events::EventPageResponse::from(snapshot);
    assert_eq!(
        json_codec::to_value(Probe(&backend, &response)).unwrap(),
        expected
    );
    drop(backend);
    assert_eq!(json_codec::to_value(response).unwrap(), expected);
}
