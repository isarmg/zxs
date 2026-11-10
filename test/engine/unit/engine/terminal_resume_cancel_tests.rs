//! Source-only regressions for actual immutable choices, cancellation ownership,
//! cold receipts, and exact successor attempts. The integration owner runs them.
use super::*;
use crate::engine::membership_transition::decision::DecisionKind;
use crate::engine::membership_transition::decision::DecisionStore;
use crate::{TerminalMetadataResumeCancelOutcome, TerminalMetadataResumeCancellationPhase};

fn cancel(pending: &mut TerminalMetadataResume) -> OperationResult {
    let status = pending.status().unwrap();
    match pending
        .cancel("admin", status.operation_id, &status.transition_hash)
        .unwrap()
    {
        TerminalMetadataResumeCancelOutcome::Cancelled(result) => result,
        TerminalMetadataResumeCancelOutcome::ContinueOriginal(_) => {
            panic!("expected a cancellable unpublished request")
        }
    }
}

fn choose(pending: &TerminalMetadataResume, count: usize, resume: bool) {
    let transition = object_hash(INTENT_KIND, &pending.proposal.intent).unwrap();
    for disk in pending
        .proposal
        .intent
        .target
        .metadata_members()
        .take(count)
    {
        let member = &pending.members[&disk.disk_id];
        member
            .root
            .write_json(&disk.root.join("disk.json"), &member.seal, false)
            .unwrap();
        let store =
            DecisionStore::open(&member.root, &member.seal.identity, &transition, true).unwrap();
        if resume {
            store
                .receipt(
                    DecisionKind::TerminalResume,
                    &pending.proposal.hash().unwrap(),
                    &pending.proposal.acceptance_hash(&disk.disk_id).unwrap(),
                    true,
                    #[cfg(feature = "fault-injection")]
                    None,
                )
                .unwrap();
        } else {
            store
                .receipt(
                    DecisionKind::TerminalResumeCancel,
                    &pending.proposal.cancellation_hash().unwrap(),
                    &pending.proposal.cancel_acceptance(&disk.disk_id).unwrap(),
                    true,
                    #[cfg(feature = "fault-injection")]
                    None,
                )
                .unwrap();
        }
    }
}

fn writer_is_held(disk: &DiskConfig) {
    let writer = File::open(disk.root.join(".writer.lock")).unwrap();
    assert!(
        rustix::fs::flock(
            &writer,
            rustix::fs::FlockOperation::NonBlockingLockExclusive
        )
        .is_err()
    );
}

fn alternate_target(fixture: &Fixture, target: &Config, suffix: &str) -> Config {
    let mut next = target.clone();
    for disk in next.disks.iter_mut().skip(fixture.original.disks.len()) {
        disk.root = fixture
            .temporary
            .path()
            .join(format!("{suffix}-{}", disk.disk_id));
        fs::create_dir(&disk.root).unwrap();
        fs::set_permissions(&disk.root, fs::Permissions::from_mode(0o700)).unwrap();
    }
    next
}

#[test]
fn prepared_cancel_preserves_old_stop_and_releases_real_new_locks_only_after_receipt() {
    let mut fixture = Fixture::new(MembershipExitAction::Deregister, 0);
    let old_files = files(&fixture.original.disks[0].root);
    let stopped = fixture.temporary.path().join("never-probe-stopped");
    fs::rename(&fixture.original.disks[0].root, &stopped).unwrap();
    let target = fixture.target(3);
    let mut pending = fixture.prepare(&target);
    let proposal = fs::read(fixture.source_path().join("resume.json")).unwrap();
    let encoded: json_codec::Value = json_codec::from_slice(&proposal).unwrap();
    assert!(encoded.get("previous_cancellation_hash").is_none());
    let status = pending.status().unwrap();
    assert_eq!(
        status.cancellation_phase,
        TerminalMetadataResumeCancellationPhase::Cancellable
    );
    let result = cancel(&mut pending);
    assert_eq!(result.operation_id, fixture.resume);
    assert_eq!(result.kind, "terminal-metadata-resume-cancelled");
    assert_eq!(result.request_digest, status.transition_hash);
    assert_eq!(result.result["state"], "cancelled");
    assert_eq!(result.result["writable"], false);
    for disk in target.disks.iter().filter(|disk| disk.metadata_member) {
        writer_is_held(disk);
    }
    assert_eq!(cancel(&mut pending), result);
    assert!(pending.synchronize().is_err());
    let management = pending.into_cancelled_management().unwrap();
    assert_eq!(management.operation(fixture.resume), Some(&result));
    assert_eq!(
        management
            .state()
            .operation_principals
            .get(&fixture.resume)
            .unwrap(),
        "admin"
    );
    assert_eq!(
        fs::read(fixture.source_path().join("resume.json")).unwrap(),
        proposal
    );
    assert_eq!(files(&stopped), old_files);
    assert!(!fixture.original.disks[0].root.exists());
    for disk in target.disks.iter().filter(|disk| disk.metadata_member) {
        let writer = File::open(disk.root.join(".writer.lock")).unwrap();
        rustix::fs::flock(
            &writer,
            rustix::fs::FlockOperation::NonBlockingLockExclusive,
        )
        .unwrap();
        assert!(disk.root.join("disk.json").is_file());
        assert!(disk.root.join("metadata/membership-decisions").is_dir());
    }
}

#[test]
fn blocking_cancellation_uses_fixed_n_minus_q_plus_one_not_old_terminal_quorum() {
    for count in [1, 2, 3, 4, 5, 6] {
        let mut fixture = Fixture::new(MembershipExitAction::PlannedOffline, 0);
        let target = fixture.target(count);
        let mut pending = fixture.prepare(&target);
        let quorum = pending.proposal.intent.target.metadata_quorum();
        choose(&pending, quorum - 1, true);
        let result = cancel(&mut pending);
        let certificate: json_codec::Value = json_codec::from_slice(
            &fs::read(fixture.source_path().join("resume-cancel.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            certificate["decisions"].as_object().unwrap().len(),
            count - quorum + 1
        );
        assert_eq!(result.result["metadata_members"], count);
        assert_eq!(result.result["metadata_quorum"], quorum);
        assert_eq!(
            pending.status().unwrap().cancellation_phase,
            TerminalMetadataResumeCancellationPhase::Cancelled
        );
    }
}

#[test]
fn observed_new_quorum_and_selected_generation_cannot_be_cancelled_even_without_cached_votes() {
    for count in [1, 3, 4] {
        let mut fixture = Fixture::new(MembershipExitAction::PlannedOffline, 0);
        let target = fixture.target(count);
        let mut pending = fixture.prepare(&target);
        choose(
            &pending,
            pending.proposal.intent.target.metadata_quorum(),
            true,
        );
        assert!(pending.votes.is_empty());
        let status = pending.status().unwrap();
        assert_eq!(
            status.cancellation_phase,
            TerminalMetadataResumeCancellationPhase::ResumeQuorumAccepted
        );
        let before = files(&fixture.system());
        assert!(matches!(
            pending
                .cancel("admin", fixture.resume, &status.transition_hash)
                .unwrap(),
            TerminalMetadataResumeCancelOutcome::ContinueOriginal(_)
        ));
        assert_eq!(files(&fixture.system()), before);
        assert!(
            !fixture
                .source_path()
                .join("resume-cancel-intent.json")
                .exists()
        );
        pending.synchronize().unwrap();
        select_generation(&pending);
        assert_eq!(
            pending.cancellation_phase().unwrap(),
            TerminalMetadataResumeCancellationPhase::GenerationSelected
        );
        assert!(matches!(
            pending
                .cancel("admin", fixture.resume, &status.transition_hash)
                .unwrap(),
            TerminalMetadataResumeCancelOutcome::ContinueOriginal(_)
        ));
    }
}

#[test]
fn partial_cancel_cold_reentry_keeps_original_actor_digest_and_real_owners() {
    let mut fixture = Fixture::new(MembershipExitAction::Deregister, 0);
    let target = fixture.target(3);
    let pending = fixture.prepare(&target);
    let digest = pending.status().unwrap().transition_hash;
    choose(&pending, 1, false);
    assert_eq!(
        pending.cancellation_phase().unwrap(),
        TerminalMetadataResumeCancellationPhase::Cancelling
    );
    let original_pending = files(&target.disks[1].root);
    drop(pending);
    let system_before = files(&fixture.system());
    assert!(
        TerminalManagement::metadata_resume_cancelled(
            &fixture.system(),
            "admin",
            fixture.resume,
            &digest
        )
        .unwrap()
        .is_none()
    );
    assert_eq!(
        TerminalManagement::metadata_resume_recovery(&fixture.system())
            .unwrap()
            .unwrap()
            .phase,
        crate::TerminalMetadataResumePhase::ResupplyDirectories
    );
    assert_eq!(files(&fixture.system()), system_before);
    let mut pending =
        TerminalManagement::reopen_metadata_resume(&fixture.system(), directories(&target))
            .unwrap();
    assert!(pending.synchronize().is_err());
    assert!(
        pending
            .cancel("other-admin", fixture.resume, &digest)
            .is_err()
    );
    assert!(
        pending
            .cancel("admin", OperationId::generate().unwrap(), &digest)
            .is_err()
    );
    assert!(
        pending
            .cancel("admin", fixture.resume, &"1".repeat(64))
            .is_err()
    );
    assert_eq!(files(&target.disks[1].root), original_pending);
    for disk in target.disks.iter().filter(|disk| disk.metadata_member) {
        writer_is_held(disk);
    }
    let result = cancel(&mut pending);
    let before = files(&fixture.system());
    // The pure lookup also works while this exact pending owner holds SYSTEM.
    assert_eq!(
        TerminalManagement::metadata_resume_cancelled(
            &fixture.system(),
            "admin",
            fixture.resume,
            &digest
        )
        .unwrap(),
        Some(result.clone())
    );
    assert!(
        TerminalManagement::metadata_resume_recovery(&fixture.system())
            .unwrap()
            .is_none()
    );
    assert_eq!(files(&fixture.system()), before);
    drop(pending);
    let management = TerminalManagement::open(&fixture.system()).unwrap();
    assert_eq!(management.operation(fixture.resume), Some(&result));
    assert!(
        TerminalManagement::metadata_resume_cancelled(
            &fixture.system(),
            "other-admin",
            fixture.resume,
            &digest
        )
        .is_err()
    );
}

#[test]
fn cancelled_attempts_are_immutable_and_new_resume_publishes_every_permanent_receipt() {
    let mut fixture = Fixture::new(MembershipExitAction::Deregister, 0);
    let target = fixture.target(1);
    let mut pending = fixture.prepare(&target);
    let initial_request = fs::read(fixture.source_path().join("resume.json")).unwrap();
    let first_result = cancel(&mut pending);
    let management = pending.into_cancelled_management().unwrap();
    let first_cancel = fs::read(fixture.source_path().join("resume-cancel.json")).unwrap();
    let first_evidence = files(&target.disks[1].root);
    let next = alternate_target(&fixture, &target, "second");
    let second_op = OperationId::generate().unwrap();
    let mut pending = management
        .prepare_metadata_resume(next.clone(), "admin", second_op, directories(&next))
        .unwrap();
    let second_result = cancel(&mut pending);
    let management = pending.into_cancelled_management().unwrap();
    assert_eq!(management.operation(fixture.resume), Some(&first_result));
    assert_eq!(management.operation(second_op), Some(&second_result));
    let final_target = alternate_target(&fixture, &target, "third");
    let final_op = OperationId::generate().unwrap();
    let pending = management
        .prepare_metadata_resume(
            final_target.clone(),
            "admin",
            final_op,
            directories(&final_target),
        )
        .unwrap();
    let engine = pending.activate().unwrap();
    assert_eq!(
        engine.state.operations.get(&fixture.resume),
        Some(&first_result)
    );
    assert_eq!(
        engine.state.operations.get(&second_op),
        Some(&second_result)
    );
    assert_eq!(
        engine.state.operations[&final_op].kind,
        "terminal-metadata-resume"
    );
    assert_eq!(engine.state.operation_principals[&fixture.resume], "admin");
    assert_eq!(
        fs::read(fixture.source_path().join("resume.json")).unwrap(),
        initial_request
    );
    assert_eq!(
        fs::read(fixture.source_path().join("resume-cancel.json")).unwrap(),
        first_cancel
    );
    assert_eq!(files(&target.disks[1].root), first_evidence);
    assert_eq!(
        crate::canonical_json(&engine.state.versions).unwrap(),
        fixture.historical_versions
    );
    assert_eq!(
        crate::canonical_json(&engine.state.commits).unwrap(),
        fixture.historical_commits
    );
    drop(engine);
    assert!(!TerminalManagement::requires_management(&fixture.system()).unwrap());
    let resumed = TerminalManagement::complete_metadata_resume(&fixture.system()).unwrap();
    assert_eq!(
        resumed.state.operations.get(&fixture.resume),
        Some(&first_result)
    );
    assert_eq!(
        resumed.state.operations.get(&second_op),
        Some(&second_result)
    );
}

#[test]
fn mixed_cancel_counts_only_voters_and_never_erases_prepared_data_material() {
    let mut fixture = Fixture::new(MembershipExitAction::Deregister, 0);
    let mut target = fixture.target(3);
    target.disks[3].metadata_member = false;
    let directories = target
        .disks
        .iter()
        .skip(1)
        .map(|disk| PendingMemberDirectory {
            disk_id: disk.disk_id.clone(),
            directory: File::open(&disk.root).unwrap(),
        })
        .collect();
    let mut pending = fixture
        .management
        .take()
        .unwrap()
        .prepare_metadata_resume(target.clone(), "admin", fixture.resume, directories)
        .unwrap();
    pending.synchronize_data().unwrap();
    let data_before = files(&target.disks[3].root);
    let result = cancel(&mut pending);
    assert_eq!(result.result["metadata_members"], 2);
    assert_eq!(result.result["metadata_quorum"], 2);
    assert_eq!(files(&target.disks[3].root), data_before);
    writer_is_held(&target.disks[3]);
    drop(pending.into_cancelled_management().unwrap());
    let writer = File::open(target.disks[3].root.join(".writer.lock")).unwrap();
    rustix::fs::flock(
        &writer,
        rustix::fs::FlockOperation::NonBlockingLockExclusive,
    )
    .unwrap();
    assert_eq!(files(&target.disks[3].root), data_before);
}

#[test]
fn unknown_attempt_or_changed_cancel_certificate_is_preserved_and_never_selected() {
    let mut fixture = Fixture::new(MembershipExitAction::Deregister, 0);
    let target = fixture.target(1);
    let mut pending = fixture.prepare(&target);
    let digest = pending.status().unwrap().transition_hash;
    cancel(&mut pending);
    drop(pending);
    let namespace = fixture.source_path().join("resume-attempts");
    fs::create_dir(&namespace).unwrap();
    fs::set_permissions(&namespace, fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir(namespace.join("f".repeat(64))).unwrap();
    let before = files(&fixture.system());
    assert!(
        TerminalManagement::metadata_resume_cancelled(
            &fixture.system(),
            "admin",
            fixture.resume,
            &digest
        )
        .is_err()
    );
    assert!(TerminalManagement::open(&fixture.system()).is_err());
    assert_eq!(files(&fixture.system()), before);
    assert!(namespace.join("f".repeat(64)).is_dir());
}

#[test]
fn insufficient_or_rebound_cancel_certificate_is_preserved_and_rejected() {
    for rebound in [false, true] {
        let mut fixture = Fixture::new(MembershipExitAction::Deregister, 0);
        let target = fixture.target(3);
        let mut pending = fixture.prepare(&target);
        let digest = pending.status().unwrap().transition_hash;
        cancel(&mut pending);
        drop(pending);
        let path = fixture.source_path().join("resume-cancel.json");
        let mut certificate: json_codec::Value =
            json_codec::from_slice(&fs::read(&path).unwrap()).unwrap();
        if rebound {
            certificate["proposal"]["intent"]["principal"] = "different-actor".into();
        } else {
            let votes = certificate["decisions"].as_object_mut().unwrap();
            let keys = votes.keys().skip(1).cloned().collect::<Vec<_>>();
            for key in keys {
                votes.remove(&key);
            }
        }
        let bytes = crate::canonical_json(&certificate).unwrap();
        fs::write(&path, &bytes).unwrap();
        assert!(
            TerminalManagement::metadata_resume_cancelled(
                &fixture.system(),
                "admin",
                fixture.resume,
                &digest
            )
            .is_err()
        );
        assert!(TerminalManagement::open(&fixture.system()).is_err());
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}

#[test]
#[ignore = "requires test/engine/unit/repair/io_tails.c as LD_PRELOAD=repair_io_tails.so"]
fn cancelled_owner_handoff_waits_for_real_file_and_directory_barriers() {
    use crate::repair_io_tail_support::{Point, SyscallGate};
    for directory_barrier in [false, true] {
        for fail in [false, true] {
            let mut fixture = Fixture::new(MembershipExitAction::Deregister, 0);
            let mut target = fixture.target(3);
            target.disks[3].metadata_member = false;
            let held = target
                .disks
                .iter()
                .skip(1)
                .map(|disk| PendingMemberDirectory {
                    disk_id: disk.disk_id.clone(),
                    directory: File::open(&disk.root).unwrap(),
                })
                .collect();
            let mut pending = fixture
                .management
                .take()
                .unwrap()
                .prepare_metadata_resume(target.clone(), "admin", fixture.resume, held)
                .unwrap();
            pending.synchronize_data().unwrap();
            let result = cancel(&mut pending);
            let data_before = files(&target.disks[3].root);
            let barrier = if directory_barrier {
                fixture.source_path()
            } else {
                fixture.source_path().join("resume.json")
            };
            // Marker files stay outside the exact SYSTEM attempt inventory.
            let handoff = std::thread::scope(|scope| {
                let gate = SyscallGate::arm(
                    fixture.temporary.path(),
                    &fixture.temporary.path().join("absent-sync-sentinel"),
                    Point::FirstSync,
                    fail,
                );
                fs::write(
                    fixture.temporary.path().join(".repair-tail-sync-target"),
                    barrier.as_os_str().as_encoded_bytes(),
                )
                .unwrap();
                let worker = scope.spawn(move || pending.into_cancelled_management());
                gate.wait();
                assert_eq!(gate.synced(Point::FirstSync), Some(barrier.clone()));
                for disk in target.disks.iter().skip(1) {
                    writer_is_held(disk);
                }
                assert_eq!(files(&target.disks[3].root), data_before);
                gate.release();
                worker.join().unwrap()
            });
            let management = if fail {
                let error = handoff
                    .err()
                    .expect("failed sync cannot hand out management");
                assert_eq!(error.code(), ErrorCode::IoError);
                let mut cause: &(dyn std::error::Error + 'static) = &error;
                while let Some(source) = cause.source() {
                    cause = source;
                }
                assert_eq!(
                    cause
                        .downcast_ref::<std::io::Error>()
                        .unwrap()
                        .raw_os_error(),
                    Some(5)
                );
                // Explicit recovery repeats the same history barriers. A visible
                // certificate alone did not let the failed call return an owner.
                TerminalManagement::open(&fixture.system()).unwrap()
            } else {
                handoff.unwrap()
            };
            assert_eq!(management.operation(fixture.resume), Some(&result));
            for disk in target.disks.iter().skip(1) {
                let writer = File::open(disk.root.join(".writer.lock")).unwrap();
                rustix::fs::flock(
                    &writer,
                    rustix::fs::FlockOperation::NonBlockingLockExclusive,
                )
                .unwrap();
            }
            assert_eq!(files(&target.disks[3].root), data_before);
        }
    }
}
