//! First ordinary membership additions, not terminal-edge replacement.
//! Engine fallback below is an admitted-pool read. The separate offline check
//! deliberately claims only the existing DISK/FILE inspection capabilities.
use super::{files, operation};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};
use storage_contract::file_container_tags::{Context, ReadRequest, ReplaceCommand, Tag, TagSet};
use storage_engine::{
    BackupConfigurationRequest, CommitRequest, CommitState, Config, DiskCatalogRole, DiskConfig,
    DiskDatabaseReadScope, DiskRegistrationIdentity, Engine, Entry, ErrorCode, FileVersion,
    ObjectDatabaseScope, OperationId, OperationResult, PendingMemberDirectory, ProtectionAtPublish,
    Repository, RepositoryName, ValidatedConfig, inspect_disk_database_material,
    inspect_object_database_material, recognize_disk,
};

const PATH: &str = "docs/report";
const FIRST: &[u8] = b"first body before a backup exists";
const SECOND: &[u8] = b"second body after the first backup is published";
const TAG: &str = "first-backup-history";

fn save(engine: &mut Engine, repository: &Repository, bytes: &[u8]) -> Entry {
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, PATH, bytes)
        .unwrap();
    let receipt = engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: operation(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "first backup additions".into(),
            request_digest: None,
        })
        .unwrap();
    assert_eq!(receipt.commit_state, CommitState::Published);
    engine
        .namespace_tree(&repository.main_branch_id)
        .unwrap()
        .get(PATH)
        .unwrap()
        .clone()
}

fn tags(repository: &Repository, file: &str) -> ReadRequest {
    ReadRequest {
        file_id: file.parse().unwrap(),
        context: Context {
            repository_id: repository.repository_id.parse().unwrap(),
            path: name_policy::PortablePath::parse(PATH).unwrap(),
        },
    }
}

fn add_empty_tier(engine: &mut Engine, disk: &DiskConfig) {
    assert_eq!(fs::read_dir(&disk.root).unwrap().count(), 0);
    let mut target = engine.config().clone();
    assert_eq!(target.disks.len() + 1, disk.level as usize);
    assert_eq!(
        disk.upstream_slot_id.as_ref(),
        Some(&target.disks.last().unwrap().slot_id)
    );
    target.disks.push(disk.clone());
    engine
        .begin_membership_handoff_with_directories(
            target.clone(),
            "admin",
            operation(),
            vec![PendingMemberDirectory {
                disk_id: disk.disk_id.clone(),
                directory: fs::File::open(&disk.root).unwrap(),
            }],
        )
        .unwrap();
    let archive = engine.build_managed_membership_control_archive().unwrap();
    engine
        .enroll_pending_members(
            &archive.archive_path,
            &archive.root_hash,
            vec![PendingMemberDirectory {
                disk_id: disk.disk_id.clone(),
                directory: fs::File::open(&disk.root).unwrap(),
            }],
        )
        .unwrap();
    assert!(
        engine
            .resume_membership_joint_decision()
            .unwrap()
            .all_certificates_synchronized()
    );
    let receipt = engine.activate_pending_membership().unwrap();
    assert_eq!(receipt.commit_state, CommitState::Published);
    assert_eq!(
        storage_engine::canonical_json(engine.config()).unwrap(),
        storage_engine::canonical_json(&target).unwrap()
    );
    assert!(!engine.is_read_only());
}

fn configure(
    engine: &mut Engine,
    repository: &Repository,
    prefixes: &BTreeMap<String, String>,
    generation: u64,
    operation_id: OperationId,
) -> OperationResult {
    let receipt = engine
        .configure_backups(BackupConfigurationRequest {
            repository_id: &repository.repository_id,
            path: PATH,
            target_copies: prefixes.len() as u32 + 1,
            backup_prefixes: prefixes,
            expected_generation: generation,
            principal: "admin",
            operation_id,
        })
        .unwrap();
    assert_eq!(receipt.kind, "backup.configure");
    assert_eq!(receipt.commit_state, CommitState::Published);
    assert_eq!(receipt.protection_at_publish, ProtectionAtPublish::TargetMet);
    receipt
}

fn relative(index: usize) -> &'static str {
    [
        PATH,
        "primary/Files/docs/report",
        "secondary/primary/Files/docs/report",
    ][index]
}

fn container(disk: &DiskConfig, version: &FileVersion, index: usize) -> PathBuf {
    disk.root
        .join("histories")
        .join(&version.history_tree_id)
        .join("file-containers")
        .join(&version.file_id)
        .join("namespace")
        .join(relative(index))
}

fn body_path(disk: &DiskConfig, version: &FileVersion, index: usize) -> PathBuf {
    container(disk, version, index)
        .join("versions")
        .join(format!("{}-report", version.storage_id))
}

fn assert_copies(engine: &Engine, versions: &[FileVersion], copies: usize) {
    assert_eq!(engine.config().disks.len(), copies);
    for version in versions {
        let placement = &engine.state().placements[&version.version_id];
        assert_eq!(placement.primary_slot_id, "P01");
        assert_eq!(placement.target_copies, copies as u32);
        assert_eq!(
            placement.route_slot_ids.iter().map(String::as_str).collect::<Vec<_>>(),
            ["P01", "B01", "C01"][..copies]
        );
        for (index, disk) in engine.config().disks.iter().enumerate() {
            if index != 0 {
                assert_eq!(
                    engine.state().file_backup_paths[&version.file_id][&disk.slot_id],
                    relative(index)
                );
            }
            let expected = if version.version_id == versions[0].version_id {
                FIRST
            } else {
                SECOND
            };
            assert_eq!(fs::read(body_path(disk, version, index)).unwrap(), expected);
            assert!(container(disk, version, index).join(".zxs-file.sqlite").is_file());
        }
    }
}

fn assert_cold_state(engine: &Engine, repository: &Repository, versions: &[FileVersion]) {
    assert!(!engine.is_read_only());
    let history = engine.file_history(&versions[0].file_id).unwrap();
    assert_eq!(
        history
            .iter()
            .map(|version| &version.version_id)
            .collect::<BTreeSet<_>>(),
        versions
            .iter()
            .map(|version| &version.version_id)
            .collect::<BTreeSet<_>>()
    );
    let snapshot = engine
        .file_container_tags(&tags(repository, &versions[0].file_id))
        .unwrap();
    assert_eq!(
        snapshot.tags.iter().map(|tag| tag.as_str()).collect::<Vec<_>>(),
        [TAG]
    );
    assert_eq!(
        engine
            .file_node_parents(&versions[1].file_id, &versions[1].version_id)
            .unwrap(),
        [versions[0].version_id.clone()]
    );
}

fn assert_engine_reads_only_surviving_tier(
    engine: &Engine,
    versions: &[FileVersion],
    temporary: &Path,
    target: usize,
) {
    // Remove all earlier BODY candidates, not their roots: a renamed root could
    // still be read through an already-held descriptor and prove no fallback.
    let mut parked = Vec::new();
    for (index, disk) in engine.config().disks.iter().enumerate().take(target) {
        for version in versions {
            let original = body_path(disk, version, index);
            let saved = temporary.join(format!("body-{target}-{index}-{}", version.version_id));
            fs::rename(&original, &saved).unwrap();
            parked.push((original, saved));
        }
    }
    let before = engine
        .config()
        .disks
        .iter()
        .map(|disk| files(&disk.root))
        .collect::<Vec<_>>();
    for (index, version) in versions.iter().enumerate() {
        let selected = body_path(&engine.config().disks[target], version, target);
        let expected = fs::metadata(&selected).unwrap();
        let mut opened = engine.open_content(&version.version_id).unwrap();
        let actual = opened.file.metadata().unwrap();
        assert_eq!((actual.dev(), actual.ino()), (expected.dev(), expected.ino()));
        let mut bytes = Vec::new();
        opened.file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, if index == 0 { FIRST } else { SECOND });
    }
    for (disk, before) in engine.config().disks.iter().zip(before) {
        assert_eq!(
            files(&disk.root),
            before,
            "reading must not repair missing upstream BODYs"
        );
    }
    for (original, saved) in parked {
        assert!(!original.exists());
        fs::rename(saved, original).unwrap();
    }
}

fn identity(config: &Config, disk: &DiskConfig) -> DiskRegistrationIdentity {
    DiskRegistrationIdentity {
        pool_id: config.pool_id.clone(),
        disk_id: disk.disk_id.clone(),
        slot_id: disk.slot_id.clone(),
        binding_generation: disk.binding_generation,
    }
}

fn inspect_offline_tier(
    engine: Engine,
    system: &Path,
    versions: &[FileVersion],
    target: usize,
) -> Engine {
    let config = engine.config().clone();
    drop(engine);
    // The public inspections below have no Engine, no SYSTEM and no upstream
    // path to consult. They do not authenticate BODY or authorize pool startup.
    let mut parked = Vec::new();
    let upstreams = config.disks[..target].iter().map(|disk| disk.root.as_path());
    for original in std::iter::once(system).chain(upstreams) {
        let saved = original.with_extension(format!("offline-{target}"));
        fs::rename(original, &saved).unwrap();
        fs::write(original, b"unavailable for offline backup inspection").unwrap();
        parked.push((original.to_path_buf(), saved));
    }
    let disk = &config.disks[target];
    let before = files(&disk.root);
    let expected = identity(&config, disk);
    let recognized =
        recognize_disk(fs::File::open(&disk.root).unwrap(), disk.root.clone()).unwrap();
    assert_eq!(recognized.identity(), &expected);
    assert_eq!(recognized.role().level(), target as u32 + 1);
    assert_eq!(
        recognized.role().upstream_slot_id(),
        disk.upstream_slot_id.as_deref()
    );
    let inventory = inspect_disk_database_material(
        fs::File::open(&disk.root).unwrap(),
        disk.root.clone(),
        &expected,
        DiskDatabaseReadScope::Both,
    )
    .unwrap();
    assert_eq!(inventory.identity(), &expected);
    assert!(inventory.history().is_some());
    let rows = &inventory.current().unwrap().versions;
    for version in versions {
        let row = rows
            .iter()
            .find(|row| row.version.version_id == version.version_id)
            .unwrap();
        assert_eq!(&row.version, version);
        assert_eq!(row.role, DiskCatalogRole::Backup);
        assert_eq!(row.owner_path.as_deref(), Some(relative(target)));
    }
    assert_eq!(
        inspect_disk_database_material(
            fs::File::open(&disk.root).unwrap(),
            disk.root.clone(),
            &identity(&config, &config.disks[0]),
            DiskDatabaseReadScope::Both,
        )
        .err()
        .unwrap()
        .code(),
        ErrorCode::DiskIdentityMismatch
    );
    let parent = container(disk, &versions[0], target);
    let scope = ObjectDatabaseScope::File {
        file_id: versions[0].file_id.clone(),
        history_tree_id: versions[0].history_tree_id.clone(),
    };
    let local =
        inspect_object_database_material(fs::File::open(&parent).unwrap(), parent, &scope).unwrap();
    for version in versions {
        let row = local
            .records()
            .iter()
            .find(|row| row.kind() == "version" && row.id() == version.version_id)
            .unwrap();
        let body: json_codec::Value = json_codec::from_str(row.body_json()).unwrap();
        assert_eq!(
            json_codec::from_value::<FileVersion>(body["version"].clone()).unwrap(),
            *version
        );
        if version.version_id == versions[1].version_id {
            assert_eq!(
                body["parent_node_ids"],
                json_codec::json!([versions[0].version_id])
            );
        }
    }
    let file = local
        .records()
        .iter()
        .find(|row| row.kind() == "file")
        .unwrap();
    let body: json_codec::Value = json_codec::from_str(file.body_json()).unwrap();
    assert_eq!(body["container_tags"]["tags"], json_codec::json!([TAG]));
    assert!(local.records().iter().any(|row| row.kind() == "branch"));
    assert_eq!(files(&disk.root), before);
    for (original, saved) in parked.into_iter().rev() {
        assert_eq!(
            fs::read(&original).unwrap(),
            b"unavailable for offline backup inspection"
        );
        fs::remove_file(&original).unwrap();
        fs::rename(saved, original).unwrap();
    }
    Engine::open(system).unwrap()
}

#[test]
fn empty_pool_adds_backup_tiers_and_cold_reads_each_real_copy() {
    let temp = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let system = temp.path().join("system");
    let topology = Config::development(
        ["primary", "secondary", "tertiary"].map(|name| temp.path().join(name)),
    )
    .unwrap();
    for path in std::iter::once(&system).chain(topology.disks.iter().map(|disk| &disk.root)) {
        fs::DirBuilder::new().mode(0o700).create(path).unwrap();
    }
    let mut initial = topology.clone();
    initial.disks.truncate(1);
    let mut engine = Engine::initialize_held(
        &system,
        ValidatedConfig::try_from(initial).unwrap(),
        vec![fs::File::open(&topology.disks[0].root).unwrap()],
    )
    .unwrap();
    let repository = engine
        .create_repository(
            &RepositoryName::parse("Files").unwrap(),
            "P01",
            1,
            "admin",
            operation(),
        )
        .unwrap();
    let first = save(&mut engine, &repository, FIRST);
    let read = tags(&repository, &first.file_id);
    let current = engine.file_container_tags(&read).unwrap();
    engine
        .replace_container_tags(
            &ReplaceCommand {
                file_id: read.file_id,
                context: read.context,
                expected_generation: current.generation,
                tags: TagSet::new([Tag::parse(TAG.into()).unwrap()]).unwrap(),
            },
            &"admin".parse().unwrap(),
            operation(),
        )
        .unwrap();
    let first_version = engine.state().versions[&first.version_id].clone();
    add_empty_tier(&mut engine, &topology.disks[1]);
    assert!(!body_path(&topology.disks[1], &first_version, 1).exists());
    let mut prefixes = BTreeMap::from([("B01".into(), "primary/Files/docs".into())]);
    let first_configuration = configure(&mut engine, &repository, &prefixes, 0, operation());
    let second = save(&mut engine, &repository, SECOND);
    assert_eq!(first.file_id, second.file_id);
    assert_ne!(first.version_id, second.version_id);
    let versions = [
        first_version,
        engine.state().versions[&second.version_id].clone(),
    ];
    drop(engine);
    engine = Engine::open(&system).unwrap();
    assert_cold_state(&engine, &repository, &versions);
    assert_copies(&engine, &versions, 2);
    assert_eq!(
        configure(&mut engine, &repository, &prefixes, 0, first_configuration.operation_id),
        first_configuration
    );
    assert_engine_reads_only_surviving_tier(&engine, &versions, temp.path(), 1);
    engine = inspect_offline_tier(engine, &system, &versions, 1);

    add_empty_tier(&mut engine, &topology.disks[2]);
    for version in &versions {
        assert!(!body_path(&topology.disks[2], version, 2).exists());
    }
    prefixes.insert("C01".into(), "secondary/primary/Files/docs".into());
    let second_configuration = configure(&mut engine, &repository, &prefixes, 1, operation());
    drop(engine);
    engine = Engine::open(&system).unwrap();
    assert_cold_state(&engine, &repository, &versions);
    assert_copies(&engine, &versions, 3);
    assert_eq!(
        configure(&mut engine, &repository, &prefixes, 1, second_configuration.operation_id),
        second_configuration
    );
    assert_engine_reads_only_surviving_tier(&engine, &versions, temp.path(), 2);
    engine = inspect_offline_tier(engine, &system, &versions, 2);
    assert_cold_state(&engine, &repository, &versions);
    assert_copies(&engine, &versions, 3);
}
