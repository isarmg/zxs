use super::*;
use std::num::NonZeroU64;

fn periodic(seconds: u64) -> IntegrityScrubPolicy {
    IntegrityScrubPolicy::periodic(
        NonZeroU64::new(seconds).unwrap(),
        NonZeroU64::new(seconds.max(120)).unwrap(),
        NonZeroU64::new(67_108_864).unwrap(),
    )
    .unwrap()
}

#[test]
fn healthy_scans_skip_occupied_ticks_and_wait_for_the_next_anchor() {
    let start = Instant::now();
    let policy = periodic(10);
    for (elapsed, remaining) in [
        (Duration::ZERO, Duration::from_secs(10)),
        (Duration::from_millis(9_750), Duration::from_millis(250)),
        (Duration::from_secs(10), Duration::from_secs(10)),
        (Duration::from_millis(10_250), Duration::from_millis(9_750)),
        (Duration::from_millis(25_500), Duration::from_millis(4_500)),
        (Duration::from_secs(30), Duration::from_secs(10)),
    ] {
        let mut cadence = Cadence::default();
        assert_eq!(cadence.wait(policy, start), Some(Duration::ZERO));
        cadence.dispatch(IntegrityScrubTrigger::Periodic, start);
        let finished = start + elapsed;
        cadence.finish(policy, true, finished);
        assert_eq!(cadence.wait(policy, finished), Some(remaining));
        assert_eq!(
            cadence.wait(policy, finished + remaining - Duration::from_nanos(1)),
            Some(Duration::from_nanos(1))
        );
        assert_eq!(
            cadence.wait(policy, finished + remaining),
            Some(Duration::ZERO)
        );
    }
}

#[test]
fn delayed_periodic_dispatch_preserves_the_original_phase() {
    let start = Instant::now();
    let policy = periodic(10);
    let mut cadence = Cadence::default();
    cadence.dispatch(IntegrityScrubTrigger::Periodic, start);
    cadence.finish(policy, true, start + Duration::from_secs(1));
    cadence.dispatch(
        IntegrityScrubTrigger::Periodic,
        start + Duration::from_millis(10_250),
    );
    let finished = start + Duration::from_secs(12);
    cadence.finish(policy, true, finished);
    assert_eq!(cadence.wait(policy, finished), Some(Duration::from_secs(8)));
}

#[test]
fn unhealthy_attempts_wait_a_full_interval_even_after_missed_ticks() {
    let start = Instant::now();
    let policy = periodic(10);
    let mut cadence = Cadence::default();
    cadence.dispatch(IntegrityScrubTrigger::Periodic, start);
    let failed = start + Duration::from_secs(25);
    cadence.finish(policy, false, failed);
    assert_eq!(cadence.wait(policy, failed), Some(Duration::from_secs(10)));
    let retry = failed + Duration::from_secs(10);
    assert_eq!(cadence.wait(policy, retry), Some(Duration::ZERO));
    cadence.dispatch(IntegrityScrubTrigger::Periodic, retry);
    cadence.finish(policy, false, retry);
    assert_eq!(cadence.wait(policy, retry), Some(Duration::from_secs(10)));
}

#[test]
fn manual_dispatch_reanchors_instead_of_catching_up_a_pending_periodic_tick() {
    let start = Instant::now();
    let policy = periodic(10);
    for manual_elapsed in [9, 20] {
        let mut cadence = Cadence::default();
        cadence.dispatch(IntegrityScrubTrigger::Periodic, start);
        cadence.finish(policy, true, start + Duration::from_secs(1));
        let manual_start = start + Duration::from_secs(manual_elapsed);
        cadence.dispatch(IntegrityScrubTrigger::Manual, manual_start);
        cadence.finish(policy, true, manual_start + Duration::from_secs(2));
        assert_eq!(
            cadence.wait(policy, manual_start + Duration::from_secs(2)),
            Some(Duration::from_secs(8))
        );
        assert_eq!(
            cadence.wait(policy, manual_start + Duration::from_secs(10)),
            Some(Duration::ZERO)
        );
    }
}

#[test]
fn storage_retry_is_bounded_and_real_dispatch_establishes_a_new_anchor() {
    let start = Instant::now();
    for (interval, retry) in [(3, 3), (10, 5)] {
        let policy = periodic(interval);
        let mut cadence = Cadence::default();
        cadence.storage_unavailable(start);
        assert_eq!(
            cadence.wait(policy, start),
            Some(Duration::from_secs(retry))
        );
        let retry_at = start + Duration::from_secs(retry);
        assert_eq!(cadence.wait(policy, retry_at), Some(Duration::ZERO));
        cadence.storage_unavailable(retry_at);
        let ready = retry_at + Duration::from_secs(retry);
        cadence.dispatch(IntegrityScrubTrigger::Periodic, ready);
        cadence.finish(policy, true, ready + Duration::from_secs(1));
        assert_eq!(
            cadence.wait(policy, ready + Duration::from_secs(1)),
            Some(Duration::from_secs(interval - 1))
        );
    }
}

#[test]
fn manual_only_policy_never_arms_a_periodic_wakeup() {
    let now = Instant::now();
    let policy = IntegrityScrubPolicy::default();
    let mut cadence = Cadence::default();
    assert_eq!(cadence.wait(policy, now), None);
    cadence.dispatch(IntegrityScrubTrigger::Manual, now);
    cadence.finish(policy, true, now + Duration::from_secs(1));
    assert_eq!(cadence.wait(policy, now + Duration::from_secs(60)), None);
    cadence.finish(policy, false, now + Duration::from_secs(60));
    assert_eq!(cadence.wait(policy, now + Duration::from_secs(120)), None);
}

#[test]
fn largest_valid_interval_does_not_require_a_far_future_instant() {
    let start = Instant::now();
    let policy = periodic(u64::MAX);
    let mut cadence = Cadence::default();
    cadence.dispatch(IntegrityScrubTrigger::Periodic, start);
    cadence.finish(policy, true, start + Duration::from_secs(1));
    assert_eq!(
        cadence.wait(policy, start + Duration::from_secs(1)),
        Some(Duration::from_secs(u64::MAX - 1))
    );
}
