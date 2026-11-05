//! Source-only regression cases. Real Engine/D3 paths are exercised when run;
//! temporary directories are not physical-media or power-loss evidence.
use super::*;
use crate::disk::MAX_METADATA;
use crate::*;
use std::{
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

fn op() -> OperationId {
    OperationId::generate().unwrap()
}

fn save(engine: &mut Engine, repository: &Repository, path: &str, bytes: &[u8]) -> Entry {
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, path, bytes)
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: op(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "current backup fixture".into(),
            request_digest: None,
        })
        .unwrap();
    engine
        .namespace_tree(&repository.main_branch_id)
        .unwrap()
        .get(path)
        .unwrap()
        .clone()
}

fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, directory: &Path, found: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(root, &path, found);
            } else {
                found.insert(
                    path.strip_prefix(root).unwrap().to_owned(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut found = BTreeMap::new();
    visit(root, root, &mut found);
    found
}

struct Fixture {
    temp: tempfile::TempDir,
    engine: Engine,
    config: Config,
    repository: Repository,
    stopped_repository: Repository,
    first: Entry,
    latest: Entry,
}
impl Fixture {
    fn new() -> Self {
        Self::with_disk_count(4)
    }

    fn with_disk_count(count: usize) -> Self {
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
        let repository = engine
            .create_repository(
                &RepositoryName::parse("live backup").unwrap(),
                "P01",
                1,
                "admin",
                op(),
            )
            .unwrap();
        let stopped_repository = engine
            .create_repository(
                &RepositoryName::parse("stopped backup").unwrap(),
                "D04",
                1,
                "admin",
                op(),
            )
            .unwrap();
        let first = save(
            &mut engine,
            &repository,
            "docs/report",
            b"first retained body",
        );
        let latest = save(
            &mut engine,
            &repository,
            "docs/report",
            b"latest retained body",
        );
        save(
            &mut engine,
            &stopped_repository,
            "private/original",
            b"never touch stopped data",
        );
        Self {
            temp,
            engine,
            config,
            repository,
            stopped_repository,
            first,
            latest,
        }
    }

    fn exit(&mut self, index: usize, action: MembershipExitAction) -> BTreeMap<PathBuf, Vec<u8>> {
        let disk = &self.config.disks[index];
        let held = Arc::clone(self.engine.disk_resources.get(disk).unwrap());
        self.engine
            .begin_disk_exit(&disk.disk_id, action, "admin", op())
            .unwrap();
        let archive = self
            .engine
            .build_managed_membership_control_archive()
            .unwrap();
        self.engine
            .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
            .unwrap();
        self.engine.resume_membership_joint_decision().unwrap();
        let before = files(&disk.root);
        let receipt = self.engine.activate_pending_disk_exit().unwrap();
        assert_eq!(receipt.result["target_io"], "closed");
        held.root.io_binding().unwrap().closed_proof().unwrap();
        assert!(self.engine.disk_resources.available(disk).is_none());
        assert_eq!(files(&disk.root), before);
        before
    }

    fn configure(
        &mut self,
        generation: u64,
        copies: u32,
        prefixes: &BTreeMap<String, String>,
        operation: OperationId,
    ) -> Result<OperationResult> {
        self.engine.configure_backups(BackupConfigurationRequest {
            repository_id: &self.repository.repository_id,
            path: "docs",
            target_copies: copies,
            backup_prefixes: prefixes,
            expected_generation: generation,
            principal: "admin",
            operation_id: operation,
        })
    }
}

fn prefixes() -> BTreeMap<String, String> {
    BTreeMap::from([("b01".into(), "chosen/archive".into())])
}

#[test]
fn current_backup_real_d3_reprotects_all_bodies_preserves_stopped_root_and_permanent_retries() {
    for action in [
        MembershipExitAction::PlannedOffline,
        MembershipExitAction::Deregister,
    ] {
        let mut fixture = Fixture::new();
        let old_workspace = fixture
            .engine
            .create_workspace(&fixture.repository.main_branch_id, "admin")
            .unwrap();
        let original = fixture.exit(3, action);
        let stopped_root = fixture.config.disks[3].root.clone();
        let retained = fixture.temp.path().join("retained-stopped-original");
        fs::rename(&stopped_root, &retained).unwrap();
        fs::write(&stopped_root, b"ordinary file at stopped root").unwrap();
        let before = fixture.engine.state.clone();
        let operation = op();
        let prefixes = prefixes();
        let result = fixture.configure(0, 2, &prefixes, operation).unwrap();
        assert_eq!(result.result["version_count"], "2");
        assert_eq!(result.result["file_count"], "1");
        assert_eq!(result.protection_at_publish, ProtectionAtPublish::TargetMet);
        assert_eq!(fixture.engine.state.commits, before.commits);
        assert_eq!(fixture.engine.state.versions, before.versions);
        assert_eq!(fixture.engine.state.file_names, before.file_names);
        assert_eq!(fixture.engine.state.file_locations, before.file_locations);
        for (entry, expected) in [
            (&fixture.first, b"first retained body".as_slice()),
            (&fixture.latest, b"latest retained body".as_slice()),
        ] {
            let placement = &fixture.engine.state.placements[&entry.version_id];
            assert_eq!(placement.route_slot_ids, ["P01", "B01"]);
            let version = &fixture.engine.state.versions[&entry.version_id];
            let directory = crate::disk::version_dir(
                &fixture.config.disks[1],
                &version.history_tree_id,
                &version.version_id,
            );
            let mut payload = crate::disk::verify_payload_held(&directory, version).unwrap();
            payload.seek(SeekFrom::Start(0)).unwrap();
            let mut bytes = Vec::new();
            payload.read_to_end(&mut bytes).unwrap();
            assert_eq!(bytes, expected);
        }
        assert_eq!(
            fixture.engine.state.file_backup_paths[&fixture.first.file_id]["B01"],
            "chosen/archive/report"
        );
        assert_eq!(
            fixture.configure(0, 2, &prefixes, operation).unwrap(),
            result
        );
        assert_eq!(
            fixture
                .configure(0, 1, &prefixes, operation)
                .unwrap_err()
                .code(),
            ErrorCode::IdempotencyKeyReused
        );
        assert_eq!(
            fixture
                .engine
                .configure_backups(BackupConfigurationRequest {
                    repository_id: &fixture.repository.repository_id,
                    path: "docs",
                    target_copies: 2,
                    backup_prefixes: &prefixes,
                    expected_generation: 0,
                    principal: "other",
                    operation_id: operation,
                })
                .unwrap_err()
                .code(),
            ErrorCode::IdempotencyKeyReused
        );
        assert_eq!(
            fixture
                .engine
                .stage_bytes(&old_workspace.workspace_id, "docs/stale", b"held")
                .unwrap_err()
                .code(),
            ErrorCode::WorkspaceChanged
        );
        assert_eq!(files(&retained), original);
        assert_eq!(
            fs::read(&stopped_root).unwrap(),
            b"ordinary file at stopped root"
        );
        let system = fixture.temp.path().join("system");
        drop(fixture.engine);
        let mut engine = Engine::open(&system).unwrap();
        assert_eq!(
            engine
                .configure_backups(BackupConfigurationRequest {
                    repository_id: &fixture.repository.repository_id,
                    path: "docs",
                    target_copies: 2,
                    backup_prefixes: &prefixes,
                    expected_generation: 0,
                    principal: "admin",
                    operation_id: operation,
                })
                .unwrap(),
            result
        );
        assert_eq!(
            engine
                .backup_configuration(&fixture.repository.repository_id, "docs")
                .unwrap()
                .generation,
            1
        );
        assert_eq!(
            engine
                .stage_bytes(&old_workspace.workspace_id, "docs/stale", b"held")
                .unwrap_err()
                .code(),
            ErrorCode::WorkspaceChanged
        );
        assert_eq!(files(&retained), original);
        assert_eq!(
            fs::read(&stopped_root).unwrap(),
            b"ordinary file at stopped root"
        );
    }
}

#[test]
fn stopped_backup_route_or_hidden_scope_is_rejected_before_target_io_and_does_not_reactivate() {
    for action in [
        MembershipExitAction::PlannedOffline,
        MembershipExitAction::Deregister,
    ] {
        let mut fixture = Fixture::new();
        let original = fixture.exit(1, action);
        let disk = fixture.config.disks[1].clone();
        let retained = fixture.temp.path().join("held-backup");
        fs::rename(&disk.root, &retained).unwrap();
        fs::write(&disk.root, b"not a directory").unwrap();
        let before = fixture.engine.state.clone();
        assert!(fixture.configure(0, 2, &prefixes(), op()).is_err());
        assert_eq!(*fixture.engine.state, *before);
        assert!(fixture.engine.disk_resources.available(&disk).is_none());
        assert_eq!(files(&retained), original);
        assert_eq!(fs::read(&disk.root).unwrap(), b"not a directory");
    }
    let mut fixture = Fixture::new();
    fixture.exit(3, MembershipExitAction::Deregister);
    let before = fixture.engine.state.clone();
    assert_eq!(
        fixture
            .engine
            .configure_backups(BackupConfigurationRequest {
                repository_id: &fixture.stopped_repository.repository_id,
                path: "private/original",
                target_copies: 1,
                backup_prefixes: &BTreeMap::new(),
                expected_generation: 0,
                principal: "admin",
                operation_id: op(),
            })
            .unwrap_err()
            .code(),
        ErrorCode::EntryNotFound
    );
    assert_eq!(*fixture.engine.state, *before);
}

#[test]
fn current_backup_preserves_child_policies_and_detaches_moved_file_bindings() {
    let mut fixture = Fixture::new();
    let child = save(
        &mut fixture.engine,
        &fixture.repository,
        "docs/child/file",
        b"independent",
    );
    fixture
        .engine
        .configure_backups(BackupConfigurationRequest {
            repository_id: &fixture.repository.repository_id,
            path: "docs/child",
            target_copies: 2,
            backup_prefixes: &BTreeMap::from([("B01".into(), "child".into())]),
            expected_generation: 0,
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    fixture.configure(0, 2, &prefixes(), op()).unwrap();
    let old_parent_policy =
        fixture.engine.state.file_backup_policies[&fixture.first.file_id].clone();
    save(
        &mut fixture.engine,
        &fixture.repository,
        "moved/marker",
        b"destination parent",
    );
    fixture
        .engine
        .file_namespace(
            &fixture.repository.repository_id,
            NamespaceOperation::parse(
                NamespaceOperationKind::Rename,
                "docs/report",
                Some("moved/report"),
            )
            .unwrap(),
            "admin",
            op(),
        )
        .unwrap();
    fixture.exit(3, MembershipExitAction::Deregister);
    let child_policy = fixture.engine.state.file_backup_policies[&child.file_id].clone();
    let before = fixture.engine.state.clone();
    let result = fixture.configure(1, 1, &BTreeMap::new(), op()).unwrap();
    assert_eq!(result.result["file_count"], "0");
    assert_eq!(result.result["version_count"], "0");
    assert_eq!(
        result.protection_at_publish,
        ProtectionAtPublish::NotApplicable
    );
    assert_eq!(
        fixture.engine.state.file_backup_policies[&child.file_id],
        child_policy
    );
    assert_eq!(
        fixture.engine.state.protection_policies[&child_policy],
        before.protection_policies[&child_policy]
    );
    let retained = &fixture.engine.state.file_backup_policies[&fixture.first.file_id];
    assert_ne!(retained, &old_parent_policy);
    assert!(!fixture.engine.state.protection_policies[retained].inheritable);
    assert_eq!(
        fixture.engine.state.protection_policies[retained].target_copies,
        2
    );
    assert_eq!(
        fixture.engine.state.file_backup_paths,
        before.file_backup_paths
    );
    assert_eq!(fixture.engine.state.placements, before.placements);
}

fn published() -> (Fixture, Arc<CurrentNamespaces>, Arc<State>, OperationResult) {
    let mut fixture = Fixture::new();
    fixture.exit(3, MembershipExitAction::Deregister);
    let current = Arc::clone(fixture.engine.current_namespaces.as_ref().unwrap());
    let previous = Arc::clone(&fixture.engine.state);
    let result = fixture.configure(0, 2, &prefixes(), op()).unwrap();
    (fixture, current, previous, result)
}

fn rejected(view: &CurrentNamespaces, previous: &State, forged: &State, label: &str) {
    assert!(view.advance(previous, forged).is_err(), "live {label}");
    let previous: State = decode_strict(&canonical_json(previous).unwrap()).unwrap();
    let forged: State = decode_strict(&canonical_json(forged).unwrap()).unwrap();
    assert!(view.advance(&previous, &forged).is_err(), "cold {label}");
}

#[test]
fn exact_backup_request_actor_registry_scope_and_receipt_are_replayed_without_tracking() {
    let (fixture, view, previous, result) = published();
    let next = fixture.engine.state.as_ref();
    view.advance(&previous, next).unwrap();
    let cold_previous: State = decode_strict(&canonical_json(&*previous).unwrap()).unwrap();
    let cold_next: State = decode_strict(&canonical_json(next).unwrap()).unwrap();
    view.advance(&cold_previous, &cold_next).unwrap();
    for key in [
        "registry",
        "mask_hash",
        "source_snapshot",
        "source_sequence",
        "source_business_generation",
        "activation_sequence",
        "previous_sequence",
        "previous_business_generation",
        "owner_branch_id",
        "owner_head_commit_id",
        "directory_heads",
    ] {
        let mut forged = next.clone();
        forged
            .operations
            .get_mut(&result.operation_id)
            .unwrap()
            .result["namespace_backup"][key] = json!("forged");
        rejected(&view, &previous, &forged, key);
    }
    for key in [
        "policy",
        "scope",
        "file_count",
        "version_count",
        "target_copies",
        "backup_prefixes",
        "backup_request",
        "generated_policy_ids",
        "version_protection",
    ] {
        let mut forged = next.clone();
        forged
            .operations
            .get_mut(&result.operation_id)
            .unwrap()
            .result[key] = json!("forged");
        rejected(&view, &previous, &forged, key);
    }
    let mut forged = next.clone();
    forged
        .operation_principals
        .insert(result.operation_id, "different actor".into());
    rejected(&view, &previous, &forged, "actor");
    let mut forged = next.clone();
    forged
        .operations
        .get_mut(&result.operation_id)
        .unwrap()
        .request_digest = "0".repeat(64);
    rejected(&view, &previous, &forged, "digest");
    for remove in [false, true] {
        let mut forged = next.clone();
        if remove {
            forged.operations.remove(&result.operation_id);
        } else {
            let receipt = forged.operations.get_mut(&result.operation_id).unwrap();
            receipt.kind = "auth.update".into();
            receipt.result = json!({});
        }
        rejected(
            &view,
            &previous,
            &forged,
            "missing or disguised native receipt",
        );
    }
    // A later ordinary state may not rewrite either half of a permanent receipt.
    let mut later = next.clone();
    later.begin_tracking();
    later
        .operation_principals
        .insert(result.operation_id, "new owner".into());
    rejected(&view, next, &later, "permanent actor");
    let mut later = next.clone();
    later.begin_tracking();
    later
        .operations
        .get_mut(&result.operation_id)
        .unwrap()
        .result["file_count"] = json!("9");
    rejected(&view, next, &later, "permanent result");
}

#[test]
fn every_state_field_rejects_an_unplanned_change_in_live_and_cold_backup_publication() {
    let (fixture, view, previous, result) = published();
    let next = fixture.engine.state.as_ref();
    let file = &fixture.first.file_id;
    let version = &fixture.first.version_id;
    let branch = &fixture.repository.main_branch_id;
    let history = &fixture.repository.history_tree_id;
    let directory = DirectoryBranch {
        directory_branch_id: "unexpected".into(),
        repository_id: fixture.repository.repository_id.clone(),
        directory_id: "unexpected".into(),
        path: "docs".into(),
        name: "unexpected".into(),
        parent_branch_id: None,
        head_tree_id: "unexpected".into(),
        commit_id: next.branches[branch].head_commit_id.clone(),
        entries: vec![],
        file_branches: BTreeMap::new(),
        file_contexts: BTreeMap::new(),
        inherited_files: BTreeMap::new(),
        directory_refs: BTreeMap::new(),
        materialized: false,
        sequence: next.sequence,
    };
    let tree = DirectoryTree {
        tree_id: "unexpected".into(),
        directory_branch_id: "unexpected".into(),
        parent_tree_id: None,
        commit_id: directory.commit_id.clone(),
        path: "docs".into(),
        entries: vec![],
        file_branches: BTreeMap::new(),
        file_contexts: BTreeMap::new(),
        inherited_files: BTreeMap::new(),
        directory_refs: BTreeMap::new(),
        sequence: next.sequence,
    };
    macro_rules! reject_fields {
        ($state:ident; $($field:ident => $change:block),+ $(,)?) => {{
            // Exhaustive, so a new State field requires an explicit attack case.
            let State { $($field: _,)+ } = next;
            $( {
                let mut $state = next.clone();
                $change
                rejected(&view, &previous, &$state, stringify!($field));
            } )+
        }};
    }
    reject_fields!(state;
        sequence => {
            state.sequence += 1;
        },
        business_generation => {
            state.business_generation += 1;
        },
        topology_generation => {
            state.topology_generation += 1;
        },
        control_auth => {
            state.control_auth = Some(json!({"unexpected":"control mutation"}));
        },
        repositories => {
            state
                .repositories
                .get_mut(&fixture.repository.repository_id)
                .unwrap()
                .name
                .push_str(" changed");
        },
        histories => {
            state.histories.get_mut(history).unwrap().generation += 1;
        },
        branches => {
            state
                .branches
                .get_mut(branch)
                .unwrap()
                .name
                .push_str(" changed");
        },
        commits => {
            state
                .commits
                .get_mut(&next.branches[branch].head_commit_id)
                .unwrap()
                .message
                .push_str(" changed");
        },
        versions => {
            state
                .versions
                .get_mut(version)
                .unwrap()
                .creation_name
                .push_str(" changed");
        },
        directory_git_repositories => {
            state.directory_git_repositories.insert("unexpected".into(), storage_contract::directory_git::DirectoryGitRepository {
                repository_id: "unexpected".into(),
                directory_id: "unexpected".into(),
                initialized_sequence: state.sequence,
                current_branch_id: "unexpected".into(),
                branches: BTreeMap::new(),
            });
        },
        directory_branches => {
            state
                .directory_branches
                .insert("unexpected".into(), directory.clone());
        },
        directory_trees => {
            state
                .directory_trees
                .insert("unexpected".into(), tree.clone());
        },
        file_selections => {
            state
                .file_selections
                .insert("unexpected".into(), branch.clone());
        },
        file_write_origins => {
            state
                .file_write_origins
                .insert("unexpected".into(), branch.clone());
        },
        file_contexts => {
            state.file_contexts.insert(
                "unexpected".into(),
                FileContext {
                    node_id: None,
                    selection_generation: 1,
                    branch_id: branch.clone(),
                    inherited_from: None,
                },
            );
        },
        file_path_nodes => {
            state.file_path_nodes.insert(
                "unexpected".into(),
                FilePathNode {
                    node_id: "unexpected".into(),
                    history_tree_id: history.clone(),
                    file_id: file.clone(),
                    version_id: version.clone(),
                    parent_node_ids: vec![],
                    branch_id: branch.clone(),
                    path: "docs/report".into(),
                    previous_path: "report".into(),
                    operation_id: op(),
                    author: "admin".into(),
                    sequence: next.sequence,
                    created_at: 1,
                },
            );
        },
        file_metadata_nodes => {
            state.file_metadata_nodes.insert(
                "unexpected".into(),
                FileMetadataNode {
                    node_id: "unexpected".into(),
                    history_tree_id: history.clone(),
                    file_id: file.clone(),
                    version_id: version.clone(),
                    parent_node_ids: vec![],
                    branch_id: branch.clone(),
                    kind: FileMetadataKind::Tags,
                    tags: BTreeSet::from(["tag".into()]),
                    operation_id: op(),
                    author: "admin".into(),
                    sequence: next.sequence,
                    created_at: 1,
                },
            );
        },
        file_branch_merges => {
            state.file_branch_merges.insert(
                "unexpected".into(),
                FileBranchMerge {
                    merge_id: "unexpected".into(),
                    history_tree_id: history.clone(),
                    file_id: file.clone(),
                    input_branch_ids: vec![branch.clone()],
                    input_node_ids: vec![version.clone()],
                    chosen_node_id: version.clone(),
                    result_node_id: version.clone(),
                    surviving_branch_id: branch.clone(),
                    reused_node: true,
                    parent_node_ids: vec![],
                    operation_id: op(),
                    sequence: next.sequence,
                },
            );
        },
        file_branch_aliases => {
            state
                .file_branch_aliases
                .insert("unexpected".into(), branch.clone());
        },
        file_node_parents => {
            state
                .file_node_parents
                .insert("unexpected".into(), vec![version.clone()]);
        },
        file_path_notes => {
            state
                .file_path_notes
                .insert("unexpected".into(), "not a configuration".into());
        },
        directory_tree_selections => {
            state
                .directory_tree_selections
                .insert("unexpected".into(), "tree".into());
        },
        file_locations => {
            state
                .file_locations
                .insert(file.clone(), "physical/relocation".into());
        },
        file_names => {
            state
                .file_names
                .insert(file.clone(), "physical-renaming".into());
        },
        file_backup_paths => {
            state.file_backup_paths.insert(
                "unexpected".into(),
                BTreeMap::from([("B01".into(), "foreign".into())]),
            );
        },
        file_backup_policies => {
            state.file_backup_policies.insert(
                "unexpected".into(),
                result.result["policy"]["policy_id"]
                    .as_str()
                    .unwrap()
                    .into(),
            );
        },
        directory_selections => {
            state
                .directory_selections
                .insert("unexpected".into(), "branch".into());
        },
        directory_selection_generations => {
            state
                .directory_selection_generations
                .insert("unexpected".into(), 1);
        },
        file_selection_generations => {
            state
                .file_selection_generations
                .insert("unexpected".into(), 1);
        },
        file_notes => {
            state
                .file_notes
                .insert(file.clone(), "unrequested note".into());
        },
        version_notes => {
            state
                .version_notes
                .insert(version.clone(), "unrequested note".into());
        },
        placements => {
            state
                .placements
                .get_mut(version)
                .unwrap()
                .route_slot_ids
                .push("D04".into());
        },
        merge_links => {
            state.merge_links.insert(
                "unexpected".into(),
                MergeLink {
                    merge_link_id: "unexpected".into(),
                    history_tree_id: history.clone(),
                    source_branch_id: branch.clone(),
                    source_commit_id: next.branches[branch].head_commit_id.clone(),
                    target_branch_id: branch.clone(),
                    target_commit_id: next.branches[branch].head_commit_id.clone(),
                    author: "admin".into(),
                    operation_id: op(),
                },
            );
        },
        history_boundaries => {
            state.history_boundaries.insert(
                "unexpected".into(),
                HistoryBoundary {
                    boundary_id: "unexpected".into(),
                    history_tree_id: history.clone(),
                    local_commit_id: next.branches[branch].head_commit_id.clone(),
                    local_branch_id: Some(branch.clone()),
                    relation: "unexpected".into(),
                    external_history_tree_id: "elsewhere".into(),
                    external_commit_id: "elsewhere".into(),
                    source_merge_link_id: None,
                },
            );
        },
        content_states => {
            state
                .content_states
                .insert(version.clone(), ContentState::Purged);
        },
        operations => {
            let mut extra = result.clone();
            extra.operation_id = op();
            extra.kind = "auth.update".into();
            extra.result = json!({});
            state.operations.insert(extra.operation_id, extra);
        },
        operation_principals => {
            state.operation_principals.insert(op(), "unrelated".into());
        },
        plans => {
            state.plans.insert(
                "unexpected".into(),
                Plan {
                    plan_id: "unexpected".into(),
                    kind: "unexpected".into(),
                    principal: "admin".into(),
                    expected_sequence: next.sequence,
                    digest: "0".repeat(64),
                    input: json!({}),
                    state: PlanState::Ready,
                },
            );
        },
        placement_rules => {
            state.placement_rules.insert(
                "unexpected".into(),
                PlacementRule {
                    rule_id: "unexpected".into(),
                    repository_id: fixture.repository.repository_id.clone(),
                    path_prefix: "docs".into(),
                    primary_slot_id: "D04".into(),
                    generation: 1,
                },
            );
        },
        protection_policies => {
            let policy = state.protection_policies.values().next().unwrap().clone();
            state
                .protection_policies
                .insert("unexpected".into(), policy);
        },
        events => {
            state.events[0].kind.push_str(" changed");
        },
    );
}

#[test]
fn cold_missing_physical_inputs_cannot_be_rebuilt_by_backup_configuration() {
    let (fixture, view, previous, _) = published();
    for names in [true, false] {
        let mut previous: State = decode_strict(&canonical_json(&*previous).unwrap()).unwrap();
        if names {
            previous.file_names.remove(&fixture.first.file_id);
        } else {
            previous.file_locations.remove(&fixture.first.file_id);
        }
        rejected(
            &view,
            &previous,
            &fixture.engine.state,
            "physical owner repair",
        );
    }
}

#[test]
fn wrong_scope_generation_prefix_and_new_operation_leave_the_published_state_unchanged() {
    let (mut fixture, _, _, _) = published();
    let before = fixture.engine.state.clone();
    for (scope, generation, prefixes) in [
        (Some("different stable identity"), 1, prefixes()),
        (None, 0, prefixes()),
        (None, 1, BTreeMap::new()),
        (
            None,
            1,
            BTreeMap::from([("P01".into(), "primary-is-not-backup".into())]),
        ),
    ] {
        assert!(
            fixture
                .engine
                .configure_backups_scoped(ScopedBackupConfigurationRequest {
                    repository_id: &fixture.repository.repository_id,
                    path: "docs",
                    expected_scope_id: scope,
                    target_copies: 2,
                    backup_prefixes: &prefixes,
                    expected_generation: generation,
                    principal: "admin",
                    operation_id: op(),
                })
                .is_err()
        );
        assert_eq!(*fixture.engine.state, *before);
    }
}

#[test]
fn backup_scope_uses_current_directory_views_without_importing_hidden_historical_files() {
    let mut fixture = Fixture::new();
    let rule = new_id("rule").unwrap();
    fixture
        .engine
        .update_placement_rule(PlacementRuleUpdate {
            rule_id: &rule,
            repository_id: &fixture.repository.repository_id,
            path_prefix: "docs/gone",
            primary_slot_id: "D04",
            expected_generation: 0,
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    let hidden = save(
        &mut fixture.engine,
        &fixture.repository,
        "docs/gone",
        b"exclusive stopped file",
    );
    crate::directory_git_fixture::checkpoint(&mut fixture.engine, &fixture.repository, "docs");
    let directory = fixture
        .engine
        .create_directory_branches(
            &fixture.repository.repository_id,
            Some(&name_policy::LookupPath::parse("docs").unwrap()),
            &BranchName::parse("retained named view").unwrap(),
            "admin",
            op(),
        )
        .unwrap()
        .result["directory_branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    fixture
        .engine
        .update_placement_rule(PlacementRuleUpdate {
            rule_id: &rule,
            repository_id: &fixture.repository.repository_id,
            path_prefix: "docs/gone",
            primary_slot_id: "P01",
            expected_generation: 1,
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    fixture.exit(3, MembershipExitAction::Deregister);
    assert!(
        fixture.engine.state.directory_branches[&directory]
            .entries
            .iter()
            .any(|entry| entry.file_id == hidden.file_id)
    );
    assert!(
        !fixture
            .engine
            .current_directory_entries(&directory)
            .unwrap()
            .iter()
            .any(|entry| entry.file_id == hidden.file_id)
    );
    let before = fixture.engine.state.clone();
    let result = fixture.configure(0, 2, &prefixes(), op()).unwrap();
    assert_eq!(result.result["file_count"], "1");
    assert_eq!(result.result["version_count"], "2");
    assert_eq!(
        fixture.engine.state.placements[&hidden.version_id],
        before.placements[&hidden.version_id]
    );
    assert_eq!(
        fixture.engine.state.file_backup_paths.get(&hidden.file_id),
        before.file_backup_paths.get(&hidden.file_id)
    );
    assert_eq!(fixture.engine.state.directory_trees, before.directory_trees);
    assert_eq!(fixture.engine.state.commits, before.commits);
}

fn planned_backup(fixture: &Fixture, operation: OperationId) -> crate::engine::MutationPlan {
    let prefixes = prefixes();
    let repository = &fixture.repository.repository_id;
    fixture
        .engine
        .plan_mutation_digest(
            KIND,
            "admin",
            operation,
            json!({"repository_id":repository,"path":"docs","expected_scope_id":null,
            "target_copies":2,"backup_prefixes":prefixes,"expected_generation":"0"}),
            None,
            |engine, next, prepared| {
                let view = engine.admitted_current_namespaces()?.unwrap();
                let plan = view.plan_backup_configuration(
                    engine.state(),
                    ScopedBackupConfigurationRequest {
                        repository_id: repository,
                        path: "docs",
                        expected_scope_id: None,
                        target_copies: 2,
                        backup_prefixes: &prefixes,
                        expected_generation: 0,
                        principal: "admin",
                        operation_id: operation,
                    },
                    || Ok(new_id("policy")?),
                )?;
                for version in &plan.versions {
                    prepared.push(Prepared {
                        version: version.clone(),
                        placement: plan.placements[&version.version_id].clone(),
                        source: engine.valid_backup_replica(&version.version_id)?,
                    });
                }
                for (id, policy) in plan.policies {
                    match policy {
                        Some(policy) => {
                            next.protection_policies.insert(id, policy);
                        }
                        None => {
                            next.protection_policies.remove(&id);
                        }
                    }
                }
                next.file_backup_policies.extend(plan.bindings);
                next.file_backup_paths.extend(plan.paths);
                next.placements.extend(plan.placements);
                let mut result = plan.result;
                result["configured_at"] = json!(crate::business::now()?.to_string());
                Ok(result)
            },
        )
        .unwrap()
}

fn copy_prepared(copies: &[Prepared]) -> Vec<Prepared> {
    copies
        .iter()
        .map(|copy| Prepared {
            version: copy.version.clone(),
            placement: copy.placement.clone(),
            source: copy.source.clone(),
        })
        .collect()
}

#[test]
fn backup_requires_exact_prepared_bodies_and_opaque_source_before_any_publication_io() {
    use crate::engine::MutationPlan;
    let mut fixture = Fixture::new();
    fixture.exit(3, MembershipExitAction::Deregister);
    let view = fixture.engine.current_namespaces.as_ref().unwrap();
    assert_eq!(
        view.require_operation(KIND).unwrap_err().code(),
        ErrorCode::RecoveryRequired
    );
    let MutationPlan::Prepared {
        next,
        prepared,
        result,
    } = planned_backup(&fixture, op())
    else {
        panic!("fresh backup must prepare its exact native delta");
    };
    assert_eq!(prepared.len(), 2);
    view.verify_new_backup_configuration(fixture.engine.state(), &next, &result)
        .unwrap();
    view.verify_backup_prepared(fixture.engine.state(), &next, &result, &prepared)
        .unwrap();
    let generic = fixture.engine.planning_context().plan_new_mutation(
        KIND,
        "admin",
        op(),
        result.request_digest.clone(),
        |_, _, _| panic!("generic kind permit must not invoke backup preparation"),
    );
    assert!(generic.is_err());
    let watched = crate::disk::version_dir(
        &fixture.config.disks[0],
        &fixture.repository.history_tree_id,
        &fixture.first.version_id,
    );
    for attack in 0..9 {
        let mut copies = copy_prepared(&prepared);
        let mut supplied = result.clone();
        match attack {
            0 => {
                copies.pop();
            }
            1 => {
                copies.extend(copy_prepared(&prepared[..1]));
            }
            2 => copies.swap(0, 1),
            3 => copies[0].version.logical_size += 1,
            4 => copies[0].placement.route_slot_ids.reverse(),
            5 => {
                copies[0].source =
                    ReadSource::external(fixture.temp.path().join("forbidden-source"))
            }
            6 => {
                let ReadSource::VerifiedReplica(source) = &copies[0].source else {
                    panic!("verified source");
                };
                copies[0].source = ReadSource::on_disk(
                    source.owner(),
                    source
                        .owner()
                        .root
                        .path()
                        .join("forbidden-same-owner-source"),
                );
            }
            7 => copies[0].source = copies[1].source.clone(),
            8 => {
                supplied.kind = "auth.update".into();
                supplied.result = json!({});
            }
            _ => unreachable!(),
        }
        let before = fixture
            .config
            .disks
            .iter()
            .map(|disk| files(&disk.root))
            .collect::<Vec<_>>();
        let watch = watch_backup_payload(&watched);
        let failure = fixture.engine.prepare_publication(
            next.as_ref().clone(),
            copies,
            supplied,
            MAX_METADATA,
        );
        assert_no_backup_payload_io(watch);
        let error = failure
            .err()
            .expect("unproven backup must not leave publication preparation");
        assert_eq!(
            error.code(),
            ErrorCode::PublicationInvalid,
            "attack {attack}"
        );
        for (disk, expected) in fixture.config.disks.iter().zip(before) {
            assert_eq!(files(&disk.root), expected, "attack {attack}");
        }
    }
    // A successful real publication still has unchanged immutable identities,
    // independent destination bytes, and the existing decision-bound checks.
    fixture
        .engine
        .persist(*next, prepared, result.clone())
        .unwrap();
    assert_eq!(
        fixture.engine.operation(result.operation_id).unwrap(),
        result
    );
}

#[test]
fn later_backup_receipt_cannot_have_its_event_changed_or_appended_independently() {
    let (fixture, view, _, result) = published();
    let previous = fixture.engine.state.as_ref();
    for append in [false, true] {
        let mut next = previous.clone();
        next.begin_tracking();
        if append {
            next.events.push(Event {
                sequence: next.sequence,
                operation_id: result.operation_id,
                kind: KIND.into(),
                object_id: result.history_tree_id.clone().unwrap(),
            });
        } else {
            next.events
                .iter_mut()
                .find(|event| event.operation_id == result.operation_id)
                .unwrap()
                .kind = "auth.update".into();
        }
        rejected(&view, previous, &next, "independent permanent event");
    }
}

fn watch_backup_payload(path: &Path) -> rustix::fd::OwnedFd {
    use rustix::fs::inotify;
    let watch =
        inotify::init(inotify::CreateFlags::CLOEXEC | inotify::CreateFlags::NONBLOCK).unwrap();
    inotify::add_watch(
        &watch,
        path,
        inotify::WatchFlags::OPEN | inotify::WatchFlags::ACCESS,
    )
    .unwrap();
    watch
}
fn assert_no_backup_payload_io(watch: rustix::fd::OwnedFd) {
    let mut events = [0u8; 4096];
    assert_eq!(
        rustix::io::read(&watch, &mut events),
        Err(rustix::io::Errno::WOULDBLOCK),
        "rejected backup must not inspect its candidate's actual body/proof directory"
    );
}

fn install_backup_decision(
    engine: &mut Engine,
    next: &State,
    result: &OperationResult,
    published: bool,
    versions: Vec<crate::journal::VersionEvidence>,
) -> crate::journal::Candidate {
    use crate::journal::*;

    let members = engine.members();
    let snapshot = prepare_snapshot(next, MAX_METADATA).unwrap();
    snapshot
        .store_new_objects_at(&engine.disk_resources, &members)
        .unwrap();
    let catalog = crate::history_package::store_history_manifests_bounded_at(
        &engine.disk_resources,
        &members,
        next,
        MAX_METADATA,
    )
    .unwrap();
    let generation = engine.config.generation().unwrap();
    let candidate = Candidate::new_with_registry(
        CandidateData {
            pool_id: engine.config.pool_id.clone(),
            sequence: next.sequence,
            transaction_id: new_id("tx").unwrap(),
            operation_id: result.operation_id,
            request_digest: result.request_digest.clone(),
            previous_snapshot: engine.snapshot.clone(),
            snapshot_hash: snapshot.hash.clone(),
            versions,
            metadata_disk_ids: members.iter().map(|disk| disk.disk_id.clone()).collect(),
            history_roots: catalog.reference().clone(),
        },
        engine.config.generation_hash().unwrap(),
        generation.portable.exit_registry().unwrap(),
    );
    engine.verify_candidate_membership(&candidate).unwrap();
    let hash = object_hash("candidate", &candidate).unwrap();
    for disk in &members {
        store_object_at(
            &engine.disk_resources.get(disk).unwrap().root,
            disk,
            "candidate",
            &candidate,
        )
        .unwrap();
        engine
            .append_journal(
                disk,
                RecordType::Prepare,
                next.sequence,
                &hash,
                &candidate.snapshot_hash,
            )
            .unwrap();
    }
    let accepts = members
        .iter()
        .map(|disk| {
            engine
                .append_journal(disk, RecordType::Accept, next.sequence, &hash, &hash)
                .unwrap()
        })
        .collect();
    if published {
        let finalize = Finalize {
            candidate_hash: hash.clone(),
            accepts,
            recovery_id: None,
        };
        let finalize_hash = object_hash("finalize", &finalize).unwrap();
        for disk in &members {
            store_object_at(
                &engine.disk_resources.get(disk).unwrap().root,
                disk,
                "finalize",
                &finalize,
            )
            .unwrap();
        }
        let finalize_receipts = members
            .iter()
            .map(|disk| {
                engine
                    .append_journal(
                        disk,
                        RecordType::Finalize,
                        next.sequence,
                        &hash,
                        &finalize_hash,
                    )
                    .unwrap()
            })
            .collect();
        let publication = Publication {
            candidate_hash: hash.clone(),
            finalize: finalize.clone(),
            finalize_receipts,
            payload_receipts: candidate
                .versions
                .iter()
                .flat_map(|evidence| {
                    evidence
                        .placement
                        .route_slot_ids
                        .iter()
                        .map(|slot| {
                            let disk = engine.disk(slot).unwrap();
                            let proof = CommitProof {
                                candidate_hash: hash.clone(),
                                finalize: finalize.clone(),
                                version_hash: object_hash("version", &evidence.version).unwrap(),
                                disk_id: disk.disk_id.clone(),
                                slot_id: slot.clone(),
                                route_slot_ids: evidence.placement.route_slot_ids.clone(),
                            };
                            PayloadReceipt {
                                version_id: evidence.version.version_id.clone(),
                                disk_id: disk.disk_id.clone(),
                                slot_id: slot.clone(),
                                proof_hash: object_hash("commit-proof", &proof).unwrap(),
                            }
                        })
                        .collect::<Vec<_>>()
                })
                .collect(),
            result: result.clone(),
            recovery: None,
        };
        let publication_hash = object_hash("publication", &publication).unwrap();
        for disk in &members {
            store_object_at(
                &engine.disk_resources.get(disk).unwrap().root,
                disk,
                "publication",
                &publication,
            )
            .unwrap();
        }
        let receipts = members
            .iter()
            .map(|disk| {
                engine
                    .append_journal(
                        disk,
                        RecordType::Publication,
                        next.sequence,
                        &hash,
                        &publication_hash,
                    )
                    .unwrap()
            })
            .collect();
        let seal = PublicationSeal {
            candidate_hash: hash,
            publication_hash,
            receipts,
        };
        verify_publication_seal(&candidate, &publication, &seal).unwrap();
        for disk in &members {
            engine
                .disk_resources
                .get(disk)
                .unwrap()
                .root
                .write_json(
                    &publication_seal_path(disk, &seal.candidate_hash),
                    &seal,
                    false,
                )
                .unwrap();
        }
    }
    candidate
}

#[test]
fn cold_backup_binds_exact_copy_set_before_body_proof_discovery_and_repair() {
    use crate::engine::MutationPlan;
    use crate::journal::{VersionEvidence, object_path};
    for published in [false, true] {
        for attack in [
            "valid",
            "omitted",
            "extra",
            "duplicate",
            "placement",
            "source",
            "retagged",
            "erased-delta",
        ] {
            let mut fixture = Fixture::with_disk_count(5);
            let other_slot = fixture.config.disks[4].slot_id.clone();
            let other = fixture
                .engine
                .create_repository(
                    &RepositoryName::parse("unrelated retained backup source").unwrap(),
                    &other_slot,
                    1,
                    "admin",
                    op(),
                )
                .unwrap();
            let unrelated = save(
                &mut fixture.engine,
                &other,
                "outside-scope",
                b"unrelated actual bytes",
            );
            let extra = VersionEvidence {
                version: fixture.engine.state.versions[&unrelated.version_id].clone(),
                placement: fixture.engine.state.placements[&unrelated.version_id].clone(),
            };
            let stopped_bytes = fixture.exit(3, MembershipExitAction::Deregister);
            let parked = fixture.temp.path().join("retained-stopped");
            fs::rename(&fixture.config.disks[3].root, &parked).unwrap();
            fs::write(
                &fixture.config.disks[3].root,
                b"stopped root must not be opened",
            )
            .unwrap();
            let MutationPlan::Prepared {
                mut next,
                prepared,
                mut result,
            } = planned_backup(&fixture, op())
            else {
                panic!("new backup must prepare");
            };
            // Complete only the legitimate copy preparation. The fixture then
            // stores real fixed-member decision frames for each forged envelope.
            for copy in &prepared {
                fixture.engine.prepare_payload(copy).unwrap();
            }
            let mut versions = prepared
                .iter()
                .map(|copy| VersionEvidence {
                    version: copy.version.clone(),
                    placement: copy.placement.clone(),
                })
                .collect::<Vec<_>>();
            match attack {
                "valid" => (),
                "omitted" => {
                    versions.pop();
                }
                "extra" => versions.push(extra.clone()),
                "duplicate" => versions.push(versions[0].clone()),
                "placement" => {
                    versions[0].placement = Placement {
                        version_id: versions[0].version.version_id.clone(),
                        primary_slot_id: other_slot.clone(),
                        target_copies: 1,
                        route_slot_ids: vec![other_slot.clone()],
                        topology_generation: extra.placement.topology_generation,
                        binding_generations: extra.placement.binding_generations.clone(),
                    };
                }
                "source" => {
                    result.result["namespace_backup"]["owner_head_commit_id"] = json!("forged");
                    next.operations.insert(result.operation_id, result.clone());
                    versions.push(extra.clone());
                }
                "retagged" => {
                    result.kind = "auth.user.create".into();
                    result.result = json!({});
                    result.request_digest =
                        Engine::native_request_digest(&result.kind, "admin", &json!({})).unwrap();
                    next.operations.insert(result.operation_id, result.clone());
                    next.events.last_mut().unwrap().kind = result.kind.clone();
                    versions.push(extra.clone());
                }
                "erased-delta" => {
                    let sequence = next.sequence;
                    *next = fixture.engine.state.as_ref().clone();
                    next.begin_tracking();
                    next.sequence = sequence;
                    next.business_generation += 1;
                    result.kind = "auth.user.create".into();
                    result.result = json!({});
                    result.request_digest =
                        Engine::native_request_digest(&result.kind, "admin", &json!({})).unwrap();
                    next.operations.insert(result.operation_id, result.clone());
                    next.operation_principals
                        .insert(result.operation_id, "admin".into());
                    next.events.push(Event {
                        sequence,
                        operation_id: result.operation_id,
                        kind: result.kind.clone(),
                        object_id: result.history_tree_id.clone().unwrap(),
                    });
                    versions = vec![extra.clone()];
                }
                _ => unreachable!(),
            }
            let candidate =
                install_backup_decision(&mut fixture.engine, &next, &result, published, versions);
            let members = fixture.engine.members();
            let missing = object_path(&members[0], "snapshot", &candidate.snapshot_hash);
            let snapshot_bytes = fs::read(&missing).unwrap();
            let unrelated_proof = crate::disk::version_dir(
                &fixture.config.disks[4],
                &extra.version.history_tree_id,
                &extra.version.version_id,
            );
            assert!(!fixture.config.disks[4].metadata_member);
            // Positive calibration observes exactly the commit-proof child that
            // local_proof_directory_at would read for this live data-only body.
            let calibration = watch_backup_payload(&unrelated_proof);
            fs::read(unrelated_proof.join("commit-proof.json")).unwrap();
            let mut events = [0u8; 4096];
            assert!(rustix::io::read(&calibration, &mut events).unwrap() > 0);
            drop(calibration);
            let live_roots = fixture
                .engine
                .config
                .io_disks()
                .map(|disk| disk.root.clone())
                .collect::<Vec<_>>();
            let system = fixture.temp.path().join("system");
            drop(fixture.engine);
            fs::remove_file(&missing).unwrap();
            let live_bytes = live_roots
                .iter()
                .map(|root| files(root))
                .collect::<Vec<_>>();
            let system_bytes = files(&system);
            let watch = watch_backup_payload(&unrelated_proof);
            let opened = Engine::open(&system);
            assert_no_backup_payload_io(watch);
            if attack == "valid" {
                let mut engine = opened.unwrap();
                assert_eq!(fs::read(&missing).unwrap(), snapshot_bytes);
                assert_eq!(
                    engine
                        .configure_backups(BackupConfigurationRequest {
                            repository_id: &fixture.repository.repository_id,
                            path: "docs",
                            target_copies: 2,
                            backup_prefixes: &prefixes(),
                            expected_generation: 0,
                            principal: "admin",
                            operation_id: result.operation_id,
                        })
                        .unwrap(),
                    result
                );
                for entry in [&fixture.first, &fixture.latest] {
                    let version = &engine.state.versions[&entry.version_id];
                    let directory = crate::disk::version_dir(
                        &fixture.config.disks[1],
                        &version.history_tree_id,
                        &version.version_id,
                    );
                    crate::disk::verify_payload_held(&directory, version).unwrap();
                }
            } else {
                let error = opened
                    .err()
                    .expect("forged backup must fail before candidate repair");
                assert_eq!(
                    error.code(),
                    ErrorCode::PublicationInvalid,
                    "{published} {attack}: {error}"
                );
                assert_eq!(
                    fs::symlink_metadata(&missing).unwrap_err().kind(),
                    std::io::ErrorKind::NotFound
                );
                assert_eq!(files(&system), system_bytes, "{published} {attack}");
                for (root, expected) in live_roots.iter().zip(live_bytes) {
                    assert_eq!(files(root), expected, "{published} {attack}");
                }
            }
            assert_eq!(files(&parked), stopped_bytes);
            assert_eq!(
                fs::read(&fixture.config.disks[3].root).unwrap(),
                b"stopped root must not be opened"
            );
        }
    }
}

#[test]
fn backup_history_selection_never_resurrects_disposed_versions() {
    let mut fixture = Fixture::new();
    fixture.exit(3, MembershipExitAction::Deregister);
    let view = fixture.engine.current_namespaces.as_ref().unwrap();
    for disposed in [ContentState::Purged, ContentState::Migrated] {
        let mut source = fixture.engine.state.as_ref().clone();
        source
            .content_states
            .insert(fixture.first.version_id.clone(), disposed);
        let prefixes = prefixes();
        let plan = view
            .plan_backup_configuration(
                &source,
                ScopedBackupConfigurationRequest {
                    repository_id: &fixture.repository.repository_id,
                    path: "docs",
                    expected_scope_id: None,
                    target_copies: 2,
                    backup_prefixes: &prefixes,
                    expected_generation: 0,
                    principal: "admin",
                    operation_id: op(),
                },
                || Ok(new_id("policy")?),
            )
            .unwrap();
        assert_eq!(plan.versions.len(), 1);
        assert_eq!(plan.versions[0].version_id, fixture.latest.version_id);
        assert!(!plan.placements.contains_key(&fixture.first.version_id));
        assert_eq!(plan.result["version_count"], "1");
    }
}

#[test]
fn current_backup_degraded_recovery_stays_closed_before_local_body_observation() {
    use crate::engine::MutationPlan;
    let mut fixture = Fixture::new();
    fixture.exit(3, MembershipExitAction::Deregister);
    let MutationPlan::Prepared {
        next,
        prepared,
        result,
    } = planned_backup(&fixture, op())
    else {
        panic!("new backup must prepare");
    };
    let versions = prepared
        .iter()
        .map(|copy| crate::journal::VersionEvidence {
            version: copy.version.clone(),
            placement: copy.placement.clone(),
        })
        .collect();
    install_backup_decision(&mut fixture.engine, &next, &result, false, versions);
    let system = fixture.temp.path().join("system");
    let proof = crate::disk::version_dir(
        &fixture.config.disks[0],
        &fixture.repository.history_tree_id,
        &fixture.first.version_id,
    );
    drop(prepared);
    drop(fixture.engine);
    let mut engine = Engine::open_for_recovery(&system).unwrap();
    let before = fixture
        .config
        .disks
        .iter()
        .map(|disk| files(&disk.root))
        .collect::<Vec<_>>();
    let system_bytes = files(&system);
    let watch = watch_backup_payload(&proof);
    let error = engine
        .prepare_degraded_recovery(result.operation_id, "admin", "explicit unavailable replica")
        .unwrap_err();
    assert_no_backup_payload_io(watch);
    assert_eq!(error.code(), ErrorCode::RecoveryRequired);
    assert!(
        error
            .to_string()
            .contains("current-registry backup does not support degraded recovery"),
        "{error}"
    );
    assert_eq!(files(&system), system_bytes);
    for (disk, expected) in fixture.config.disks.iter().zip(before) {
        assert_eq!(files(&disk.root), expected);
    }
}

#[test]
fn current_backup_get_reports_a_stopped_downstream_and_allows_explicit_copy_reduction() {
    for action in [
        MembershipExitAction::PlannedOffline,
        MembershipExitAction::Deregister,
    ] {
        let mut fixture = Fixture::new();
        fixture.configure(0, 2, &prefixes(), op()).unwrap();
        let stopped = fixture.exit(1, action);
        let old_root = fixture.config.disks[1].root.clone();
        let parked = fixture.temp.path().join("stopped-backup-root");
        fs::rename(&old_root, &parked).unwrap();
        fs::write(&old_root, b"never reopen stopped backup").unwrap();
        let current = fixture
            .engine
            .backup_configuration(&fixture.repository.repository_id, "docs")
            .unwrap();
        assert_eq!(current.generation, 1);
        assert_eq!(current.target_copies, 2);
        assert_eq!(current.max_copies, 1);
        assert_eq!(current.available_route_slot_ids, ["P01"]);
        assert!(current.route_slot_ids.is_empty());
        assert!(fixture.configure(1, 2, &prefixes(), op()).is_err());
        let operation = op();
        let result = fixture
            .configure(1, 1, &BTreeMap::new(), operation)
            .unwrap();
        for entry in [&fixture.first, &fixture.latest] {
            assert_eq!(
                fixture.engine.state.placements[&entry.version_id].route_slot_ids,
                ["P01"]
            );
        }
        assert!(
            fixture
                .engine
                .disk_resources
                .available(&fixture.config.disks[1])
                .is_none()
        );
        let system = fixture.temp.path().join("system");
        drop(fixture.engine);
        let mut engine = Engine::open(&system).unwrap();
        let current = engine
            .backup_configuration(&fixture.repository.repository_id, "docs")
            .unwrap();
        assert_eq!(current.generation, 2);
        assert_eq!(current.target_copies, 1);
        assert_eq!(current.route_slot_ids, ["P01"]);
        assert_eq!(
            engine
                .configure_backups(BackupConfigurationRequest {
                    repository_id: &fixture.repository.repository_id,
                    path: "docs",
                    target_copies: 1,
                    backup_prefixes: &BTreeMap::new(),
                    expected_generation: 1,
                    principal: "admin",
                    operation_id: operation,
                })
                .unwrap(),
            result
        );
        assert_eq!(files(&parked), stopped);
        assert_eq!(fs::read(&old_root).unwrap(), b"never reopen stopped backup");
    }
}

#[test]
fn current_backup_survives_successor_metadata_exit_and_frozen_archive_replay() {
    let mut fixture = Fixture::new();
    fixture.exit(3, MembershipExitAction::PlannedOffline);
    let stopped_data = fixture.config.disks[3].root.clone();
    let parked_data = fixture.temp.path().join("parked-data-before-backup");
    fs::rename(&stopped_data, &parked_data).unwrap();
    fs::write(&stopped_data, b"stopped data path is not storage").unwrap();
    let original_data = files(&parked_data);

    let operation = op();
    let prefixes = prefixes();
    let result = fixture.configure(0, 2, &prefixes, operation).unwrap();
    assert_eq!(result.result["version_count"], "2");

    // C01 is outside this exact two-copy route, but is a real old metadata
    // voter. Its successor D3 freezes and replays the current backup itself.
    let original_metadata = fixture.exit(2, MembershipExitAction::Deregister);
    assert_eq!(fixture.engine.config.metadata_members().count(), 2);
    let stopped_metadata = fixture.config.disks[2].root.clone();
    let parked_metadata = fixture.temp.path().join("parked-metadata-after-backup");
    fs::rename(&stopped_metadata, &parked_metadata).unwrap();
    fs::write(&stopped_metadata, b"stopped metadata path is not storage").unwrap();

    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    let mut engine = Engine::open(&system).unwrap();
    assert_eq!(engine.config.metadata_members().count(), 2);
    assert_eq!(
        engine
            .configure_backups(BackupConfigurationRequest {
                repository_id: &fixture.repository.repository_id,
                path: "docs",
                target_copies: 2,
                backup_prefixes: &prefixes,
                expected_generation: 0,
                principal: "admin",
                operation_id: operation,
            })
            .unwrap(),
        result
    );
    for (entry, expected) in [
        (&fixture.first, b"first retained body".as_slice()),
        (&fixture.latest, b"latest retained body".as_slice()),
    ] {
        let mut content = engine.open_content(&entry.version_id).unwrap();
        let mut actual = Vec::new();
        content.file.read_to_end(&mut actual).unwrap();
        assert_eq!(actual, expected);
        assert_eq!(
            engine.state.placements[&entry.version_id].route_slot_ids,
            ["P01", "B01"]
        );
    }
    assert_eq!(files(&parked_data), original_data);
    assert_eq!(files(&parked_metadata), original_metadata);
    assert_eq!(
        fs::read(&stopped_data).unwrap(),
        b"stopped data path is not storage"
    );
    assert_eq!(
        fs::read(&stopped_metadata).unwrap(),
        b"stopped metadata path is not storage"
    );
}
