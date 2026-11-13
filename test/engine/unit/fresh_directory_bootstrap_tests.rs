//! Real private roots and durable bootstrap bytes, never synthetic registration authority.
use super::*;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

fn fixture() -> (tempfile::TempDir, std::path::PathBuf, crate::Config) {
    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let system = temp.path().join("system");
    fs::create_dir(&system).unwrap();
    fs::set_permissions(&system, fs::Permissions::from_mode(0o700)).unwrap();
    let roots = (0..3).map(|index| temp.path().join(format!("disk-{index}")))
        .collect::<Vec<_>>();
    for root in &roots {
        fs::create_dir(root).unwrap();
        fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let mut config = crate::Config::development(roots).unwrap();
    for disk in &mut config.disks {
        disk.level = 1;
        disk.upstream_slot_id = None;
    }
    (temp, system, config)
}
fn image(root: &Path) -> BTreeMap<std::path::PathBuf, (u64, u64, u32, Vec<u8>)> {
    fn walk(root: &Path, path: &Path, rows: &mut BTreeMap<std::path::PathBuf,
        (u64, u64, u32, Vec<u8>)>)
    {
        let stat = fs::symlink_metadata(path).unwrap();
        let bytes = if stat.is_dir() {
            Vec::new()
        } else {
            assert!(stat.is_file());
            fs::read(path).unwrap()
        };
        rows.insert(path.strip_prefix(root).unwrap().to_owned(),
            (stat.dev(), stat.ino(), stat.mode(), bytes));
        if stat.is_dir() {
            for child in fs::read_dir(path).unwrap() {
                walk(root, &child.unwrap().path(), rows);
            }
        }
    }
    let mut rows = BTreeMap::new();
    walk(root, root, &mut rows);
    rows
}

#[test]
fn selector_is_first_and_old_writer_refuses_without_sidecar_or_disk_writes() {
    let (temp, system, config) = fixture();
    let operation = OperationId::generate().unwrap();
    let first = FreshBootstrap::prepare(
        &system, ValidatedConfig::try_from(config.clone()).unwrap(), operation,
    ).unwrap();
    assert_eq!(fs::read_dir(&system).unwrap().count(), 1);
    assert!(system.join("initialized.json").is_file());
    for disk in &config.disks {
        assert_eq!(fs::read_dir(&disk.root).unwrap().count(), 0);
    }
    drop(first);
    let before = image(temp.path());
    assert!(Engine::initialize(&system, config.clone()).is_err());
    assert_eq!(image(temp.path()), before);
    let retry = FreshBootstrap::prepare(
        &system, ValidatedConfig::try_from(config).unwrap(), operation,
    ).unwrap();
    assert_eq!(image(temp.path()), before);
    drop(retry);

    // A pre-admitted SYSTEM/DISK handle cannot silently select a replacement
    // directory at the same name. Refusal must precede the first selector write.
    for replace_system in [false, true] {
        let (held_temp, held_system, held_config) = fixture();
        let config = ValidatedConfig::try_from(held_config).unwrap();
        let system_handle = PrivateDirectory::open_existing(&held_system).unwrap();
        let directories = config.disks.iter().map(|disk| {
            PrivateDirectory::open_existing(&disk.root).unwrap()
        }).collect::<Vec<_>>();
        let replaced = if replace_system { &held_system } else { &config.disks[0].root };
        fs::rename(replaced, held_temp.path().join("original-root")).unwrap();
        fs::create_dir(replaced).unwrap();
        fs::set_permissions(replaced, fs::Permissions::from_mode(0o700)).unwrap();
        let before = image(held_temp.path());
        let admitted = SelectionGuard::acquire_held(system_handle);
        if replace_system {
            assert!(admitted.is_err());
        } else {
            assert!(admitted.unwrap().select_fresh_held(
                &config, operation, &directories,
            ).is_err());
        }
        assert!(!held_system.join("initialized.json").exists());
        assert_eq!(image(held_temp.path()), before);
    }

    let (temp, system, config) = fixture();
    let admitted = FreshBootstrap::prepare(
        &system, ValidatedConfig::try_from(config).unwrap(), operation,
    ).unwrap();
    fs::rename(&system, temp.path().join("original-system")).unwrap();
    fs::create_dir(&system).unwrap();
    fs::set_permissions(&system, fs::Permissions::from_mode(0o700)).unwrap();
    let before = image(temp.path());
    assert!(crate::engine::config::acquire_locks_selected(
        &system, &admitted.resources, admitted.selection,
    ).is_err());
    assert_eq!(image(temp.path()), before);
}

#[test]
fn occupied_roots_and_changed_operation_preserve_all_existing_material() {
    let (temp, system, config) = fixture();
    // Both are otherwise valid old topologies. A must not erase their source
    // chains by registering every member as a new independent primary root.
    for full_chain in [false, true] {
        let mut descendant = config.clone();
        descendant.disks[1].level = 2;
        descendant.disks[1].upstream_slot_id = Some(config.disks[0].slot_id.clone());
        if full_chain {
            descendant.disks[2].level = 3;
            descendant.disks[2].upstream_slot_id = Some(config.disks[1].slot_id.clone());
        }
        let before = image(temp.path());
        let result = Engine::initialize_directory_storage(
            &system, descendant, OperationId::generate().unwrap(),
        );
        assert!(matches!(result, Err(crate::Error::FreshBootstrap(error))
            if matches!(*error, FreshBootstrapError::Topology)));
        assert!(!system.join("initialized.json").exists());
        assert_eq!(image(temp.path()), before);
    }
    let unknown = config.disks[1].root.join("operator-data");
    fs::write(&unknown, b"do not adopt or delete").unwrap();
    let before = image(temp.path());
    assert!(Engine::initialize_directory_storage(
        &system, config.clone(), OperationId::generate().unwrap(),
    ).is_err());
    assert_eq!(image(temp.path()), before);
    fs::remove_file(unknown).unwrap();
    let original = FreshBootstrap::prepare(
        &system, ValidatedConfig::try_from(config.clone()).unwrap(),
        OperationId::generate().unwrap(),
    ).unwrap();
    drop(original);
    let before = image(temp.path());
    assert!(FreshBootstrap::prepare(
        &system, ValidatedConfig::try_from(config).unwrap(), OperationId::generate().unwrap(),
    ).is_err());
    assert_eq!(image(temp.path()), before);
}

#[test]
fn complete_fresh_bootstrap_reopens_exact_roots_and_rejects_replaced_child() {
    let (temp, system, config) = fixture();
    let operation = OperationId::generate().unwrap();
    let mut engine = Engine::initialize_directory_storage(&system, config.clone(), operation)
        .unwrap();
    // A fresh seal must close through the local FILE membership reader too;
    // merely reopening an empty Engine would not exercise that proof boundary.
    let repository = engine.create_repository(
        &storage_contract::RepositoryName::parse("bootstrap-source").unwrap(),
        "P01", 1, "admin", OperationId::generate().unwrap(),
    ).unwrap();
    let workspace = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
    engine.stage_bytes(&workspace.workspace_id, "source.txt", b"fresh FILE proof").unwrap();
    engine.commit_workspace(crate::CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: OperationId::generate().unwrap(),
        principal: "admin".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "fresh bootstrap source".into(),
        request_digest: None,
    }).unwrap();
    let file = engine.list_tree(&repository.main_branch_id).unwrap().into_iter()
        .find(|entry| entry.path() == "source.txt").unwrap();
    assert_eq!(
        fs::read(engine.valid_replica(&file.version_id).unwrap().0.path()).unwrap(),
        b"fresh FILE proof",
    );
    let root_id = RootId::from_random(identity(operation, &config.disks[0].disk_id, "root"));
    let binding = engine.fresh_directory_root(root_id).unwrap().binding().clone();
    let before_query = image(temp.path());
    let descriptions = engine.directory_storage_roots().unwrap();
    assert_eq!(descriptions.len(), config.disks.len());
    assert!(descriptions.iter().any(|root| root == &binding));
    assert_eq!(image(temp.path()), before_query);
    let mut untrusted = descriptions[0].clone();
    untrusted.root_id = RootId::from_random([255; 16]);
    assert!(engine.fresh_directory_root(untrusted.root_id).is_err());
    assert_eq!(image(temp.path()), before_query);
    require_complete(&system, &engine.config).unwrap();
    let selector = fs::read(system.join("initialized.json")).unwrap();
    let roots = fs::read(system.join(ROOTS)).unwrap();
    drop(engine);
    // A complete cold admission consumes the selected descriptors, validates
    // original selector/operation plus every root and writer, and writes nothing.
    let before_held = image(temp.path());
    let selected_system = PrivateDirectory::open_existing(&system).unwrap();
    let selected_disks = config.disks.iter().map(|disk| {
        PrivateDirectory::open_existing(&disk.root).unwrap()
    }).collect::<Vec<_>>();
    let held = FreshBootstrap::prepare_held(
        selected_system, ValidatedConfig::try_from(config.clone()).unwrap(), operation,
        selected_disks,
    ).unwrap();
    require_complete_held(
        &held.selection.selected_directory().unwrap(), &held.config, &held.resources,
    ).unwrap();
    assert_eq!(image(temp.path()), before_held);
    drop(held);
    let reopened = Engine::initialize_directory_storage(
        &system, config.clone(), operation,
    ).unwrap();
    assert_eq!(reopened.fresh_directory_root(root_id).unwrap().binding(), &binding);
    let before_query = image(temp.path());
    assert_eq!(reopened.directory_storage_roots().unwrap(), descriptions);
    assert_eq!(image(temp.path()), before_query);
    assert_eq!(
        fs::read(reopened.valid_replica(&file.version_id).unwrap().0.path()).unwrap(),
        b"fresh FILE proof",
    );
    assert_eq!(fs::read(system.join("initialized.json")).unwrap(), selector);
    assert_eq!(fs::read(system.join(ROOTS)).unwrap(), roots);
    let disk_selector = config.disks[0].root.join(crate::authority_selection::FRESH_DISK_SELECTOR);
    let retained_selector = disk_selector.with_extension("retained-original");
    fs::rename(&disk_selector, &retained_selector).unwrap();
    let unavailable = image(temp.path());
    assert!(reopened.directory_storage_roots().is_err());
    assert_eq!(image(temp.path()), unavailable);
    fs::rename(&retained_selector, &disk_selector).unwrap();
    assert_eq!(reopened.directory_storage_roots().unwrap(), descriptions);
    let child = config.disks[0].root.join(layout::COMMON_DATA_ROOT.as_path());
    fs::rename(&child, config.disks[0].root.join("original-common")).unwrap();
    fs::create_dir(&child).unwrap();
    fs::set_permissions(&child, fs::Permissions::from_mode(0o700)).unwrap();
    let before = image(temp.path());
    assert!(reopened.fresh_directory_root(root_id).is_err());
    assert!(reopened.directory_storage_roots().is_err());
    assert_eq!(image(temp.path()), before);
    drop(reopened);
    let cold_before = image(temp.path());
    assert!(Engine::open(&system).is_err());
    assert_eq!(image(temp.path()), cold_before);
}

#[test]
fn unclaimed_partial_bootstrap_cannot_be_reinitialized_or_promoted() {
    let (temp, system, config) = fixture();
    let operation = OperationId::generate().unwrap();
    let bootstrap = FreshBootstrap::prepare(
        &system, ValidatedConfig::try_from(config.clone()).unwrap(), operation,
    ).unwrap();
    // Real interrupted-target obstruction after the durable selector. A name
    // and empty directory are not the missing original identity receipt.
    let partial = config.disks[0].root.join(layout::COMMON_DATA_ROOT.as_path());
    fs::create_dir(&partial).unwrap();
    fs::set_permissions(&partial, fs::Permissions::from_mode(0o700)).unwrap();
    let before = image(temp.path());
    assert!(matches!(bootstrap.initialize(), Err(crate::Error::FreshBootstrap(error))
        if matches!(*error, FreshBootstrapError::Incomplete)));
    assert_eq!(image(temp.path()), before);
    assert!(Engine::initialize_directory_storage(&system, config, operation).is_err());
    assert_eq!(image(temp.path()), before);
}

#[test]
fn unknown_selector_and_missing_completion_are_never_upgraded_or_rebuilt() {
    let (temp, system, config) = fixture();
    fs::write(system.join("initialized.json"), b"{\"format\":\"old-independent-format\"}")
        .unwrap();
    let before = image(temp.path());
    assert!(Engine::initialize_directory_storage(
        &system, config, OperationId::generate().unwrap(),
    ).is_err());
    assert!(Engine::open(&system).is_err());
    assert_eq!(image(temp.path()), before);

    let (temp, system, config) = fixture();
    let operation = OperationId::generate().unwrap();
    let engine = Engine::initialize_directory_storage(&system, config.clone(), operation).unwrap();
    drop(engine);
    fs::rename(system.join(COMPLETE), system.join("preserved-completion")).unwrap();
    let before = image(temp.path());
    assert!(Engine::open(&system).is_err());
    assert!(Engine::initialize_directory_storage(&system, config, operation).is_err());
    assert_eq!(image(temp.path()), before);
}

#[test]
fn missing_original_writer_is_not_recreated_by_cold_open() {
    for system_writer in [false, true] {
        let (temp, system, config) = fixture();
        let engine = Engine::initialize_directory_storage(
            &system, config.clone(), OperationId::generate().unwrap(),
        ).unwrap();
        let writer = if system_writer {
            system.join(layout::CONTROL_WRITER_LOCK.as_path())
        } else {
            config.disks[0].root.join(layout::DISK_WRITER_LOCK.as_path())
        };
        fs::rename(&writer, writer.with_extension("retained-original")).unwrap();
        let before = image(temp.path());
        assert!(engine.directory_storage_roots().is_err());
        assert_eq!(image(temp.path()), before);
        drop(engine);
        let before = image(temp.path());
        assert!(Engine::open(&system).is_err());
        assert_eq!(image(temp.path()), before);
        assert!(!writer.exists());
    }
}

#[test]
fn cold_bootstrap_preserves_original_root_and_writer_after_preflight() {
    use crate::engine::disk_lifecycle::bootstrap::{runtime, store::BootstrapStore};
    for replace_root in [true, false] {
        let (temp, system, config) = fixture();
        let engine = Engine::initialize_directory_storage(
            &system, config, OperationId::generate().unwrap(),
        ).unwrap();
        let proof = require_complete(&system, &engine.config).unwrap();
        let admitted = engine.config.clone().with_fresh_completion(proof);
        drop(engine);
        let path = system.join(runtime::DIRECTORY);
        if replace_root {
            fs::rename(&path, system.join("original-bootstrap-retained")).unwrap();
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            let before = image(temp.path());
            assert!(runtime::open_store_for_config(&system, &admitted).is_err());
            assert_eq!(image(temp.path()), before);
        } else {
            let held_system = PrivateDirectory::open_existing(&system).unwrap();
            let held = OpenAt2Root::open(&path, MountPolicy::SameMount).unwrap();
            let writer = admit_bootstrap_handle(&held_system, &admitted, held.identity()).unwrap();
            let lock = path.join(layout::CONTROL_WRITER_LOCK.as_path());
            fs::rename(&lock, path.join("original-writer-retained")).unwrap();
            fs::write(&lock, b"").unwrap();
            fs::set_permissions(&lock, fs::Permissions::from_mode(0o600)).unwrap();
            let before = image(temp.path());
            assert!(BootstrapStore::hold_existing(held.into_directory(), path, writer).is_err());
            assert_eq!(image(temp.path()), before);
        }
    }
}

#[test]
fn cold_bootstrap_refuses_missing_writer_or_old_completion_without_repair() {
    for missing_writer in [true, false] {
        let (temp, system, config) = fixture();
        let engine = Engine::initialize_directory_storage(
            &system, config, OperationId::generate().unwrap(),
        ).unwrap();
        drop(engine);
        if missing_writer {
            fs::remove_file(system.join(
                crate::engine::disk_lifecycle::bootstrap::runtime::DIRECTORY,
            ).join(layout::CONTROL_WRITER_LOCK.as_path())).unwrap();
        } else {
            let bytes = fs::read(system.join(ROOTS)).unwrap();
            let mut old: json_codec::Value = json_codec::from_slice(&bytes).unwrap();
            let fields = old.as_object_mut().unwrap();
            fields.remove("bootstrap_root");
            fields.remove("bootstrap_writer");
            let bytes = storage_contract::canonical_json(&old).unwrap();
            // Both matching legacy records still lack mandatory original authority.
            fs::write(system.join(ROOTS), &bytes).unwrap();
            fs::write(system.join(COMPLETE), &bytes).unwrap();
        }
        let before = image(temp.path());
        assert!(Engine::open(&system).is_err());
        assert_eq!(image(temp.path()), before);
    }
}

#[test]
fn permanent_registration_binds_roots_and_exact_retry_restores_only_completion() {
    let (temp, system, config) = fixture();
    let operation = OperationId::generate().unwrap();
    let engine = Engine::initialize_storage(&system, config.clone(), operation).unwrap();
    let roots = engine.directory_storage_roots().unwrap();
    assert_eq!(roots.len(), config.disks.len());
    let original = engine.state().operations.get(&operation).unwrap().clone();
    assert_eq!(original.kind, "storage.initialize");
    assert_eq!(original.sequence, 1);
    assert_eq!(engine.state().operation_principals.get(&operation).map(String::as_str),
        Some("system:storage-initializer"));
    let complete = fs::read(system.join(COMPLETE)).unwrap();
    let record = RootRecord::decode(&fs::read(system.join(ROOTS)).unwrap()).unwrap();
    decision::BootstrapDecision::decode(&complete, &record).unwrap();
    for disk in &config.disks {
        assert_eq!(fs::read(disk.root.join(layout::STORAGE_INITIALIZATION_PROOF.as_path()))
            .unwrap(), complete);
    }
    drop(engine);
    fs::remove_file(system.join(COMPLETE)).unwrap();
    let before = image(temp.path());
    assert!(Engine::open(&system).is_err());
    assert_eq!(image(temp.path()), before);
    let engine = Engine::initialize_storage(&system, config, operation).unwrap();
    assert_eq!(engine.state().operations.get(&operation), Some(&original));
    assert_eq!(engine.directory_storage_roots().unwrap(), roots);
    assert_eq!(fs::read(system.join(COMPLETE)).unwrap(), complete);
    drop(engine);
    let mut after = image(temp.path());
    // Cold-open disposable database side effects are not the installation proof.
    // Assert immutable bootstrap and every DISK proof separately above; the
    // same operation must not append another permanent operation.
    after.remove(std::path::Path::new(COMPLETE));
    for (path, value) in before {
        if path.starts_with("disk-0") || path.starts_with("disk-1")
            || path.starts_with("disk-2")
        {
            assert_eq!(after.get(&path), Some(&value));
        }
    }
}

#[test]
fn incomplete_disk_installation_proof_never_authorizes_completion_repair() {
    for missing in [true, false] {
        let (temp, system, config) = fixture();
        let operation = OperationId::generate().unwrap();
        let engine = Engine::initialize_storage(&system, config.clone(), operation).unwrap();
        drop(engine);
        fs::remove_file(system.join(COMPLETE)).unwrap();
        let proof = config.disks[1].root.join(layout::STORAGE_INITIALIZATION_PROOF.as_path());
        if missing {
            fs::rename(&proof, config.disks[1].root.join("preserved-original-proof")).unwrap();
            // A missing known output is now repairable only from a complete
            // initial journal decision; incomplete journal evidence is not.
            let log = crate::journal::log_path(&config.disks[1]);
            let length = fs::metadata(&log).unwrap().len();
            fs::OpenOptions::new().write(true).open(log).unwrap().set_len(length - 1).unwrap();
        } else {
            fs::set_permissions(&proof, fs::Permissions::from_mode(0o600)).unwrap();
            fs::write(&proof, b"{}\n").unwrap();
        }
        let before = image(temp.path());
        assert!(Engine::initialize_storage(&system, config, operation).is_err());
        assert_eq!(image(temp.path()), before);
        assert!(!system.join(COMPLETE).exists());
    }
}

#[test]
fn original_finalized_journals_resume_without_completion_or_disk_installation_proofs() {
    use std::fs::OpenOptions;
    let (temp, system, config) = fixture();
    let operation = OperationId::generate().unwrap();
    let engine = Engine::initialize_storage(&system, config.clone(), operation).unwrap();
    let original = engine.state().clone();
    let admitted_config = engine.config.clone();
    let roots = engine.directory_storage_roots().unwrap();
    let completion = fs::read(system.join(COMPLETE)).unwrap();
    drop(engine);
    fs::remove_file(system.join(COMPLETE)).unwrap();
    // Retain precisely the actual PREPARE/ACCEPT/FINALIZE bytes, not fabricated
    // receipts. This is the durable boundary immediately before publication.
    for disk in &config.disks {
        fs::remove_file(disk.root.join(layout::STORAGE_INITIALIZATION_PROOF.as_path())).unwrap();
        let root = crate::disk::DiskRoot::open(&disk.root).unwrap();
        let scan = crate::journal::scan_configured_at(
            &root, disk, &admitted_config,
        ).unwrap();
        assert_eq!(scan.records.len(), 4);
        let length = scan.records.iter().take(3).map(|(record, _)| {
            12 + storage_contract::canonical_json(record).unwrap().len() as u64 + 32
        }).sum::<u64>();
        let log = crate::journal::log_path(disk);
        OpenOptions::new().write(true).open(log).unwrap().set_len(length).unwrap();
        fs::remove_file(crate::journal::publication_seal_path(
            disk, &scan.records[0].0.candidate_hash,
        )).unwrap();
    }
    let before = image(temp.path());
    assert!(Engine::open(&system).is_err());
    assert_eq!(image(temp.path()), before);
    let engine = Engine::initialize_storage(&system, config.clone(), operation).unwrap();
    assert_eq!(engine.state(), &original);
    assert_eq!(engine.directory_storage_roots().unwrap(), roots);
    assert_eq!(fs::read(system.join(COMPLETE)).unwrap(), completion);
    drop(engine);
    let reopened = Engine::initialize_storage(&system, config, operation).unwrap();
    assert_eq!(reopened.state(), &original);
}

#[test]
fn incomplete_initial_journal_is_rejected_before_any_cache_or_material_repair() {
    let (temp, system, config) = fixture();
    let operation = OperationId::generate().unwrap();
    let engine = Engine::initialize_storage(&system, config.clone(), operation).unwrap();
    drop(engine);
    fs::remove_file(system.join(COMPLETE)).unwrap();
    fs::remove_file(config.disks[0].root.join(layout::STORAGE_INITIALIZATION_PROOF.as_path())).unwrap();
    let log = crate::journal::log_path(&config.disks[1]);
    let size = fs::metadata(&log).unwrap().len();
    std::fs::OpenOptions::new().write(true).open(log).unwrap().set_len(size - 1).unwrap();
    let before = image(temp.path());
    assert!(Engine::initialize_storage(&system, config, operation).is_err());
    assert_eq!(image(temp.path()), before);
}

#[test]
fn system_only_activation_probe_precedes_disk_access_and_preserves_pristine_state_proof() {
    let (temp, system, config) = fixture();
    let operation = OperationId::generate().unwrap();
    let engine = Engine::initialize_storage(&system, config.clone(), operation).unwrap();
    engine.require_pristine_storage_initialization(operation).unwrap();
    drop(engine);
    let missing = config.disks[0].root.with_extension("held-original");
    fs::rename(&config.disks[0].root, &missing).unwrap();
    let before = image(temp.path());
    let admitted = ValidatedConfig::try_from(config.clone()).unwrap();
    let probe = Engine::begin_initial_storage_activation_probe(
        PrivateDirectory::open_existing(&system).unwrap(), &admitted, operation,
    ).unwrap();
    assert_eq!(image(temp.path()), before);
    drop(probe);
    fs::rename(missing, &config.disks[0].root).unwrap();
    let probe = Engine::begin_initial_storage_activation_probe(
        PrivateDirectory::open_existing(&system).unwrap(), &admitted, operation,
    ).unwrap();
    let handles = config.disks.iter().map(|disk| {
        opened(PrivateDirectory::open_existing(&disk.root).unwrap()).unwrap().into_directory()
    }).collect();
    let mut engine = Engine::open_initial_activation_for_recovery_held(probe, None, handles).unwrap();
    engine.require_pristine_storage_initialization(operation).unwrap();
    engine.create_repository(
        &storage_contract::RepositoryName::parse("explicit successor").unwrap(),
        "P01", 1, "admin", OperationId::generate().unwrap(),
    ).unwrap();
    let state = engine.state().clone();
    let before = image(temp.path());
    assert!(engine.require_pristine_storage_initialization(operation).is_err());
    // Activation receipt retry permits ordinary/authentication successors while
    // keeping the original storage registration and locator lifecycle fixed.
    engine.require_initial_storage_activation(&admitted, operation).unwrap();
    assert_eq!(engine.state(), &state);
    assert_eq!(image(temp.path()), before);
}
