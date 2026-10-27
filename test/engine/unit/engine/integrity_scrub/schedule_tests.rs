use super::*;
use crate::{Config, IntegrityScrubCancellation, OperationId, RepositoryName};

fn fixture() -> (tempfile::TempDir, Engine) {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temp.path().join("system"),
        Config::development([temp.path().join("disk")]),
    )
    .unwrap();
    engine
        .create_repository(
            &RepositoryName::parse("lease").unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate(),
        )
        .unwrap();
    (temp, engine)
}

fn schedule(engine: &mut Engine) -> IntegrityScrubSchedule {
    engine
        .begin_integrity_scrub_schedule(Duration::from_secs(60), Duration::from_secs(120))
        .unwrap()
}

fn run(
    engine: &Engine,
    schedule: &IntegrityScrubSchedule,
    cancellation: &IntegrityScrubCancellation,
) -> IntegrityScrubOutcome {
    engine
        .prepare_scheduled_integrity_scrub(schedule)
        .unwrap()
        .run(
            cancellation,
            |plan, id| engine.admit_integrity_scrub_batch(plan, id),
            |_| {},
            |_| {},
        )
}

#[test]
fn registration_and_a_real_manual_scan_never_claim_a_periodic_owner_completed_coverage() {
    let (_temp, mut engine) = fixture();
    let owner = schedule(&mut engine);
    assert!(!engine.has_current_integrity_scrub_coverage());
    assert!(
        engine
            .scrub(&IntegrityScrubCancellation::new())
            .unwrap()
            .is_healthy()
    );
    assert!(!engine.has_current_integrity_scrub_coverage());
    let outcome = run(&engine, &owner, &IntegrityScrubCancellation::new());
    assert!(
        !engine.has_current_integrity_scrub_coverage(),
        "the executed result still requires live finish admission"
    );
    assert!(engine.finish_integrity_scrub(outcome).is_healthy());
    assert!(engine.has_current_integrity_scrub_coverage());
}

#[test]
fn publication_does_not_refresh_the_scan_start_or_the_next_schedule_deadline() {
    let (_temp, mut engine) = fixture();
    let owner = schedule(&mut engine);
    let outcome = run(&engine, &owner, &IntegrityScrubCancellation::new());
    engine.finish_integrity_scrub(outcome);
    let (started, completed, sequence) = {
        let observation = owner.state.observation.lock().unwrap();
        let coverage = observation.coverage.as_ref().unwrap();
        (coverage.started, coverage.completed, coverage.sequence)
    };
    engine
        .persist_auth_state(
            json_codec::json!({"auth":"later"}),
            "admin",
            OperationId::generate(),
        )
        .unwrap();
    assert!(engine.state.sequence > sequence);
    assert!(engine.has_current_integrity_scrub_coverage());
    let observation = owner.state.observation.lock().unwrap();
    let coverage = observation.coverage.as_ref().unwrap();
    assert_eq!((coverage.started, coverage.completed), (started, completed));
    drop(observation);
    assert!(!engine.scrub_coverage_at(completed + owner.state.interval));
    assert!(!engine.scrub_coverage_at(started + owner.state.max_age));
}

#[test]
fn revoked_or_dropped_owner_and_its_old_completed_outcome_cannot_revive_coverage() {
    let (_temp, mut engine) = fixture();
    let owner = schedule(&mut engine);
    assert!(
        matches!(engine.begin_integrity_scrub_schedule(Duration::from_secs(60), Duration::from_secs(120)), Err(error) if error.code() == ErrorCode::ConfigConflict)
    );
    let old = run(&engine, &owner, &IntegrityScrubCancellation::new());
    owner.revoke();
    assert!(
        matches!(engine.prepare_scheduled_integrity_scrub(&owner), Err(error) if error.code() == ErrorCode::PreparationStale)
    );
    let replacement = schedule(&mut engine);
    engine.finish_integrity_scrub(old);
    assert!(!engine.has_current_integrity_scrub_coverage());
    let outcome = run(&engine, &replacement, &IntegrityScrubCancellation::new());
    engine.finish_integrity_scrub(outcome);
    assert!(engine.has_current_integrity_scrub_coverage());
    drop(replacement);
    assert!(!engine.has_current_integrity_scrub_coverage());
}

#[test]
fn cancelled_scan_clears_its_previous_periodic_coverage() {
    let (_temp, mut engine) = fixture();
    let owner = schedule(&mut engine);
    engine.finish_integrity_scrub(run(&engine, &owner, &IntegrityScrubCancellation::new()));
    assert!(engine.has_current_integrity_scrub_coverage());
    let cancelled = IntegrityScrubCancellation::new();
    cancelled.cancel();
    let report = engine.finish_integrity_scrub(run(&engine, &owner, &cancelled));
    assert!(matches!(
        report.completion,
        IntegrityScrubCompletion::Cancelled
    ));
    assert!(!engine.has_current_integrity_scrub_coverage());
}

#[test]
fn opaque_results_require_the_exact_live_engine_and_an_unrevoked_validation_epoch() {
    let (_temp, mut engine) = fixture();
    let (_other_temp, other) = fixture();
    let owner = schedule(&mut engine);
    assert!(
        matches!(other.prepare_scheduled_integrity_scrub(&owner), Err(error) if error.code() == ErrorCode::PreparationStale)
    );
    let report =
        other.finish_integrity_scrub(run(&engine, &owner, &IntegrityScrubCancellation::new()));
    assert!(matches!(
        report.completion,
        IntegrityScrubCompletion::Superseded
    ));
    assert!(!engine.has_current_integrity_scrub_coverage());
    let outcome = run(&engine, &owner, &IntegrityScrubCancellation::new());
    engine.metadata_integrity.invalidate();
    let report = engine.finish_integrity_scrub(outcome);
    assert!(matches!(
        report.completion,
        IntegrityScrubCompletion::Superseded
    ));
    assert!(!engine.has_current_integrity_scrub_coverage());
}
