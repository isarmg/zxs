//! Source regressions for the original Disabled directory, not fresh enrollment.
//! These cases must run against real roots/publications; static formatting is not
//! evidence that the scenarios passed.
use super::*;
use crate::engine::membership_transition::terminal_resume::TerminalMetadataResumeAdmission;

fn retained_directory(disk: &DiskConfig) -> PendingMemberDirectory {
    PendingMemberDirectory {
        disk_id: disk.disk_id.clone(),
        directory: File::open(&disk.root).unwrap(),
    }
}

fn prepare_retained(fixture: &mut Fixture) -> TerminalMetadataResume {
    let disk = &fixture.original.disks[0];
    fixture
        .management
        .take()
        .unwrap()
        .prepare_disk_readmission(
            &disk.disk_id,
            "admin",
            fixture.resume,
            retained_directory(disk),
        )
        .unwrap()
}

#[test]
fn single_disabled_metadata_returns_with_identity_history_data_and_real_receipt() {
    let mut fixture = Fixture::new(MembershipExitAction::PlannedOffline, 1);
    let old = fixture.original.disks[0].clone();
    let management = fixture.management.as_ref().unwrap();
    let preview = management.preview_disk_readmission(&old.disk_id).unwrap();
    let initial = management.operation(fixture.initial).unwrap().clone();
    let stopped = management.result().clone();
    let records = files(&fixture.system().join("bootstrap-discovery"));
    assert!(preview.metadata_member);
    assert_eq!(
        management
            .readmission_metadata_member(&old.disk_id)
            .unwrap(),
        Some(true)
    );
    assert_eq!(
        management
            .readmission_metadata_member(&fixture.original.disks[1].disk_id)
            .unwrap(),
        None
    );
    let old_seal = fs::read(old.root.join("disk.json")).unwrap();
    let mut pending = prepare_retained(&mut fixture);
    let status = pending.synchronize().unwrap();
    assert_eq!(
        (
            status.target_metadata_members,
            status.target_metadata_quorum
        ),
        (1, 1)
    );
    assert!(status.all_control_metadata_synchronized());
    assert!(matches!(
        status.admission,
        TerminalMetadataResumeAdmission::RetainedMetadata { .. }
    ));
    assert!(pending.proposal.intent.added_roots.is_empty());
    assert_eq!(
        fs::read(old.root.join("disk.json")).unwrap(),
        old_seal,
        "preparation cannot rewrite an original root as membership-pending"
    );
    assert!(Engine::open(&fixture.system()).is_err());
    let engine = pending.activate().unwrap();
    let result = engine.state.operations[&fixture.resume].clone();
    assert_eq!(result.kind, "disk-readmit");
    assert_eq!(result.result["authority_kind"], "terminal-metadata-resume");
    assert_eq!(
        result.result["terminal_authority"],
        preview.terminal_authority_hash
    );
    assert_eq!(result.result["disk_id"], old.disk_id);
    assert_eq!(result.result["slot_id"], old.slot_id);
    assert_eq!(
        result.result["binding_generation"],
        old.binding_generation.to_string()
    );
    assert_eq!(result.result["io_admission_epoch"], result.request_digest);
    assert_eq!(engine.state.operations[&fixture.initial], initial);
    assert_eq!(engine.state.operations[&fixture.terminal], stopped);
    assert_eq!(
        crate::canonical_json(&engine.state.versions).unwrap(),
        fixture.historical_versions
    );
    assert_eq!(
        crate::canonical_json(&engine.state.commits).unwrap(),
        fixture.historical_commits
    );
    assert_eq!(engine.config.disks.len(), fixture.original.disks.len());
    assert_eq!(
        engine.config.io_admission_epoch(&old.disk_id).unwrap(),
        Some(result.request_digest.clone())
    );
    let registered = engine.registered_disk(&old.disk_id).unwrap();
    assert_eq!(
        registered.registration_mode,
        crate::RegistrationMode::Enabled
    );
    assert_eq!(registered.binding_generation, old.binding_generation);
    assert!(registered.metadata_member);
    assert!(
        engine
            .disk_resources
            .available(&engine.config.disks[1])
            .is_some()
    );
    assert_eq!(engine.list_tree(&fixture.branch).unwrap().len(), 1);
    for (path, bytes) in records {
        if path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("record-")
        {
            assert_eq!(
                fs::read(fixture.system().join("bootstrap-discovery").join(path)).unwrap(),
                bytes
            );
        }
    }
    drop(engine);
    let reopened = Engine::open(&fixture.system()).unwrap();
    assert_eq!(reopened.state.operations[&fixture.resume], result);
    assert_eq!(reopened.list_tree(&fixture.branch).unwrap().len(), 1);
}

#[test]
fn retained_readmission_never_reopens_a_closed_old_handle_or_epoch() {
    let temp = tempfile::tempdir().unwrap();
    let original = Config::development([temp.path().join("retained")]).unwrap();
    let system = temp.path().join("system");
    let mut engine = Engine::initialize(&system, original.clone()).unwrap();
    let disk = original.disks[0].clone();
    engine
        .create_repository(
            &RepositoryName::parse("old-handles").unwrap(),
            &disk.slot_id,
            1,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let old_config = engine.config.clone();
    let old = Arc::clone(engine.disk_resources.get(&disk).unwrap());
    let binding = old.root.io_binding().unwrap().clone();
    let observer = engine
        .open_disk_observation_directory(&disk.slot_id)
        .unwrap();
    engine
        .begin_disk_exit(
            &disk.disk_id,
            MembershipExitAction::PlannedOffline,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let mut exit = engine.prepare_terminal_disk_exit().unwrap();
    let management = exit.complete(&mut engine).unwrap();
    drop(exit);
    drop(engine);
    binding.closed_proof().unwrap();
    let resumed = management
        .prepare_disk_readmission(
            &disk.disk_id,
            "admin",
            OperationId::generate().unwrap(),
            retained_directory(&disk),
        )
        .unwrap()
        .activate()
        .unwrap();
    assert!(binding.admit().is_err());
    assert!(old.root.directory(&disk.root).is_err());
    assert!(observer.metadata().is_err());
    assert!(DiskIoBinding::new(&old.seal, &old_config).is_err());
    resumed
        .open_disk_observation_directory(&disk.slot_id)
        .unwrap()
        .metadata()
        .unwrap();
}

#[test]
fn preview_does_not_probe_and_commit_rejects_missing_or_replaced_original_directory() {
    let mut fixture = Fixture::new(MembershipExitAction::PlannedOffline, 0);
    let disk = fixture.original.disks[0].clone();
    let moved = fixture.temporary.path().join("original-retained-away");
    fs::rename(&disk.root, &moved).unwrap();
    let management = fixture.management.take().unwrap();
    assert!(
        management
            .preview_disk_readmission(&disk.disk_id)
            .unwrap()
            .metadata_member
    );
    assert!(
        management
            .prepare_disk_readmission(
                &disk.disk_id,
                "admin",
                fixture.resume,
                PendingMemberDirectory {
                    disk_id: disk.disk_id.clone(),
                    directory: File::open(&moved).unwrap()
                }
            )
            .is_err()
    );
    assert!(!fixture.source_path().join("resume.json").exists());
    fs::create_dir(&disk.root).unwrap();
    fs::set_permissions(&disk.root, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
        disk.root.join("disk.json"),
        fs::read(moved.join("disk.json")).unwrap(),
    )
    .unwrap();
    let management = TerminalManagement::open(&fixture.system()).unwrap();
    assert!(
        management
            .prepare_disk_readmission(
                &disk.disk_id,
                "admin",
                fixture.resume,
                retained_directory(&disk)
            )
            .is_err()
    );
    assert!(!fixture.source_path().join("resume.json").exists());
}

#[test]
fn detached_and_data_only_cannot_forge_terminal_metadata_readmission() {
    let mut detached = Fixture::new(MembershipExitAction::Deregister, 1);
    let disk = detached.original.disks[0].clone();
    let management = detached.management.take().unwrap();
    assert_eq!(
        management
            .readmission_metadata_member(&disk.disk_id)
            .unwrap(),
        None
    );
    assert!(management.preview_disk_readmission(&disk.disk_id).is_err());
    assert!(
        management
            .prepare_disk_readmission(
                &disk.disk_id,
                "admin",
                detached.resume,
                retained_directory(&disk)
            )
            .is_err()
    );
    let fixture = Fixture::new(MembershipExitAction::PlannedOffline, 1);
    let data = &fixture.original.disks[1];
    assert!(
        fixture
            .management
            .as_ref()
            .unwrap()
            .preview_disk_readmission(&data.disk_id)
            .is_err()
    );
    assert!(!fixture.source_path().join("resume.json").exists());
}

#[test]
fn saved_retained_resume_rejects_other_actor_operation_and_fresh_discriminator() {
    let mut fixture = Fixture::new(MembershipExitAction::PlannedOffline, 0);
    let disk = fixture.original.disks[0].clone();
    let pending = prepare_retained(&mut fixture);
    assert_eq!(pending.principal(), "admin");
    assert_eq!(pending.operation_id(), fixture.resume);
    let mut encoded = json_codec::to_value(&pending.proposal).unwrap();
    encoded.as_object_mut().unwrap().remove("admission");
    let forged: ResumeProposal = json_codec::from_value(encoded).unwrap();
    assert!(forged.verify().is_err());
    drop(pending);
    for (actor, operation) in [
        ("other", fixture.resume),
        ("admin", OperationId::generate().unwrap()),
    ] {
        let management = TerminalManagement::open_for_resume(&fixture.system()).unwrap();
        assert!(
            management
                .prepare_disk_readmission(
                    &disk.disk_id,
                    actor,
                    operation,
                    retained_directory(&disk)
                )
                .is_err()
        );
    }
    let recovery = TerminalManagement::metadata_resume_recovery(&fixture.system())
        .unwrap()
        .unwrap();
    assert_eq!(recovery.operation_id, fixture.resume);
    assert_eq!(recovery.principal, "admin");
    assert!(matches!(
        recovery.admission,
        TerminalMetadataResumeAdmission::RetainedMetadata { .. }
    ));
    let pending = TerminalManagement::reopen_metadata_resume(
        &fixture.system(),
        vec![retained_directory(&disk)],
    )
    .unwrap();
    assert_eq!(pending.operation_id(), fixture.resume);
}

#[test]
fn retained_resume_faults_recover_only_the_exact_original_operation() {
    for phase in [
        WriteFault::BeforeRecord,
        WriteFault::AfterRecord,
        WriteFault::BeforeHead,
        WriteFault::AfterHead,
    ] {
        let mut fixture = Fixture::new(MembershipExitAction::PlannedOffline, 0);
        let terminal = fixture.management.as_ref().unwrap().result().clone();
        let locator_path = fixture.system().join("bootstrap-discovery");
        let stopped_locator = files(&locator_path);
        let pending = prepare_retained(&mut fixture);
        inject_next(phase);
        assert!(pending.activate().is_err(), "{phase:?}");
        assert!(!fixture.source_path().join("resume-complete.json").exists());
        assert!(Engine::open(&fixture.system()).is_err());
        let recovery = TerminalManagement::metadata_resume_recovery(&fixture.system())
            .unwrap()
            .unwrap();
        assert_eq!(recovery.operation_id, fixture.resume);
        assert!(matches!(
            recovery.admission,
            TerminalMetadataResumeAdmission::RetainedMetadata { .. }
        ));
        let engine = TerminalManagement::complete_metadata_resume(&fixture.system()).unwrap();
        let result = engine.state.operations[&fixture.resume].clone();
        assert_eq!(result.kind, "disk-readmit");
        assert_eq!(engine.state.operations[&fixture.terminal], terminal);
        assert_eq!(
            engine
                .config
                .io_admission_epoch(&fixture.original.disks[0].disk_id)
                .unwrap(),
            Some(result.request_digest.clone())
        );
        let resumed_locator = files(&locator_path);
        assert_ne!(
            resumed_locator[Path::new("head.json")],
            stopped_locator[Path::new("head.json")],
            "readmission must select its new epoch, not roll back the old Stop"
        );
        for (path, bytes) in stopped_locator {
            if path.file_name().unwrap() != "head.json" {
                assert_eq!(resumed_locator[&path], bytes);
            }
        }
        drop(engine);
        let reopened = Engine::open(&fixture.system()).unwrap();
        assert_eq!(reopened.state.operations[&fixture.resume], result);
        assert_eq!(reopened.state.operations[&fixture.terminal], terminal);
        assert_eq!(files(&locator_path), resumed_locator);
    }
}

#[test]
fn completed_retained_resume_requires_its_system_probe_certificate_on_cold_start() {
    let mut fixture = Fixture::new(MembershipExitAction::PlannedOffline, 0);
    let engine = prepare_retained(&mut fixture).activate().unwrap();
    let result = engine.state.operations[&fixture.resume].clone();
    drop(engine);
    let probe = fixture
        .system()
        .join("readmission-probe-admissions")
        .join(format!("{}.json", result.request_digest));
    let saved = fs::read(&probe).unwrap();
    fs::write(&probe, b"{}").unwrap();
    assert!(Engine::open(&fixture.system()).is_err());
    fs::write(&probe, saved).unwrap();
    let engine = Engine::open(&fixture.system()).unwrap();
    assert_eq!(engine.state.operations[&fixture.resume], result);
}

#[test]
fn repeated_terminal_readmission_keeps_every_stop_receipt_and_creates_distinct_epochs() {
    let mut fixture = Fixture::new(MembershipExitAction::PlannedOffline, 0);
    let disk = fixture.original.disks[0].clone();
    let mut engine = prepare_retained(&mut fixture).activate().unwrap();
    let first = engine.state.operations[&fixture.resume].clone();
    let stop = OperationId::generate().unwrap();
    engine
        .begin_disk_exit(
            &disk.disk_id,
            MembershipExitAction::PlannedOffline,
            "admin",
            stop,
        )
        .unwrap();
    let mut exit = engine.prepare_terminal_disk_exit().unwrap();
    let management = exit.complete(&mut engine).unwrap();
    drop(exit);
    drop(engine);
    let second_operation = OperationId::generate().unwrap();
    let engine = management
        .prepare_disk_readmission(
            &disk.disk_id,
            "admin",
            second_operation,
            retained_directory(&disk),
        )
        .unwrap()
        .activate()
        .unwrap();
    let second = &engine.state.operations[&second_operation];
    assert_ne!(first.request_digest, second.request_digest);
    assert_eq!(engine.state.operations[&fixture.resume], first);
    assert_eq!(engine.state.operations[&stop].kind, "disk-planned-offline");
    assert_eq!(
        engine.config.disks[0].binding_generation,
        disk.binding_generation
    );
}

#[test]
fn earlier_disabled_metadata_can_resume_after_another_final_voter_is_detached() {
    let temp = tempfile::tempdir().unwrap();
    let original =
        Config::development((0..2).map(|n| temp.path().join(format!("root-{n}")))).unwrap();
    let system = temp.path().join("system");
    let mut engine = Engine::initialize(&system, original.clone()).unwrap();
    let retained = &original.disks[0];
    engine
        .create_repository(
            &RepositoryName::parse("retained-first").unwrap(),
            &retained.slot_id,
            1,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let first_stop = OperationId::generate().unwrap();
    engine
        .begin_disk_exit(
            &retained.disk_id,
            MembershipExitAction::PlannedOffline,
            "admin",
            first_stop,
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
    let final_disk = &original.disks[1];
    let terminal_operation = OperationId::generate().unwrap();
    engine
        .begin_disk_exit(
            &final_disk.disk_id,
            MembershipExitAction::Deregister,
            "admin",
            terminal_operation,
        )
        .unwrap();
    let mut exit = engine.prepare_terminal_disk_exit().unwrap();
    let management = exit.complete(&mut engine).unwrap();
    drop(exit);
    drop(engine);
    assert!(!management.original_configuration().disks[0].metadata_member);
    assert_eq!(
        management
            .readmission_metadata_member(&retained.disk_id)
            .unwrap(),
        Some(true)
    );
    assert_eq!(
        management
            .readmission_metadata_member(&final_disk.disk_id)
            .unwrap(),
        None
    );
    let moved = temp.path().join("detached-unavailable");
    fs::rename(&final_disk.root, &moved).unwrap();
    let before = files(&moved);
    let resume = OperationId::generate().unwrap();
    let engine = management
        .prepare_disk_readmission(
            &retained.disk_id,
            "admin",
            resume,
            retained_directory(retained),
        )
        .unwrap()
        .activate()
        .unwrap();
    assert_eq!(engine.config.metadata_members().len(), 1);
    assert_eq!(
        engine.config.metadata_members().next().unwrap().disk_id,
        retained.disk_id
    );
    assert!(engine.registered_disk(&final_disk.disk_id).is_none());
    assert_eq!(
        engine.state.operations[&first_stop].kind,
        "disk-planned-offline"
    );
    assert_eq!(
        engine.state.operations[&terminal_operation].kind,
        "disk-deregister"
    );
    assert_eq!(
        files(&moved),
        before,
        "unselected Detached root must remain unopened"
    );
    drop(engine);
    assert!(Engine::open(&system).is_ok());
}

#[test]
fn explicit_retained_activation_and_cold_completion_keep_host_workspace_limits() {
    for cold in [false, true] {
        let mut fixture = Fixture::new(MembershipExitAction::PlannedOffline, 0);
        let config = &fixture.original;
        let limits = crate::RetainedWorkspaceLimits {
            max_retained_workspaces: config.max_retained_workspaces + 1,
            max_retained_workspaces_per_principal: config.max_retained_workspaces_per_principal + 1,
            max_retained_workspace_metadata_bytes: config.max_retained_workspace_metadata_bytes
                + 1024,
            max_retained_workspace_metadata_bytes_per_principal: config
                .max_retained_workspace_metadata_bytes_per_principal
                + 1024,
        };
        let mut pending = prepare_retained(&mut fixture);
        let engine = if cold {
            pending.synchronize().unwrap();
            select_generation(&pending);
            drop(pending);
            TerminalManagement::complete_metadata_resume_with_workspace_limits(
                &fixture.system(),
                Some(limits),
            )
            .unwrap()
        } else {
            pending
                .activate_with_workspace_limits(Some(limits))
                .unwrap()
        };
        assert_eq!(engine.effective_workspace_limits(), limits);
    }
}

#[test]
fn original_root_cannot_replace_its_independent_new_decision_with_old_archive_evidence() {
    let mut fixture = Fixture::new(MembershipExitAction::PlannedOffline, 0);
    let mut pending = prepare_retained(&mut fixture);
    pending.synchronize().unwrap();
    let transition = object_hash(INTENT_KIND, &pending.proposal.intent).unwrap();
    let root = member_directory(&fixture.original.disks[0], &transition)
        .join(CONTROL_ARCHIVE_DIRECTORY)
        .join("archive-root.json");
    select_generation(&pending);
    drop(pending);
    fs::remove_file(root).unwrap();
    assert!(TerminalManagement::complete_metadata_resume(&fixture.system()).is_err());
    assert!(!fixture.source_path().join("resume-complete.json").exists());
    assert!(Engine::open(&fixture.system()).is_err());
}

#[test]
fn retained_resume_requires_its_actual_new_decision_despite_intact_portable_certificates() {
    let mut fixture = Fixture::new(MembershipExitAction::PlannedOffline, 0);
    let disk = fixture.original.disks[0].clone();
    let mut pending = prepare_retained(&mut fixture);
    pending.synchronize().unwrap();
    let transition = object_hash(INTENT_KIND, &pending.proposal.intent).unwrap();
    let decision = disk
        .root
        .join("metadata/membership-decisions")
        .join(&transition)
        .join("decision.json");
    let original_decision = fs::read(&decision).unwrap();
    let mut another_choice: json_codec::Value = decode_strict(&original_decision).unwrap();
    assert_eq!(another_choice["choice"]["kind"], "TerminalResume");
    // The same identity and transition still form a structurally valid record.
    // Its other choice cannot substitute for the independent new resume vote.
    another_choice["choice"]["kind"] = json_codec::json!("TerminalResumeCancel");
    let another_choice = crate::canonical_json(&another_choice).unwrap();
    let member = member_directory(&disk, &transition);
    let certificate = member.join("resume-certificate.json");
    let certificate_bytes = fs::read(&certificate).unwrap();
    let archive = member.join(CONTROL_ARCHIVE_DIRECTORY);
    let archive_bytes = files(&archive);
    assert!(!archive_bytes.is_empty());
    select_generation(&pending);
    drop(pending);
    let selected = fixture.source_path().join("resume-generation.json");
    let selected_bytes = fs::read(&selected).unwrap();
    let completed = fixture.source_path().join("resume-complete.json");
    for fault in ["missing", "malformed", "another-choice"] {
        match fault {
            "missing" => fs::remove_file(&decision).unwrap(),
            "malformed" => fs::write(&decision, b"{").unwrap(),
            "another-choice" => fs::write(&decision, &another_choice).unwrap(),
            _ => unreachable!(),
        }
        let before_system = files(&fixture.system());
        let before_root = files(&disk.root);
        let observed = TerminalManagement::metadata_resume_recovery(&fixture.system())
            .unwrap()
            .unwrap();
        assert_eq!(observed.operation_id, fixture.resume);
        assert_eq!(
            observed.phase,
            crate::TerminalMetadataResumePhase::CompleteGeneration
        );
        let error = TerminalManagement::complete_metadata_resume(&fixture.system())
            .err()
            .expect("portable certificates cannot replace the target's actual new decision");
        match fault {
            "missing" => assert_eq!(error.code(), ErrorCode::RecoveryRequired),
            "malformed" => {
                assert_eq!(error.code(), ErrorCode::InvalidJson);
                let mut source: Option<&(dyn std::error::Error + 'static)> = Some(&error);
                let mut json_cause = false;
                while let Some(cause) = source {
                    json_cause |= cause.downcast_ref::<json_codec::Error>().is_some();
                    source = cause.source();
                }
                assert!(
                    json_cause,
                    "the actual decision parse cause must remain available"
                );
            }
            "another-choice" => assert_eq!(error.code(), ErrorCode::LogFork),
            _ => unreachable!(),
        }
        assert!(!completed.exists());
        assert!(Engine::open(&fixture.system()).is_err());
        assert_eq!(fs::read(&selected).unwrap(), selected_bytes);
        assert_eq!(fs::read(&certificate).unwrap(), certificate_bytes);
        assert_eq!(files(&archive), archive_bytes);
        assert_eq!(files(&fixture.system()), before_system, "{fault}");
        assert_eq!(files(&disk.root), before_root, "{fault}");
        fs::write(&decision, &original_decision).unwrap();
    }
    let engine = TerminalManagement::complete_metadata_resume(&fixture.system()).unwrap();
    assert_eq!(
        engine.state.operations[&fixture.resume].kind,
        "disk-readmit"
    );
    assert!(completed.is_file());
    drop(engine);
    assert!(Engine::open(&fixture.system()).is_ok());
}

#[test]
fn cancelled_fresh_attempt_can_select_retained_metadata_without_rewriting_old_evidence() {
    let mut fixture = Fixture::new(MembershipExitAction::PlannedOffline, 0);
    let fresh = fixture.target(3);
    let mut pending = fixture.prepare(&fresh);
    let cancelled_operation = fixture.resume;
    let status = pending.status().unwrap();
    let cancelled = match pending
        .cancel("admin", cancelled_operation, &status.transition_hash)
        .unwrap()
    {
        crate::TerminalMetadataResumeCancelOutcome::Cancelled(result) => result,
        crate::TerminalMetadataResumeCancelOutcome::ContinueOriginal(_) => {
            panic!("unpublished fresh attempt must be cancellable")
        }
    };
    fixture.management = Some(pending.into_cancelled_management().unwrap());
    let cancelled_evidence = files(&fixture.source_path());
    let fresh_evidence: Vec<_> = fresh
        .disks
        .iter()
        .filter(|disk| disk.metadata_member)
        .map(|disk| (disk.root.clone(), files(&disk.root)))
        .collect();
    fixture.resume = OperationId::generate().unwrap();
    let mut retained = prepare_retained(&mut fixture);
    assert_eq!(
        retained.cancellation_phase().unwrap(),
        crate::TerminalMetadataResumeCancellationPhase::RetainedReadmission
    );
    let status = retained.status().unwrap();
    assert_eq!(
        retained
            .cancel("admin", fixture.resume, &status.transition_hash)
            .unwrap_err()
            .code(),
        ErrorCode::ConfigConflict
    );
    let engine = retained.activate().unwrap();
    assert_eq!(engine.state.operations[&cancelled_operation], cancelled);
    assert_eq!(
        engine.state.operations[&fixture.resume].kind,
        "disk-readmit"
    );
    for (path, bytes) in cancelled_evidence {
        assert_eq!(fs::read(fixture.source_path().join(path)).unwrap(), bytes);
    }
    for (root, bytes) in fresh_evidence {
        assert_eq!(files(&root), bytes, "cancelled fresh roots stay unopened");
    }
    drop(engine);
    let reopened = Engine::open(&fixture.system()).unwrap();
    assert_eq!(reopened.state.operations[&cancelled_operation], cancelled);
    assert_eq!(
        reopened.state.operations[&fixture.resume].kind,
        "disk-readmit"
    );
}

#[test]
fn system_loss_relocates_terminal_readmission_root_and_preserves_epoch_chain() {
    let mut fixture = Fixture::new(MembershipExitAction::PlannedOffline, 0);
    let mut disk = fixture.original.disks[0].clone();
    let engine = prepare_retained(&mut fixture).activate().unwrap();
    let admitted = engine.state.operations[&fixture.resume].clone();
    drop(engine);
    // Neither the old SYSTEM path nor the old root address can serve as an
    // implicit fallback. Only this relocated original physical directory is supplied.
    let lost_system = fixture.temporary.path().join("lost-system");
    fs::rename(fixture.system(), &lost_system).unwrap();
    fs::write(fixture.system(), b"original SYSTEM is unavailable").unwrap();
    let lost_system_bytes = files(&lost_system);
    let old_root = disk.root.clone();
    let moved = fixture.temporary.path().join("relocated-original-metadata");
    fs::rename(&old_root, &moved).unwrap();
    fs::write(&old_root, b"old root path must not be probed").unwrap();
    disk.root = moved;
    let original_seal = fs::read(disk.root.join("disk.json")).unwrap();
    let restored = fixture.temporary.path().join("recovered-system");
    let mut engine =
        Engine::restore_from_disks(&restored, std::slice::from_ref(&disk.root)).unwrap();
    assert_eq!(engine.state.operations[&fixture.resume], admitted);
    assert_eq!(engine.config.disks[0].root, disk.root);
    assert_eq!(
        fs::read(disk.root.join("disk.json")).unwrap(),
        original_seal
    );
    assert_eq!(
        engine.config.io_admission_epoch(&disk.disk_id).unwrap(),
        Some(admitted.request_digest.clone())
    );
    let seed_path = restored.join("bootstrap-discovery/record-00000000000000000001.json");
    let seed = fs::read(&seed_path).unwrap();
    let record: json_codec::Value = decode_strict(&seed).unwrap();
    assert_eq!(
        record["recovered_readmissions"][&disk.disk_id]["intent_digest"],
        admitted.request_digest
    );
    drop(engine);
    let probe = restored
        .join("readmission-probe-admissions")
        .join(format!("{}.json", admitted.request_digest));
    let certificate = fs::read(&probe).unwrap();
    fs::remove_file(&probe).unwrap();
    assert!(
        Engine::open(&restored).is_err(),
        "a seed alone is not a probe permit"
    );
    fs::write(&probe, certificate).unwrap();
    engine = Engine::open(&restored).unwrap();
    assert_eq!(engine.config.disks[0].root, disk.root);
    assert_eq!(
        engine.config.io_admission_epoch(&disk.disk_id).unwrap(),
        Some(admitted.request_digest.clone())
    );
    let stop = OperationId::generate().unwrap();
    engine
        .begin_disk_exit(
            &disk.disk_id,
            MembershipExitAction::PlannedOffline,
            "admin",
            stop,
        )
        .unwrap();
    let mut exit = engine.prepare_terminal_disk_exit().unwrap();
    let management = exit.complete(&mut engine).unwrap();
    let stopped = management.result().clone();
    drop(exit);
    drop(engine);
    let next_operation = OperationId::generate().unwrap();
    let engine = management
        .prepare_disk_readmission(
            &disk.disk_id,
            "admin",
            next_operation,
            retained_directory(&disk),
        )
        .unwrap()
        .activate()
        .unwrap();
    let next = engine.state.operations[&next_operation].clone();
    assert_ne!(next.request_digest, admitted.request_digest);
    assert_eq!(
        engine.config.io_admission_epoch(&disk.disk_id).unwrap(),
        Some(next.request_digest.clone())
    );
    assert_eq!(engine.state.operations[&fixture.resume], admitted);
    assert_eq!(engine.state.operations[&stop], stopped);
    assert_eq!(fs::read(&seed_path).unwrap(), seed);
    let identity = crate::DiskRegistrationIdentity {
        pool_id: engine.config.pool_id.clone(),
        disk_id: disk.disk_id.clone(),
        slot_id: disk.slot_id.clone(),
        binding_generation: disk.binding_generation,
    };
    let authority = &engine.config.generation().unwrap().portable;
    authority
        .verify_bootstrap_stop_epoch(
            &identity,
            stop,
            &stopped.request_digest,
            Some(&admitted.request_digest),
        )
        .unwrap();
    assert!(
        authority
            .verify_bootstrap_stop_epoch(&identity, stop, &stopped.request_digest, None)
            .is_err()
    );
    assert!(
        authority
            .verify_bootstrap_stop_epoch(
                &identity,
                stop,
                &stopped.request_digest,
                Some(&next.request_digest)
            )
            .is_err()
    );
    drop(engine);
    let reopened = Engine::open(&restored).unwrap();
    assert_eq!(reopened.state.operations[&stop], stopped);
    assert_eq!(reopened.state.operations[&fixture.resume], admitted);
    assert_eq!(reopened.state.operations[&next_operation], next);
    assert_eq!(reopened.config.disks[0].root, disk.root);
    assert_eq!(
        reopened.config.io_admission_epoch(&disk.disk_id).unwrap(),
        Some(next.request_digest.clone())
    );
    assert_eq!(fs::read(&seed_path).unwrap(), seed);
    assert_eq!(files(&lost_system), lost_system_bytes);
    assert_eq!(
        fs::read(fixture.system()).unwrap(),
        b"original SYSTEM is unavailable"
    );
    assert_eq!(
        fs::read(&old_root).unwrap(),
        b"old root path must not be probed"
    );
}

#[test]
fn checked_retained_resume_binds_every_projection_before_locator_and_target_io() {
    for selected in [false, true] {
        let mut fixture = Fixture::new(MembershipExitAction::PlannedOffline, 1);
        let disk = fixture.original.disks[0].clone();
        let mut pending = prepare_retained(&mut fixture);
        if selected {
            pending.synchronize().unwrap();
            select_generation(&pending);
        }
        drop(pending);
        let source = TerminalManagement::metadata_resume_recovery(&fixture.system())
            .unwrap()
            .unwrap();
        for changed in 0..13 {
            let mut expected = source.clone();
            match changed {
                0 => expected.principal = "another actor".into(),
                1 => expected.operation_id = OperationId::generate().unwrap(),
                2 => expected.terminal_operation_id = OperationId::generate().unwrap(),
                3 => expected.terminal_authority_hash = "0".repeat(64),
                4 => expected.transition_hash = "0".repeat(64),
                5 => expected.admission = TerminalMetadataResumeAdmission::FreshDirectories,
                6 => expected.readmission_preview = None,
                7 => {
                    expected
                        .readmission_preview
                        .as_mut()
                        .unwrap()
                        .source_sequence += 1
                }
                8 => expected.original.disks[0].binding_generation += 1,
                9 => expected.target.disks[0].root = fixture.temporary.path().join("other-root"),
                10 => expected.control_archive.root_hash = "0".repeat(64),
                11 => {
                    expected.control_archive.archive_path =
                        fixture.temporary.path().join("other-archive")
                }
                12 => {
                    expected.phase = if selected {
                        crate::TerminalMetadataResumePhase::ResupplyDirectories
                    } else {
                        crate::TerminalMetadataResumePhase::CompleteGeneration
                    };
                }
                _ => unreachable!(),
            }
            let before = files(&fixture.system());
            let watch = watch_resume_recovery_paths(
                fixture
                    .original
                    .disks
                    .iter()
                    .map(|disk| disk.root.clone())
                    .chain([fixture.system().join("bootstrap-discovery")]),
            );
            let error =
                TerminalManagement::open_metadata_resume_checked(&fixture.system(), &expected)
                    .err()
                    .expect("a changed recovery projection must fail while only SYSTEM is open");
            assert_eq!(
                error.code(),
                ErrorCode::IdempotencyKeyReused,
                "{selected} {changed}"
            );
            assert_no_resume_recovery_path_open(&watch);
            drop(watch);
            assert_eq!(files(&fixture.system()), before, "{selected} {changed}");
        }
        let owner =
            TerminalManagement::open_metadata_resume_checked(&fixture.system(), &source).unwrap();
        let competing =
            TerminalManagement::open_metadata_resume_checked(&fixture.system(), &source)
                .err()
                .expect("the checked owner retains the real SYSTEM writer");
        assert_eq!(competing.code(), ErrorCode::WriterAlreadyActive);
        let engine = if selected {
            owner.complete_metadata_resume_held(None).unwrap()
        } else {
            owner
                .reopen_metadata_resume_held(vec![retained_directory(&disk)])
                .unwrap()
                .activate()
                .unwrap()
        };
        let result = &engine.state.operations[&fixture.resume];
        assert_eq!(result.kind, "disk-readmit");
        assert_eq!(result.operation_id, fixture.resume);
        assert_eq!(engine.state.operation_principals[&fixture.resume], "admin");
    }
}
