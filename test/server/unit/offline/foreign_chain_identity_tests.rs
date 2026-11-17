use super::*;
use json_codec::json;
use storage_engine::{ForeignDatabaseStatus, ForeignOriginalDatabasesReport, Mode};

fn fixture() -> (Config, Plan, databases::Observation) {
    let configuration = Config::independent_roots(
        Mode::Development,
        [(
            name_policy::Name::parse("Current").unwrap(),
            std::path::PathBuf::from("/zxs/data/disks/current"),
        )],
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
                    name: name_policy::Name::parse(&format!("Lower archive {level}")).unwrap(),
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
            source_name: name_policy::Name::parse("Imported archive").unwrap(),
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
        .map(|report| ForeignDatabaseStatus::Verified {
            report: Box::new(report),
        })
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
    let (proposal, proposed) = Proposal::capture(&configuration, &plan, &observation).unwrap();
    let (again, _) = Proposal::capture(&configuration, &plan, &observation).unwrap();
    assert_eq!(digest(&proposal).unwrap(), digest(&again).unwrap());
    assert_eq!(proposal.targets.len(), 3);
    assert_eq!(proposal.targets[0].name, plan.binding.lower[0].directory.name);
    assert_eq!(proposal.targets[1].name, plan.binding.lower[1].directory.name);
    assert_eq!(proposal.targets[2].name, plan.binding.source_name);
    assert_eq!(
        digest(&proposed).unwrap(),
        proposal.proposed_configuration_digest
    );
    assert_eq!(
        proposed.disks.len(),
        configuration.disks.len() + proposal.targets.len()
    );
    assert_eq!(
        json_codec::to_value(&proposed.disks[..configuration.disks.len()]).unwrap(),
        json_codec::to_value(&configuration.disks).unwrap()
    );
    for (disk, target) in proposed.disks[configuration.disks.len()..]
        .iter()
        .zip(&proposal.targets)
    {
        assert_eq!(disk.name, target.name);
        assert_eq!(disk.disk_id, target.registration.disk_id);
        assert_eq!(disk.slot_id, target.registration.slot_id);
        assert_eq!(
            disk.binding_generation,
            target.registration.binding_generation
        );
        assert_eq!(disk.root, std::path::PathBuf::from(&target.root_path));
        assert_eq!(disk.level, target.level);
        assert_eq!(disk.upstream_slot_id, target.upstream_slot_id);
        assert_eq!(disk.metadata_member, target.metadata_member);
        assert_eq!(disk.capacity_bytes, None);
    }
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
        report: Box::new(observation.targets[1].clone()),
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
    let (configuration, mut plan, mut observation) = fixture();
    let (first, _) = Proposal::capture(&configuration, &plan, &observation).unwrap();
    let mut renamed = plan.clone();
    renamed.binding.lower[0].directory.name = name_policy::Name::parse("Other archive").unwrap();
    assert_ne!(digest(&renamed.binding).unwrap(), digest(&plan.binding).unwrap());
    assert_ne!(digest(&renamed).unwrap(), first.request_digest);
    let mut renamed = plan.clone();
    renamed.binding.source_name = name_policy::Name::parse("Other source").unwrap();
    assert_ne!(digest(&renamed.binding).unwrap(), digest(&plan.binding).unwrap());
    plan.binding.lower[0].selection.root_identity.inode += 1;
    assert!(Proposal::capture(&configuration, &plan, &observation).is_err());
    // A different original batch needs freshly bound target evidence. The old
    // database completion cannot merely be carried across the root change.
    let chain = digest(&plan.binding).unwrap();
    for report in &mut observation.targets {
        report.chain_digest = chain.clone();
    }
    observation.statuses = observation.targets.iter().cloned()
        .map(|report| ForeignDatabaseStatus::Verified { report: Box::new(report) })
        .collect();
    let (second, _) = Proposal::capture(&configuration, &plan, &observation).unwrap();
    assert_ne!(digest(&first).unwrap(), digest(&second).unwrap());
    assert_ne!(
        first.targets[0].registration.disk_id,
        second.targets[0].registration.disk_id
    );
}

#[test]
fn capsule_completion_requires_every_exact_ordered_plan_target() {
    let (_, plan, _) = fixture();
    let report = Report {
        targets: plan.binding.lower.iter().zip(&plan.capsule_digests)
            .map(|(selected, capsule_digest)| TargetProgress {
                level: selected.directory.level,
                path: format!("{}/{CAPSULE}", selected.directory.root_path),
                capsule_digest: capsule_digest.clone(),
                progress: Progress::PublishedCapsule,
            }).collect(),
    };
    report.require_plan(&plan).unwrap();
    assert!(report.all_published());
    let round_trip: Report = json_codec::from_slice(&json_codec::to_vec(&report).unwrap()).unwrap();
    round_trip.require_plan(&plan).unwrap();
    assert_eq!(json_codec::to_vec(&round_trip).unwrap(), json_codec::to_vec(&report).unwrap());

    for change in 0..7 {
        let mut changed = report.clone();
        match change {
            0 => { changed.targets.pop(); }
            1 => changed.targets.swap(0, 1),
            2 => changed.targets[1] = changed.targets[0].clone(),
            3 => changed.targets[1].level += 1,
            4 => changed.targets[1].path.push_str("-other"),
            5 => changed.targets[1].capsule_digest = "f".repeat(64),
            6 => changed.targets.push(changed.targets[0].clone()),
            _ => unreachable!(),
        }
        assert!(changed.all_published());
        assert!(changed.require_plan(&plan).is_err());
    }
    let mut pending = report;
    pending.targets[1].progress = Progress::Reserved;
    pending.require_plan(&plan).unwrap();
    assert!(!pending.all_published());
}

#[test]
fn self_consistent_database_status_cannot_rebind_another_chain_or_scope() {
    let (configuration, plan, observed) = fixture();
    observed.expected(&plan).unwrap();
    for change in 0..8 {
        let mut changed = observed.clone();
        match change {
            0 => { changed.targets.pop(); }
            1 => changed.targets.swap(0, 1),
            2 => changed.targets[1].capsule_digest = "f".repeat(64),
            3 => changed.targets[1].chain_digest = "0".repeat(64),
            4 => changed.targets[1].target_level += 1,
            5 => changed.targets[1].original.state_hash = "0".repeat(64),
            6 => changed.targets[1].manifest_hash = "0".repeat(64),
            7 => changed.targets[1].database_bytes += 1,
            _ => unreachable!(),
        }
        changed.statuses = changed.targets.iter().cloned()
            .map(|report| ForeignDatabaseStatus::Verified { report: Box::new(report) })
            .collect();
        assert!(changed.completed());
        assert!(changed.expected(&plan).is_err());
        assert!(Proposal::capture(&configuration, &plan, &changed).is_err());
    }
    let mut absent_status = observed.clone();
    absent_status.statuses.pop();
    assert!(absent_status.expected(&plan).is_err());
    let mut swapped_status = observed;
    swapped_status.statuses.swap(0, 1);
    assert!(swapped_status.expected(&plan).is_err());
}

fn object_report(
    plan: &Plan,
    databases: &databases::Observation,
) -> storage_engine::ForeignIdentityClosureReport {
    use storage_engine::{ForeignIdentityBaseline, ForeignIdentityClosureReport};
    ForeignIdentityClosureReport {
        baseline: ForeignIdentityBaseline {
            pool_id: "pool_current".into(),
            sequence: 4,
            state_hash: "1".repeat(64),
            configuration_hash: "2".repeat(64),
            proposed_configuration_hash: "3".repeat(64),
            operation_id: plan.binding.operation_id,
            request_digest: digest(plan).unwrap(),
            chain_digest: digest(&plan.binding).unwrap(),
            registration_proposal_digest: "4".repeat(64),
        },
        original: databases.targets[0].original.clone(),
        mapping_digest: "5".repeat(64),
        objects_by_domain: [("file".into(), 2)].into_iter().collect(),
        objects: 2,
        archived_operations: 2,
        archived_principals: 2,
        archived_plans: 0,
        archived_events: 2,
        mapping_bytes: 4096,
    }
}

#[test]
fn identity_preflight_matches_complete_reports_without_consuming_the_original_retry() {
    let (_, plan, databases) = fixture();
    let expected_databases = databases.expected(&plan).unwrap();
    let expected = object_report(&plan, &databases);
    let observed = objects::Observation {
        expected: expected.clone(),
        statuses: vec![objects::Progress::Preparing, objects::Progress::NotStarted],
    };
    let original_bytes = json_codec::to_vec(&observed).unwrap();
    objects::match_expected(&plan, &expected_databases, &observed, &expected).unwrap();

    let changes: &[fn(&mut storage_engine::ForeignIdentityClosureReport)] = &[
        |report| report.baseline.pool_id = "pool_other".into(),
        |report| report.baseline.sequence += 1,
        |report| report.baseline.state_hash = "0".repeat(64),
        |report| report.baseline.configuration_hash = "0".repeat(64),
        |report| report.baseline.proposed_configuration_hash = "0".repeat(64),
        |report| report.baseline.registration_proposal_digest = "0".repeat(64),
        |report| report.original.snapshot_hash = "0".repeat(64),
        |report| report.original.registrations[0].binding_generation += 1,
        |report| report.mapping_digest = "0".repeat(64),
        |report| report.objects += 1,
        |report| report.mapping_bytes += 1,
        |report| report.archived_operations += 1,
        |report| report.archived_principals += 1,
        |report| report.archived_plans += 1,
        |report| report.archived_events += 1,
        |report| report.objects_by_domain.clear(),
    ];
    for change in changes {
        let mut altered = expected.clone();
        change(&mut altered);
        assert!(objects::match_expected(
            &plan, &expected_databases, &observed, &altered,
        ).is_err());
        assert_eq!(json_codec::to_vec(&observed).unwrap(), original_bytes);
        objects::match_expected(&plan, &expected_databases, &observed, &expected).unwrap();
    }
}

#[test]
fn identity_observation_requires_the_exact_original_and_every_lower_status() {
    let (_, plan, databases) = fixture();
    let expected_databases = databases.expected(&plan).unwrap();
    let expected = object_report(&plan, &databases);
    for change in [
        (|report: &mut storage_engine::ForeignIdentityClosureReport| {
            report.baseline.request_digest = "0".repeat(64);
        }) as fn(&mut storage_engine::ForeignIdentityClosureReport),
        |report| report.baseline.chain_digest = "0".repeat(64),
        |report| {
            report.baseline.operation_id = "op_22222222222222222222222222222222".parse().unwrap();
        },
        |report| report.original.prefix_hash = "0".repeat(64),
        |report| report.original.registrations.clear(),
    ] {
        let mut altered = expected.clone();
        change(&mut altered);
        let observed = objects::Observation {
            expected: altered.clone(),
            statuses: vec![objects::Progress::Verified, objects::Progress::Verified],
        };
        assert!(observed.completed(&plan));
        assert!(objects::match_expected(
            &plan, &expected_databases, &observed, &altered,
        ).is_err());
    }
    for count in [0, 1, 3] {
        let observed = objects::Observation {
            expected: expected.clone(),
            statuses: (0..count).map(|_| objects::Progress::Verified).collect(),
        };
        assert!(objects::match_expected(
            &plan, &expected_databases, &observed, &expected,
        ).is_err());
    }
}

#[test]
fn database_preflight_rejects_self_consistent_source_drift_and_keeps_exact_retry() {
    let (_, plan, observed) = fixture();
    let expected = observed.expected(&plan).unwrap();
    let original_bytes = json_codec::to_vec(&expected).unwrap();
    let mut altered = observed.clone();
    for target in &mut altered.targets {
        target.original.state_hash = "0".repeat(64);
    }
    altered.statuses = altered.targets.iter().cloned()
        .map(|report| ForeignDatabaseStatus::Verified { report: Box::new(report) })
        .collect();
    assert!(altered.completed());
    altered.expected(&plan).unwrap();
    assert!(databases::match_expected(&plan, &altered, &expected).is_err());
    assert_eq!(json_codec::to_vec(&expected).unwrap(), original_bytes);
    databases::match_expected(&plan, &observed, &expected).unwrap();
    expected.require_original(&plan, &observed.targets[0].original).unwrap();
    assert!(expected.require_original(&plan, &altered.targets[0].original).is_err());
}

#[test]
fn material_intent_preflight_preserves_absent_conflicting_and_exact_original_records() {
    let (_, plan, _) = fixture();
    let directory = tempfile::tempdir().unwrap();
    let host = crate::offline::private_io::BundleDestination::open(directory.path()).unwrap();
    assert!(!inspect_material_intent(&host, &plan, false).unwrap());
    assert!(inspect_material_intent(&host, &plan, true).is_err());
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);

    let name = operation_name(plan.binding.operation_id);
    let bytes = json_codec::to_vec(&plan).unwrap();
    host.publish(&name, &[("plan.json", &bytes)]).unwrap();
    assert!(inspect_material_intent(&host, &plan, true).unwrap());
    let mut changed = plan.clone();
    changed.binding.source.evidence_digest = "0".repeat(64);
    for completed in [false, true] {
        assert!(inspect_material_intent(&host, &changed, completed).is_err());
        assert_eq!(
            host.read_operation(&name, "plan.json", MAX_RECORD_BYTES).unwrap(),
            Some(bytes.clone()),
        );
        assert!(inspect_material_intent(&host, &plan, completed).unwrap());
    }
}

#[test]
fn empty_pending_material_intent_cannot_be_treated_as_completed_or_reconstructed() {
    use std::os::unix::fs::PermissionsExt;
    let (_, plan, _) = fixture();
    let directory = tempfile::tempdir().unwrap();
    let pending = directory.path().join(format!(
        "{}.pending", operation_name(plan.binding.operation_id),
    ));
    std::fs::create_dir(&pending).unwrap();
    std::fs::set_permissions(&pending, std::fs::Permissions::from_mode(0o700)).unwrap();
    let host = crate::offline::private_io::BundleDestination::open(directory.path()).unwrap();
    assert!(!inspect_material_intent(&host, &plan, false).unwrap());
    assert!(inspect_material_intent(&host, &plan, true).is_err());
    assert_eq!(std::fs::read_dir(&pending).unwrap().count(), 0);
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn complete_foreign_scope_delivery_counts_the_repeated_original_proofs_and_both_external_envelopes() {
    use crate::offline::service_worker;
    let (_, seed, seed_observation) = fixture();
    let mut accepted = false;
    let mut worker_blocked = false;
    let mut maintenance_blocked = false;
    for count in 2..=16_u32 {
        let mut plan = seed.clone();
        plan.binding.lower = (1..=count).map(|level| {
            let mut item = seed.binding.lower[0].clone();
            item.directory.name = name_policy::Name::parse(&format!("Lower archive {level}")).unwrap();
            item.directory.root_path = format!("/zxs/data/disks/new_{level}");
            item.directory.slot_id = format!("new_{level}");
            item.directory.level = level;
            item.directory.upstream_slot_id = (level > 1).then(|| format!("new_{}", level - 1));
            item.selection.root_path = item.directory.root_path.clone();
            item.selection.root_identity.inode = u64::from(level) + 10;
            item
        }).collect();
        plan.capsule_digests = (1..=count).map(|i| format!("{i:064x}")).collect();
        plan.binding.source.level = count + 1;
        plan.binding.source_slot_id = "source_top".into();
        plan.binding.source_upstream_slot_id = format!("new_{count}");
        plan.validate().unwrap();
        let input = Original {service_user:"zxs".into(), operation_id:plan.binding.operation_id,
            request_digest:digest(&plan).unwrap(), external_writers_stopped:true};
        let mut template = seed_observation.targets[0].clone();
        for index in 1..32 {
            template.original.registrations.push(DiskRegistrationIdentity {
                pool_id: template.original.pool_id.clone(), disk_id: format!("disk_{index:032x}"),
                slot_id: format!("original_slot_{index}"), binding_generation: 1,
            });
        }
        template.original_disks = template.original.registrations.len();
        template.databases = 1 + template.original_disks * 2 + template.file_databases + template.directory_databases;
        template.database_bytes = template.databases as u64 * 32768;
        let chain = digest(&plan.binding).unwrap();
        let targets = (1..=count).map(|level| {
            let mut report = template.clone();
            report.capsule_digest = plan.capsule_digests[level as usize - 1].clone();
            report.chain_digest = chain.clone(); report.target_level = level; report
        }).collect::<Vec<_>>();
        let pending = databases::Observation {targets:targets.clone(), statuses:vec![ForeignDatabaseStatus::NotStarted; count as usize]};
        let expected = pending.expected(&plan).unwrap();
        let envelope = WorkerRequest {context:plan.binding.context.clone(), action:Action::Databases {
            plan:plan.clone(), prepare:Some(expected.clone()), completed:false,
        }};
        if json_codec::to_vec(&envelope).unwrap().len() > MAX_RECORD_BYTES
            || !service_worker::success_fits::<WorkerRequest>(&Output::Databases {report:pending}).unwrap() { continue; }
        let final_report = databases::Observation {targets:targets.clone(), statuses:targets.into_iter().map(|r| ForeignDatabaseStatus::Verified { report:Box::new(r) }).collect()};
        let worker = json_codec::to_vec(&json!({"status":"success","result":Output::Databases{report:final_report.clone()}})).unwrap().len();
        let actual_public = databases::response(&input, expected.clone(), final_report, true).unwrap();
        let public = json_codec::to_vec(&actual_public).unwrap().len();
        let checked = delivery::databases(&input, &plan, &expected);
        if worker > MAX_RECORD_BYTES {
            assert!(checked.unwrap_err().to_string().contains("worker success")); worker_blocked = true;
        } else if public > MAX_RECORD_BYTES {
            assert!(checked.unwrap_err().to_string().contains("maintenance result")); maintenance_blocked = true;
        } else { checked.unwrap(); accepted = true; }
    }
    assert!(accepted && worker_blocked && maintenance_blocked);
}

#[test]
fn exact_compact_byte_limit_counts_escaped_utf8_and_includes_the_success_wrapper() {
    use crate::offline::service_worker;
    let value = json!({"path":"文件\\\"\n","items":["a", "b"]});
    let n = json_codec::to_vec(&value).unwrap().len();
    assert!(service_worker::encoded_fits(&value, n).unwrap());
    assert!(!service_worker::encoded_fits(&value, n - 1).unwrap());
    // Make the actual worker success frame exactly 64 KiB, not its inner DTO.
    let sample = |path: String| Output::Report {report:Report {targets:vec![TargetProgress {
        level:1, path, capsule_digest:"a".repeat(64), progress:Progress::PublishedCapsule,
    }]}};
    let empty = json_codec::to_vec(&json!({"status":"success","result":sample(String::new())})).unwrap().len();
    assert!(service_worker::success_fits::<WorkerRequest>(&sample("x".repeat(MAX_RECORD_BYTES - empty))).unwrap());
    assert!(!service_worker::success_fits::<WorkerRequest>(&sample("x".repeat(MAX_RECORD_BYTES - empty + 1))).unwrap());
}
