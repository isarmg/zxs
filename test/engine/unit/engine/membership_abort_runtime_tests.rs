use super::*;

fn hide_rejected_target(fixture: &Fixture) -> PathBuf {
    let target = &fixture.target.disks.last().unwrap().root;
    let preserved = fixture.temp.path().join("rejected-target-preserved");
    fs::rename(target, &preserved).unwrap();
    std::os::unix::fs::symlink("/must-not-open-rejected-target", target).unwrap();
    preserved
}

#[test]
fn production_abort_publishes_permanent_result_without_target_io() {
    let mut fixture = Fixture::new(3);
    let fence = fixture.engine.membership_fence_status().unwrap();
    let original_logs = fixture
        .original
        .disks
        .iter()
        .map(|disk| fs::read(crate::journal::log_path(disk)).unwrap())
        .collect::<Vec<_>>();
    let preserved = hide_rejected_target(&fixture);
    let result = fixture
        .engine
        .cancel_membership_handoff(&fixture.source, &fixture.archive.root_hash)
        .unwrap();
    assert_eq!(result.kind, "membership-add-cancelled");
    assert_eq!(result.operation_id, fence.operation_id);
    assert_eq!(result.request_digest, fence.transition_hash);
    assert_eq!(
        fixture.engine.config.disks.len(),
        fixture.original.disks.len()
    );
    assert!(!fixture.engine.blocked);
    assert!(preserved.read_dir().unwrap().next().is_some());
    for (disk, before) in fixture.original.disks.iter().zip(original_logs) {
        assert_eq!(fs::read(crate::journal::log_path(disk)).unwrap(), before);
    }
    let again = fixture
        .engine
        .cancel_membership_handoff(&fixture.source, &fixture.archive.root_hash)
        .unwrap();
    assert_eq!(again, result);
    let root = fixture.temp.path().join("system");
    drop(fixture.engine);
    let mut reopened = Engine::open(&root).unwrap();
    assert_eq!(reopened.state.operations[&result.operation_id], result);
    reopened
        .create_repository(
            &RepositoryName::parse("after-cancel").unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
}

#[test]
fn production_abort_preserves_minority_joint_and_uses_fixed_old_majority() {
    let mut fixture = Fixture::new(5);
    let context = fixture.context();
    let joint0 = fixture.joint(&context, 0);
    let joint1 = fixture.joint(&context, 1);
    let status = fixture
        .engine
        .certify_membership_abort(&fixture.source, &fixture.archive.root_hash)
        .unwrap();
    assert_eq!(
        (
            status.confirmed_members,
            status.original_members,
            status.original_quorum
        ),
        (3, 5, 3)
    );
    assert!(fixture.engine.blocked);
    fixture
        .store(&context, 0)
        .verify_receipt(&joint0, false)
        .unwrap();
    fixture
        .store(&context, 1)
        .verify_receipt(&joint1, false)
        .unwrap();
    let result = fixture.engine.activate_pending_membership().unwrap();
    assert_eq!(result.kind, "membership-add-cancelled");
    assert!(!fixture.engine.blocked);
}

#[test]
fn production_abort_cannot_escape_joint_quorum_or_reduce_denominator() {
    let mut fixture = Fixture::new(4);
    let context = fixture.context();
    for index in 0..3 {
        fixture.joint(&context, index);
    }
    let before = fixture.engine.config.generation_hash().map(str::to_owned);
    let error = fixture
        .engine
        .certify_membership_abort(&fixture.source, &fixture.archive.root_hash)
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::RecoveryRequired);
    assert!(error.to_string().contains("Q=3 of N=4"));
    assert_eq!(fixture.engine.config.generation_hash(), before.as_deref());
    assert!(fixture.engine.membership_fence.is_some());
    assert!(fixture.engine.blocked);
}

#[test]
fn complete_archive_certifies_with_only_old_quorum_but_does_not_restore_writes() {
    let mut fixture = Fixture::new(3);
    let missing = fixture.id(2).to_owned();
    fixture.engine.disk_resources = std::sync::Arc::new(
        fixture
            .engine
            .disk_resources
            .excluding(&[missing].into_iter().collect()),
    );
    hide_rejected_target(&fixture);
    let status = fixture
        .engine
        .certify_membership_abort(&fixture.source, &fixture.archive.root_hash)
        .unwrap();
    assert_eq!(
        (
            status.confirmed_members,
            status.original_members,
            status.original_quorum
        ),
        (2, 3, 2)
    );
    assert_eq!(fixture.engine.config.disks.len(), 3);
    assert!(fixture.engine.blocked);
    assert_eq!(
        fixture
            .engine
            .activate_pending_membership()
            .unwrap_err()
            .code(),
        ErrorCode::MetadataQuorumUnavailable
    );
    let root = fixture.temp.path().join("system");
    drop(fixture.engine);
    let recovered = Engine::resume_membership_activation(&root).unwrap();
    assert_eq!(recovered.config.disks.len(), 3);
    assert!(!recovered.blocked);
}

#[test]
fn production_abort_can_start_and_publish_a_fresh_addition() {
    let mut fixture = Fixture::new(1);
    let cancellation = fixture
        .engine
        .cancel_membership_handoff(&fixture.source, &fixture.archive.root_hash)
        .unwrap();
    let abort_hash = fixture.engine.config.generation_hash().unwrap().to_owned();
    let mut target = fixture.original.clone();
    let spare = Config::development([
        fixture.temp.path().join("spare-unused"),
        fixture.temp.path().join("next-add"),
    ])
    .unwrap();
    let disk = spare.disks.last().unwrap().clone();
    private_dir(&disk.root).unwrap();
    target.disks.push(disk.clone());
    fixture
        .engine
        .begin_membership_handoff(target, "admin", OperationId::generate().unwrap())
        .unwrap();
    assert_eq!(
        fixture
            .engine
            .membership_fence
            .as_ref()
            .unwrap()
            .intent
            .origin
            .predecessor(),
        Some(abort_hash.as_str())
    );
    let archive = fixture
        .engine
        .build_managed_membership_control_archive()
        .unwrap();
    fixture
        .engine
        .enroll_pending_members(
            &archive.archive_path,
            &archive.root_hash,
            vec![pending::PendingMemberDirectory {
                disk_id: disk.disk_id,
                directory: fs::File::open(&disk.root).unwrap(),
            }],
        )
        .unwrap();
    fixture.engine.resume_membership_joint_decision().unwrap();
    let added = fixture.engine.activate_pending_membership().unwrap();
    assert_eq!(added.kind, "membership-add");
    assert_eq!(
        fixture.engine.state.operations[&cancellation.operation_id],
        cancellation
    );
}

#[test]
fn production_marker_keeps_valid_alternate_witness_bytes_for_the_same_generation() {
    let mut fixture = Fixture::new(3);
    let context = fixture.context();
    fixture
        .engine
        .certify_membership_abort(&fixture.source, &fixture.archive.root_hash)
        .unwrap();
    let votes = fixture.votes(&context, &[1, 2]);
    let portable = pending::activation::PortableAuthority::new_abort(
        context.core.clone(),
        AbortAuthorityEvidence {
            archive: PortableAbortArchive::read(
                DiskRoot::open(&fixture.source).unwrap(),
                &fixture.archive,
            )
            .unwrap(),
            witness: context.certify(&votes).unwrap(),
        },
        None,
    )
    .unwrap();
    let marker = fixture.original.disks[0]
        .root
        .join("metadata/pool-membership/generation.json");
    let before = fs::read(&marker).unwrap();
    let mut generation = (**fixture.engine.config.generation().unwrap()).clone();
    assert_eq!(generation.hash, portable.hash().unwrap());
    assert_ne!(before, canonical_json(&portable).unwrap());
    generation.portable = portable;
    fixture.engine.config = ValidatedConfig::try_from((*fixture.engine.config).clone())
        .unwrap()
        .with_generation(std::sync::Arc::new(generation))
        .unwrap();
    fixture.engine.activate_pending_membership().unwrap();
    assert_eq!(fs::read(&marker).unwrap(), before);
}

#[cfg(feature = "fault-injection")]
#[test]
fn production_abort_recovers_each_generation_publication_boundary() {
    for stage in [
        "marker",
        "seal",
        "after_prepare_1",
        "after_accept_1",
        "after_finalize",
        "after_publication_1",
        "before_membership_activation_projection",
    ] {
        let mut fixture = Fixture::new(3);
        let operation = fixture
            .engine
            .membership_fence_status()
            .unwrap()
            .operation_id;
        let point = match stage {
            "marker" => format!("after_membership_abort_marker_{}", fixture.id(0)),
            "seal" => format!("after_membership_activation_seal_{}", fixture.id(0)),
            other => other.to_owned(),
        };
        hide_rejected_target(&fixture);
        fixture.engine.inject_fault_once(&point);
        assert!(
            fixture
                .engine
                .cancel_membership_handoff(&fixture.source, &fixture.archive.root_hash)
                .is_err(),
            "{stage}"
        );
        let system = fixture.temp.path().join("system");
        drop(fixture.engine);
        let mut recovered = Engine::resume_membership_activation(&system).unwrap();
        let result = recovered.activate_pending_membership().unwrap();
        assert_eq!(result.operation_id, operation);
        assert_eq!(result.kind, "membership-add-cancelled");
        assert!(!recovered.blocked);
        assert_eq!(recovered.state.operations[&operation], result);
    }
}

#[test]
fn impossible_abort_quorum_does_not_consume_the_remaining_empty_slots() {
    let mut fixture = Fixture::new(4);
    let context = fixture.context();
    fixture.joint(&context, 0);
    fixture.joint(&context, 1);
    let error = fixture
        .engine
        .certify_membership_abort(&fixture.source, &fixture.archive.root_hash)
        .unwrap_err();
    assert!(error.to_string().contains("eligible=2"));
    for index in [2, 3] {
        assert!(!fixture.decision_path(index).exists());
        fixture
            .store(&context, index)
            .require_joint_allowed()
            .unwrap();
    }
    assert!(fixture.engine.membership_fence.is_some());
    assert!(fixture.engine.blocked);
}
