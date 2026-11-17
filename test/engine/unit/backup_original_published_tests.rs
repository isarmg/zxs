//! Real original FILEs after the original members' first Backup Publication.
//! No fabricated State/capability and no claim that directory Git is supported.
use super::*;
use crate::container_storage::backup_databases::PreparedDatabases;
use crate::engine::backup_preparation::native_control::databases::scoped::cutover::registration;
use crate::engine::backup_preparation::PreparedBackupRegistry;
use crate::{
    BackupPreparationPhase, BackupPreparationRequest, BackupTopologyTarget,
    BackupTransactionRequest, CommitRequest, Config, NamespaceOperation,
    NamespaceOperationKind, Repository,
};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

fn operation() -> OperationId {
    OperationId::generate().unwrap()
}

fn save(engine: &mut Engine, repository: &Repository) -> Result<()> {
    let workspace = engine.create_workspace(&repository.main_branch_id, "admin")?;
    for (path, bytes) in [
        ("nested/one.txt", b"first actual body".as_slice()),
        ("nested/two.txt", b"second actual body".as_slice()),
    ] {
        engine.stage_bytes(&workspace.workspace_id, path, bytes)?;
    }
    engine.commit_workspace(CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: operation(),
        principal: "admin".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "registered original FILEs".into(),
        request_digest: None,
    })?;
    Ok(())
}

fn prepared_entities(engine: &mut Engine, request: &BackupTransactionRequest) -> Result<()> {
    let operation = request.preparation.operation_id;
    engine.start_backup_transaction(
        request.clone(),
        request.preparation.target_paths.iter()
            .map(fs::File::open).collect::<std::io::Result<Vec<_>>>()?,
    )?;
    for phase in [
        BackupPreparationPhase::HistoryPrepared,
        BackupPreparationPhase::NativeControlPrepared,
        BackupPreparationPhase::ObjectDatabasesPrepared,
        BackupPreparationPhase::ScopedDatabasesPrepared,
        BackupPreparationPhase::CutoverInputsPrepared,
    ] {
        assert_eq!(engine.wait_backup_preparation()?.phase, phase);
        assert!(engine.membership_fence_status().is_none());
        assert!(!engine.state().operations.contains_key(&operation));
        if phase != BackupPreparationPhase::CutoverInputsPrepared {
            engine.continue_backup_transaction(operation, "admin")?;
        }
    }
    engine.continue_backup_transaction(operation, "admin")?;
    engine.begin_backup_cutover_fence(operation, "admin")?;
    engine.continue_backup_transaction(operation, "admin")?;
    assert_eq!(
        engine.wait_backup_preparation()?.phase,
        BackupPreparationPhase::TargetEntitiesPrepared,
    );
    assert_eq!(
        engine.continue_backup_transaction(operation, "admin")?.phase,
        BackupPreparationPhase::TargetEntitiesPrepared,
    );
    Ok(())
}

fn identity(path: &Path) -> (u64, u64) {
    let metadata = fs::metadata(path).unwrap();
    (metadata.dev(), metadata.ino())
}

fn media(path: &Path) -> BTreeMap<PathBuf, ((u64, u64), Option<Vec<u8>>)> {
    let mut result = BTreeMap::new();
    for entry in fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        let metadata = fs::symlink_metadata(&path).unwrap();
        assert!(!metadata.file_type().is_symlink());
        if metadata.is_dir() {
            result.insert(path.clone(), (identity(&path), None));
            result.extend(media(&path));
        } else {
            result.insert(path.clone(), (identity(&path), Some(fs::read(path).unwrap())));
        }
    }
    result
}

fn with_original(
    action: impl FnOnce(&Path, &PublishedBackupOriginal<'_>, &BTreeMap<String, PathBuf>)
        -> Result<()>,
) -> Result<()> {
    let temporary = tempfile::tempdir().unwrap();
    let roots = ["primary", "near", "far"].map(|name| temporary.path().join(name));
    let config = Config::development(
        ["Primary", "Near", "Far"].into_iter().zip(roots.iter()).map(|(name, root)| {
            (name_policy::Name::parse(name).unwrap(), root.clone())
        }),
    )?;
    let mut engine = Engine::initialize(&temporary.path().join("system"), config)?;
    let repository = engine.create_disk_root("P01", "admin", operation())?;
    let policy = crate::new_id("policy")?;
    engine.update_protection_policy(crate::ProtectionPolicyUpdate {
        policy_id: &policy,
        repository_id: &repository.repository_id,
        path_prefix: "",
        target_copies: 3,
        expected_generation: 0,
        principal: "admin",
        operation_id: operation(),
    })?;
    engine.file_namespace(
        &repository.repository_id,
        NamespaceOperation::parse(NamespaceOperationKind::Mkdir, "nested", None)?,
        "admin",
        operation(),
    )?;
    save(&mut engine, &repository)?;
    assert_eq!(engine.state().file_containers.len(), 2);
    assert!(engine.state().directory_git_repositories.is_empty());
    let disk = &engine.config.disks[0];
    let mut bodies = BTreeMap::new();
    for version in engine.state().versions.values() {
        let scope = crate::engine::fresh_layout::containers::observe_replica(
            &engine.config, &engine.disk_resources, engine.state(),
            version.file_id.parse()?, disk,
        )?;
        bodies.insert(version.file_id.clone(), scope.body_path(version)?);
    }
    let paths = ["selected-one", "selected-two"].map(|name| {
        let path = temporary.path().join(name);
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        path
    }).to_vec();
    let request = BackupTransactionRequest {
        preparation: BackupPreparationRequest {
            operation_id: operation(),
            principal: "admin".into(),
            source_slot_id: "B01".into(),
            target_paths: paths.clone(),
            expected_sequence: engine.state().sequence,
        },
        targets: paths.iter().enumerate().map(|(index, _)| BackupTopologyTarget {
            name: name_policy::Name::parse(&format!("Selected {}", index + 1)).unwrap(),
            disk_id: crate::new_id("disk").unwrap(),
            slot_id: format!("C{:02}", index + 2),
            upstream_slot_id: "B01".into(),
        }).collect(),
    };
    prepared_entities(&mut engine, &request)?;
    let operation = request.preparation.operation_id;
    Engine::require_actor("admin")?;
    engine.require_no_commit_preparation()?;
    let task = registration::Task {
        format: registration::FORMAT.into(),
        installation: InstallationTask::current(&engine, operation, "admin")?,
    };
    if let Some(worker) = engine.backup_preparation.as_mut() {
        worker.join()?;
    }
    let system = DiskRoot::open(&engine.root)?;
    assert_eq!(task.installation.bytes()?, InstallationTask::read(&system)?.bytes()?);
    assert!(!preparation_path_exists(&system, &system.path().join(registration::TASK))?);
    let (targets, choices) = registration::runtime::acquire(&engine, &task)?;
    install_preparation_bytes(
        &system, &system.path().join(registration::TASK), &task.bytes()?,
    )?;
    // Keep the runtime's actual owners, old-source handles and quiesced SYSTEM
    // through the callback. No report or target copy substitutes for these.
    let _locks = engine.retain_writer_locks()?;
    let sources = common_databases::Sources::capture(&engine, &system)?;
    let index = engine.quiesce_backup_system_index()?;
    let stop = AtomicBool::new(false);
    let prepared = PreparedBackupRegistry::prepare(task, targets, &choices, &stop)?;
    match choices.recover_decision_if_present(&prepared, &stop)? {
        Some(_) => {}
        None => {
            registration::serving::require_capacity_before_choice(&prepared, &stop)?;
            sources.require_capacity_before_choice(&system)?;
            choices.decide(&prepared, &stop)?;
        }
    }
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
    let original = snapshot::read(
        &prepared.targets[0].root, prepared.task.selection().first(), &stop,
    )?;
    let mut action = Some(action);
    sources.for_each_disk(&stop, |root, identity| {
        if identity.slot_id != "P01" {
            return Ok(());
        }
        let metadata = root.private_child(&root.path().join(
            crate::persist::layout::BACKUP_NATIVE_METADATA.as_path(),
        ))?;
        let published = candidate.read_state_at(&metadata, &stop)?;
        let source = PublishedBackupOriginal::observe(
            root, identity, &candidate, &publication, &original, &published, &stop,
        )?;
        action.take().expect("the fixed original primary occurs once")(
            temporary.path(), &source, &bodies,
        )
    }, &prepared.task.selection().first().pool_id)?;
    assert!(action.is_none());
    choices.observe_first_publication(&prepared, &candidate, &stop)?;
    sources.recheck(&system)?;
    index.require_root(&system)?;
    prepared.recheck_names()?;
    assert!(engine.is_read_only());
    assert!(engine.membership_fence_status().is_some());
    assert!(!engine.state().operations.contains_key(&operation));
    Ok(())
}

fn expect_rejected(
    temporary: &Path,
    action: impl FnOnce() -> Result<()>,
) -> Error {
    let before = media(temporary);
    let error = action().err().expect("invalid original must be rejected");
    assert_eq!(media(temporary), before);
    error
}

#[test]
fn published_original_files_keep_no_clobber_readback_and_same_decision_retry() -> Result<()> {
    with_original(|temporary, source, bodies| {
        let stop = AtomicBool::new(false);
        let original = PreparedDatabases::capture(source.original(), &stop)?;
        let published = PreparedDatabases::capture(source.published(), &stop)?;
        assert_eq!((original.file_count(), published.file_count()), (2, 2));
        assert_eq!((original.directory_count(), published.directory_count()), (0, 0));
        let mut databases = Vec::new();
        for (before, after) in original.databases().iter().zip(published.databases()) {
            assert_eq!(before.path(), after.path());
            before.verify(before.bytes())?;
            after.verify(after.bytes())?;
            let path = before.path();
            let file = path.parent().unwrap().file_name().unwrap().to_str().unwrap();
            let body = &bodies[file];
            let container = body.parent().unwrap().parent().unwrap();
            let database = container.join(path.file_name().unwrap());
            assert_eq!(fs::read(&database)?, before.bytes());
            assert_ne!(before.bytes(), after.bytes());
            databases.push((database, before.bytes().to_vec(), after.bytes().to_vec()));
        }
        let body_media = bodies.values().map(|path| {
            (path.clone(), (identity(path), fs::read(path).unwrap()))
        }).collect::<BTreeMap<_, _>>();
        let plan = BackupOriginalImage::capture(source, &stop)?;
        let recipe = plan.recipe_hash().to_owned();
        let old_identities = databases.iter().map(|(path, _, _)| identity(path))
            .collect::<Vec<_>>();
        expect_rejected(temporary, || plan.require_capacity(
            InstallAdmission::NewOperation, u64::MAX, &stop,
        ));
        expect_rejected(temporary, || {
            BackupOriginalImage::capture(source, &stop)?
                .apply_preserving(&AtomicBool::new(true)).map(|_| ())
        });
        drop(plan);

        let (database, original_bytes, next_bytes) = &databases[0];
        let parked_before = temporary.join("parked-before-selection");
        fs::rename(database, &parked_before)?;
        expect_rejected(temporary, || BackupOriginalImage::capture(source, &stop).map(|_| ()));
        assert!(!database.exists());
        fs::rename(&parked_before, database)?;

        // The correct successor SQLite, with no local exchanged pair, cannot
        // replace this root's own registered predecessor or create a receipt.
        fs::write(database, next_bytes)?;
        expect_rejected(temporary, || BackupOriginalImage::capture(source, &stop).map(|_| ()));
        fs::write(database, original_bytes)?;

        // Valid SQL and the correct FILE/history alone do not authorize a
        // different local row set. The actual original snapshot is immutable.
        let mut other = source.original().state().clone();
        let path = original.databases()[0].path();
        let file = path.parent().unwrap().file_name().unwrap().to_str().unwrap();
        other.file_notes.insert(file.to_owned(), "unpublished changed note".to_owned());
        let other = crate::history_package::AdmittedState::admit_replayed(other)?;
        let other = PreparedDatabases::capture(&other, &stop)?;
        let wrong = other.databases().iter().find(|db| db.path() == path).unwrap();
        wrong.verify(wrong.bytes())?;
        assert_ne!(wrong.bytes(), original_bytes.as_slice());
        fs::write(database, wrong.bytes())?;
        expect_rejected(temporary, || BackupOriginalImage::capture(source, &stop).map(|_| ()));
        fs::write(database, original_bytes)?;

        for suffix in ["-wal", "-shm", "-journal", ".unknown"] {
            let sibling = database.with_file_name(format!(
                "{}{suffix}", database.file_name().unwrap().to_str().unwrap(),
            ));
            fs::write(&sibling, b"uncheckpointed or unknown database companion")?;
            assert_eq!(expect_rejected(temporary, || {
                BackupOriginalImage::capture(source, &stop).map(|_| ())
            }).code(), ErrorCode::ContainerLayoutInvalid);
            assert_eq!(fs::read(&sibling)?, b"uncheckpointed or unknown database companion");
            fs::remove_file(sibling)?;
        }
        let corrupt = b"not a scoped SQLite image";
        fs::write(database, corrupt)?;
        expect_rejected(temporary, || {
            BackupOriginalImage::capture(source, &stop).map(|_| ())
        });
        assert_eq!(fs::read(database)?, corrupt);
        fs::write(database, original_bytes)?;

        // A late companion must also stop an already captured plan before its
        // first FILE exchange, not merely fail the post-write completion read.
        let plan = BackupOriginalImage::capture(source, &stop)?;
        let late = database.with_file_name(format!(
            "{}-wal", database.file_name().unwrap().to_str().unwrap(),
        ));
        fs::write(&late, b"late database companion")?;
        assert_eq!(
            expect_rejected(temporary, || plan.apply_preserving(&stop).map(|_| ())).code(),
            ErrorCode::ContainerLayoutInvalid,
        );
        fs::remove_file(late)?;

        // The second real FILE disappears after capture. The first preserving
        // exchange may durably finish, but no completion escapes this failure.
        let plan = BackupOriginalImage::capture(source, &stop)?;
        let second_container = databases[1].0.parent().unwrap().to_path_buf();
        let parked = temporary.join("parked-original-container");
        fs::rename(&second_container, &parked)?;
        assert!(plan.apply_preserving(&stop).is_err());
        assert_eq!(fs::read(&databases[0].0)?, databases[0].2);
        assert_eq!(fs::read(parked.join(databases[1].0.file_name().unwrap()))?, databases[1].1);
        let partial = media(temporary);
        assert!(BackupOriginalImage::capture(source, &stop).is_err());
        assert_eq!(media(temporary), partial);
        assert!(!second_container.exists());
        fs::rename(&parked, &second_container)?;

        let resumed = BackupOriginalImage::capture(source, &stop)?;
        assert_eq!(resumed.recipe_hash(), recipe);
        assert!(resumed.verify(&stop).is_err());
        let completion = resumed.apply_preserving(&stop)?;
        let cold = BackupOriginalImage::capture(source, &stop)?;
        assert_eq!(cold.recipe_hash(), recipe);
        assert_eq!(cold.verify(&stop)?.bytes(), completion.bytes());
        for (path, (_, expected)) in &body_media {
            assert_eq!(fs::read(path)?, *expected);
            assert_eq!(identity(path), body_media[path].0);
        }
        for (path, _, expected) in &databases {
            assert_eq!(fs::read(path)?, *expected);
        }
        let installed_media = media(temporary);
        for ((active, before, _), identity) in databases.iter().zip(&old_identities) {
            let retained = installed_media.iter().filter(|(_, (found, _))| found == identity)
                .collect::<Vec<_>>();
            assert_eq!(retained.len(), 1);
            assert_ne!(retained[0].0, active);
            assert_eq!(retained[0].1.1.as_deref(), Some(before.as_slice()));
        }

        // Missing displaced material invalidates even an already captured
        // proof; neither the new live file nor another original can refill it.
        let original_inode = identity(&databases[0].0);
        let retained = installed_media.iter().find(|(_, (inode, _))| *inode == old_identities[0])
            .unwrap().0;
        let parked = temporary.join("parked-retained-FILE");
        fs::rename(retained, &parked)?;
        expect_rejected(temporary, || cold.verify(&stop).map(|_| ()));
        expect_rejected(temporary, || BackupOriginalImage::capture(source, &stop).map(|_| ()));
        assert_eq!(identity(&databases[0].0), original_inode);
        fs::rename(&parked, retained)?;

        // Not even a new completion with the same logical bytes is allowed for
        // a substituted inode: restore only the actual old/new exchanged pair.
        let path = &databases[0].0;
        let held = fs::File::open(path)?;
        let replacement = temporary.join("same-byte-FILE");
        fs::write(&replacement, &databases[0].2)?;
        fs::set_permissions(&replacement, fs::Permissions::from_mode(0o600))?;
        let exact = temporary.join("parked-exact-installed-FILE");
        fs::rename(path, &exact)?;
        fs::rename(&replacement, path)?;
        assert_ne!(held.metadata()?.ino(), fs::metadata(path)?.ino());
        expect_rejected(temporary, || cold.verify(&stop).map(|_| ()));
        expect_rejected(temporary, || BackupOriginalImage::capture(source, &stop).map(|_| ()));
        fs::rename(path, &replacement)?;
        expect_rejected(temporary, || cold.verify(&stop).map(|_| ()));
        assert!(!path.exists());
        fs::rename(&exact, path)?;
        assert_eq!(BackupOriginalImage::capture(source, &stop)?.verify(&stop)?.bytes(),
            completion.bytes());

        // A late SQLite sibling invalidates completed readback without cleanup.
        let sidecar = path.with_file_name(format!(
            "{}-shm", path.file_name().unwrap().to_str().unwrap(),
        ));
        fs::write(&sidecar, b"late completed companion")?;
        assert_eq!(
            expect_rejected(temporary, || cold.verify(&stop).map(|_| ())).code(),
            ErrorCode::ContainerLayoutInvalid,
        );
        fs::remove_file(sidecar)?;
        assert_eq!(BackupOriginalImage::capture(source, &stop)?.verify(&stop)?.bytes(),
            completion.bytes());
        Ok(())
    })
}
