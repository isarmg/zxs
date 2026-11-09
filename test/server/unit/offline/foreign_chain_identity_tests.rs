use super::*;
use json_codec::json;
use storage_engine::{ForeignDatabaseStatus, ForeignOriginalDatabasesReport, Mode};

fn fixture() -> (Config, Plan, databases::Observation) {
    let configuration = Config::independent_roots(
        Mode::Development,
        [std::path::PathBuf::from("/zxs/data/disks/current")],
    )
    .unwrap();
    let id = json!({"device":"1","inode":"2"});
    let context = json_codec::from_value(json!({"host":id,"data":id,"state":id,
        "management":id,"discovery":id,"account":{"name":"zxs","uid":1000,"gid":1000,
        "source_device":"1","source_inode":"3","source_sha256":"a".repeat(64)}}))
    .unwrap();
    let registration = DiskRegistrationIdentity {
        pool_id: "pool_source".into(),
        disk_id: "disk_source".into(),
        slot_id: "original_tertiary".into(),
        binding_generation: 7,
    };
    let lower: Vec<SelectedLower> = (1..=2)
        .map(|level| {
            let root = format!("/zxs/data/disks/new_{level}");
            let physical = json!({"device":"2","inode":(10 + level).to_string(),
            "ctime_seconds":"0","ctime_nanoseconds":0});
            SelectedLower {
                directory: LowerDirectory {
                    root_path: root.clone(),
                    slot_id: format!("new_{level}"),
                    level,
                    upstream_slot_id: (level == 2).then(|| "new_1".into()),
                    metadata_member: true,
                },
                selection: json_codec::from_value(json!({"selection_id":"a".repeat(64),
                "discovery_root":layout::DISKS,"discovery_identity":physical,
                "mount_path":layout::DISKS,"root_path":root,"root_identity":physical,
                "mount_identity":{"mount_id":"1","device_major":0,"device_minor":1,
                    "root_inode":"1","filesystem":"test","filesystem_root":"/"}}))
                .unwrap(),
            }
        })
        .collect();
    let plan = Plan {
        binding: Binding {
            format: Format::ChainMaterial,
            context,
            credential: json_codec::from_value(
                json!({"principal":"admin","generation":"1","fingerprint":"a".repeat(64)}),
            )
            .unwrap(),
            configuration_digest: digest(&configuration).unwrap(),
            registered_roots: vec!["/zxs/data/disks/current".into()],
            operation_id: "op_11111111111111111111111111111111".parse().unwrap(),
            source: Source {
                root_path: "/zxs/data/disks/foreign".into(),
                physical: Physical {
                    device: "3".into(),
                    inode: "4".into(),
                },
                registration: registration.clone(),
                level: 3,
                evidence_digest: "b".repeat(64),
                histories: 1,
                temporarily_unavailable_versions: 0,
            },
            source_slot_id: "new_3".into(),
            source_upstream_slot_id: "new_2".into(),
            source_metadata_member: true,
            lower,
        },
        capsule_digests: vec!["c".repeat(64), "d".repeat(64)],
    };
    let original = ForeignOriginalStateReport {
        pool_id: registration.pool_id.clone(),
        configuration_hash: "e".repeat(64),
        registrations: vec![registration],
        sequence: 2,
        snapshot_hash: "f".repeat(64),
        state_hash: "a".repeat(64),
        prefix_hash: "b".repeat(64),
        publications: 2,
        operations: 2,
        principals: 2,
        events: 2,
    };
    let reports: Vec<_> = (1..=2)
        .map(|level| ForeignOriginalDatabasesReport {
            original: original.clone(),
            capsule_digest: plan.capsule_digests[level as usize - 1].clone(),
            chain_digest: digest(&plan.binding).unwrap(),
            target_level: level,
            manifest_hash: "a".repeat(64),
            original_disks: 1,
            file_databases: 1,
            directory_databases: 0,
            databases: 4,
            database_bytes: 4096,
        })
        .collect();
    let statuses = reports
        .iter()
        .cloned()
        .map(|report| ForeignDatabaseStatus::Verified { report })
        .collect();
    (
        configuration,
        plan,
        databases::Observation {
            targets: reports,
            statuses,
        },
    )
}

#[test]
fn entire_chain_has_distinct_stable_target_identities_without_reusing_original_seal() {
    let (configuration, plan, observation) = fixture();
    let proposal = Proposal::capture(&configuration, &plan, &observation).unwrap();
    let again = Proposal::capture(&configuration, &plan, &observation).unwrap();
    assert_eq!(digest(&proposal).unwrap(), digest(&again).unwrap());
    assert_eq!(proposal.targets.len(), 3);
    let mut ids = BTreeSet::new();
    for (index, target) in proposal.targets.iter().enumerate() {
        assert_eq!(target.level, index as u32 + 1);
        assert_eq!(target.registration.pool_id, configuration.pool_id);
        assert_eq!(target.registration.binding_generation, 1);
        assert!(ids.insert(&target.registration.disk_id));
        assert_ne!(
            target.registration.disk_id,
            plan.binding.source.registration.disk_id
        );
        assert_eq!(
            target.upstream_slot_id,
            index
                .checked_sub(1)
                .map(|i| proposal.targets[i].registration.slot_id.clone())
        );
    }
    assert_eq!(
        proposal.source_registration,
        plan.binding.source.registration
    );
    let value = response(proposal).unwrap();
    assert_eq!(value["target_identity_proposal_only"], true);
    for key in [
        "target_identity_databases_installed",
        "foreign_admitted",
        "changes_existing_pool_membership",
        "source_modified",
        "starts_service",
        "complete_installation_ready",
    ] {
        assert_eq!(value[key], false);
    }
}

#[test]
fn incomplete_or_changed_local_database_proof_cannot_produce_target_identity_proposal() {
    let (configuration, plan, mut observation) = fixture();
    observation.statuses[1] = ForeignDatabaseStatus::Preparing;
    assert!(Proposal::capture(&configuration, &plan, &observation).is_err());
    observation.statuses[1] = ForeignDatabaseStatus::Verified {
        report: observation.targets[1].clone(),
    };
    observation.targets[1].original.state_hash = "0".repeat(64);
    assert!(Proposal::capture(&configuration, &plan, &observation).is_err());
    observation.targets.pop();
    observation.statuses.pop();
    assert!(Proposal::capture(&configuration, &plan, &observation).is_err());
}

#[test]
fn proposal_rejects_pool_baseline_drift_colliding_slots_and_incomplete_chain() {
    let (mut configuration, mut plan, observation) = fixture();
    configuration.max_entries += 1;
    assert!(Proposal::capture(&configuration, &plan, &observation).is_err());
    configuration.max_entries -= 1;
    configuration.disks[0].slot_id = plan.binding.source_slot_id.to_ascii_uppercase();
    plan.binding.configuration_digest = digest(&configuration).unwrap();
    assert!(Proposal::capture(&configuration, &plan, &observation).is_err());
    plan.binding.lower.remove(0);
    assert!(Proposal::capture(&configuration, &plan, &observation).is_err());
}

#[test]
fn original_full_state_must_bind_the_exact_foreign_source_registration() {
    let (configuration, mut plan, observation) = fixture();
    plan.binding.source.registration.binding_generation += 1;
    assert!(Proposal::capture(&configuration, &plan, &observation).is_err());
}

#[test]
fn proposal_digest_binds_every_selected_physical_root_and_the_original_operation() {
    let (configuration, mut plan, observation) = fixture();
    let first = Proposal::capture(&configuration, &plan, &observation).unwrap();
    plan.binding.lower[0].selection.root_identity.inode += 1;
    let second = Proposal::capture(&configuration, &plan, &observation).unwrap();
    assert_ne!(digest(&first).unwrap(), digest(&second).unwrap());
    assert_ne!(
        first.targets[0].registration.disk_id,
        second.targets[0].registration.disk_id
    );
}
