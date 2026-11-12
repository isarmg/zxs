//! Real native first-publication component costs, not a serving-success certificate.
//! Timings are observations only. The original 900-second full transaction stays separate.
use super::*;
use crate::engine::backup_preparation::native_control::databases::scoped::cutover::registration;
use crate::{
    BackupPreparationPhase, BackupPreparationRequest, BackupTopologyTarget,
    BackupTransactionRequest, CommitRequest, Config, Entry, NamespaceOperation,
    NamespaceOperationKind, OperationId, Repository, RepositoryName,
    container_storage::backup_databases::PreparedDatabases,
};
use std::{fs, io::Write, os::unix::fs::DirBuilderExt, path::Path, time::Instant};

fn operation() -> OperationId {
    OperationId::generate().unwrap()
}

fn save(engine: &mut Engine, repository: &Repository, body: &[u8]) -> Entry {
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "nested/file", body)
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: operation(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "preparation fixture".into(),
            request_digest: None,
        })
        .unwrap();
    engine
        .namespace_tree(&repository.main_branch_id)
        .unwrap()
        .get("nested/file")
        .unwrap()
        .clone()
}

fn target(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
    path
}

fn fixture() -> (tempfile::TempDir, Engine, BackupTransactionRequest) {
    let temp = tempfile::Builder::new()
        .permissions(std::os::unix::fs::PermissionsExt::from_mode(0o700))
        .tempdir()
        .unwrap();
    let config =
        Config::development((0..3).map(|index| temp.path().join(format!("disk-{index}"))))
            .unwrap();
    let mut engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
    engine
        .create_disk_root("P01", "admin", operation())
        .unwrap();
    let repository = engine
        .create_repository(
            &RepositoryName::parse("WholeRoute").unwrap(),
            "P01",
            3,
            "admin",
            operation(),
        )
        .unwrap();
    let old = save(&mut engine, &repository, b"retained first body");
    let latest = save(&mut engine, &repository, b"retained current body");
    assert_eq!(old.file_id, latest.file_id);
    assert_ne!(old.version_id, latest.version_id);
    engine
        .file_namespace(
            &repository.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::Mkdir, "nested/empty", None)
                .unwrap(),
            "admin",
            operation(),
        )
        .unwrap();
    assert_eq!(engine.state().versions.len(), 2);
    assert!(engine.state().directory_git_repositories.is_empty());
    let paths = vec![
        target(temp.path(), "selected-one"),
        target(temp.path(), "selected-two"),
    ];
    let request = BackupTransactionRequest {
        preparation: BackupPreparationRequest {
            operation_id: operation(),
            principal: "admin".into(),
            source_slot_id: "B01".into(),
            target_paths: paths.clone(),
            expected_sequence: engine.state().sequence,
        },
        targets: paths
            .iter()
            .enumerate()
            .map(|(index, _)| BackupTopologyTarget {
                disk_id: crate::new_id("disk").unwrap(),
                slot_id: format!("C{:02}", index + 2),
                upstream_slot_id: "B01".into(),
            })
            .collect(),
    };
    (temp, engine, request)
}

fn prepare_entities(engine: &mut Engine, request: &BackupTransactionRequest) {
    let operation = request.preparation.operation_id;
    engine
        .start_backup_transaction(
            request.clone(),
            request
                .preparation
                .target_paths
                .iter()
                .map(|path| fs::File::open(path).unwrap())
                .collect(),
        )
        .unwrap();
    for phase in [
        BackupPreparationPhase::HistoryPrepared,
        BackupPreparationPhase::NativeControlPrepared,
        BackupPreparationPhase::ObjectDatabasesPrepared,
        BackupPreparationPhase::ScopedDatabasesPrepared,
        BackupPreparationPhase::CutoverInputsPrepared,
    ] {
        assert_eq!(engine.wait_backup_preparation().unwrap().phase, phase);
        assert!(engine.membership_fence_status().is_none());
        assert!(!engine.state().operations.contains_key(&operation));
        if phase != BackupPreparationPhase::CutoverInputsPrepared {
            engine
                .continue_backup_transaction(operation, "admin")
                .unwrap();
        }
    }
    assert_eq!(
        engine.continue_backup_transaction(operation, "admin").unwrap().phase,
        BackupPreparationPhase::CutoverInputsPrepared,
    );
    engine
        .begin_backup_cutover_fence(operation, "admin")
        .unwrap();
    engine
        .continue_backup_transaction(operation, "admin")
        .unwrap();
    assert_eq!(
        engine.wait_backup_preparation().unwrap().phase,
        BackupPreparationPhase::TargetEntitiesPrepared,
    );
    assert_eq!(
        engine.continue_backup_transaction(operation, "admin").unwrap().phase,
        BackupPreparationPhase::TargetEntitiesPrepared,
    );
    for disk in &engine.config().disks {
        assert!(!disk.root.join("metadata/membership-generations").exists());
    }
}

fn measured<T>(
    stage: &'static str,
    iteration: usize,
    target: usize,
    action: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let start = Instant::now();
    let result = action();
    let _ = writeln!(
        std::io::stderr(),
        "[backup-component-us inclusive] stage={stage} iteration={iteration} target={target} \
         elapsed_us={} ok={} targets=2 files=1 versions=2 body_bytes=40 directory_scopes=0",
        start.elapsed().as_micros(),
        result.is_ok(),
    );
    result
}

fn measure_upstream(
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &crate::engine::membership_transition::PreparedBackupFirstCandidate,
    iteration: usize,
    stop: &AtomicBool,
) -> Result<()> {
    first_candidate::cost_components::observe(prepared, decision, expected, iteration, stop)?;
    let (report, observed) = measured("first_candidate_read_verified", iteration, 0, || {
        first_candidate::read_verified(prepared, decision, stop)
    })?;
    assert_eq!(report.candidate_hash, expected.hash());
    assert_eq!(report.snapshot_hash, expected.snapshot_hash());
    assert_eq!(report.intended_first_sequence, expected.sequence());
    assert_eq!(report.metadata_bytes_per_target, expected.bytes() as u64);
    assert_eq!(observed.files().collect::<Vec<_>>(), expected.files().collect::<Vec<_>>());
    let inputs = measured("generation_inputs_observe", iteration, 0, || {
        generation_inputs::observe(prepared, decision, stop)
    })?;
    assert_eq!(inputs.operation_id, decision.operation());
    assert_eq!(inputs.decision_hash, *expected.decision_hash());
    assert_eq!(inputs.intended_first_sequence, expected.sequence());
    assert_eq!(inputs.targets, prepared.targets.len());
    for index in 0..prepared.targets.len() {
        let (control, control_report) =
            measured("generation_control_read_authority", iteration, index, || {
                generation_control::read_authority_input(
                    prepared, decision, &inputs, index, stop,
                )
            })?;
        assert_eq!(control.root_hash(), control_report.native_control_root);
        assert_eq!(control_report.operation_id, decision.operation());
        assert_eq!(control_report.decision_hash, *expected.decision_hash());
        assert_eq!(control_report.targets, prepared.targets.len());
        let source = measured("source_snapshot_read", iteration, index, || {
            snapshot::read(
                &prepared.targets[index].root,
                &prepared.task.selection().jobs()[index],
                stop,
            )
        })?;
        assert_eq!(
            object_hash("backup-prepared-state", source.state())?,
            prepared.source_evidence().state_hash(),
        );
        let rebuilt_authority = measured("authority_capture", iteration, index, || {
            crate::engine::membership_transition::PreparedBackupAuthority::capture(
                decision,
                &control,
                &inputs,
                &control_report,
                source.state(),
            )
        })?;
        assert_eq!(rebuilt_authority.hash(), expected.authority_hash());
        let (authority, admitted) =
            measured("generation_authority_read_candidate", iteration, index, || {
                generation_authority::read_candidate_input(
                    prepared, decision, &inputs, index, stop,
                )
            })?;
        assert_eq!(authority.hash(), expected.authority_hash());
        assert_eq!(authority.bytes(), rebuilt_authority.bytes());
        assert_eq!(
            canonical_json(admitted.state())?,
            canonical_json(source.state())?,
        );
        let candidate = measured("authority_first_candidate", iteration, index, || {
            authority.first_candidate(admitted.state(), stop)
        })?;
        assert_eq!(candidate.hash(), expected.hash());
        assert_eq!(candidate.snapshot_hash(), expected.snapshot_hash());
        assert_eq!(candidate.sequence(), expected.sequence());
        assert_eq!(candidate.bytes(), expected.bytes());
        assert_eq!(
            candidate.files().collect::<Vec<_>>(),
            expected.files().collect::<Vec<_>>(),
        );
        preparation_checkpoint(stop)?;
        named_target(&prepared.targets[index].root, &prepared.task.selection().jobs()[index])?;
    }
    decision.verify_targets(prepared, stop)?;
    recovery::require_isolated(&prepared.targets)?;
    prepared.recheck_names()
}

#[test]
fn real_native_serving_component_costs_keep_all_admission_and_readback() -> Result<()> {
    let (_temp, mut engine, request) = fixture();
    prepare_entities(&mut engine, &request);
    let operation = request.preparation.operation_id;
    // Match start_backup_registry_placement_decision before its worker spawn.
    Engine::require_actor("admin")?;
    engine.require_no_commit_preparation()?;
    let task = registration::Task {
        format: registration::FORMAT.into(),
        installation: InstallationTask::current(&engine, operation, "admin")?,
    };
    assert!(
        !engine
            .backup_preparation
            .as_ref()
            .and_then(|worker| worker.thread.as_ref())
            .is_some_and(|thread| !thread.is_finished())
    );
    if let Some(worker) = engine.backup_preparation.as_mut() {
        worker.join()?;
    }
    let system = DiskRoot::open(&engine.root)?;
    assert_eq!(task.installation.bytes()?, InstallationTask::read(&system)?.bytes()?);
    assert!(!preparation_path_exists(&system, &system.path().join(registration::TASK))?);
    let (targets, choices) = registration::runtime::acquire(&engine, &task)?;
    install_preparation_bytes(
        &system,
        &system.path().join(registration::TASK),
        &task.bytes()?,
    )?;
    // Match spawn ordering. All original ownership, leases and quiescence tokens
    // live through the final observation; no fabricated source capability exists.
    let _locks = engine.retain_writer_locks()?;
    let sources = common_databases::Sources::capture(&engine, &system)?;
    let index = engine.quiesce_backup_system_index()?;
    let stop = AtomicBool::new(false);
    let mut prepared = PreparedBackupRegistry::capture(task, targets, &stop)?;
    prepared.scope_encoding = Some(published_scopes::encoding_memo::ScopeEncodingMemo::new());
    assert_eq!(prepared.targets.len(), 2);
    assert_eq!(prepared.task.selection().first().total_files, 2);
    assert_eq!(prepared.task.selection().first().total_bytes, 40);
    // Same recover/new-choice branch and all native observe gates as runtime::run.
    let _hash = match choices.recover_decision_if_present(&prepared, &stop)? {
        Some(hash) => hash,
        None => {
            require_capacity_before_choice(&prepared, &stop)?;
            sources.require_capacity_before_choice(&system)?;
            choices.decide(&prepared, &stop)?
        }
    };
    let decision = choices.observe_decision(&prepared, &stop)?;
    writer_routes::prepare(&prepared, &decision, &stop)?;
    choices.observe_decision(&prepared, &stop)?;
    generation_inputs::prepare(&prepared, &decision, &stop)?;
    choices.observe_decision(&prepared, &stop)?;
    generation_control::prepare(&prepared, &decision, &choices, &stop)?;
    generation_authority::prepare(&prepared, &decision, &stop)?;
    choices.observe_decision(&prepared, &stop)?;
    first_candidate::prepare(&prepared, &decision, &stop)?;
    choices.observe_decision(&prepared, &stop)?;
    let (_, candidate) = first_candidate::read_verified(&prepared, &decision, &stop)?;
    choices.finalize_first_candidate(&prepared, &candidate, &stop)?;
    let publication = choices.publish_first_candidate(&prepared, &candidate, &stop)?;
    let retained = first_publication::prepare(
        &prepared,
        &decision,
        &candidate,
        publication,
        &stop,
    )?;
    choices.observe_first_publication(&prepared, &candidate, &stop)?;
    let expected = BackupFirstPublicationHash::parse(&retained.publication.publication_hash)?;
    let configured = configuration::inspect(&prepared, &decision, &expected, &stop)?;
    choices.observe_first_publication(&prepared, &candidate, &stop)?;
    assert_eq!(configured.complete_data_route, ["P01", "B01", "C02", "C03"]);
    assert_eq!(configured.retained_replaced_terminal.slot_id, "C01");
    let registry = published_registry::install(&prepared, &decision, &expected, &stop)?;
    choices.observe_first_publication(&prepared, &candidate, &stop)?;
    assert_eq!(registry.branch_scopes, 2);
    assert_eq!(registry.directory_scopes, 0);
    // Real install, never a forged manifest or manually synthesized target tree.
    let installed = install(&prepared, &decision, &expected, &stop)?;
    choices.observe_first_publication(&prepared, &candidate, &stop)?;
    assert_eq!(installed.targets.len(), 2);
    let mut previous_completion = None;
    let mut previous_databases = BTreeMap::new();
    for iteration in 0..2 {
        measure_upstream(&prepared, &decision, &candidate, iteration, &stop)?;
        let publication = measured("first_publication_observe", iteration, 0, || {
            first_publication::observe(&prepared, &decision, &expected, &stop)
        })?;
        assert_eq!(publication.targets, retained.targets);
        assert_eq!(
            publication.publication.publication_hash,
            retained.publication.publication_hash,
        );
        assert_eq!(
            publication.publication.snapshot_hash,
            retained.publication.snapshot_hash,
        );
        assert_eq!(
            publication.publication.first_sequence,
            retained.publication.first_sequence,
        );
        let configuration = measured("configuration_inspect", iteration, 0, || {
            configuration::inspect(&prepared, &decision, &expected, &stop)
        })?;
        assert_eq!(configuration.config_hash, configured.config_hash);
        assert_eq!(configuration.complete_data_route, configured.complete_data_route);
        assert_eq!(
            canonical_json(&configuration.registry)?,
            canonical_json(&configured.registry)?,
        );
        let registry_plan = measured("published_registry_capture", iteration, 0, || {
            published_registry::Plan::capture(&prepared, &decision, &expected, &stop)
        })?;
        assert_eq!(
            registry_plan.configuration().config_hash,
            installed.registry.configuration.config_hash,
        );
        assert_eq!(
            registry_plan.configuration().publication.publication.publication_hash,
            retained.publication.publication_hash,
        );
        assert_eq!(
            canonical_json(&registry_plan.configuration().registry)?,
            canonical_json(&installed.registry.configuration.registry)?,
        );
        let registry_observed =
            measured("published_registry_observe", iteration, 0, || {
                published_registry::observe_with_plan(
                    &prepared,
                    &decision,
                    &registry_plan,
                    &stop,
                )
            })?;
        assert_eq!(registry_observed.branch_scopes, installed.registry.branch_scopes);
        assert_eq!(registry_observed.directory_scopes, installed.registry.directory_scopes);
        assert_eq!(
            registry_observed.metadata_bytes_per_target,
            installed.registry.metadata_bytes_per_target,
        );
        assert_eq!(
            registry_observed.configuration.publication.targets,
            installed.targets.len(),
        );
        assert_eq!(
            registry_observed.configuration.config_hash,
            installed.registry.configuration.config_hash,
        );
        assert_eq!(
            canonical_json(&registry_observed.configuration.registry)?,
            canonical_json(&installed.registry.configuration.registry)?,
        );
        let plan = measured("plan_capture", iteration, 0, || {
            Plan::capture(&prepared, &decision, &expected, &stop)
        })?;
        assert_eq!(plan.report.targets, installed.targets);
        if let Some(previous) = &previous_completion {
            assert_eq!(previous, &plan.completion);
        }
        previous_completion = Some(plan.completion.clone());
        for target_index in 0..prepared.targets.len() {
            let target = &prepared.targets[target_index];
            let node = &prepared.projection.targets[target_index].node;
            let job = &prepared.task.selection().jobs()[target_index];
            preparation_checkpoint(&stop)?;
            named_target(&target.root, job)?;
            let graph = measured("registry_verify", iteration, target_index, || {
                plan.registry.verify(target, &stop)
            })?;
            assert_eq!(
                graph.registration(&node.identity)?,
                Some(crate::RegistrationMode::Enabled),
            );
            let admitted = measured("state_at", iteration, target_index, || {
                plan.registry.state_at(target, &stop)
            })?;
            assert_eq!(admitted.state().versions.len(), 2);
            let databases = measured("databases_capture", iteration, target_index, || {
                PreparedDatabases::capture(&admitted, &stop)
            })?;
            assert_eq!(databases.file_count(), 1);
            assert_eq!(databases.directory_count(), 0);
            let bytes: BTreeMap<_, _> = databases
                .databases()
                .iter()
                .map(|database| (database.path(), database.bytes().to_vec()))
                .collect();
            if let Some(previous) = previous_databases.insert(target_index, bytes.clone()) {
                assert_eq!(previous, bytes);
            }
            measured("databases_verify", iteration, target_index, || {
                for database in databases.databases() {
                    preparation_checkpoint(&stop)?;
                    database.verify(database.bytes())?;
                }
                Ok(())
            })?;
            let publication = &plan.registry.configuration().publication.publication;
            let image = measured("image_capture", iteration, target_index, || {
                Image::capture_published(
                    &target.root,
                    &admitted,
                    &node.source_prefix,
                    PublishedImageBinding {
                        source_fence_hash: prepared.source_fence_hash(),
                        decision_hash: publication.decision_hash.as_str(),
                        publication_hash: &publication.publication_hash,
                        snapshot_hash: &publication.snapshot_hash,
                        registry: graph.commitment(),
                        target: &node.identity,
                    },
                    &stop,
                )
            })?;
            assert_eq!(image.file_units, 1);
            assert_eq!(image.directory_repositories, 0);
            assert_eq!(image.bodies, job.total_files);
            assert_eq!(image.body_bytes, job.total_bytes);
            assert_eq!(
                plan.target_report(&prepared, target_index, &image),
                installed.targets[target_index],
            );
            measured("image_verify", iteration, target_index, || {
                image.verify(&target.root, &stop)
            })?;
            named_target(&target.root, job)?;
            preparation_checkpoint(&stop)?;
            let observed = measured("with_image_verify", iteration, target_index, || {
                plan.with_image(&prepared, target_index, &stop, |image, state| {
                    assert_eq!(state.sequence, admitted.state().sequence);
                    assert_eq!(
                        image.manifest_hash(),
                        installed.targets[target_index].image_manifest_hash,
                    );
                    image.verify(&target.root, &stop)?;
                    Ok(plan.target_report(&prepared, target_index, image))
                })
            })?;
            assert_eq!(observed, installed.targets[target_index]);
            plan.require_completion(target)?;
        }
        let observed = plan.observe(&prepared, &decision, &expected, &stop)?;
        assert_eq!(observed.targets, installed.targets);
        choices.observe_first_publication(&prepared, &candidate, &stop)?;
        sources.recheck(&system)?;
        index.require_root(&system)?;
        prepared.recheck_names()?;
    }
    assert!(engine.is_read_only());
    assert!(engine.membership_fence_status().is_some());
    assert!(!engine.state().operations.contains_key(&operation));
    Ok(())
}
