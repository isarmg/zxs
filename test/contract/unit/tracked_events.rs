use super::*;

fn event(sequence: u64) -> Event {
    Event {
        sequence,
        operation_id: format!("op_{sequence:032x}").parse().unwrap(),
        kind: "published".into(),
        object_id: format!("object-{sequence}"),
    }
}

#[test]
fn append_provenance_is_exact_and_snapshots_remain_isolated() {
    let before: TrackedEvents = vec![event(1)].into();
    let mut after = before.clone();
    after.begin_tracking();
    assert_eq!(
        after.changes_since(&before),
        SequenceChanges::Append { start: 1 }
    );
    after.extend(std::iter::empty());
    assert!(Arc::ptr_eq(&after.identity, &before.identity));
    after.push(event(2));
    let mut branch = after.clone();
    branch.extend([event(3), event(4)]);
    assert_eq!(
        after.changes_since(&before),
        SequenceChanges::Append { start: 1 }
    );
    assert_eq!(
        branch.changes_since(&before),
        SequenceChanges::Append { start: 1 }
    );
    assert_eq!(before.as_slice(), [event(1)]);
    assert_eq!(after.as_slice(), [event(1), event(2)]);
    assert_eq!(branch.as_slice(), [event(1), event(2), event(3), event(4)]);
    // Both candidates descend from before, not from one another.
    assert_eq!(branch.changes_since(&after), SequenceChanges::All);
}

#[test]
fn mutable_access_replacement_and_cold_decode_cannot_claim_append_only() {
    let before: TrackedEvents = vec![event(1), event(2), event(3)].into();
    let mut after = before.clone();
    after.begin_tracking();
    after[0] = event(9);
    after.push(event(4));
    assert_eq!(after.changes_since(&before), SequenceChanges::All);
    after.begin_tracking();
    assert_eq!(after.changes_since(&before), SequenceChanges::All);
    let mut retained = before.clone();
    retained.begin_tracking();
    retained.retain(|_| true);
    assert_eq!(retained.changes_since(&before), SequenceChanges::All);
    let mut cleared = before.clone();
    cleared.begin_tracking();
    cleared.clear();
    assert_eq!(cleared.changes_since(&before), SequenceChanges::All);
    let foreign: TrackedEvents = before.iter().cloned().collect();
    assert_eq!(foreign.changes_since(&before), SequenceChanges::All);
    let bytes = crate::canonical_json(&before).unwrap();
    assert_eq!(bytes, crate::canonical_json(&*before).unwrap());
    let decoded: TrackedEvents = json_codec::from_slice(&bytes).unwrap();
    assert_eq!(decoded, before);
    assert_eq!(decoded.changes_since(&before), SequenceChanges::All);
}

#[test]
fn a_new_tracking_epoch_proves_only_its_exact_base() {
    let empty = TrackedEvents::default();
    let mut next = empty.clone();
    next.begin_tracking();
    next.push(event(1));
    let mut last = next.clone();
    last.begin_tracking();
    last.push(event(2));
    assert_eq!(
        last.changes_since(&next),
        SequenceChanges::Append { start: 1 }
    );
    assert_eq!(last.changes_since(&empty), SequenceChanges::All);
}
