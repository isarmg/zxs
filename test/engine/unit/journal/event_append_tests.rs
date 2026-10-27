use super::*;

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
        events: (0..count).map(event).collect(),
        ..Default::default()
    }
}
fn append(before: &State, count: usize) -> State {
    let mut after = before.clone();
    after.begin_tracking();
    after
        .events
        .extend((before.events.len()..before.events.len() + count).map(event));
    after.sequence += count as u64;
    after
}
fn config(temp: &std::path::Path) -> Config {
    Config::development([temp.join("one"), temp.join("two"), temp.join("three")])
}
fn assert_exact_encoding(actual: &PreparedSnapshot, state: &State) {
    let expected = prepare_snapshot_reference(state, MAX_METADATA).unwrap();
    assert_eq!(actual.hash, expected.hash);
    assert_eq!(actual.event_tail_start, expected.event_tail_start);
    assert_eq!(actual.descriptor.pages, expected.descriptor.pages);
    assert_eq!(actual.descriptor.scalars, expected.descriptor.scalars);
    for object in &actual.objects {
        let reference = expected
            .objects
            .iter()
            .find(|entry| entry.hash == object.hash)
            .unwrap();
        assert_eq!(object.kind, reference.kind);
        assert_eq!(object.bytes, reference.bytes);
    }
}

#[test]
fn event_appends_encode_only_the_tail_and_match_every_current_format_byte() {
    for count in [0, 1, 127, 128, 129, 2000] {
        let before = state(count);
        let base = prepare(&before, MAX_METADATA).unwrap();
        let basis = base.reuse_basis().unwrap();
        for appended in [0, 1, 2, 130] {
            let after = append(&before, appended);
            take_encoding_counts();
            let actual = prepare_incremental(&before, &after, &basis, MAX_METADATA).unwrap();
            let encoded = take_encoding_counts().get("events").copied().unwrap_or(0);
            if appended == 0 {
                assert_eq!(encoded, 0);
                assert_eq!(
                    actual.fields["events"].reused_pages,
                    base.descriptor.pages["events"].len()
                );
            } else {
                let fresh_pages = actual.fields["events"].objects.len();
                assert!(fresh_pages > 0 && fresh_pages <= appended + 1);
                assert!(encoded > 0);
                let prefix = base.descriptor.pages["events"].len().saturating_sub(1);
                assert_eq!(actual.fields["events"].reused_pages, prefix);
                assert_eq!(
                    actual.descriptor.pages["events"][..prefix],
                    base.descriptor.pages["events"][..prefix]
                );
            }
            // CountedPage measures Serialize invocations, not page emissions:
            // canonical_json visits a value for integer validation and JSON
            // construction. Compare against the full typed encoder's measured
            // per-page work, independently of that implementation detail.
            take_encoding_counts();
            let full = prepare(&after, MAX_METADATA).unwrap();
            let full_serializations = take_encoding_counts().get("events").copied().unwrap_or(0);
            assert_eq!(
                encoded * full.fields["events"].objects.len(),
                full_serializations * actual.fields["events"].objects.len()
            );
            assert_exact_encoding(&actual, &after);
        }
    }
}

#[test]
fn event_mutation_replacement_and_decode_take_the_full_encoding_path() {
    let before = state(300);
    let base = prepare(&before, MAX_METADATA).unwrap();
    let basis = base.reuse_basis().unwrap();
    let mut states = Vec::new();
    let mut changed = append(&before, 1);
    changed.events[0].object_id = "changed prefix".into();
    states.push(changed);
    let mut retained = append(&before, 0);
    retained.events.retain(|event| event.sequence % 2 == 0);
    states.push(retained);
    let mut replaced = append(&before, 0);
    replaced.events = before.events.iter().cloned().collect();
    states.push(replaced);
    states.push(json_codec::from_slice(&canonical_json(&before).unwrap()).unwrap());
    for after in states {
        assert_eq!(
            after.events.changes_since(&before.events),
            storage_contract::SequenceChanges::All
        );
        take_encoding_counts();
        let actual = prepare_incremental(&before, &after, &basis, MAX_METADATA).unwrap();
        let serializations = take_encoding_counts()["events"];
        prepare(&after, MAX_METADATA).unwrap();
        assert_eq!(serializations, take_encoding_counts()["events"]);
        assert_eq!(
            actual.fields["events"].objects.len(),
            actual.descriptor.pages["events"].len()
        );
        assert_eq!(actual.fields["events"].reused_pages, 0);
        assert_exact_encoding(&actual, &after);
    }
}

#[test]
fn appended_events_reconstruct_a_missing_prefix_once_then_read_back_all_members() {
    let temp = tempfile::tempdir().unwrap();
    let config = config(temp.path());
    let before = state(300);
    let base = prepare(&before, MAX_METADATA).unwrap();
    base.store(&config.disks, &before).unwrap();
    let after = append(&before, 1);
    let actual =
        prepare_incremental(&before, &after, &base.reuse_basis().unwrap(), MAX_METADATA).unwrap();
    assert!(actual.fields["events"].reused_pages > 2);
    let missing = &actual.descriptor.pages["events"][1];
    for disk in &config.disks {
        std::fs::remove_file(object_path(disk, "state-page", missing)).unwrap();
    }
    let mut visited = Vec::new();
    actual
        .visit_field_pages("events", &config.disks, &after, |hash, bytes| {
            visited.push(hash.to_owned());
            for disk in &config.disks {
                write_object_bytes(disk, "state-page", hash, bytes)?;
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(visited, actual.descriptor.pages["events"]);
    actual.store(&config.disks, &after).unwrap();
    for disk in &config.disks {
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
    let before = state(300);
    let base = prepare(&before, MAX_METADATA).unwrap();
    base.store(&config.disks, &before).unwrap();
    let after = append(&before, 1);
    let actual =
        prepare_incremental(&before, &after, &base.reuse_basis().unwrap(), MAX_METADATA).unwrap();
    let hash = &actual.descriptor.pages["events"][0];
    for disk in &config.disks {
        let path = object_path(disk, "state-page", hash);
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[0] ^= 1;
        std::fs::write(path, bytes).unwrap();
    }
    assert_eq!(
        actual.store(&config.disks, &after).unwrap_err().code(),
        storage_contract::ErrorCode::ObjectCollision
    );
    assert!(!object_path(&config.disks[0], "snapshot", &actual.hash).exists());
}

#[test]
fn event_page_callback_failure_does_not_trigger_reconstruction() {
    let temp = tempfile::tempdir().unwrap();
    let config = config(temp.path());
    let before = state(300);
    let base = prepare(&before, MAX_METADATA).unwrap();
    base.store(&config.disks, &before).unwrap();
    let after = append(&before, 1);
    let actual =
        prepare_incremental(&before, &after, &base.reuse_basis().unwrap(), MAX_METADATA).unwrap();
    take_encoding_counts();
    let mut callbacks = 0;
    let error = actual
        .visit_field_pages("events", &config.disks, &after, |_, _| {
            callbacks += 1;
            Err(Error::new(
                storage_contract::ErrorCode::InjectedFailure,
                "failed member barrier",
            ))
        })
        .unwrap_err();
    assert_eq!(error.code(), storage_contract::ErrorCode::InjectedFailure);
    assert_eq!(callbacks, 1);
    assert!(take_encoding_counts().is_empty());
}
