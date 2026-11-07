//! Public native writer tests. Temporary media is not physical power-loss QA.
use json_codec::json;
use std::{collections::BTreeMap, fs, path::Path};
use storage_engine::{
    CommitRequest, CommitState, Config, Engine, Entry, ErrorCode, MembershipExitAction,
    NamespaceOperation, NamespaceOperationKind, OperationId, OperationResult, PlacementRuleUpdate,
    ProtectionAtPublish, RecursiveBackupCapture, RecursiveBackupConfigurationRequest, Repository,
    RepositoryName, Result,
};

fn operation() -> OperationId {
    OperationId::generate().unwrap()
}

struct Fixture {
    temp: tempfile::TempDir,
    config: Config,
    engine: Engine,
    repository: Repository,
}

impl Fixture {
    fn new(disk_root: bool) -> Self {
        let mut fixture = Self::initial(disk_root, 4);
        fixture.exit(3, MembershipExitAction::Deregister);
        fixture
    }

    fn initial(disk_root: bool, count: usize) -> Self {
        let temp = tempfile::Builder::new()
            .permissions(std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .tempdir()
            .unwrap();
        let mut config =
            Config::development((0..count).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
        for disk in config.disks.iter_mut().skip(3) {
            disk.level = 1;
            disk.upstream_slot_id = None;
            disk.metadata_member = false;
        }
        let mut engine = Engine::initialize(&temp.path().join("system"), config.clone()).unwrap();
        let repository = if disk_root {
            engine
                .create_disk_root("P01", "admin", operation())
                .unwrap()
        } else {
            engine
                .create_repository(
                    &RepositoryName::parse("Repository").unwrap(),
                    "P01",
                    1,
                    "admin",
                    operation(),
                )
                .unwrap()
        };
        Self {
            temp,
            config,
            engine,
            repository,
        }
    }

    fn exit(&mut self, index: usize, action: MembershipExitAction) {
        self.engine
            .begin_disk_exit(
                &self.config.disks[index].disk_id,
                action,
                "admin",
                operation(),
            )
            .unwrap();
        let archive = self
            .engine
            .build_managed_membership_control_archive()
            .unwrap();
        self.engine
            .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
            .unwrap();
        self.engine.resume_membership_joint_decision().unwrap();
        self.engine.activate_pending_disk_exit().unwrap();
    }

    fn save(&mut self, path: &str, bytes: &[u8]) -> Entry {
        let workspace = self
            .engine
            .create_workspace(&self.repository.main_branch_id, "admin")
            .unwrap();
        self.engine
            .stage_bytes(&workspace.workspace_id, path, bytes)
            .unwrap();
        self.engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: operation(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "recursive native path regression".into(),
                request_digest: None,
            })
            .unwrap();
        self.engine
            .namespace_tree(&self.repository.main_branch_id)
            .unwrap()
            .get(path)
            .unwrap()
            .clone()
    }

    fn configure(
        &mut self,
        path: &str,
        capture: &RecursiveBackupCapture,
        generation: u64,
        id: OperationId,
    ) -> Result<OperationResult> {
        let scope = self
            .engine
            .backup_configuration(&self.repository.repository_id, path)?
            .scope;
        self.engine
            .configure_recursive_backups(RecursiveBackupConfigurationRequest {
                repository_id: &self.repository.repository_id,
                path,
                expected_scope_id: &scope.id,
                target_copies: 3,
                expected_generation: generation,
                capture,
                principal: "admin",
                operation_id: id,
            })
    }

    fn assert_body_paths(&self, entry: &Entry, relative: &str, body: &[u8]) {
        let version = &self.engine.state().versions[&entry.version_id];
        let repository = if self.repository.file_manager_root {
            String::new()
        } else {
            format!("{}/", self.repository.name)
        };
        for (index, prefix) in [(1, "disk-0"), (2, "disk-1/disk-0")] {
            let disk = &self.config.disks[index];
            let expected = format!("{prefix}/{repository}{relative}");
            assert_eq!(
                self.engine.state().file_backup_paths[&entry.file_id][&disk.slot_id],
                expected
            );
            let body_path = disk
                .root
                .join("histories")
                .join(&version.history_tree_id)
                .join("file-containers")
                .join(&entry.file_id)
                .join("namespace")
                .join(&expected)
                .join("versions")
                .join(&version.storage_name);
            assert_eq!(fs::read(body_path).unwrap(), body);
        }
    }
}

fn files(root: &Path) -> BTreeMap<std::path::PathBuf, Vec<u8>> {
    fn visit(root: &Path, directory: &Path, result: &mut BTreeMap<std::path::PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(root, &path, result);
            } else {
                result.insert(
                    path.strip_prefix(root).unwrap().to_owned(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}

#[test]
fn recursive_named_repository_reprotects_retained_bytes_and_retries_after_cold_open() {
    let mut fixture = Fixture::new(false);
    let first = fixture.save("ancestor/docs/report", b"first retained");
    let latest = fixture.save("ancestor/docs/report", b"latest retained");
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let receipt = fixture
        .configure("ancestor/docs", &capture, 0, operation())
        .unwrap();
    assert_eq!(receipt.kind, "backup.configure-recursive");
    assert_eq!(receipt.commit_state, CommitState::Published);
    assert_eq!(
        receipt.protection_at_publish,
        ProtectionAtPublish::TargetMet
    );
    fixture.assert_body_paths(&first, "ancestor/docs/report", b"first retained");
    fixture.assert_body_paths(&latest, "ancestor/docs/report", b"latest retained");
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    fixture.engine = Engine::open(&system).unwrap();
    assert_eq!(
        fixture.engine.operation(receipt.operation_id).unwrap(),
        receipt
    );
    assert_eq!(
        fixture
            .configure("ancestor/docs", &capture, 0, receipt.operation_id)
            .unwrap(),
        receipt
    );
    fixture.assert_body_paths(&first, "ancestor/docs/report", b"first retained");
    fixture.assert_body_paths(&latest, "ancestor/docs/report", b"latest retained");
}

#[test]
fn recursive_disk_root_preserves_ancestors_and_new_files_inherit_the_real_paths() {
    let mut fixture = Fixture::new(true);
    let first = fixture.save("ancestor/docs/report", b"existing");
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    fixture
        .configure("ancestor/docs", &capture, 0, operation())
        .unwrap();
    fixture.assert_body_paths(&first, "ancestor/docs/report", b"existing");
    let next = fixture.save("ancestor/docs/later", b"after publication");
    fixture.assert_body_paths(&next, "ancestor/docs/later", b"after publication");
}

#[test]
fn recursive_file_configuration_rejects_changed_capture_before_any_disk_write() {
    let mut fixture = Fixture::new(false);
    let entry = fixture.save("ancestor/docs/report", b"exact source");
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let original = json_codec::to_value(&capture).unwrap();
    let mut missing_upstream = original.clone();
    missing_upstream["disks"]["P01"]
        .as_object_mut()
        .unwrap()
        .remove("upstream_slot_id");
    assert!(json_codec::from_value::<RecursiveBackupCapture>(missing_upstream).is_err());
    let before_state = json_codec::to_vec(fixture.engine.state()).unwrap();
    let before_files = fixture
        .config
        .disks
        .iter()
        .map(|disk| files(&disk.root))
        .collect::<Vec<_>>();
    for (disk, key, value) in [
        ("P01", "name", json!("different-primary")),
        ("B01", "name", json!("different-direct-source")),
        ("C01", "upstream_slot_id", json!("P01")),
    ] {
        let mut body = original.clone();
        body["disks"][disk][key] = value;
        let changed = json_codec::from_value(body).unwrap();
        assert_eq!(
            fixture
                .configure("ancestor/docs/report", &changed, 0, operation())
                .unwrap_err()
                .code(),
            ErrorCode::ScopeChanged
        );
    }
    let mut rebound = original;
    rebound["disks"]["P01"]["identity"]["binding_generation"] = json!("99");
    let rebound = json_codec::from_value(rebound).unwrap();
    assert_eq!(
        fixture
            .configure("ancestor/docs/report", &rebound, 0, operation())
            .unwrap_err()
            .code(),
        ErrorCode::ScopeChanged
    );
    assert_eq!(
        json_codec::to_vec(fixture.engine.state()).unwrap(),
        before_state
    );
    for (disk, before) in fixture.config.disks.iter().zip(before_files) {
        assert_eq!(files(&disk.root), before);
    }
    let receipt = fixture
        .configure("ancestor/docs/report", &capture, 0, operation())
        .unwrap();
    fixture.assert_body_paths(&entry, "ancestor/docs/report", b"exact source");
    let fresh = fixture.engine.recursive_backup_capture().unwrap();
    assert_eq!(
        fixture
            .configure("ancestor/docs/report", &fresh, 0, receipt.operation_id)
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyKeyReused
    );
    assert_eq!(
        fixture
            .configure("ancestor/docs/report", &capture, 1, operation())
            .unwrap_err()
            .code(),
        ErrorCode::ScopeChanged
    );
}

#[test]
fn recursive_empty_scope_publishes_but_stopped_sources_still_refuse_writes() {
    let mut fixture = Fixture::new(false);
    fixture.save("docs/report", b"source");
    fixture
        .engine
        .file_namespace(
            &fixture.repository.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::Mkdir, "empty", None).unwrap(),
            "admin",
            operation(),
        )
        .unwrap();
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let versions = fixture.engine.state().versions.clone();
    let receipt = fixture
        .configure("empty", &capture, 0, operation())
        .unwrap();
    assert_eq!(receipt.commit_state, CommitState::Published);
    assert_eq!(
        receipt.protection_at_publish,
        ProtectionAtPublish::NotApplicable
    );
    assert_eq!(fixture.engine.state().versions, versions);
    assert_eq!(
        fixture.engine.published_backup_directories("C01").unwrap()[0]
            .path()
            .as_str(),
        "disk-1/disk-0/Repository/empty"
    );
    fixture.exit(1, MembershipExitAction::PlannedOffline);
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let stopped = files(&fixture.config.disks[1].root);
    assert_eq!(
        fixture
            .configure("docs", &capture, 0, operation())
            .unwrap_err()
            .code(),
        ErrorCode::PlacementInvalid
    );
    assert_eq!(files(&fixture.config.disks[1].root), stopped);
}

fn assert_independent_route(
    fixture: &Fixture,
    entry: &Entry,
    expected: &[(usize, &str)],
    body: &[u8],
) {
    let version = &fixture.engine.state().versions[&entry.version_id];
    let placement = &fixture.engine.state().placements[&entry.version_id];
    assert_eq!(placement.route_slot_ids.len(), 3);
    assert_eq!(placement.primary_slot_id, placement.route_slot_ids[0]);
    for (route_index, (disk_index, relative)) in expected.iter().enumerate() {
        let disk = &fixture.config.disks[*disk_index];
        assert_eq!(placement.route_slot_ids[route_index + 1], disk.slot_id);
        assert_eq!(
            fixture.engine.state().file_backup_paths[&entry.file_id][&disk.slot_id],
            *relative
        );
        let actual = disk
            .root
            .join("histories")
            .join(&version.history_tree_id)
            .join("file-containers")
            .join(&entry.file_id)
            .join("namespace")
            .join(relative)
            .join("versions")
            .join(&version.storage_name);
        assert_eq!(fs::read(actual).unwrap(), body);
    }
}

#[test]
fn recursive_mixed_primaries_keep_each_direct_source_chain_after_cold_open() {
    let temp = tempfile::Builder::new()
        .permissions(std::os::unix::fs::PermissionsExt::from_mode(0o700))
        .tempdir()
        .unwrap();
    let names = [
        "磁盘a", "a二级", "a三级", "磁盘b", "b二级", "b三级", "spare",
    ];
    let mut config = Config::development(names.iter().map(|name| temp.path().join(name))).unwrap();
    for (index, level, upstream) in [
        (3, 1, None),
        (4, 2, Some("D04")),
        (5, 3, Some("D05")),
        (6, 1, None),
    ] {
        config.disks[index].level = level;
        config.disks[index].upstream_slot_id = upstream.map(str::to_owned);
    }
    config.disks[6].metadata_member = false;
    let mut engine = Engine::initialize(&temp.path().join("system"), config.clone()).unwrap();
    let repository = engine
        .create_repository(
            &RepositoryName::parse("Repository").unwrap(),
            "P01",
            1,
            "admin",
            operation(),
        )
        .unwrap();
    let mut fixture = Fixture {
        temp,
        config,
        engine,
        repository,
    };
    fixture.exit(6, MembershipExitAction::Deregister);
    fixture
        .engine
        .update_placement_rule(PlacementRuleUpdate {
            rule_id: &storage_engine::new_id("rule").unwrap(),
            repository_id: &fixture.repository.repository_id,
            path_prefix: "root/second",
            primary_slot_id: "D04",
            expected_generation: 0,
            principal: "admin",
            operation_id: operation(),
        })
        .unwrap();
    let first = fixture.save("root/first/item", b"first primary");
    let second = fixture.save("root/second/item", b"second primary");
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let receipt = fixture.configure("root", &capture, 0, operation()).unwrap();
    let first_route = [
        (1, "磁盘a/Repository/root/first/item"),
        (2, "a二级/磁盘a/Repository/root/first/item"),
    ];
    let second_route = [
        (4, "磁盘b/Repository/root/second/item"),
        (5, "b二级/磁盘b/Repository/root/second/item"),
    ];
    assert_independent_route(&fixture, &first, &first_route, b"first primary");
    assert_independent_route(&fixture, &second, &second_route, b"second primary");
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    fixture.engine = Engine::open(&system).unwrap();
    assert_eq!(
        fixture
            .configure("root", &capture, 0, receipt.operation_id)
            .unwrap(),
        receipt
    );
    assert_independent_route(&fixture, &first, &first_route, b"first primary");
    assert_independent_route(&fixture, &second, &second_route, b"second primary");
    let next = fixture.save("root/second/later", b"inherited second primary");
    assert_independent_route(
        &fixture,
        &next,
        &[
            (4, "磁盘b/Repository/root/second/later"),
            (5, "b二级/磁盘b/Repository/root/second/later"),
        ],
        b"inherited second primary",
    );
}

fn assert_frozen_file_unchanged(fixture: &Fixture, before: &storage_engine::State, entry: &Entry) {
    let current = fixture.engine.state();
    assert_eq!(current.file_deletions, before.file_deletions);
    assert_eq!(current.branches, before.branches);
    assert_eq!(current.versions, before.versions);
    assert_eq!(current.file_selections, before.file_selections);
    assert_eq!(current.file_write_origins, before.file_write_origins);
    assert_eq!(current.file_contexts, before.file_contexts);
    assert_eq!(current.commits, before.commits);
    assert_eq!(current.file_path_nodes, before.file_path_nodes);
    assert_eq!(current.file_metadata_nodes, before.file_metadata_nodes);
    assert_eq!(current.file_node_parents, before.file_node_parents);
    assert_eq!(current.file_branch_merges, before.file_branch_merges);
    assert_eq!(current.file_branch_aliases, before.file_branch_aliases);
    assert_eq!(current.directory_branches, before.directory_branches);
    assert_eq!(current.directory_trees, before.directory_trees);
    assert_eq!(current.file_names, before.file_names);
    assert_eq!(current.file_locations, before.file_locations);
    let namespace = fixture
        .engine
        .namespace_tree(&fixture.repository.main_branch_id)
        .unwrap();
    assert!(
        !namespace
            .visible_entries()
            .any(|candidate| candidate.file_id == entry.file_id)
    );
    assert!(
        namespace
            .entries()
            .iter()
            .any(|candidate| candidate == entry)
    );
    assert!(
        fixture
            .engine
            .namespace_tree(&fixture.repository.main_branch_id)
            .unwrap()
            .get(entry.path())
            .is_none()
    );
}

#[test]
fn recursive_backup_preserves_soft_deletion_through_cold_replay() {
    use storage_engine::history_window::{HistoryFileId, HistoryRepositoryId};
    use storage_engine::{DeleteFileRequest, FileDeletionTarget, UndoFileDeletionRequest};
    let mut fixture = Fixture::new(true);
    let entry = fixture.save("ancestor/docs/report", b"retained while hidden");
    let repository = HistoryRepositoryId::parse(&fixture.repository.repository_id).unwrap();
    let file = HistoryFileId::parse(&entry.file_id).unwrap();
    let path = name_policy::LookupPath::parse("ancestor/docs/report").unwrap();
    fixture
        .engine
        .soft_delete_file(DeleteFileRequest {
            target: FileDeletionTarget {
                repository_id: &repository,
                file_id: &file,
                path: &path,
            },
            principal: "admin",
            operation_id: operation(),
        })
        .unwrap();
    let before = fixture.engine.state().clone();
    let node = fixture
        .engine
        .file_deletion(&entry.file_id)
        .unwrap()
        .clone();
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let receipt = fixture
        .configure("ancestor/docs", &capture, 0, operation())
        .unwrap();
    assert_frozen_file_unchanged(&fixture, &before, &entry);
    fixture.assert_body_paths(&entry, "ancestor/docs/report", b"retained while hidden");
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    fixture.engine = Engine::open(&system).unwrap();
    assert_frozen_file_unchanged(&fixture, &before, &entry);
    assert_eq!(
        fixture
            .configure("ancestor/docs", &capture, 0, receipt.operation_id)
            .unwrap(),
        receipt
    );
    assert_frozen_file_unchanged(&fixture, &before, &entry);
    fixture.assert_body_paths(&entry, "ancestor/docs/report", b"retained while hidden");
    fixture
        .engine
        .undo_file_deletion(UndoFileDeletionRequest {
            file_id: &file,
            deletion_node_id: &node.node_id,
            principal: "admin",
            operation_id: operation(),
        })
        .unwrap();
    assert!(fixture.engine.file_deletion(&entry.file_id).is_none());
    assert_eq!(
        fixture
            .engine
            .namespace_tree(&fixture.repository.main_branch_id)
            .unwrap()
            .get("ancestor/docs/report")
            .unwrap(),
        &entry
    );
    fixture.assert_body_paths(&entry, "ancestor/docs/report", b"retained while hidden");
}

#[path = "recursive_backup_empty_directories.rs"]
mod empty_directories;

#[path = "recursive_backup_follow.rs"]
mod follow;

#[path = "recursive_backup_empty_follow.rs"]
mod empty_follow;
