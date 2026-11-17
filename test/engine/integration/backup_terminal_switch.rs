//! Independent normal two-copy replacement, not the three-tier fanout regression.
//! Only a fresh target is selected; no enrolled disk or original bytes are reset.
use super::{ProgressDiagnostic, body, diagnostic, op, prepared, save, target};
use std::{
    collections::BTreeSet,
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};
use storage_contract::file_container::ContainerEntity;
use storage_engine::{
    BackupPreparationPhase, BackupPreparationRequest, BackupTopologyTarget,
    BackupTransactionRequest, CommitState, Config, Engine, FileVersion, NamespaceOperation,
    NamespaceOperationKind, ObjectDatabaseScope, ProtectionPolicyUpdate,
    inspect_object_database_material,
};

const FIRST: &[u8] = b"first body on the original two-copy route";
const SECOND: &[u8] = b"second body retained through terminal replacement";
const THIRD: &[u8] = b"ordinary successor after exact cold reopen";

fn directory(path: &Path, expected: ContainerEntity) {
    let metadata = fs::symlink_metadata(path).unwrap();
    assert!(metadata.is_dir());
    assert!(!metadata.file_type().is_symlink());
    assert_eq!(
        (metadata.dev(), metadata.ino()),
        (expected.device(), expected.inode())
    );
}

// Expected addresses are assertions only. Production selects the original
// published source/target occurrences, not these fixture path calculations.
fn materials(
    engine: &Engine,
    versions: &[FileVersion],
    bodies: &[&[u8]],
    backup: &name_policy::SlotId,
) -> [PathBuf; 2] {
    assert_eq!(versions.len(), bodies.len());
    let owner = engine
        .config()
        .disks
        .iter()
        .find(|disk| disk.slot_id == "P01")
        .unwrap();
    let file = &engine.state().file_containers[&versions[0].file_id.parse().unwrap()];
    let empty = engine
        .state()
        .namespace_directories
        .values()
        .find(|entry| entry.name.as_str() == "empty")
        .unwrap();
    let mut body_entities = vec![BTreeSet::new(); versions.len()];
    let mut databases = BTreeSet::new();
    let mut containers = Vec::new();
    for (index, slot) in ["P01", backup.as_str()].into_iter().enumerate() {
        let disk = engine
            .config()
            .disks
            .iter()
            .find(|disk| disk.slot_id == slot)
            .unwrap();
        let id = disk.disk_id.parse().unwrap();
        let replica = &file.replicas[&id];
        assert_eq!(replica.root_id, engine.config().storage_roots[&id].root_id);
        assert_eq!(replica.prefixes.len(), index);
        let mut area = disk.root.clone();
        if index == 1 {
            let prefix = &replica.prefixes[0];
            assert_eq!(prefix.source_disk.to_string(), owner.disk_id);
            area.push(format!(
                "{}--{}",
                owner.name,
                owner.disk_id.strip_prefix("disk_").unwrap()
            ));
            directory(&area, prefix.entity);
            for (relative, entity) in [
                ("common-data", prefix.areas.common_data),
                ("deleted-data", prefix.areas.deleted_data),
                ("zxs-data", prefix.areas.zxs_data.root),
                ("zxs-data/common-data", prefix.areas.zxs_data.common_data),
                ("zxs-data/deleted-data", prefix.areas.zxs_data.deleted_data),
            ] {
                directory(&area.join(relative), entity);
            }
        }
        let common = area.join("common-data");
        directory(&common.join("empty"), empty.replicas[&id].entity);
        assert_eq!(fs::read_dir(common.join("empty")).unwrap().count(), 0);
        let container = common.join("report");
        directory(&container, replica.entity);
        let database = fs::metadata(container.join(".zxs-file.sqlite")).unwrap();
        assert!(database.is_file());
        assert!(databases.insert((database.dev(), database.ino())));
        for (version_index, (version, expected)) in versions.iter().zip(bodies).enumerate() {
            let placement = &engine.state().placements[&version.version_id];
            assert_eq!(placement.route_slot_ids, ["P01", backup.as_str()]);
            assert_eq!(placement.target_copies, 2);
            let path = container
                .join("versions")
                .join(format!("{}-report", version.storage_id));
            let metadata = fs::symlink_metadata(&path).unwrap();
            assert!(metadata.is_file());
            assert!(body_entities[version_index].insert((metadata.dev(), metadata.ino())));
            assert_eq!(fs::read(path).unwrap(), *expected);
        }
        assert_eq!(
            fs::read_dir(container.join("versions")).unwrap().count(),
            versions.len()
        );
        containers.push(container);
    }
    containers.try_into().unwrap()
}

// Use only each stopped FILE store. This does not claim standalone pool rescue.
fn local_file_histories(containers: &[PathBuf; 2], versions: &[FileVersion]) {
    let scope = ObjectDatabaseScope::File {
        file_id: versions[0].file_id.clone(),
        history_tree_id: versions[0].history_tree_id.clone(),
    };
    for container in containers {
        let material = inspect_object_database_material(
            fs::File::open(container).unwrap(),
            container.clone(),
            &scope,
        )
        .unwrap();
        assert_eq!(material.scope(), &scope);
        for (index, version) in versions.iter().enumerate() {
            let row = material
                .records()
                .iter()
                .find(|row| row.kind() == "version" && row.id() == version.version_id)
                .unwrap();
            let value: json_codec::Value = json_codec::from_str(row.body_json()).unwrap();
            assert_eq!(
                json_codec::from_value::<FileVersion>(value["version"].clone()).unwrap(),
                *version
            );
            if index > 0 {
                assert_eq!(
                    value["parent_node_ids"],
                    json_codec::json!([versions[index - 1].version_id])
                );
            }
        }
    }
}

#[test]
fn two_copy_terminal_switch_preserves_history_and_cold_successor() {
    let started = std::time::Instant::now();
    let temp = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let system = temp.path().join("system");
    let config = Config::development([
        (
            name_policy::Name::parse("Primary").unwrap(),
            temp.path().join("disk-a"),
        ),
        (
            name_policy::Name::parse("Original archive").unwrap(),
            temp.path().join("disk-b"),
        ),
    ])
    .unwrap();
    let mut engine = Engine::initialize(&system, config).unwrap();
    let _progress = ProgressDiagnostic::start(system.clone(), started);
    let repository = engine.create_disk_root("P01", "admin", op()).unwrap();
    let policy = storage_engine::new_id("policy").unwrap();
    engine
        .update_protection_policy(ProtectionPolicyUpdate {
            policy_id: &policy,
            repository_id: &repository.repository_id,
            path_prefix: "",
            target_copies: 2,
            expected_generation: 0,
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    let first = save(&mut engine, &repository, "report", FIRST);
    let second = save(&mut engine, &repository, "report", SECOND);
    assert_eq!(first.file_id, second.file_id);
    engine
        .file_namespace(
            &repository.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::Mkdir, "empty", None).unwrap(),
            "admin",
            op(),
        )
        .unwrap();
    // Future FILE protection does not back up an unrelated empty directory.
    // Select its real occurrence through the existing directory operation.
    let empty_path = name_policy::PortablePath::parse("empty").unwrap();
    let directory_id = engine
        .namespace_tree(&repository.main_branch_id)
        .unwrap()
        .get(empty_path.as_str())
        .unwrap()
        .entry_id
        .parse()
        .unwrap();
    let principal = "admin".parse().unwrap();
    let configured = engine
        .configure_automatic_directory_backups(storage_engine::AutomaticDirectoryBackupRequest {
            repository_id: repository.repository_id.parse().unwrap(),
            path: &empty_path,
            directory_id,
            target_copies: 2,
            expected_generation: storage_contract::Generation::new(0),
            principal: &principal,
            operation_id: op(),
        })
        .unwrap();
    assert_eq!(configured.kind, "backup.configure-automatic-directory");
    assert_eq!(configured.commit_state, CommitState::Published);
    assert_eq!(configured.result["file_count"], "0");
    assert_eq!(configured.result["version_count"], "0");
    let mut versions = vec![
        engine.state().versions[&first.version_id].clone(),
        engine.state().versions[&second.version_id].clone(),
    ];
    let original = materials(
        &engine,
        &versions,
        &[FIRST, SECOND],
        &"B01".parse().unwrap(),
    );
    let originals = engine.config().disks.clone();
    let receipts = engine.state().operations.clone();
    let discovery = fs::read(system.join("config.json")).unwrap();
    let destination = target(temp.path(), "replacement-b");
    assert_eq!(fs::read_dir(&destination).unwrap().count(), 0);
    let operation = op();
    let request = BackupTransactionRequest {
        preparation: BackupPreparationRequest {
            operation_id: operation,
            principal: "admin".into(),
            source_slot_id: "P01".into(),
            target_paths: vec![destination],
            expected_sequence: engine.state().sequence,
        },
        targets: vec![BackupTopologyTarget {
            name: name_policy::Name::parse("Replacement archive").unwrap(),
            disk_id: storage_engine::new_id("disk").unwrap(),
            slot_id: "B02".into(),
            upstream_slot_id: "P01".into(),
        }],
    };
    diagnostic(started, "two-copy: prepare exact terminal replacement");
    prepared(&mut engine, &request);
    let configuration = engine
        .inspect_backup_published_configuration(operation, "admin")
        .unwrap();
    assert_eq!(configuration.complete_data_route, ["P01", "B02"]);
    assert_eq!(
        configuration.retained_replaced_terminal.disk_id,
        originals[1].disk_id
    );
    assert_eq!(engine.state().operations, receipts);
    assert!(engine.is_read_only());
    diagnostic(started, "two-copy: activate original transaction");
    let receipt = engine
        .complete_backup_registry_placement_decision(operation, "admin")
        .unwrap();
    assert_eq!(receipt.kind, "backup-target-replacement");
    assert_eq!(receipt.commit_state, CommitState::Published);
    assert!(!engine.is_read_only());
    assert_eq!(
        engine.backup_preparation_status().unwrap().phase,
        BackupPreparationPhase::GenerationServingActive
    );
    assert_eq!(
        engine.current_backup_transaction_request("admin").unwrap(),
        Some(request.clone())
    );
    assert_eq!(
        engine
            .start_backup_transaction(request.clone(), Vec::new())
            .unwrap()
            .phase,
        BackupPreparationPhase::GenerationServingActive
    );
    for (operation, original) in receipts.iter() {
        assert_eq!(engine.operation(*operation).unwrap(), *original);
    }
    assert_eq!(fs::read(system.join("config.json")).unwrap(), discovery);
    for (version, expected) in versions.iter().zip([FIRST, SECOND]) {
        assert_eq!(body(&engine, &version.version_id), expected);
        assert_eq!(
            fs::read(
                original[1]
                    .join("versions")
                    .join(format!("{}-report", version.storage_id))
            )
            .unwrap(),
            expected
        );
    }
    let containers = materials(
        &engine,
        &versions,
        &[FIRST, SECOND],
        &"B02".parse().unwrap(),
    );
    drop(engine);
    local_file_histories(&containers, &versions);
    diagnostic(
        started,
        "two-copy: cold reopen and original-operation retry",
    );
    let mut engine = Engine::open(&system).unwrap();
    assert!(!engine.is_read_only());
    assert_eq!(engine.operation(operation).unwrap(), receipt);
    assert_eq!(
        engine
            .backup_transaction_request(operation, "admin")
            .unwrap(),
        Some(request.clone())
    );
    assert_eq!(
        engine
            .complete_backup_registry_placement_decision(operation, "admin")
            .unwrap(),
        receipt
    );
    assert_eq!(
        engine
            .start_backup_transaction(request, Vec::new())
            .unwrap()
            .phase,
        BackupPreparationPhase::GenerationServingActive
    );
    materials(
        &engine,
        &versions,
        &[FIRST, SECOND],
        &"B02".parse().unwrap(),
    );
    for (version, expected) in versions.iter().zip([FIRST, SECOND]) {
        assert_eq!(body(&engine, &version.version_id), expected);
    }
    diagnostic(started, "two-copy: ordinary successor after cold reopen");
    let third = save(&mut engine, &repository, "report", THIRD);
    assert_eq!(third.file_id, first.file_id);
    assert_eq!(
        engine
            .file_node_parents(&third.file_id, &third.version_id)
            .unwrap(),
        [second.version_id]
    );
    versions.push(engine.state().versions[&third.version_id].clone());
    let containers = materials(
        &engine,
        &versions,
        &[FIRST, SECOND, THIRD],
        &"B02".parse().unwrap(),
    );
    assert_eq!(engine.operation(operation).unwrap(), receipt);
    drop(engine);
    local_file_histories(&containers, &versions);
    let engine = Engine::open(&system).unwrap();
    assert!(!engine.is_read_only());
    assert_eq!(engine.operation(operation).unwrap(), receipt);
    materials(
        &engine,
        &versions,
        &[FIRST, SECOND, THIRD],
        &"B02".parse().unwrap(),
    );
    for (version, expected) in versions.iter().zip([FIRST, SECOND, THIRD]) {
        assert_eq!(body(&engine, &version.version_id), expected);
    }
    diagnostic(
        started,
        "two-copy: normal replacement and cold successor complete",
    );
}
