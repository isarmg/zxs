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
