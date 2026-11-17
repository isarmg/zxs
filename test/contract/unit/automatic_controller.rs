use super::*;

fn rev(n: u64) -> WorkingRevision { WorkingRevision::new(Generation::new(n)) }
fn at(n: u64) -> Timestamp { Timestamp::from_unix_seconds(n) }
fn save(previous: &AutomaticController, n: u64, seconds: u64) -> AutomaticController {
    previous.saved(rev(n), OperationId::generate().unwrap(), Sequence::new(n), at(seconds)).unwrap()
}

#[test]
fn threshold_save_is_latest_and_fenced_through_reopen_until_explicit_resume() {
    let mut controller = AutomaticController::enabled(rev(1), AutomaticPolicy::TwentySavesInSixtySeconds);
    for n in 2..=21 { controller = save(&controller, n, 100); }
    assert_eq!(controller.pending_save().unwrap().revision, rev(21));
    assert!(controller.protection().is_none());
    // Wall clock rollback does not make the 21st distinct revision disappear.
    controller = save(&controller, 22, 90);
    let trigger = *controller.protection().unwrap().decision();
    assert_eq!(trigger.revision, rev(22));
    assert_eq!(trigger.server_time, at(90));
    assert_eq!(trigger.effective_time, at(100));
    assert!(controller.pending_save().is_none());
    let encoded = crate::canonical_json(&controller).unwrap();
    let restored: AutomaticController = crate::decode_strict(&encoded).unwrap();
    restored.validate(rev(22)).unwrap();
    assert_eq!(restored, controller);
    let later = save(&restored, 23, 10_000);
    assert_eq!(later.latest_save().unwrap().revision, rev(23));
    assert_eq!(*later.protection().unwrap().decision(), trigger);
    assert!(later.pending_save().is_none());
    assert!(later.resumed(Generation::new(2), rev(24)).is_err());
    let resumed = later.resumed(later.epoch(), rev(24)).unwrap();
    assert_eq!(resumed.latest_save(), later.latest_save());
    assert!(resumed.pending_save().is_none());
    assert!(resumed.protection().is_none());
    let newest = save(&resumed, 25, 9_000);
    assert_eq!(newest.epoch(), Generation::new(24));
    assert_eq!(newest.pending_save().unwrap().revision, rev(25));
    assert_eq!(newest.pending_save().unwrap().effective_time, at(10_000));
}

#[test]
fn exactly_sixty_seconds_expires_and_an_older_captured_save_cannot_publish() {
    let mut controller = AutomaticController::enabled(rev(1), AutomaticPolicy::TwentySavesInSixtySeconds);
    for n in 2..=21 { controller = save(&controller, n, 5); }
    let observed = *controller.pending_save().unwrap();
    let next = save(&controller, 22, 65);
    assert!(next.protection().is_none());
    assert!(next.saved(rev(22), observed.operation, Sequence::new(22), at(65)).is_err());
    let node = AutomaticNodeDecision {
        epoch: next.epoch(), save: observed,
        node: crate::FileNodeId::Version(crate::VersionId::generate().unwrap()),
        operation: OperationId::generate().unwrap(), sequence: Sequence::new(23), published_at: at(66),
    };
    assert!(matches!(next.published(node, rev(23)), Err(ControllerError::Fence)));
    let current = AutomaticNodeDecision { save: *next.pending_save().unwrap(), ..node };
    let published = next.published(current, rev(23)).unwrap();
    assert_eq!(published.last_node(), Some(&current));
    assert!(published.pending_save().is_none());
    assert!(published.published(current, rev(24)).is_err());
}
