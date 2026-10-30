use super::*;

fn ready_without_worker(fixture: &Fixture) -> &Control {
    let control = &fixture.backend.scrub.control;
    assert!(
        control
            .auth
            .set(crate::lock(&fixture.auth).reader())
            .is_ok()
    );
    crate::lock(&control.state).lifecycle = Lifecycle::Ready;
    control
}

#[test]
fn healthy_periodic_completion_keeps_the_anchor_after_real_reads() {
    let policy = periodic_policy(60);
    let fixture = Fixture::dormant(policy);
    let control = ready_without_worker(&fixture);
    let job = next_job(control, &Arc::downgrade(&fixture.backend.inner)).unwrap();
    assert_eq!(job.admission.trigger(), IntegrityScrubTrigger::Periodic);
    let mut schedule = None;
    let completed = run_job(control, &job, &mut schedule).unwrap();
    assert!(completed.report.is_healthy());
    assert!(completed.metadata_coverage_at_finish);
    // Control only the scheduling clock; the real scan and its coverage clocks
    // remain untouched. This represents a healthy twenty-second attempt.
    let anchor = Instant::now() - Duration::from_secs(20);
    {
        let mut state = crate::lock(&control.state);
        state.cadence = Cadence::default();
        state
            .cadence
            .dispatch(IntegrityScrubTrigger::Periodic, anchor);
    }
    finish_job(control, job, Ok(completed));
    let now = Instant::now();
    let state = crate::lock(&control.state);
    let wait = state.cadence.wait(policy, now).unwrap();
    assert_eq!(wait, Duration::from_secs(60) - now.duration_since(anchor));
    assert!(wait > Duration::ZERO);
    assert!(matches!(state.activity, Activity::Idle));
    assert!(state.latest.as_ref().unwrap().diagnostics.bytes_read > 0);
}

#[test]
fn manual_and_periodic_dispatch_share_one_slot_and_cancel_keeps_a_full_retry_wait() {
    for manual_first in [true, false] {
        let policy = periodic_policy(60);
        let fixture = Fixture::dormant(policy);
        let control = ready_without_worker(&fixture);
        if manual_first {
            fixture
                .backend
                .start_integrity_scrub(&fixture.identity)
                .unwrap();
            assert_eq!(
                fixture
                    .backend
                    .start_integrity_scrub(&fixture.identity)
                    .unwrap_err()
                    .status,
                http::StatusCode::CONFLICT
            );
        }
        let dispatched_before = Instant::now();
        let job = next_job(control, &Arc::downgrade(&fixture.backend.inner)).unwrap();
        assert_eq!(
            job.admission.trigger(),
            if manual_first {
                IntegrityScrubTrigger::Manual
            } else {
                IntegrityScrubTrigger::Periodic
            }
        );
        assert_eq!(
            fixture
                .backend
                .start_integrity_scrub(&fixture.identity)
                .unwrap_err()
                .status,
            http::StatusCode::CONFLICT
        );
        let now = Instant::now();
        assert!(
            crate::lock(&control.state)
                .cadence
                .wait(policy, now)
                .unwrap()
                >= Duration::from_secs(60) - now.duration_since(dispatched_before)
        );
        fixture
            .backend
            .cancel_integrity_scrub(&fixture.identity, job.id)
            .unwrap();
        let mut schedule = None;
        let completed = run_job(control, &job, &mut schedule).unwrap();
        assert!(matches!(
            completed.report.completion,
            IntegrityScrubCompletion::Cancelled
        ));
        {
            let mut state = crate::lock(&control.state);
            state.cadence = Cadence::default();
            state.cadence.dispatch(
                IntegrityScrubTrigger::Periodic,
                Instant::now() - Duration::from_secs(125),
            );
        }
        let before = Instant::now();
        finish_job(control, job, Ok(completed));
        let after = Instant::now();
        let state = crate::lock(&control.state);
        let wait = state.cadence.wait(policy, after).unwrap();
        assert!(wait >= Duration::from_secs(60) - after.duration_since(before));
        assert!(wait <= Duration::from_secs(60));
        assert!(matches!(state.activity, Activity::Idle));
        let run = state.latest.as_ref().unwrap();
        assert_eq!(run.diagnostics.bytes_read, 0);
        assert_eq!(run.diagnostics.metadata_coverage_at_finish, Some(false));
    }
}

#[test]
fn revoked_manual_admission_waits_a_full_interval_after_rejection() {
    let policy = periodic_policy(60);
    let fixture = Fixture::dormant(policy);
    let control = ready_without_worker(&fixture);
    fixture
        .backend
        .start_integrity_scrub(&fixture.identity)
        .unwrap();
    let job = next_job(control, &Arc::downgrade(&fixture.backend.inner)).unwrap();
    crate::lock(&fixture.auth).logout(&fixture.token).unwrap();
    let mut schedule = None;
    let outcome = run_job(control, &job, &mut schedule);
    assert_eq!(
        outcome.as_ref().err().unwrap().code,
        storage_contract::ErrorCode::Unauthorized
    );
    let before = Instant::now();
    finish_job(control, job, outcome);
    let after = Instant::now();
    let state = crate::lock(&control.state);
    let wait = state.cadence.wait(policy, after).unwrap();
    assert!(wait >= Duration::from_secs(60) - after.duration_since(before));
    let run = state.latest.as_ref().unwrap();
    assert_eq!(run.diagnostics.bytes_read, 0);
    assert!(run.diagnostics.expected_payload_bytes.is_none());
    assert_eq!(run.diagnostics.metadata_coverage_at_finish, None);
}
