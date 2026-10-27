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
    let FieldRoot::Events(events) = &prepared.fields["events"] else {
        panic!("event provenance must retain ordinal keys")
    };
    events
        .visit_nodes("", &mut |root, prefix| {
            nodes.insert(prefix.to_owned(), root.reference().clone());
            Ok(())
        })
        .unwrap();
    nodes
}
fn reused_event_pages(base: &PreparedSnapshot, actual: &PreparedSnapshot) -> Vec<String> {
    let before = event_nodes(base);
    event_nodes(actual)
        .into_iter()
        .filter(|(prefix, reference)| {
            reference.kind() == crate::metadata_map::ReferenceKind::Page
                && before.get(prefix) == Some(reference)
        })
        .map(|(_, reference)| reference.hash().to_owned())
        .collect()
}

#[test]
fn event_appends_encode_only_affected_radix_leaves_and_match_every_current_format_byte() {
    for count in [0, 1, 127, 128, 129, 2000] {
        let before = state(count);
        let base = prepare(&before, MAX_METADATA).unwrap();
        let basis = base.reuse_basis();
        for appended in [0, 1, 2, 130] {
            let after = append(&before, appended);
            assert_eq!(
                after.events.changes_since(&before.events),
                storage_contract::SequenceChanges::Append { start: count }
            );
            take_encoding_counts();
            let actual = prepare_incremental(&before, &after, &basis, MAX_METADATA).unwrap();
            let encoded = take_encoding_counts().get("events").copied().unwrap_or(0);
            if appended == 0 {
                assert_eq!(encoded, 0);
                assert_eq!(actual.descriptor.events, base.descriptor.events);
                assert_eq!(
                    actual.objects.len(),
                    1,
                    "only the snapshot root is prepared"
                );
            } else {
                // Each appended ordinal can touch at most one old bounded leaf.
                // The counter measures logical records, not canonical JSON passes.
                assert!(encoded >= appended);
                assert!(encoded <= appended + count.min(STATE_PAGE_ENTRIES * appended));
                assert!(encoded <= after.events.len());
                assert_ne!(actual.descriptor.events, base.descriptor.events);
                let dirty_routes = (count..count + appended)
                    .map(|ordinal| {
                        blake3::hash(format!("{ordinal:020}").as_bytes())
                            .to_hex()
                            .to_string()
                    })
                    .collect::<Vec<_>>();
                let current = event_nodes(&actual);
                for (prefix, reference) in event_nodes(&base) {
                    if dirty_routes.iter().all(|route| !route.starts_with(&prefix)) {
                        assert_eq!(current.get(&prefix), Some(&reference), "{prefix}");
                    }
                }
            }
            take_encoding_counts();
            prepare(&after, MAX_METADATA).unwrap();
            assert_eq!(
                take_encoding_counts().get("events").copied().unwrap_or(0),
                after.events.len()
            );
            assert_exact_encoding(&actual, &after);
        }
    }
}

#[test]
fn event_mutation_replacement_and_decode_take_the_full_encoding_path() {
    let before = state(300);
    let base = prepare(&before, MAX_METADATA).unwrap();
    let basis = base.reuse_basis();
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
    let mut empty = append(&before, 0);
    empty.events.clear();
    states.push(empty);
    states.push(json_codec::from_slice(&canonical_json(&before).unwrap()).unwrap());
    for after in states {
        assert_eq!(
            after.events.changes_since(&before.events),
            storage_contract::SequenceChanges::All
        );
        take_encoding_counts();
        let actual = prepare_incremental(&before, &after, &basis, MAX_METADATA).unwrap();
        assert_eq!(
            take_encoding_counts().get("events").copied().unwrap_or(0),
            after.events.len()
        );
        for reference in event_nodes(&actual).values() {
            assert!(actual.objects.iter().any(|object| {
                object.kind == reference.kind().domain() && object.hash == reference.hash()
            }));
        }
        assert_exact_encoding(&actual, &after);
    }
}

#[test]
fn appended_events_reconstruct_missing_reused_leaves_once_and_read_back_all_members() {
    let temp = tempfile::tempdir().unwrap();
    let config = config(temp.path());
    let before = state(300);
    let base = prepare(&before, MAX_METADATA).unwrap();
    base.store(&config.disks, &before).unwrap();
    let after = append(&before, 1);
    let actual = prepare_incremental(&before, &after, &base.reuse_basis(), MAX_METADATA).unwrap();
    let reused = reused_event_pages(&base, &actual);
    assert!(reused.len() > 2);
    for hash in &reused[..2] {
        for disk in &config.disks {
            std::fs::remove_file(object_path(disk, "state-page", hash)).unwrap();
        }
    }
    take_encoding_counts();
    actual.store(&config.disks, &after).unwrap();
    assert_eq!(
        take_encoding_counts(),
        BTreeMap::from([("events".into(), after.events.len())])
    );
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
    let actual = prepare_incremental(&before, &after, &base.reuse_basis(), MAX_METADATA).unwrap();
    let hash = &reused_event_pages(&base, &actual)[0];
    let mut corrupt = std::fs::read(object_path(&config.disks[0], "state-page", hash)).unwrap();
    corrupt[0] ^= 1;
    for disk in &config.disks {
        std::fs::write(object_path(disk, "state-page", hash), &corrupt).unwrap();
    }
    take_encoding_counts();
    assert_eq!(
        actual.store(&config.disks, &after).unwrap_err().code(),
        storage_contract::ErrorCode::ObjectCollision
    );
    assert_eq!(
        take_encoding_counts(),
        BTreeMap::from([("events".into(), after.events.len())])
    );
    for disk in &config.disks {
        assert_eq!(
            std::fs::read(object_path(disk, "state-page", hash)).unwrap(),
            corrupt
        );
        assert!(!object_path(disk, "snapshot", &actual.hash).exists());
    }
}

#[test]
fn event_installation_failure_with_a_healthy_source_does_not_trigger_reconstruction() {
    let temp = tempfile::tempdir().unwrap();
    let config = config(temp.path());
    let before = state(300);
    let base = prepare(&before, MAX_METADATA).unwrap();
    base.store(&config.disks, &before).unwrap();
    let after = append(&before, 1);
    let actual = prepare_incremental(&before, &after, &base.reuse_basis(), MAX_METADATA).unwrap();
    let hash = &reused_event_pages(&base, &actual)[0];
    let path = object_path(&config.disks[0], "state-page", hash);
    let mut corrupt = std::fs::read(&path).unwrap();
    corrupt[0] ^= 1;
    std::fs::write(&path, &corrupt).unwrap();
    take_encoding_counts();
    assert_eq!(
        actual.store(&config.disks, &after).unwrap_err().code(),
        storage_contract::ErrorCode::ObjectCollision
    );
    assert!(take_encoding_counts().is_empty());
    assert_eq!(std::fs::read(path).unwrap(), corrupt);
    for disk in &config.disks {
        assert!(!object_path(disk, "snapshot", &actual.hash).exists());
    }
}
