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
// Exercise the history descriptor path rather than only the snapshot auth map.
fn prepare_history_change(engine: &Engine) -> DetachedPublication {
    try_prepare_history_change(engine).unwrap()
}

fn try_prepare_history_change(engine: &Engine) -> Result<DetachedPublication> {
    let branch = engine.state.branches.values().next().unwrap();
    let branch_id = branch.branch_id.clone();
    let history = branch.history_tree_id.clone();
    let name = format!("{} changed", branch.name);
    let prior = engine
        .published_state_base
        .as_ref()
        .unwrap()
        .roots
        .catalog_binding(&history)
        .unwrap();
    let digest = object_hash("history-name-test", &name).unwrap();
    let plan = engine
        .planning_context()
        .plan_new_mutation(
            "history.rename-test",
            "alice",
            OperationId::generate(),
            digest,
            |_, next, _| {
                next.branches.get_mut(&branch_id).unwrap().name = name;
                Ok(json!({"history": &history}))
            },
        )
        .unwrap();
    let crate::engine::MutationPlan::Prepared {
        next,
        prepared,
        result,
    } = plan
    else {
        panic!("new history operation must prepare")
    };
    let detached = engine.prepare_publication(*next, prepared, result, MAX_METADATA)?;
    let updated = detached.history_basis.catalog_binding(&history).unwrap();
    assert_ne!(updated.hash, prior.hash);
    assert_eq!(updated.encoder.is_some(), prior.encoder.is_some());
    if let (Some(prior), Some(updated)) = (&prior.encoder, &updated.encoder) {
        assert!(!Arc::ptr_eq(prior, updated));
    }
    Ok(detached)
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
        let prepared = prepare_history_change(&engine);
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
    let prepared = prepare_history_change(&engine);
    scan(&engine, &owner);
    engine.decide_publication(prepared).unwrap();
    let prepared = prepare_history_change(&engine);
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
        let prepared = prepare_history_change(&engine);
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
    let prepared = prepare_history_change(&engine);
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
    let prepared = prepare_history_change(&engine);
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
        let prepared = prepare_history_change(&engine);
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
    let prepared = prepare_history_change(&engine);
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
    let prepared = prepare_history_change(&engine);
    engine.decide_publication(prepared).unwrap();
}

#[test]
fn changed_history_omits_old_dependencies_but_export_scrub_and_reopen_still_read_them() {
    for missing in [false, true] {
        let (temp, mut engine) = fixture();
        let owner = covered(&mut engine);
        let history = engine.state.histories.keys().next().unwrap().clone();
        let base = engine.published_state_base.as_ref().unwrap();
        let binding = base.roots.catalog_binding(&history).unwrap();
        assert!(binding.encoder.is_some());
        let members = engine.members();
        let root =
            load_authenticated_object(&members[0], "history-manifest", &binding.hash).unwrap();
        let decoded = crate::history_package::decode_history_root(&root).unwrap();
        // A branch-name edit must retain this nonempty commits map. The snapshot
        // also reuses its exact old dependency, so neither path may reinstall it.
        let unchanged = &decoded.maps["commits"];
        let paths: Vec<_> = members
            .iter()
            .map(|member| object_path(member, unchanged.kind().domain(), unchanged.hash()))
            .collect();
        let mut damaged = std::fs::read(&paths[0]).unwrap();
        assert!(!damaged.is_empty());
        damaged[0] = b'!';
        for path in &paths {
            if missing {
                std::fs::remove_file(path).unwrap();
            } else {
                std::fs::write(path, &damaged).unwrap();
            }
        }
        let prepared = prepare_history_change(&engine);
        assert!(prepared.reused_dependencies.is_some());
        engine.decide_publication(prepared).unwrap();
        assert!(engine.has_current_integrity_scrub_coverage());
        for path in &paths {
            if missing {
                assert!(!path.exists());
            } else {
                assert_eq!(std::fs::read(path).unwrap(), damaged);
            }
        }
        assert!(
            engine
                .export_history_package(&history, &temp.path().join("broken-export"))
                .is_err(),
            "an encoder descriptor is not evidence that current disk bytes are readable"
        );
        assert!(!engine.has_current_integrity_scrub_coverage());
        let report = engine.scrub(&IntegrityScrubCancellation::new()).unwrap();
        assert!(report.metadata_failures > 0);
        assert!(!report.is_healthy());
        let root = engine.root.clone();
        drop(owner);
        drop(engine);
        assert!(
            Engine::open(&root).is_err(),
            "cold recovery must reject an exact dependency with no healthy copy"
        );
    }
}

#[test]
fn descriptors_do_not_omit_source_reads_without_a_periodic_owner() {
    for manual_scan in [false, true] {
        let (_temp, mut engine) = fixture();
        if manual_scan {
            assert!(
                engine
                    .scrub(&IntegrityScrubCancellation::new())
                    .unwrap()
                    .is_healthy()
            );
        }
        let base = engine.published_state_base.clone().unwrap();
        let history = engine.state.histories.keys().next().unwrap();
        let binding = base.roots.catalog_binding(history).unwrap();
        assert!(binding.encoder.is_some());
        let path = object_path(&engine.members()[0], "history-manifest", &binding.hash);
        std::fs::remove_file(&path).unwrap();
        let prepared = prepare_history_change(&engine);
        assert!(prepared.reused_dependencies.is_none());
        // A healthy other member lets the existing full path finish, but the
        // failed source is observed and cannot mint a new successful receipt.
        engine.decide_publication(prepared).unwrap();
        assert!(engine.published_state_base.is_none());
        assert!(!base.matches(
            &engine.planning_context(),
            &object_hash("publication-config", &engine.config).unwrap()
        ));
        assert!(!path.exists(), "the discarded old root is not repaired");
    }
}

#[test]
fn changed_history_index_failure_cannot_publish_its_new_descriptor() {
    let (_temp, mut engine) = fixture();
    let _owner = covered(&mut engine);
    let base = engine.published_state_base.clone().unwrap();
    let prepared = prepare_history_change(&engine);
    assert!(prepared.reused_dependencies.is_some());
    let sequence = prepared.candidate.sequence;
    let operation = prepared.result.operation_id;
    let index = engine.root.join("index.sqlite");
    std::fs::rename(&index, engine.root.join("saved-index.sqlite")).unwrap();
    std::fs::create_dir(&index).unwrap();
    assert!(engine.decide_publication(prepared).is_err());
    assert!(engine.blocked);
    assert_eq!(engine.state.sequence, sequence);
    assert!(engine.state.operations.contains_key(&operation));
    assert_eq!(accepts(&engine, sequence), engine.members().len());
    assert!(Arc::ptr_eq(
        &base,
        engine.published_state_base.as_ref().unwrap()
    ));
    assert!(!base.matches(
        &engine.planning_context(),
        &object_hash("publication-config", &engine.config).unwrap()
    ));
}

#[test]
#[ignore = "requires history_package/immutable_io/run.sh isolated libc read observer"]
fn changed_history_reads_in_isolated_child() {
    let root = std::path::PathBuf::from(std::env::var_os("ZXS_HISTORY_IO_ROOT").unwrap());
    let log = std::path::PathBuf::from(std::env::var_os("ZXS_HISTORY_IO_LOG").unwrap());
    assert_eq!(std::env::var("ZXS_HISTORY_IO_STAGE").unwrap(), "read");
    assert_eq!(std::env::var("ZXS_HISTORY_IO_MEMBER").unwrap(), "-1");
    let (_temp, mut engine) = fixture();
    let history = engine.state.histories.keys().next().unwrap();
    let binding = engine
        .published_state_base
        .as_ref()
        .unwrap()
        .roots
        .catalog_binding(history)
        .unwrap();
    let members = engine.members();
    let object = load_authenticated_object(&members[0], "history-manifest", &binding.hash).unwrap();
    let decoded = crate::history_package::decode_history_root(&object).unwrap();
    let unchanged = &decoded.maps["commits"];
    assert_eq!(unchanged.kind(), crate::metadata_map::ReferenceKind::Page);
    let page = load_authenticated_object(&members[0], unchanged.kind().domain(), unchanged.hash())
        .unwrap();
    assert!(!page.value().as_object().unwrap().is_empty());
    let mut targets = String::new();
    for member in &members {
        let path = object_path(member, unchanged.kind().domain(), unchanged.hash());
        targets.push_str(path.to_str().unwrap());
        targets.push('\n');
    }
    std::fs::write(root.join("targets"), targets).unwrap();
    std::fs::write(&log, b"").unwrap();
    std::fs::write(root.join("observe"), b"").unwrap();
    let prepared = prepare_history_change(&engine);
    assert!(prepared.reused_dependencies.is_none());
    engine.decide_publication(prepared).unwrap();
    std::fs::remove_file(root.join("observe")).unwrap();
    let full_events = std::fs::read_to_string(&log).unwrap();
    assert!(
        full_events.lines().any(|line| line.ends_with(" read data")),
        "the complete path must positively demonstrate this libc read hook"
    );

    let _owner = covered(&mut engine);
    std::fs::write(&log, b"").unwrap();
    std::fs::write(root.join("observe"), b"").unwrap();
    let prepared = prepare_history_change(&engine);
    assert!(prepared.reused_dependencies.is_some());
    engine.decide_publication(prepared).unwrap();
    let qualified_events = std::fs::read_to_string(&log).unwrap();
    assert!(
        qualified_events
            .lines()
            .all(|line| !line.contains(" read ")),
        "the unchanged exact commits page must not be read on a qualified history update: {qualified_events}"
    );
    let history = engine.state.histories.keys().next().unwrap();
    engine
        .export_history_package(history, &root.join("export"))
        .unwrap();
    let export_events = std::fs::read_to_string(&log).unwrap();
    assert!(
        export_events
            .lines()
            .any(|line| line.ends_with(" read data")),
        "export must independently read the same unchanged graph dependency"
    );
    std::fs::remove_file(root.join("observe")).unwrap();
    std::fs::write(root.join("completed"), b"read:-1").unwrap();
}

#[test]
fn new_same_hash_history_root_still_compares_every_required_member() {
    let (_temp, mut engine) = fixture();
    let _owner = covered(&mut engine);
    let original = engine.published_state_base.clone().unwrap();
    let sequence = engine.state.sequence;
    let slot = engine.next_slot;
    // A successful preparation installs the new graph but is not a publication.
    // Repeating the same logical branch edit therefore creates the same history
    // root, even though the new operation receipt and snapshot differ.
    let prepared = prepare_history_change(&engine);
    assert!(prepared.reused_dependencies.is_some());
    let history = engine.state.histories.keys().next().unwrap();
    let hash = prepared
        .history_basis
        .catalog_binding(history)
        .unwrap()
        .hash
        .clone();
    drop(prepared);
    let members = engine.members();
    for member in &members {
        load_authenticated_object(member, "history-manifest", &hash).unwrap();
    }
    let damaged = object_path(members.last().unwrap(), "history-manifest", &hash);
    std::fs::write(&damaged, b"different unpublished history bytes").unwrap();
    let error = match try_prepare_history_change(&engine) {
        Ok(_) => panic!("fresh same-hash history output must compare every required member"),
        Err(error) => error,
    };
    assert_eq!(error.code(), ErrorCode::ObjectCollision);
    assert_eq!(engine.state.sequence, sequence);
    assert_eq!(engine.next_slot, slot);
    assert_eq!(accepts(&engine, slot), 0);
    assert!(Arc::ptr_eq(
        &original,
        engine.published_state_base.as_ref().unwrap()
    ));
    assert!(!original.matches(
        &engine.planning_context(),
        &object_hash("publication-config", &engine.config).unwrap()
    ));
    assert_eq!(
        std::fs::read(damaged).unwrap(),
        b"different unpublished history bytes"
    );
}

#[test]
fn qualified_descriptor_miss_reads_exact_graph_without_promoting_or_masking_errors() {
    for missing in [false, true] {
        let (_temp, mut engine) = fixture();
        let _owner = covered(&mut engine);
        let history = engine.state.histories.keys().next().unwrap().clone();
        // Remove only this optional in-memory optimization. The exact published
        // root and all receipt/schedule identities remain genuine and unchanged.
        let base = Arc::make_mut(engine.published_state_base.as_mut().unwrap());
        let mut binding = base.roots.catalog_binding(&history).unwrap().clone();
        binding.encoder = None;
        let hash = binding.hash.clone();
        let mut encoder = crate::metadata_map::Encoder::new(MAX_METADATA);
        let catalog = encoder
            .update_catalog(
                &base.roots,
                std::collections::BTreeSet::from([history.clone()]),
                |_| Ok(Some(binding.clone())),
            )
            .unwrap();
        assert_eq!(catalog.reference(), base.roots.reference());
        base.roots = catalog;
        let members = engine.members();
        if missing {
            for member in &members {
                std::fs::remove_file(object_path(member, "history-manifest", &hash)).unwrap();
            }
            let expected = match load_authenticated_object(&members[0], "history-manifest", &hash) {
                Ok(_) => panic!("missing root unexpectedly readable"),
                Err(error) => error,
            };
            let sequence = engine.state.sequence;
            let slot = engine.next_slot;
            let error = match try_prepare_history_change(&engine) {
                Ok(_) => panic!("qualified miss cannot invent a source or mask its failure"),
                Err(error) => error,
            };
            assert_eq!(error.code(), expected.code());
            assert_eq!(error.to_string(), expected.to_string());
            assert_eq!(engine.state.sequence, sequence);
            assert_eq!(engine.next_slot, slot);
            assert_eq!(accepts(&engine, slot), 0);
            assert!(!engine.has_current_integrity_scrub_coverage());
        } else {
            let prepared = prepare_history_change(&engine);
            assert!(prepared.reused_dependencies.is_some());
            assert!(
                prepared
                    .history_basis
                    .catalog_binding(&history)
                    .unwrap()
                    .encoder
                    .is_none()
            );
            engine.decide_publication(prepared).unwrap();
            assert!(
                engine
                    .published_state_base
                    .as_ref()
                    .unwrap()
                    .roots
                    .catalog_binding(&history)
                    .unwrap()
                    .encoder
                    .is_none()
            );
        }
    }
}
