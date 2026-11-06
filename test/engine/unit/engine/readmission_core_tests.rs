//! A real admitted proposal still checks every witness when its pure view is reused.
use super::*;
use std::{fs, os::unix::fs::PermissionsExt};

fn admitted_core() -> (tempfile::TempDir, ReadmissionCore) {
    let temp = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let config =
        Config::development((0..3).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
    let mut engine = Engine::initialize(&temp.path().join("system"), config.clone()).unwrap();
    engine
        .create_repository(
            &RepositoryName::parse("verified-readmission").unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let disk = &config.disks[0];
    engine
        .begin_disk_exit(
            &disk.disk_id,
            MembershipExitAction::PlannedOffline,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let archive = engine.build_managed_membership_control_archive().unwrap();
    engine
        .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
        .unwrap();
    assert!(
        engine
            .resume_membership_joint_decision()
            .unwrap()
            .all_certificates_synchronized()
    );
    engine.activate_pending_disk_exit().unwrap();
    let directory = || PendingMemberDirectory {
        disk_id: disk.disk_id.clone(),
        directory: fs::File::open(&disk.root).unwrap(),
    };
    engine
        .begin_disk_readmission(
            &disk.disk_id,
            "admin",
            OperationId::generate().unwrap(),
            directory(),
        )
        .unwrap();
    let archive = engine.build_managed_membership_control_archive().unwrap();
    engine
        .activate_disk_readmission(&archive.archive_path, &archive.root_hash, directory())
        .unwrap();
    let core = engine
        .config
        .generation()
        .unwrap()
        .portable
        .readmission_core()
        .unwrap()
        .clone();
    (temp, core)
}

fn rejected_at_both_entries(core: &ReadmissionCore, expected: ErrorCode) {
    assert_eq!(core.intent().unwrap_err().code(), expected);
    match core.verified_registry() {
        Err(error) => assert_eq!(error.code(), expected),
        Ok(_) => panic!("registry must not bypass failed proposal or decision admission"),
    }
}

#[test]
fn verified_readmission_proposal_preserves_all_decisions_and_rejects_tampering() {
    let (_temp, core) = admitted_core();
    let bytes = canonical_json(&core).unwrap();
    let intent = core.intent().unwrap();
    assert_eq!(intent.origin.config().metadata_members().len(), 2);
    assert_eq!(core.decisions.len(), 2);
    assert_eq!(
        core.verified_registry().unwrap().commitment(),
        &core.proposal.registry
    );
    let decoded: ReadmissionCore = decode_strict(&bytes).unwrap();
    assert_eq!(canonical_json(&decoded).unwrap(), bytes);
    assert_eq!(
        decoded.proposal.hash().unwrap(),
        core.proposal.hash().unwrap()
    );
    assert_eq!(canonical_json(&core).unwrap(), bytes);
    assert_eq!(
        canonical_json(&decoded.intent().unwrap()).unwrap(),
        canonical_json(&intent).unwrap()
    );

    let original = core.decisions.keys().cloned().collect::<Vec<_>>();
    let mut missing = core.clone();
    assert!(missing.decisions.remove(&original[0]).is_some());
    rejected_at_both_entries(&missing, ErrorCode::MetadataQuorumUnavailable);

    let mut swapped = core.clone();
    assert!(
        swapped
            .decisions
            .insert(original[0].clone(), core.decisions[&original[1]].clone())
            .is_some()
    );
    rejected_at_both_entries(&swapped, ErrorCode::LogFork);

    let mut outsider = core.clone();
    let decision = outsider.decisions.remove(&original[0]).unwrap();
    assert!(
        outsider
            .decisions
            .insert("outside-original-voters".into(), decision)
            .is_none()
    );
    rejected_at_both_entries(&outsider, ErrorCode::MetadataQuorumUnavailable);

    let mut target = core.clone();
    target.target_decision = core.decisions[&original[0]].clone();
    rejected_at_both_entries(&target, ErrorCode::LogFork);

    let mut graph = core.clone();
    assert!(!graph.proposal.objects.is_empty());
    for object in graph.proposal.objects.values_mut() {
        object.kind = "another-readmission-graph-kind".into();
    }
    rejected_at_both_entries(&graph, ErrorCode::ObjectCorrupt);

    let mut transition = core.clone();
    transition.proposal.transition_hash = "0".repeat(64);
    rejected_at_both_entries(&transition, ErrorCode::ObjectCorrupt);

    let mut receipt = json_codec::to_value(&core).unwrap();
    receipt["target_decision"]["record_hash"] = json_codec::Value::String("0".repeat(64));
    let receipt: ReadmissionCore = json_codec::from_value(receipt).unwrap();
    rejected_at_both_entries(&receipt, ErrorCode::LogFork);
}
