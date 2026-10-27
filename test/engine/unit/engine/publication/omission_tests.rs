use super::*;
use crate::{IntegrityScrubCancellation, IntegrityScrubSchedule};
use std::{
    sync::{Arc, Barrier},
    time::Duration,
};

fn fixture() -> (tempfile::TempDir, Engine) {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temp.path().join("system"),
        Config::development([
            temp.path().join("a"),
            temp.path().join("b"),
            temp.path().join("c"),
        ]),
    )
    .unwrap();
    engine
        .create_repository(
            &RepositoryName::parse("reuse").unwrap(),
            "P01",
            1,
            "alice",
            OperationId::generate(),
        )
        .unwrap();
    (temp, engine)
}
fn scan(engine: &Engine, owner: &IntegrityScrubSchedule) {
    let outcome = engine
        .prepare_scheduled_integrity_scrub(owner)
        .unwrap()
        .run(
            &IntegrityScrubCancellation::new(),
            |plan, id| engine.admit_integrity_scrub_batch(plan, id),
            |_| {},
            |_| {},
        );
    assert!(engine.finish_integrity_scrub(outcome).is_healthy());
}
fn covered(engine: &mut Engine) -> IntegrityScrubSchedule {
    let owner = engine
        .begin_integrity_scrub_schedule(Duration::from_secs(3600), Duration::from_secs(7200))
        .unwrap();
    scan(engine, &owner);
    owner
}
fn prepare_auth(engine: &Engine) -> DetachedPublication {
    let value = json!({"auth": "prepared"});
    let digest = object_hash("auth-state", &value).unwrap();
    let plan = engine
        .planning_context()
        .plan_new_mutation(
            "auth.control-state",
            "alice",
            OperationId::generate(),
            digest.clone(),
            |_, next, _| {
                next.control_auth = Some(value);
                Ok(json!({"auth_state_digest": digest}))
            },
        )
        .unwrap();
    let crate::engine::MutationPlan::Prepared {
        next,
        prepared,
        result,
    } = plan
    else {
        panic!("new operation must prepare")
    };
    engine
        .prepare_publication(*next, prepared, result, MAX_METADATA)
        .unwrap()
}
fn accepts(engine: &Engine, sequence: u64) -> usize {
    engine
        .members()
        .iter()
        .map(|disk| {
            crate::journal::scan(disk)
                .unwrap()
                .records
                .into_iter()
                .filter(|(record, _)| {
                    record.sequence == sequence && record.record_type == RecordType::Accept
                })
                .count()
        })
        .sum()
}

#[test]
fn omission_requires_both_a_successful_base_and_a_real_completed_periodic_scan() {
    let (_temp, mut engine) = fixture();
    assert!(prepare_auth(&engine).reused_dependencies.is_none());
    let owner = engine
        .begin_integrity_scrub_schedule(Duration::from_secs(3600), Duration::from_secs(7200))
        .unwrap();
    assert!(prepare_auth(&engine).reused_dependencies.is_none());
    assert!(
        engine
            .scrub(&IntegrityScrubCancellation::new())
            .unwrap()
            .is_healthy()
    );
    assert!(prepare_auth(&engine).reused_dependencies.is_none());
    scan(&engine, &owner);
    assert!(prepare_auth(&engine).reused_dependencies.is_some());
    engine.published_state_base = None;
    assert!(prepare_auth(&engine).reused_dependencies.is_none());
}

#[test]
fn untouched_published_fault_is_deferred_to_real_observation_not_claimed_healthy_per_write() {
    let (_temp, mut engine) = fixture();
    let owner = covered(&mut engine);
    let history = engine.state.histories.keys().next().unwrap();
    let hash = engine
        .published_state_base
        .as_ref()
        .unwrap()
        .roots
        .catalog_binding(history)
        .unwrap()
        .hash
        .clone();
    let path = object_path(&engine.members()[0], "history-manifest", &hash);
    std::fs::remove_file(&path).unwrap();
    let prepared = prepare_auth(&engine);
    assert!(prepared.reused_dependencies.is_some());
    engine.decide_publication(prepared).unwrap();
    assert!(
        !path.exists(),
        "ordinary unrelated writes no longer inspect or repair untouched history"
    );
    let outcome = engine
        .prepare_scheduled_integrity_scrub(&owner)
        .unwrap()
        .run(
            &IntegrityScrubCancellation::new(),
            |plan, id| engine.admit_integrity_scrub_batch(plan, id),
            |_| {},
            |_| {},
        );
    let report = engine.finish_integrity_scrub(outcome);
    assert!(report.metadata_failures > 0);
    assert!(!engine.has_current_integrity_scrub_coverage());
    assert!(!engine.published_state_base.as_ref().unwrap().matches(
        &engine.planning_context(),
        &object_hash("publication-config", &engine.config).unwrap()
    ));
    assert!(prepare_auth(&engine).reused_dependencies.is_none());
    assert!(
        path.is_file(),
        "the unchanged full path still repairs from an exact healthy copy"
    );
}

#[test]
fn stopped_dropped_or_replaced_owner_cannot_launder_an_old_detached_permit() {
    for replacement in [false, true] {
        let (_temp, mut engine) = fixture();
        let owner = covered(&mut engine);
        let prepared = prepare_auth(&engine);
        assert!(prepared.reused_dependencies.is_some());
        let sequence = prepared.candidate.sequence;
        owner.revoke();
        drop(owner);
        let _new_owner = replacement.then(|| covered(&mut engine));
        assert_eq!(
            engine.decide_publication(prepared).unwrap_err().code(),
            ErrorCode::PreparationStale
        );
        assert_eq!(accepts(&engine, sequence), 0);
        assert!(!engine.blocked);
    }
}

#[test]
fn healthy_refresh_preserves_original_permit_but_failed_run_then_success_does_not_revive_it() {
    let (_temp, mut engine) = fixture();
    let owner = covered(&mut engine);
    let prepared = prepare_auth(&engine);
    scan(&engine, &owner);
    engine.decide_publication(prepared).unwrap();
    let prepared = prepare_auth(&engine);
    let cancellation = IntegrityScrubCancellation::new();
    cancellation.cancel();
    let outcome = engine
        .prepare_scheduled_integrity_scrub(&owner)
        .unwrap()
        .run(
            &cancellation,
            |plan, id| engine.admit_integrity_scrub_batch(plan, id),
            |_| {},
            |_| {},
        );
    engine.finish_integrity_scrub(outcome);
    scan(&engine, &owner);
    assert!(engine.has_current_integrity_scrub_coverage());
    assert_eq!(
        engine.decide_publication(prepared).unwrap_err().code(),
        ErrorCode::PreparationStale
    );
}

#[test]
fn stop_and_shared_fault_are_ordered_at_the_final_admission_after_all_prepare_records() {
    for (after_admission, shared_fault) in
        [(false, false), (false, true), (true, false), (true, true)]
    {
        let (_temp, mut engine) = fixture();
        let owner = covered(&mut engine);
        let prepared = prepare_auth(&engine);
        assert!(prepared.reused_dependencies.is_some());
        let sequence = prepared.candidate.sequence;
        let prior_sequence = engine.state.sequence;
        let probe = engine.prepare_metadata_member_probe();
        let member = engine.members()[0].clone();
        let barrier = Arc::new(Barrier::new(2));
        engine.inject_pause_once(
            if after_admission {
                "after_decision_admission"
            } else {
                "before_decision_admission"
            },
            Arc::clone(&barrier),
        );
        let (engine, result) = std::thread::scope(|scope| {
            let worker = scope.spawn(move || {
                let result = engine.decide_publication(prepared);
                (engine, result)
            });
            barrier.wait();
            if shared_fault {
                let path = member.root.join("disk.json");
                let bytes = std::fs::read(&path).unwrap();
                std::fs::write(&path, b"broken seal").unwrap();
                assert!(probe.verify().is_err());
                std::fs::write(&path, bytes).unwrap();
            } else {
                owner.revoke();
            }
            barrier.wait();
            worker.join().unwrap()
        });
        let records = crate::journal::scan(&member).unwrap().records;
        assert!(
            records.iter().any(|(record, _)| record.sequence == sequence
                && record.record_type == RecordType::Prepare)
        );
        if after_admission {
            result.unwrap();
            assert_eq!(engine.state.sequence, sequence);
            assert_eq!(accepts(&engine, sequence), engine.members().len());
            if shared_fault {
                assert!(engine.published_state_base.is_none());
            }
        } else {
            assert_eq!(result.unwrap_err().code(), ErrorCode::PreparationStale);
            assert_eq!(engine.state.sequence, prior_sequence);
            assert_eq!(accepts(&engine, sequence), 0);
        }
        assert!(!engine.blocked);
    }
}

#[test]
fn original_lease_expiry_during_prepare_journal_io_rejects_before_any_accept() {
    let (_temp, mut engine) = fixture();
    let interval = Duration::from_secs(10);
    let owner = engine
        .begin_integrity_scrub_schedule(interval, interval * 2)
        .unwrap();
    scan(&engine, &owner);
    let prepared = prepare_auth(&engine);
    assert!(prepared.reused_dependencies.is_some());
    let sequence = prepared.candidate.sequence;
    let barrier = Arc::new(Barrier::new(2));
    engine.inject_pause_once("before_decision_admission", Arc::clone(&barrier));
    let (engine, result) = std::thread::scope(|scope| {
        let worker = scope.spawn(move || {
            let result = engine.decide_publication(prepared);
            (engine, result)
        });
        barrier.wait();
        std::thread::sleep(interval);
        barrier.wait();
        worker.join().unwrap()
    });
    assert_eq!(result.unwrap_err().code(), ErrorCode::PreparationStale);
    assert_eq!(accepts(&engine, sequence), 0);
    assert!(!engine.blocked);
}

#[test]
fn a_failure_after_final_admission_preserves_indeterminate_decision_ownership() {
    let (_temp, mut engine) = fixture();
    let _owner = covered(&mut engine);
    let prepared = prepare_auth(&engine);
    let original = engine.published_state_base.clone().unwrap();
    engine.inject_fault_once("after_decision_admission");
    let error = engine.decide_publication(prepared).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InDoubt);
    assert!(
        std::error::Error::source(&error)
            .unwrap()
            .to_string()
            .contains("after_decision_admission")
    );
    assert!(engine.blocked);
    assert!(Arc::ptr_eq(
        &original,
        engine.published_state_base.as_ref().unwrap()
    ));
    assert!(!original.matches(
        &engine.planning_context(),
        &object_hash("publication-config", &engine.config).unwrap()
    ));
}

#[test]
fn a_new_encoded_same_hash_still_compares_every_required_member_and_cannot_mint_on_collision() {
    let (_temp, mut engine) = fixture();
    let _owner = covered(&mut engine);
    let members = engine.members();
    let value = json!({"auth":"prepared"});
    let object = PreparedMetadataObject::new("state-control-auth", &value, MAX_METADATA).unwrap();
    object.store(&members).unwrap();
    let damaged = object_path(members.last().unwrap(), object.kind(), &object.hash);
    std::fs::write(&damaged, b"different unpublished bytes").unwrap();
    let baseline = engine.published_state_base.clone().unwrap();
    let sequence = engine.state.sequence;
    let slot = engine.next_slot;
    let error = engine
        .persist_auth_state(value, "alice", OperationId::generate())
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::ObjectCollision);
    assert_eq!(engine.state.sequence, sequence);
    assert_eq!(engine.next_slot, slot);
    assert_eq!(accepts(&engine, slot), 0);
    assert!(Arc::ptr_eq(
        &baseline,
        engine.published_state_base.as_ref().unwrap()
    ));
    assert!(!baseline.matches(
        &engine.planning_context(),
        &object_hash("publication-config", &engine.config).unwrap()
    ));
    assert_eq!(
        std::fs::read(damaged).unwrap(),
        b"different unpublished bytes"
    );
}

#[test]
fn real_detached_workspace_carries_the_original_owner_without_keeping_it_alive() {
    let (_temp, mut engine) = fixture();
    let branch = engine.state.branches.keys().next().unwrap().clone();
    let workspace = engine.create_workspace(&branch, "alice").unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "new.txt", b"new protected bytes")
        .unwrap();
    let owner = covered(&mut engine);
    let prepared = engine
        .prepare_workspace_commit(
            CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: OperationId::generate(),
                principal: "alice".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "detached".into(),
                request_digest: None,
            },
            None,
        )
        .unwrap()
        .execute()
        .unwrap();
    let sequence = engine.next_slot;
    drop(owner);
    assert!(!engine.has_current_integrity_scrub_coverage());
    assert_eq!(
        engine.finish_workspace_commit(prepared).unwrap_err().code(),
        ErrorCode::PreparationStale
    );
    assert_eq!(accepts(&engine, sequence), 0);
    assert!(!engine.blocked);
}

#[test]
fn full_installation_uses_the_same_admission_boundary_for_observed_shared_faults() {
    for after_admission in [false, true] {
        let (_temp, mut engine) = fixture();
        let prepared = prepare_auth(&engine);
        assert!(prepared.reused_dependencies.is_none());
        let sequence = prepared.candidate.sequence;
        let prior_sequence = engine.state.sequence;
        let probe = engine.prepare_metadata_member_probe();
        let member = engine.members()[0].clone();
        let barrier = Arc::new(Barrier::new(2));
        engine.inject_pause_once(
            if after_admission {
                "after_decision_admission"
            } else {
                "before_decision_admission"
            },
            Arc::clone(&barrier),
        );
        let (engine, result) = std::thread::scope(|scope| {
            let worker = scope.spawn(move || {
                let result = engine.decide_publication(prepared);
                (engine, result)
            });
            barrier.wait();
            let path = member.root.join("disk.json");
            let bytes = std::fs::read(&path).unwrap();
            std::fs::write(&path, b"broken seal").unwrap();
            assert!(probe.verify().is_err());
            std::fs::write(path, bytes).unwrap();
            barrier.wait();
            worker.join().unwrap()
        });
        let records = crate::journal::scan(&member).unwrap().records;
        assert!(
            records.iter().any(|(record, _)| record.sequence == sequence
                && record.record_type == RecordType::Prepare)
        );
        if after_admission {
            result.unwrap();
            assert_eq!(engine.state.sequence, sequence);
            assert_eq!(accepts(&engine, sequence), engine.members().len());
            assert!(engine.published_state_base.is_none());
        } else {
            assert_eq!(result.unwrap_err().code(), ErrorCode::PreparationStale);
            assert_eq!(engine.state.sequence, prior_sequence);
            assert_eq!(accepts(&engine, sequence), 0);
        }
        assert!(!engine.blocked);
    }
}

#[test]
fn terminal_offlock_scan_revokes_before_finish_can_acquire_the_publication_lock() {
    let (_temp, mut engine) = fixture();
    let owner = covered(&mut engine);
    let plan = engine.prepare_scheduled_integrity_scrub(&owner).unwrap();
    let prepared = prepare_auth(&engine);
    assert!(prepared.reused_dependencies.is_some());
    let sequence = prepared.candidate.sequence;
    let epoch = engine.metadata_integrity.capture();
    let integrity = Arc::clone(&engine.metadata_integrity);
    let barrier = Arc::new(Barrier::new(2));
    engine.inject_pause_once("before_decision_admission", Arc::clone(&barrier));
    let (mut engine, result, outcome) = std::thread::scope(|scope| {
        let worker = scope.spawn(move || {
            let result = engine.decide_publication(prepared);
            (engine, result)
        });
        barrier.wait();
        let cancellation = IntegrityScrubCancellation::new();
        cancellation.cancel();
        let mut terminal_observed = false;
        let outcome = plan.run(
            &cancellation,
            |_, _| panic!("cancelled before batch admission"),
            |_| {},
            |report| {
                if matches!(
                    report.completion,
                    crate::IntegrityScrubCompletion::Cancelled
                ) {
                    terminal_observed = true;
                }
            },
        );
        assert!(terminal_observed);
        assert!(
            epoch.matches(&integrity),
            "cancellation must not masquerade as a metadata fault"
        );
        // No Engine finish is possible until this publication returns. The
        // terminal run itself must already have revoked its original admission.
        barrier.wait();
        let (engine, result) = worker.join().unwrap();
        (engine, result, outcome)
    });
    assert_eq!(result.unwrap_err().code(), ErrorCode::PreparationStale);
    assert_eq!(accepts(&engine, sequence), 0);
    assert!(!engine.blocked);
    scan(&engine, &owner);
    assert!(engine.has_current_integrity_scrub_coverage());
    engine.finish_integrity_scrub(outcome);
    assert!(
        engine.has_current_integrity_scrub_coverage(),
        "late finish cannot revoke the new generation a second time"
    );
    let prepared = prepare_auth(&engine);
    engine.decide_publication(prepared).unwrap();
}
