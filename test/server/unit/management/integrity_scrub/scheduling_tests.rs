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

fn publish_payload(fixture: &Fixture, payload: &[u8]) -> PathBuf {
    fixture.engine().test_with_engine_locked(|engine| {
        let branch = engine
            .state()
            .repositories
            .values()
            .next()
            .unwrap()
            .main_branch_id
            .clone();
        let workspace = engine.create_workspace(&branch, "admin").unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, "file.txt", payload)
            .unwrap();
        engine
            .commit_workspace(storage_engine::CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: OperationId::generate(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "scrub budget and retry fixture".into(),
                request_digest: None,
            })
            .unwrap();
        let version = engine.state().versions.values().next().unwrap();
        fixture
            ._directory
            .path()
            .join("disk/histories")
            .join(&version.history_tree_id)
            .join("versions")
            .join(&version.version_id)
            .join("publication-proof.json")
    })
}

#[test]
fn real_read_budget_can_finish_healthy_after_both_freshness_and_coverage_expire() {
    const PAYLOAD_BYTES: usize = 96 * 1024;
    let policy =
        IntegrityScrubPolicy::periodic(positive(1), positive(1), positive(65_536)).unwrap();
    let fixture = Fixture::dormant(policy);
    publish_payload(&fixture, &vec![37; PAYLOAD_BYTES]);
    let control = ready_without_worker(&fixture);
    let job = next_job(control, &Arc::downgrade(&fixture.backend.inner)).unwrap();
    let mut schedule = None;
    // The actual read budget supplies the elapsed time; no sleep or clock
    // rewrite stands in for the scan, pacer or live-owner finish.
    let completed = run_job(control, &job, &mut schedule).unwrap();
    assert!(completed.report.is_healthy());
    assert!(!completed.metadata_coverage_at_finish);
    finish_job(control, job, Ok(completed));
    let status = fixture.status();
    assert_eq!(status.freshness, IntegrityScrubFreshness::Expired);
    assert!(status.last_complete_success_at.is_some());
    let run = status.latest.as_ref().unwrap();
    let IntegrityScrubOutcome::Report(report) = &run.outcome else {
        panic!("budgeted healthy report");
    };
    assert!(report.is_healthy());
    assert_eq!(
        run.diagnostics.expected_payload_bytes,
        Some(PAYLOAD_BYTES as u64)
    );
    assert!(run.diagnostics.bytes_read > PAYLOAD_BYTES as u128);
    assert!(run.diagnostics.scan_elapsed.unwrap() >= policy.max_age());
    assert_eq!(run.diagnostics.metadata_coverage_at_finish, Some(false));
    assert_eq!(
        run.diagnostics.feasibility(),
        crate::integrity_scrub::IntegrityScrubFeasibility::PayloadCannotFitMaxAge
    );
    assert!(!coverage(&fixture.engine()));
}

#[test]
fn repaired_proof_failure_retries_on_the_next_periodic_tick_without_manual_dispatch() {
    let policy = periodic_policy(60);
    let fixture = Fixture::dormant(policy);
    let proof = publish_payload(&fixture, b"same retained full version across retry");
    let held_proof = fixture
        ._directory
        .path()
        .join("held-publication-proof.json");
    std::fs::rename(&proof, &held_proof).unwrap();
    fixture.start();
    let failed = eventually(|| fixture.status().latest);
    let IntegrityScrubOutcome::Report(failed_report) = &failed.outcome else {
        panic!("real proof failure must retain its report");
    };
    assert_eq!(failed.trigger, IntegrityScrubTrigger::Periodic);
    assert!(!failed_report.is_healthy());
    assert!(failed_report.metadata_failures > 0);
    assert_eq!(
        fixture.status().freshness,
        IntegrityScrubFreshness::Unavailable
    );
    assert!(fixture.status().last_complete_success_at.is_none());
    let control = &fixture.backend.scrub.control;
    {
        let state = crate::lock(&control.state);
        assert!(matches!(state.activity, Activity::Idle));
        assert!(state.cadence.wait(policy, Instant::now()).unwrap() > Duration::ZERO);
        assert_eq!(state.next_id, failed.job_id.0 + 1);
    }
    // Restore only this temporary fixture's original proof. Advance the
    // existing scheduling clock instead of waiting 60s.
    std::fs::rename(held_proof, proof).unwrap();
    {
        let mut state = crate::lock(&control.state);
        state.cadence = Cadence::default();
        state.cadence.dispatch(
            IntegrityScrubTrigger::Periodic,
            Instant::now() - policy.interval().unwrap(),
        );
    }
    control.wake.notify_one();
    let recovered = eventually(|| {
        fixture
            .status()
            .latest
            .filter(|run| run.job_id != failed.job_id)
    });
    assert_eq!(recovered.job_id.0, failed.job_id.0 + 1);
    assert_eq!(recovered.trigger, IntegrityScrubTrigger::Periodic);
    let IntegrityScrubOutcome::Report(report) = &recovered.outcome else {
        panic!("repaired proof must be scanned again");
    };
    assert!(report.is_healthy());
    assert_eq!(report.sequence, failed_report.sequence);
    assert_eq!(report.snapshot_hash, failed_report.snapshot_hash);
    assert!(recovered.diagnostics.bytes_read > 0);
    assert_eq!(fixture.status().freshness, IntegrityScrubFreshness::Fresh);
    assert!(
        !failed_report.is_healthy(),
        "a later success cannot rewrite the old run"
    );
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
