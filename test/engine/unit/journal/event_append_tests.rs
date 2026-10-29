use super::*;
use std::collections::BTreeSet;
use std::sync::Arc;
use storage_contract::{EVENT_SEGMENT_KIND, EVENT_SEGMENT_SPAN, SegmentChanges};

fn event(index: usize) -> Event {
    Event {
        sequence: index as u64,
        operation_id: format!("op_{index:032x}").parse().unwrap(),
        kind: "MUTATION".into(),
        object_id: "permanent event".into(),
    }
}
fn state(count: usize) -> State {
    State {
        sequence: count as u64,
        events: storage_contract::SegmentedEvents::from_events((0..count).map(event).collect())
            .unwrap(),
        ..Default::default()
    }
}
fn append(before: &State, count: usize) -> State {
    let mut after = before.clone();
    after.begin_tracking();
    let start = before.events.len() as usize;
    for index in start..start + count {
        after.events.push(event(index)).unwrap();
    }
    after.sequence += count as u64;
    after
}
fn config(temp: &std::path::Path) -> Config {
    Config::development([temp.join("one"), temp.join("two"), temp.join("three")])
}
/// Sealed segment bodies live on the members since the publication that
/// sealed them; a fixture state assembled in memory installs them the same
/// way before its first store, because only the open tail and one fresh seal
/// stay resident.
fn install_sealed_bodies(config: &Config, state: &State) {
    for (ordinal, reference) in state.events.sealed().iter().enumerate() {
        let segment = storage_contract::EventSegment {
            format: Default::default(),
            events: (ordinal * EVENT_SEGMENT_SPAN..(ordinal + 1) * EVENT_SEGMENT_SPAN)
                .map(event)
                .collect(),
        };
        for disk in &config.disks {
            assert_eq!(
                crate::journal::store_object(disk, EVENT_SEGMENT_KIND, &segment).unwrap(),
                reference.hash
            );
        }
    }
}
fn assert_exact_encoding(actual: &PreparedSnapshot, state: &State) {
    let expected = prepare_snapshot_reference(state, MAX_METADATA).unwrap();
    let full = prepare(state, MAX_METADATA).unwrap();
    assert_eq!(actual.hash, expected.hash);
    assert_eq!(full.hash, expected.hash);
    assert_eq!(
        canonical_json(&actual.descriptor).unwrap(),
        canonical_json(&expected.descriptor).unwrap()
    );
    for prepared in [actual, &full] {
        for object in &prepared.objects {
            let reference = expected
                .objects
                .iter()
                .find(|entry| entry.kind == object.kind && entry.hash == object.hash)
                .unwrap();
            assert_eq!(object.bytes, reference.bytes);
        }
    }
    assert_eq!(full.objects.len(), expected.objects.len());
}
fn event_nodes(prepared: &PreparedSnapshot) -> BTreeMap<String, crate::metadata_map::Reference> {
    let mut nodes = BTreeMap::new();
    let FieldRoot::Segments(events) = &prepared.fields["events"] else {
        panic!("event provenance must retain typed segment ordinals")
    };
    events
        .visit_nodes("", &mut |root, prefix| {
            nodes.insert(prefix.to_owned(), root.reference().clone());
            Ok(())
        })
        .unwrap();
    nodes
}
fn event_segment_objects(prepared: &PreparedSnapshot) -> Vec<&PreparedMetadataObject> {
    prepared
        .objects
        .iter()
        .filter(|object| object.kind == EVENT_SEGMENT_KIND)
        .collect()
}

#[test]
fn tail_appends_touch_only_the_open_segment_and_match_every_current_format_byte() {
    for count in [
        0,
        1,
        EVENT_SEGMENT_SPAN - 1,
        EVENT_SEGMENT_SPAN,
        3 * EVENT_SEGMENT_SPAN,
    ] {
        let before = state(count);
        let base = prepare(&before, MAX_METADATA).unwrap();
        let basis = base.reuse_basis();
        for appended in [0, 1, 2] {
            let after = append(&before, appended);
            let changes = after.events.changes_since(&before.events);
            let actual = prepare_incremental(
                &before,
                &after,
                &basis,
                MAX_METADATA,
                &mut no_reused_sources,
            )
            .unwrap();
            // Each appended event dirties exactly the ordinal it lands in: a
            // crossing append keeps the sealed ordinal dirty and the empty
            // new tail adds no key.
            let mut dirty = BTreeSet::new();
            let mut tail_len = before.events.tail().len();
            let mut ordinal = before.events.tail_ordinal();
            for _ in 0..appended {
                dirty.insert(ordinal);
                tail_len += 1;
                if tail_len == EVENT_SEGMENT_SPAN {
                    ordinal += 1;
                    tail_len = 0;
                }
            }
            assert_eq!(changes, SegmentChanges::Segments(Arc::new(dirty.clone())));
            if appended == 0 {
                assert_eq!(actual.descriptor.events, base.descriptor.events);
            } else {
                assert_ne!(actual.descriptor.events, base.descriptor.events);
                // One prepared body per dirty ordinal: the fresh seal when the
                // span was crossed, and the non-empty open tail.
                let bodies = event_segment_objects(&actual);
                assert_eq!(bodies.len(), dirty.len());
                for body in &bodies {
                    assert!(body.bytes.len() < MAX_METADATA);
                }
                // Every segment outside the dirty routes keeps its exact
                // reference; only the dirty ordinals' directory routes
                // re-encode.
                let dirty_routes = dirty
                    .iter()
                    .map(|ordinal| {
                        blake3::hash(format!("{ordinal:020}").as_bytes())
                            .to_hex()
                            .to_string()
                    })
                    .collect::<Vec<_>>();
                let current = event_nodes(&actual);
                for (prefix, reference) in event_nodes(&base) {
                    if !dirty_routes.iter().any(|route| route.starts_with(&prefix)) {
                        assert_eq!(current.get(&prefix), Some(&reference), "{prefix}");
                    }
                }
            }
            assert_exact_encoding(&actual, &after);
        }
    }
}

#[test]
fn crossing_the_span_seals_an_immutable_segment_and_the_next_transaction_reuses_it() {
    let before = state(EVENT_SEGMENT_SPAN - 1);
    let base = prepare(&before, MAX_METADATA).unwrap();
    // Appending two events seals segment 0 at exactly the span and starts a
    // new tail: both ordinals are dirty and both bodies are prepared.
    let sealed = append(&before, 2);
    assert_eq!(sealed.events.sealed().len(), 1);
    assert_eq!(sealed.events.tail().len(), 1);
    assert_eq!(
        sealed.events.changes_since(&before.events),
        SegmentChanges::Segments(Arc::new(BTreeSet::from([0, 1])))
    );
    let actual = prepare_incremental(
        &before,
        &sealed,
        &base.reuse_basis(),
        MAX_METADATA,
        &mut no_reused_sources,
    )
    .unwrap();
    let bodies = event_segment_objects(&actual);
    assert_eq!(bodies.len(), 2);
    assert!(
        bodies
            .iter()
            .any(|object| object.hash == sealed.events.sealed()[0].hash)
    );
    assert_exact_encoding(&actual, &sealed);
    // The following transaction re-prepares only its own open tail; the
    // sealed body is byte-stable and already installed.
    let next = append(&sealed, 1);
    let actual_next = prepare_incremental(
        &sealed,
        &next,
        &actual.reuse_basis(),
        MAX_METADATA,
        &mut no_reused_sources,
    )
    .unwrap();
    assert_eq!(
        next.events.changes_since(&sealed.events),
        SegmentChanges::Segments(Arc::new(BTreeSet::from([1])))
    );
    let bodies = event_segment_objects(&actual_next);
    assert_eq!(bodies.len(), 1);
    assert_ne!(bodies[0].hash, next.events.sealed()[0].hash);
    assert_exact_encoding(&actual_next, &next);
}

#[test]
fn event_mutation_replacement_and_decode_take_the_full_encoding_path() {
    let before = state(300);
    let base = prepare(&before, MAX_METADATA).unwrap();
    let basis = base.reuse_basis();
    let mut states = Vec::new();
    let mut changed_events = (0..301).map(event).collect::<Vec<_>>();
    changed_events[0].object_id = "changed prefix".into();
    let mut changed = before.clone();
    changed.events = storage_contract::SegmentedEvents::from_events(changed_events).unwrap();
    changed.sequence += 1;
    states.push(changed);
    let mut retained = before.clone();
    retained.events = storage_contract::SegmentedEvents::from_events(
        (0..300)
            .map(event)
            .filter(|e| e.sequence % 2 == 0)
            .collect(),
    )
    .unwrap();
    states.push(retained);
    let mut replaced = before.clone();
    replaced.events =
        storage_contract::SegmentedEvents::from_events((0..300).map(event).collect()).unwrap();
    states.push(replaced);
    let mut empty = before.clone();
    empty.events = storage_contract::SegmentedEvents::new();
    states.push(empty);
    states.push(json_codec::from_slice(&canonical_json(&before).unwrap()).unwrap());
    for after in states {
        assert_eq!(
            after.events.changes_since(&before.events),
            SegmentChanges::All
        );
        let actual = prepare_incremental(
            &before,
            &after,
            &basis,
            MAX_METADATA,
            &mut no_reused_sources,
        )
        .unwrap();
        for reference in event_nodes(&actual).values() {
            assert!(actual.objects.iter().any(|object| {
                object.kind == reference.kind().domain() && object.hash == reference.hash()
            }));
        }
        assert_exact_encoding(&actual, &after);
    }
}

#[test]
fn appended_events_reconstruct_missing_reused_directory_pages_and_read_back_all_members() {
    let temp = tempfile::tempdir().unwrap();
    let config = config(temp.path());
    // More than 128 segment references force the directory to have multiple
    // radix leaves. Repeated nondecreasing events let all sealed references
    // share one real, hash-verified body without constructing 256 full bodies.
    let sealed_segments = 256;
    let segment = storage_contract::EventSegment {
        format: Default::default(),
        events: vec![event(0); EVENT_SEGMENT_SPAN],
    };
    let sealed = storage_contract::object_hash(EVENT_SEGMENT_KIND, &segment).unwrap();
    for disk in &config.disks {
        assert_eq!(
            store_object(disk, EVENT_SEGMENT_KIND, &segment).unwrap(),
            sealed
        );
    }
    let tail_sequence = sealed_segments * EVENT_SEGMENT_SPAN;
    let before = State {
        sequence: tail_sequence as u64,
        events: json_codec::from_value(json_codec::json!({
            "sealed": vec![storage_contract::EventSegmentRef {
                hash: sealed.clone(),
                first_sequence: 0,
            }; sealed_segments],
            "tail": [event(tail_sequence)],
        }))
        .unwrap(),
        ..Default::default()
    };
    let base = prepare(&before, MAX_METADATA).unwrap();
    base.store(&config.disks, &before).unwrap();
    let after = append(&before, 1);
    let actual = prepare_incremental(
        &before,
        &after,
        &base.reuse_basis(),
        MAX_METADATA,
        &mut no_reused_sources,
    )
    .unwrap();
    // A reused directory page missing on every member reconstructs from the
    // resident directory entries; a sealed body missing on one member reads
    // back from another and resyncs. Bodies are history, not resident state,
    // so a body missing on every member fails closed instead.
    let current_nodes = event_nodes(&actual);
    let reused_pages = event_nodes(&base)
        .into_values()
        .filter_map(|reference| {
            (reference.kind() == crate::metadata_map::ReferenceKind::Page
                && current_nodes.values().any(|kept| kept == &reference))
            .then(|| reference.hash().to_owned())
        })
        .collect::<Vec<_>>();
    assert!(
        reused_pages.len() > 2,
        "the fixture must exercise reconstruction of reused directory leaves"
    );
    let missing_pages = reused_pages[..2]
        .iter()
        .map(|hash| {
            (
                hash,
                std::fs::read(object_path(&config.disks[0], "state-page", hash)).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    let body = std::fs::read(object_path(&config.disks[0], EVENT_SEGMENT_KIND, &sealed)).unwrap();
    for disk in &config.disks {
        std::fs::remove_file(object_path(disk, EVENT_SEGMENT_KIND, &sealed)).unwrap();
    }
    assert_eq!(
        actual.store(&config.disks, &after).unwrap_err().code(),
        storage_contract::ErrorCode::ObjectCorrupt
    );
    for disk in &config.disks {
        assert!(!object_path(disk, "snapshot", &actual.hash).exists());
    }
    std::fs::write(
        object_path(&config.disks[1], EVENT_SEGMENT_KIND, &sealed),
        &body,
    )
    .unwrap();
    for disk in &config.disks {
        for (hash, _) in &missing_pages {
            std::fs::remove_file(object_path(disk, "state-page", hash)).unwrap();
        }
    }
    actual.store(&config.disks, &after).unwrap();
    for disk in &config.disks {
        assert_eq!(
            std::fs::read(object_path(disk, EVENT_SEGMENT_KIND, &sealed)).unwrap(),
            body
        );
        for (hash, bytes) in &missing_pages {
            assert_eq!(
                std::fs::read(object_path(disk, "state-page", hash)).unwrap(),
                *bytes
            );
        }
        assert_eq!(
            load_raw_snapshot_fixture(disk, &actual.hash).unwrap(),
            after
        );
    }
    assert_exact_encoding(&actual, &after);
}

#[test]
fn event_reuse_keeps_collision_failures_and_does_not_publish_the_root() {
    let temp = tempfile::tempdir().unwrap();
    let config = config(temp.path());
    let before = state(2 * EVENT_SEGMENT_SPAN + 10);
    install_sealed_bodies(&config, &before);
    let base = prepare(&before, MAX_METADATA).unwrap();
    base.store(&config.disks, &before).unwrap();
    let after = append(&before, 1);
    let actual = prepare_incremental(
        &before,
        &after,
        &base.reuse_basis(),
        MAX_METADATA,
        &mut no_reused_sources,
    )
    .unwrap();
    // A reused sealed body with foreign bytes on every member fails closed:
    // nothing overwrites the damage, and the snapshot root is not published.
    let sealed = after.events.sealed()[0].hash.clone();
    let body = std::fs::read(object_path(&config.disks[0], EVENT_SEGMENT_KIND, &sealed)).unwrap();
    let mut corrupt = body.clone();
    corrupt[0] ^= 1;
    for disk in &config.disks {
        std::fs::write(object_path(disk, EVENT_SEGMENT_KIND, &sealed), &corrupt).unwrap();
    }
    assert_eq!(
        actual.store(&config.disks, &after).unwrap_err().code(),
        storage_contract::ErrorCode::ObjectCorrupt
    );
    for disk in &config.disks {
        assert_eq!(
            std::fs::read(object_path(disk, EVENT_SEGMENT_KIND, &sealed)).unwrap(),
            corrupt
        );
        assert!(!object_path(disk, "snapshot", &actual.hash).exists());
        std::fs::write(object_path(disk, EVENT_SEGMENT_KIND, &sealed), &body).unwrap();
    }
    // Foreign bytes already sitting at a NEW prepared body's address are a
    // collision: the existing bytes stay and the root is still unpublished.
    let bodies = event_segment_objects(&actual);
    assert_eq!(bodies.len(), 1);
    let (kind, hash) = (bodies[0].kind, bodies[0].hash.clone());
    for disk in &config.disks {
        let path = object_path(disk, kind, &hash);
        crate::disk::private_dir(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"foreign body bytes").unwrap();
    }
    assert_eq!(
        actual.store(&config.disks, &after).unwrap_err().code(),
        storage_contract::ErrorCode::ObjectCollision
    );
    for disk in &config.disks {
        assert_eq!(
            std::fs::read(object_path(disk, kind, &hash)).unwrap(),
            b"foreign body bytes"
        );
        assert!(!object_path(disk, "snapshot", &actual.hash).exists());
    }
}

#[test]
fn boundary_tail_update_reopens_the_exact_sealed_segment_and_reseals_deterministically() {
    // Recovery's tail update at a seal boundary: reopen, replace, re-seal.
    let mut log = storage_contract::SegmentedEvents::from_events(
        (0..EVENT_SEGMENT_SPAN).map(event).collect(),
    )
    .unwrap();
    assert_eq!(log.sealed().len(), 1);
    assert!(log.tail().is_empty());
    let records = (0..EVENT_SEGMENT_SPAN).map(event).collect::<Vec<_>>();
    log.reopen_last_sealed(records).unwrap();
    assert!(log.sealed().is_empty());
    assert_eq!(log.tail().len(), EVENT_SEGMENT_SPAN);
    let mut corrected = event(EVENT_SEGMENT_SPAN - 1);
    corrected.object_id = "recovered result".into();
    log.replace_last(corrected.clone()).unwrap();
    // The seal rule is a pure function of the final list: the reopened
    // segment immediately re-seals under a new hash.
    assert_eq!(log.sealed().len(), 1);
    assert!(log.tail().is_empty());
    let (ordinal, segment) = log.fresh_seal().unwrap();
    assert_eq!(ordinal, 0);
    assert_eq!(segment.events.last().unwrap(), &corrected);
    // Reopening with bytes that differ from the sealed hash fails closed.
    let mut log = storage_contract::SegmentedEvents::from_events(
        (0..EVENT_SEGMENT_SPAN).map(event).collect(),
    )
    .unwrap();
    let mut wrong = (0..EVENT_SEGMENT_SPAN).map(event).collect::<Vec<_>>();
    wrong[0].object_id = "forged".into();
    assert_eq!(
        log.reopen_last_sealed(wrong).unwrap_err().code(),
        storage_contract::ErrorCode::ObjectCorrupt
    );
}
