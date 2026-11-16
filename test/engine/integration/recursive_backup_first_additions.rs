//! First ordinary membership additions, not terminal-edge replacement.
//! Engine fallback below is an admitted-pool read. The separate offline check
//! deliberately claims only the existing DISK/FILE inspection capabilities.
use super::{files, operation};
use std::{
    collections::BTreeSet,
    fs,
    io::Read,
    os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};
use storage_contract::file_container_tags::{Context, ReadRequest, ReplaceCommand, Tag, TagSet};
use storage_engine::{
    AutomaticBackupConfigurationRequest, BranchKind, BranchName, BranchState, CommitRequest,
    CommitState,
    Config, DiskCatalogRole, DiskConfig,
    DiskDatabaseReadScope, DiskRegistrationIdentity, Engine, Entry, ErrorCode, FileVersion,
    ObjectDatabaseScope, OperationId, OperationResult, PendingMemberDirectory, ProtectionAtPublish,
    NamespaceOperation, Repository, ValidatedConfig, inspect_disk_database_material,
    inspect_object_database_material, recognize_disk,
};

const PATH: &str = "docs/report";
const FIRST: &[u8] = b"first body before a backup exists";
const SECOND: &[u8] = b"second body after the first backup is published";
const TAG: &str = "first-backup-history";
const BRANCH: &str = "backup version chain";

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

fn add_empty_tier(engine: &mut Engine, repository: &Repository, disk: &DiskConfig) {
    assert_eq!(fs::read_dir(&disk.root).unwrap().count(), 0);
    let mut target = engine.config().clone();
    let old_roots = target.storage_roots.clone();
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
    let pending_state = storage_engine::canonical_json(engine.state()).unwrap();
    let pending_roots = target.disks.iter().map(|disk| files(&disk.root)).collect::<Vec<_>>();
    assert!(configure_result(engine, repository, disk.level, disk.level as u64 - 2,
        operation()).is_err());
    assert_eq!(storage_engine::canonical_json(engine.state()).unwrap(), pending_state);
    assert_eq!(target.disks.iter().map(|disk| files(&disk.root)).collect::<Vec<_>>(),
        pending_roots);
    let receipt = engine.activate_pending_membership().unwrap();
    assert_eq!(receipt.commit_state, CommitState::Published);
    for (id, root) in &old_roots {
        assert_eq!(engine.config().storage_roots.get(id), Some(root));
    }
    let added = disk.disk_id.parse::<storage_contract::DiskId>().unwrap();
    let root = &engine.config().storage_roots[&added];
    assert_eq!(root.operation_id, receipt.operation_id);
    assert_eq!(root.registration, identity(engine.config(), disk));
    assert_eq!(root.source_chain.iter().map(|entry| entry.disk_id.as_str()).collect::<Vec<_>>(),
        target.disks[..target.disks.len() - 1].iter().rev()
            .map(|entry| entry.disk_id.as_str()).collect::<Vec<_>>());
    let actual = fs::metadata(&disk.root).unwrap();
    assert_eq!((root.root_entity.device(), root.root_entity.inode()),
        (actual.dev(), actual.ino()));
    target.storage_roots = engine.config().storage_roots.clone();
    assert_eq!(storage_engine::canonical_json(engine.config()).unwrap(),
        storage_engine::canonical_json(&target).unwrap());
    assert!(!engine.is_read_only());
}

fn configure_result(
    engine: &mut Engine,
    repository: &Repository,
    copies: u32,
    generation: u64,
    operation_id: OperationId,
) -> storage_engine::Result<OperationResult> {
    let path = name_policy::PortablePath::parse(PATH).unwrap();
    let principal = "admin".parse().unwrap();
    engine.configure_automatic_backups(AutomaticBackupConfigurationRequest {
        repository_id: repository.repository_id.parse().unwrap(),
        path: Some(&path), target_copies: copies,
        expected_generation: storage_contract::Generation::new(generation),
        principal: &principal, operation_id,
    })
}

fn configure(
    engine: &mut Engine,
    repository: &Repository,
    copies: u32,
    generation: u64,
    operation_id: OperationId,
) -> OperationResult {
    let receipt = configure_result(engine, repository, copies, generation, operation_id).unwrap();
    assert_eq!(receipt.kind, "backup.configure-automatic");
    assert_eq!(receipt.commit_state, CommitState::Published);
    assert_eq!(receipt.protection_at_publish, ProtectionAtPublish::TargetMet);
    receipt
}

fn source_area(config: &Config, index: usize) -> PathBuf {
    let mut path = config.disks[index].root.clone();
    for disk in config.disks[..index].iter().rev() {
        let uuid = disk.disk_id.strip_prefix("disk_").unwrap();
        path.push(format!("{}--{uuid}", disk.name));
    }
    path
}

fn container(config: &Config, index: usize) -> PathBuf {
    source_area(config, index).join("common-data").join(PATH)
}

fn body_path(config: &Config, version: &FileVersion, index: usize) -> PathBuf {
    container(config, index).join("versions").join(format!("{}-report", version.storage_id))
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
            let id = disk.disk_id.parse::<storage_contract::DiskId>().unwrap();
            let file = version.file_id.parse::<storage_contract::FileId>().unwrap();
            let replica = &engine.state().file_containers[&file].replicas[&id];
            assert_eq!(replica.root_id, engine.config().storage_roots[&id].root_id);
            assert_eq!(replica.prefixes.iter().map(|entry| entry.source_disk.to_string())
                .collect::<Vec<_>>(), engine.config().disks[..index].iter().rev()
                    .map(|source| source.disk_id.clone()).collect::<Vec<_>>());
            let actual = fs::metadata(container(engine.config(), index)).unwrap();
            assert_eq!((replica.entity.device(), replica.entity.inode()),
                (actual.dev(), actual.ino()));
            let expected = if version.version_id == versions[0].version_id {
                FIRST
            } else {
                SECOND
            };
            assert_eq!(fs::read(body_path(engine.config(), version, index)).unwrap(), expected);
            assert!(container(engine.config(), index).join(".zxs-file.sqlite").is_file());
        }
    }
}

fn assert_cold_state(
    engine: &Engine,
    repository: &Repository,
    versions: &[FileVersion],
    branch_id: &str,
) {
    assert!(!engine.is_read_only());
    let file = versions[0].file_id.as_str();
    assert_eq!(engine.state().file_selections[file], branch_id);
    let branch = &engine.state().branches[branch_id];
    assert_eq!(branch.branch_kind, BranchKind::File);
    assert_eq!(branch.file_id.as_deref(), Some(file));
    assert_eq!(branch.history_tree_id, versions[0].history_tree_id);
    assert_eq!(branch.name, BRANCH);
    assert_eq!(branch.state, BranchState::Active);
    assert!(!branch.pending_file_versions.contains_key(file));
    assert_eq!(engine.file_branch_head(branch_id).unwrap(), versions[1].version_id);
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
    for index in 0..target {
        for version in versions {
            let original = body_path(engine.config(), version, index);
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
        let selected = body_path(engine.config(), version, target);
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
    branch_id: &str,
    target: usize,
) -> Engine {
    let config = engine.config().clone();
    let logical_paths = engine.state().file_backup_paths.clone();
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
        DiskDatabaseReadScope::Current,
    )
    .unwrap();
    assert_eq!(inventory.identity(), &expected);
    // This acceptance covers DISK Current and the actual FILE history below.
    // Current does not inspect the optional DISK event-history representation.
    assert!(matches!(inventory.scope(), DiskDatabaseReadScope::Current));
    let rows = &inventory.current().unwrap().versions;
    for version in versions {
        let row = rows
            .iter()
            .find(|row| row.version.version_id == version.version_id)
            .unwrap();
        assert_eq!(&row.version, version);
        assert_eq!(row.role, DiskCatalogRole::Backup);
        // This row's optional owner_path is the older explicit namespace
        // projection, not authority for an automatic source-prefix path.
        // Root/ordered source/real container identity is checked by assert_copies.
        assert_eq!(row.owner_path.as_ref(), logical_paths.get(&version.file_id)
            .and_then(|paths| paths.get(&disk.slot_id)));
    }
    assert_eq!(
        inspect_disk_database_material(
            fs::File::open(&disk.root).unwrap(),
            disk.root.clone(),
            &identity(&config, &config.disks[0]),
            DiskDatabaseReadScope::Current,
        )
        .err()
        .unwrap()
        .code(),
        ErrorCode::DiskIdentityMismatch
    );
    let parent = container(&config, target);
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
    assert_eq!(body["selected_branch_id"], json_codec::json!(branch_id));
    let branch = local
        .records()
        .iter()
        .find(|row| row.kind() == "branch" && row.id() == branch_id)
        .unwrap();
    let body: json_codec::Value = json_codec::from_str(branch.body_json()).unwrap();
    assert_eq!(body["branch_id"], json_codec::json!(branch_id));
    assert_eq!(body["file_id"], json_codec::json!(versions[0].file_id));
    assert_eq!(body["history_tree_id"], json_codec::json!(versions[0].history_tree_id));
    assert_eq!(body["name"], json_codec::json!(BRANCH));
    assert_eq!(body["state"], json_codec::json!("active"));
    assert_eq!(body["head_node_id"], json_codec::json!(versions[1].version_id));
    assert_eq!(body["pending_version_id"], json_codec::Value::Null);
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
        ["primary", "secondary", "tertiary"].map(|name| {
            (name_policy::Name::parse(name).unwrap(), temp.path().join(name))
        }),
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
    let repository = engine.create_disk_root("P01", "admin", operation()).unwrap();
    engine.file_namespace(&repository.repository_id, NamespaceOperation::Mkdir {
        path: name_policy::PortablePath::parse("docs").unwrap(),
    }, "admin", operation()).unwrap();
    let first = save(&mut engine, &repository, FIRST);
    // A directory main branch is not a FILE branch. Create and select one
    // through the public mutation path before either backup is added.
    let created = engine
        .create_file_branch(
            &first.file_id,
            &first.version_id,
            &BranchName::parse(BRANCH).unwrap(),
            "admin",
            operation(),
        )
        .unwrap();
    assert_eq!(created.commit_state, CommitState::Published);
    let branch_id = created.branch_id.unwrap();
    let selected = engine
        .select_file_branch_at(
            &repository.repository_id,
            &name_policy::PortablePath::parse(PATH).unwrap(),
            &first.file_id,
            &branch_id,
            "admin",
            operation(),
        )
        .unwrap();
    assert_eq!(selected.commit_state, CommitState::Published);
    assert_eq!(selected.branch_id.as_deref(), Some(branch_id.as_str()));
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
    add_empty_tier(&mut engine, &repository, &topology.disks[1]);
    assert!(!body_path(engine.config(), &first_version, 1).exists());
    let first_operation = operation();
    #[cfg(feature = "fault-injection")]
    {
        engine.inject_fault_once("after_accept_1");
        let error = configure_result(&mut engine, &repository, 2, 0, first_operation).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InDoubt);
        let mut source = &error;
        while let storage_engine::Error::Context { source: inner, .. } = source {
            source = inner;
        }
        assert_eq!(source.code(), ErrorCode::InjectedFailure, "{error}");
        assert_eq!(source.message().as_ref(), "interrupted at after_accept_1");
        drop(engine);
        engine = Engine::open(&system).unwrap();
    }
    let first_configuration = configure(&mut engine, &repository, 2, 0, first_operation);
    let second = save(&mut engine, &repository, SECOND);
    assert_eq!(first.file_id, second.file_id);
    assert_ne!(first.version_id, second.version_id);
    let versions = [
        first_version,
        engine.state().versions[&second.version_id].clone(),
    ];
    drop(engine);
    engine = Engine::open(&system).unwrap();
    assert_cold_state(&engine, &repository, &versions, &branch_id);
    assert_copies(&engine, &versions, 2);
    assert_eq!(
        configure(&mut engine, &repository, 2, 0, first_configuration.operation_id),
        first_configuration
    );
    assert_engine_reads_only_surviving_tier(&engine, &versions, temp.path(), 1);
    engine = inspect_offline_tier(engine, &system, &versions, &branch_id, 1);

    add_empty_tier(&mut engine, &repository, &topology.disks[2]);
    for version in &versions {
        assert!(!body_path(engine.config(), version, 2).exists());
    }
    let second_configuration = configure(&mut engine, &repository, 3, 1, operation());
    drop(engine);
    engine = Engine::open(&system).unwrap();
    assert_cold_state(&engine, &repository, &versions, &branch_id);
    assert_copies(&engine, &versions, 3);
    assert_eq!(
        configure(&mut engine, &repository, 3, 1, second_configuration.operation_id),
        second_configuration
    );
    assert_engine_reads_only_surviving_tier(&engine, &versions, temp.path(), 2);
    engine = inspect_offline_tier(engine, &system, &versions, &branch_id, 2);
    assert_cold_state(&engine, &repository, &versions, &branch_id);
    assert_copies(&engine, &versions, 3);
}
