//! Real temporary Engine D3 publications followed by metadata-only archival.
//! No private synthetic registry installation or physical-media claim.
use super::*;
use crate::engine::MAX_METADATA;
use crate::*;
use std::{
    fs,
    path::{Path, PathBuf},
};
use storage_contract::{BranchName, RepositoryName};

fn op() -> OperationId {
    OperationId::generate().unwrap()
}

fn name(value: &str) -> BranchName {
    BranchName::parse(value).unwrap()
}

fn request(workspace: &Workspace) -> CommitRequest {
    CommitRequest {
        workspace_id: workspace.workspace_id.clone(),
        operation_id: op(),
        principal: workspace.principal.clone(),
        expected_head: workspace.base_head.clone(),
        fencing_token: workspace.fencing_token,
        message: "current branch archive fixture".into(),
        request_digest: None,
    }
}

fn save(engine: &mut Engine, branch: &str, path: &str, bytes: &[u8]) -> String {
    let workspace = engine.create_workspace(branch, "admin").unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, path, bytes)
        .unwrap();
    engine
        .commit_workspace(request(&workspace))
        .unwrap()
        .commit_id
        .unwrap()
}

fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, directory: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for child in fs::read_dir(directory).unwrap() {
            let path = child.unwrap().path();
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

struct Fixture {
    temp: tempfile::TempDir,
    engine: Engine,
    config: Config,
    repository: Repository,
    source: String,
    hidden: Entry,
    retained: Entry,
    branch: Branch,
    alias: Branch,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let mut config =
            Config::development((0..4).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
        config.disks[3].level = 1;
        config.disks[3].upstream_slot_id = None;
        config.disks[3].metadata_member = false;
        let mut engine = Engine::initialize(&temp.path().join("system"), config.clone()).unwrap();
        let repository = engine
            .create_repository(
                &RepositoryName::parse("branch archives").unwrap(),
                "D04",
                1,
                "admin",
                op(),
            )
            .unwrap();
        for path in ["dir/gone", "dir/live"] {
            engine
                .update_placement_rule(PlacementRuleUpdate {
                    rule_id: &new_id("rule").unwrap(),
                    repository_id: &repository.repository_id,
                    path_prefix: path,
                    primary_slot_id: "P01",
                    expected_generation: 0,
                    principal: "admin",
                    operation_id: op(),
                })
                .unwrap();
        }
        engine
            .update_protection_policy(ProtectionPolicyUpdate {
                policy_id: &new_id("policy").unwrap(),
                repository_id: &repository.repository_id,
                path_prefix: "dir/live",
                target_copies: 2,
                expected_generation: 0,
                principal: "admin",
                operation_id: op(),
            })
            .unwrap();
        save(
            &mut engine,
            &repository.main_branch_id,
            "dir/gone",
            b"detached exclusive body",
        );
        let source = save(
            &mut engine,
            &repository.main_branch_id,
            "dir/live",
            b"retained shared body",
        );
        let namespace = engine.namespace_tree(&repository.main_branch_id).unwrap();
        let hidden = namespace.get("dir/gone").unwrap().clone();
        let retained = namespace.get("dir/live").unwrap().clone();
        let branch = engine
            .create_branch(&source, &name("archive me"), "admin", op())
            .unwrap();
        let alias = engine
            .create_branch(&source, &name("keep shared alias"), "admin", op())
            .unwrap();
        Self {
            temp,
            engine,
            config,
            repository,
            source,
            hidden,
            retained,
            branch,
            alias,
        }
    }

    fn exit(&mut self, index: usize, action: MembershipExitAction) -> BTreeMap<PathBuf, Vec<u8>> {
        let disk = self.config.disks[index].clone();
        let resource = Arc::clone(self.engine.disk_resources.get(&disk).unwrap());
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
        resource.root.io_binding().unwrap().closed_proof().unwrap();
        assert!(self.engine.disk_resources.available(&disk).is_none());
        assert_eq!(files(&disk.root), before);
        before
    }

    fn park_root(&self, index: usize) -> PathBuf {
        let parked = self.temp.path().join(format!("parked-{index}"));
        fs::rename(&self.config.disks[index].root, &parked).unwrap();
        fs::write(
            &self.config.disks[index].root,
            b"stopped root must not be opened",
        )
        .unwrap();
        parked
    }

    fn archive(&mut self, operation: OperationId) -> Result<OperationResult> {
        self.engine
            .archive_branch(&self.branch.branch_id, "admin", operation)
    }
}

#[test]
fn multiple_d3_archival_preserves_projection_history_pending_and_permanent_results() {
    let mut fixture = Fixture::new();
    let held = fixture
        .engine
        .create_workspace(&fixture.repository.main_branch_id, "admin")
        .unwrap();
    fixture
        .engine
        .stage_bytes(&held.workspace_id, "held", b"keep staged bytes")
        .unwrap();
    let first = fixture.exit(0, MembershipExitAction::Deregister);
    let first_parked = fixture.park_root(0);
    let second = fixture.exit(2, MembershipExitAction::Deregister);
    let second_parked = fixture.park_root(2);
    let before = Arc::clone(&fixture.engine.state);
    let old_view = fixture.engine.current_namespaces.clone().unwrap();
    let operation = op();
    let result = fixture.archive(operation).unwrap();
    let after = &fixture.engine.state;
    let archived = &after.branches[&fixture.branch.branch_id];
    assert_eq!(archived.state, BranchState::Archived);
    assert_eq!(archived.generation, fixture.branch.generation + 1);
    assert_eq!(
        archived.pending_file_versions,
        fixture.branch.pending_file_versions
    );
    assert_eq!(archived.head_commit_id, fixture.branch.head_commit_id);
    assert_eq!(after.commits, before.commits);
    assert_eq!(after.versions, before.versions);
    assert_eq!(after.placements, before.placements);
    assert_eq!(after.file_branch_aliases, before.file_branch_aliases);
    assert_eq!(after.file_contexts, before.file_contexts);
    assert_eq!(after.branches[&fixture.alias.branch_id], fixture.alias);
    let new_view = fixture.engine.current_namespaces.as_ref().unwrap();
    assert!(Arc::ptr_eq(&old_view.branches, &new_view.branches));
    assert!(Arc::ptr_eq(&old_view.directories, &new_view.directories));
    assert!(
        fixture
            .engine
            .namespace_tree(&fixture.branch.branch_id)
            .unwrap()
            .get("dir/gone")
            .is_none()
    );
    assert_eq!(
        fixture
            .engine
            .pending_file_source(&fixture.branch.branch_id, &fixture.retained.file_id),
        Some(fixture.retained.version_id.as_str())
    );
    assert!(
        fixture
            .engine
            .namespace_tree_at(&fixture.branch.branch_id, &fixture.source)
            .unwrap()
            .get("dir/gone")
            .is_some()
    );
    assert_eq!(
        fixture
            .engine
            .stage_bytes(&held.workspace_id, "blocked", b"not admitted")
            .unwrap_err()
            .code(),
        ErrorCode::WorkspaceChanged
    );
    assert_eq!(
        fixture
            .engine
            .commit_workspace(request(&held))
            .unwrap_err()
            .code(),
        ErrorCode::WorkspaceChanged
    );
    assert!(
        fixture
            .engine
            .workspace(&held.workspace_id)
            .unwrap()
            .files
            .values()
            .any(|file| file.path.as_str() == "held")
    );
    assert_eq!(
        fixture
            .engine
            .create_workspace(&fixture.branch.branch_id, "admin")
            .unwrap_err()
            .code(),
        ErrorCode::BranchArchived
    );
    let second_operation = op();
    let again = fixture.archive(second_operation).unwrap();
    assert_eq!(
        again.result["branch_generation"],
        json!((fixture.branch.generation + 2).to_string())
    );
    assert_eq!(fixture.archive(operation).unwrap(), result);
    assert_eq!(
        fixture
            .engine
            .archive_branch(&fixture.branch.branch_id, "another actor", operation)
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyKeyReused
    );
    assert_eq!(
        fixture
            .engine
            .archive_branch(&fixture.alias.branch_id, "admin", operation)
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyKeyReused
    );
    assert_eq!(files(&first_parked), first);
    assert_eq!(files(&second_parked), second);
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    let mut engine = Engine::open(&system).unwrap();
    assert_eq!(
        engine
            .archive_branch(&fixture.branch.branch_id, "admin", operation)
            .unwrap(),
        result
    );
    assert_eq!(
        engine
            .archive_branch(&fixture.branch.branch_id, "admin", second_operation)
            .unwrap(),
        again
    );
    assert!(
        engine
            .namespace_tree(&fixture.branch.branch_id)
            .unwrap()
            .get("dir/gone")
            .is_none()
    );
    assert_eq!(
        engine.pending_file_source(&fixture.branch.branch_id, &fixture.retained.file_id),
        Some(fixture.retained.version_id.as_str())
    );
    assert_eq!(
        engine
            .stage_bytes(&held.workspace_id, "blocked", b"still not admitted")
            .unwrap_err()
            .code(),
        ErrorCode::WorkspaceChanged
    );
    assert_eq!(
        engine.commit_workspace(request(&held)).unwrap_err().code(),
        ErrorCode::WorkspaceChanged
    );
    assert_eq!(files(&first_parked), first);
    assert_eq!(files(&second_parked), second);
}

#[test]
fn original_pre_registry_archive_receipt_survives_later_d3_and_cold_replay() {
    let mut fixture = Fixture::new();
    let original_operation = op();
    let original = fixture.archive(original_operation).unwrap();
    assert!(original.result.get("namespace_archive").is_none());
    fixture.exit(0, MembershipExitAction::Deregister);
    fixture.exit(2, MembershipExitAction::Deregister);
    assert_eq!(fixture.archive(original_operation).unwrap(), original);
    let current_operation = op();
    let current = fixture.archive(current_operation).unwrap();
    assert!(current.result.get("namespace_archive").is_some());
    assert_eq!(fixture.archive(original_operation).unwrap(), original);
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    let mut engine = Engine::open(&system).unwrap();
    assert_eq!(
        engine
            .archive_branch(&fixture.branch.branch_id, "admin", original_operation)
            .unwrap(),
        original
    );
    assert_eq!(
        engine
            .archive_branch(&fixture.branch.branch_id, "admin", current_operation)
            .unwrap(),
        current
    );
}

#[test]
fn archival_of_disabled_or_detached_owner_never_opens_its_body_root() {
    for action in [
        MembershipExitAction::PlannedOffline,
        MembershipExitAction::Deregister,
    ] {
        let mut fixture = Fixture::new();
        let stopped = fixture.exit(3, action);
        let parked = fixture.park_root(3);
        let before = fixture.engine.current_namespaces.clone().unwrap();
        assert!(!before.branches[&fixture.branch.branch_id].writable);
        let operation = op();
        let result = fixture.archive(operation).unwrap();
        let after = fixture.engine.current_namespaces.as_ref().unwrap();
        assert!(Arc::ptr_eq(&before.branches, &after.branches));
        assert!(!after.branches[&fixture.branch.branch_id].writable);
        assert_eq!(
            fixture
                .engine
                .create_workspace(&fixture.repository.main_branch_id, "admin")
                .unwrap_err()
                .code(),
            ErrorCode::BranchNotActive
        );
        assert_eq!(files(&parked), stopped);
        let system = fixture.temp.path().join("system");
        drop(fixture.engine);
        let mut engine = Engine::open(&system).unwrap();
        assert_eq!(
            engine
                .archive_branch(&fixture.branch.branch_id, "admin", operation)
                .unwrap(),
            result
        );
        assert!(
            !engine.current_namespaces.as_ref().unwrap().branches[&fixture.branch.branch_id]
                .writable
        );
        assert_eq!(files(&parked), stopped);
    }
}

#[test]
fn current_archival_keeps_main_actor_and_open_workspace_guards() {
    let mut fixture = Fixture::new();
    let held = fixture
        .engine
        .create_workspace(&fixture.branch.branch_id, "admin")
        .unwrap();
    fixture.exit(0, MembershipExitAction::Deregister);
    let before = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(
        fixture
            .engine
            .archive_branch(&fixture.repository.main_branch_id, "admin", op())
            .unwrap_err()
            .code(),
        ErrorCode::MainBranchRequired
    );
    assert_eq!(
        fixture.archive(op()).unwrap_err().code(),
        ErrorCode::WorkspaceBusy
    );
    assert_eq!(
        fixture
            .engine
            .archive_branch(&fixture.alias.branch_id, "", op())
            .unwrap_err()
            .code(),
        ErrorCode::Unauthenticated
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    fixture
        .engine
        .abort_workspace(&held.workspace_id, "admin")
        .unwrap();
    fixture.archive(op()).unwrap();
}

#[test]
fn current_archival_keeps_selected_guard_and_archives_unselected_file_branch() {
    let mut fixture = Fixture::new();
    let root = fixture
        .engine
        .create_disk_root("D04", "admin", op())
        .unwrap();
    save(
        &mut fixture.engine,
        &root.main_branch_id,
        "selected",
        b"live selected file",
    );
    let entry = fixture
        .engine
        .namespace_tree(&root.main_branch_id)
        .unwrap()
        .get("selected")
        .unwrap()
        .clone();
    let selected = fixture.engine.state.file_selections[&entry.file_id].clone();
    let pending = fixture
        .engine
        .create_file_branch(
            &entry.file_id,
            &entry.version_id,
            &name("archive pending file"),
            "admin",
            op(),
        )
        .unwrap();
    let branch = pending.branch_id.unwrap();
    let pending_before = fixture.engine.state.branches[&branch]
        .pending_file_versions
        .clone();
    fixture.exit(0, MembershipExitAction::Deregister);
    let before = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(
        fixture
            .engine
            .archive_branch(&selected, "admin", op())
            .unwrap_err()
            .code(),
        ErrorCode::FileBranchSelected
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    fixture
        .engine
        .archive_branch(&branch, "admin", op())
        .unwrap();
    assert_eq!(
        fixture.engine.state.branches[&branch].pending_file_versions,
        pending_before
    );
    assert_eq!(
        fixture.engine.state.file_selections[&entry.file_id],
        selected
    );
    assert_eq!(
        fixture.engine.pending_file_source(&branch, &entry.file_id),
        Some(entry.version_id.as_str())
    );
}

fn decoded(state: &State) -> State {
    json_codec::from_slice(&canonical_json(state).unwrap()).unwrap()
}

#[test]
fn archival_replay_rejects_changed_request_head_namespace_and_unrelated_writes() {
    let mut fixture = Fixture::new();
    fixture.exit(0, MembershipExitAction::Deregister);
    fixture.exit(2, MembershipExitAction::Deregister);
    let previous = Arc::clone(&fixture.engine.state);
    let view = fixture.engine.current_namespaces.clone().unwrap();
    let operation = op();
    fixture.archive(operation).unwrap();
    let published = Arc::clone(&fixture.engine.state);
    view.advance(&previous, &published).unwrap();
    let cold_previous = decoded(&previous);
    view.advance(&cold_previous, &decoded(&published)).unwrap();
    for attack in 0..40 {
        let mut forged = published.as_ref().clone();
        match attack {
            0 => {
                forged
                    .operation_principals
                    .insert(operation, "other".into());
            }
            1 => {
                forged
                    .operations
                    .get_mut(&operation)
                    .unwrap()
                    .request_digest = "f".repeat(64);
            }
            2 => {
                forged.operations.get_mut(&operation).unwrap().operation_id = op();
            }
            3 => {
                forged.operations.get_mut(&operation).unwrap().kind = "auth.user.create".into();
            }
            4 => {
                forged.operations.get_mut(&operation).unwrap().result["namespace_archive"]["registry"] =
                    Value::Null;
            }
            5 => {
                forged.operations.get_mut(&operation).unwrap().result["namespace_archive"]["head_commit_id"] =
                    json!(fixture.source);
            }
            6 => {
                forged
                    .branches
                    .get_mut(&fixture.branch.branch_id)
                    .unwrap()
                    .generation += 1;
            }
            7 => {
                forged
                    .branches
                    .get_mut(&fixture.branch.branch_id)
                    .unwrap()
                    .state = BranchState::Active;
            }
            8 => {
                forged
                    .branches
                    .get_mut(&fixture.branch.branch_id)
                    .unwrap()
                    .head_commit_id = fixture.source.clone();
            }
            9 => {
                forged
                    .commits
                    .get_mut(&fixture.branch.head_commit_id)
                    .unwrap()
                    .entries
                    .push(fixture.hidden.clone());
            }
            10 => {
                forged
                    .branches
                    .get_mut(&fixture.branch.branch_id)
                    .unwrap()
                    .pending_file_versions
                    .clear();
            }
            11 => {
                forged
                    .branches
                    .get_mut(&fixture.alias.branch_id)
                    .unwrap()
                    .state = BranchState::Archived;
            }
            12 => {
                forged
                    .versions
                    .get_mut(&fixture.retained.version_id)
                    .unwrap()
                    .logical_size += 1;
            }
            13 => {
                forged
                    .placements
                    .get_mut(&fixture.retained.version_id)
                    .unwrap()
                    .primary_slot_id = "D04".into();
            }
            14 => {
                forged.file_branch_aliases.insert(
                    fixture.branch.branch_id.clone(),
                    fixture.alias.branch_id.clone(),
                );
            }
            15 => {
                forged.file_write_origins.insert(
                    fixture.retained.file_id.clone(),
                    fixture.branch.branch_id.clone(),
                );
            }
            16 => {
                forged
                    .repositories
                    .get_mut(&fixture.repository.repository_id)
                    .unwrap()
                    .main_branch_id = fixture.branch.branch_id.clone();
            }
            17 => {
                forged
                    .operations
                    .get_mut(&previous.commits[&fixture.source].operation_id)
                    .unwrap()
                    .kind = "auth.user.create".into();
            }
            18 => {
                forged.events.last_mut().unwrap().operation_id = op();
            }
            19 => {
                forged.operations.get_mut(&operation).unwrap().result["namespace_archive"]["activation_sequence"] =
                    json!("0");
            }
            20 => {
                forged.operations.get_mut(&operation).unwrap().result["namespace_archive"]["mask_hash"] =
                    Value::Null;
            }
            21 => {
                forged
                    .file_locations
                    .insert(fixture.retained.file_id.clone(), "new-owner".into());
            }
            22 => {
                forged.branches = previous.branches.clone();
            }
            23 => {
                let receipt = forged.operations.get_mut(&operation).unwrap();
                receipt.kind = "auth.user.create".into();
                receipt
                    .result
                    .as_object_mut()
                    .unwrap()
                    .remove("namespace_archive");
            }
            24 => {
                forged.operations = previous.operations.clone();
            }
            25 => {
                forged.file_contexts.insert(
                    "forged-current-path".into(),
                    FileContext {
                        node_id: None,
                        selection_generation: 1,
                        branch_id: fixture.branch.branch_id.clone(),
                        inherited_from: None,
                    },
                );
            }
            26 => {
                forged
                    .operation_principals
                    .get_mut(&previous.commits[&fixture.source].operation_id)
                    .unwrap()
                    .push_str(" altered");
            }
            27 => {
                forged.operations.get_mut(&operation).unwrap().result["timestamp"] = json!("01");
            }
            28 => {
                forged.operations.get_mut(&operation).unwrap().result["membership_epoch"] =
                    json!("-1");
            }
            29 => {
                forged.operations.get_mut(&operation).unwrap().result["unrequested"] = json!(true);
            }
            30 => {
                forged.operations.get_mut(&operation).unwrap().result["namespace_archive"]["unexpected"] =
                    json!(true);
            }
            31 => {
                forged
                    .operations
                    .get_mut(&operation)
                    .unwrap()
                    .current_health = CurrentHealth::Unavailable;
            }
            32 => {
                forged.operations.get_mut(&operation).unwrap().commit_state =
                    CommitState::PublishedDegraded;
            }
            33 => {
                forged.operations.get_mut(&operation).unwrap().sequence += 1;
            }
            34 => {
                forged.operations.get_mut(&operation).unwrap().commit_id =
                    Some(fixture.source.clone());
            }
            35 => {
                forged.operations.get_mut(&operation).unwrap().result["version_protection"] =
                    json!([{}]);
            }
            36 => {
                forged.events[0].kind = "rewritten unrelated event".into();
            }
            37 => {
                forged.topology_generation += 1;
            }
            38 => {
                forged.business_generation += 1;
            }
            39 => {
                forged.control_auth = Some(json!({"unrequested": true}));
            }
            _ => unreachable!(),
        }
        assert!(
            view.advance(&previous, &forged).is_err(),
            "tracked attack {attack}"
        );
        assert!(
            view.advance(&cold_previous, &decoded(&forged)).is_err(),
            "cold attack {attack}"
        );
    }
}

#[test]
fn archive_write_set_never_fills_or_retargets_physical_names_and_locations() {
    let mut fixture = Fixture::new();
    fixture.exit(0, MembershipExitAction::Deregister);
    let view = fixture.engine.current_namespaces.clone().unwrap();
    let previous = fixture.engine.state.as_ref().clone();
    fixture.archive(op()).unwrap();
    let published = fixture.engine.state.as_ref();
    for field in ["name", "location"] {
        let mut incomplete = previous.clone();
        if field == "name" {
            incomplete.file_names.remove(&fixture.retained.file_id);
        } else {
            incomplete.file_locations.remove(&fixture.retained.file_id);
        }
        // Real cold replay loses tracking, not physical-owner values. An
        // incomplete prior State must not acquire them through archival.
        assert!(
            view.advance(&decoded(&incomplete), &decoded(published))
                .is_err()
        );
    }
    let mut forged = published.clone();
    forged
        .file_names
        .insert(fixture.retained.file_id.clone(), "not-the-main-name".into());
    assert!(view.advance(&previous, &forged).is_err());
    forged = published.clone();
    forged
        .file_locations
        .insert(fixture.retained.file_id.clone(), "another/alias".into());
    assert!(view.advance(&previous, &forged).is_err());
}

#[test]
fn archival_preserves_sealed_workspace_fence_and_staged_bytes() {
    let mut fixture = Fixture::new();
    let workspace = fixture
        .engine
        .create_workspace(&fixture.branch.branch_id, "admin")
        .unwrap();
    fixture
        .engine
        .stage_bytes(&workspace.workspace_id, "staged", b"retained staging")
        .unwrap();
    fixture
        .engine
        .seal_workspace(&workspace.workspace_id, "admin")
        .unwrap();
    fixture.exit(0, MembershipExitAction::Deregister);
    let before = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(
        fixture.archive(op()).unwrap_err().code(),
        ErrorCode::WorkspaceBusy
    );
    let sealed = fixture.engine.workspace(&workspace.workspace_id).unwrap();
    assert_eq!(sealed.state, WorkspaceState::Sealed);
    assert!(
        sealed
            .files
            .values()
            .any(|file| file.path.as_str() == "staged")
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    fixture
        .engine
        .abort_workspace(&workspace.workspace_id, "admin")
        .unwrap();
    fixture.archive(op()).unwrap();
}

#[test]
fn archival_requires_closed_planning_proof_instead_of_generic_kind_admission() {
    use crate::engine::MutationPlan;

    let mut fixture = Fixture::new();
    fixture.exit(0, MembershipExitAction::Deregister);
    let previous = Arc::clone(&fixture.engine.state);
    let view = fixture.engine.current_namespaces.clone().unwrap();
    assert_eq!(
        view.require_operation(KIND).unwrap_err().code(),
        ErrorCode::RecoveryRequired
    );
    let operation = op();
    let receipt = fixture.archive(operation).unwrap();
    let published = Arc::clone(&fixture.engine.state);
    let published_bytes = canonical_json(published.as_ref()).unwrap();
    let mut context = fixture.engine.planning_context();
    context.state = &previous;
    context.next_slot = published.sequence;
    let generic = context.plan_new_mutation(
        KIND,
        "admin",
        operation,
        receipt.request_digest.clone(),
        |_, _, _| panic!("general kind admission must reject before archival preparation"),
    );
    let Err(error) = generic else {
        panic!("archival must not become a generic allowed kind")
    };
    assert_eq!(error.code(), ErrorCode::RecoveryRequired);
    assert!(
        view.verify_new_branch_archive(&previous, &previous, &receipt)
            .is_err()
    );
    for attack in 0..6 {
        let digest = if attack == 1 {
            "f".repeat(64)
        } else {
            receipt.request_digest.clone()
        };
        let plan = context.plan_branch_archive_mutation(
            "admin",
            operation,
            digest,
            |_, next, prepared| {
                if attack != 5 {
                    next.branches.insert(
                        fixture.branch.branch_id.clone(),
                        published.branches[&fixture.branch.branch_id].clone(),
                    );
                }
                let mut result = receipt.result.clone();
                if attack == 2 {
                    result["namespace_archive"]["registry"] = Value::Null;
                }
                if attack == 3 {
                    next.file_notes
                        .insert(fixture.retained.file_id.clone(), "unrequested".into());
                }
                if attack == 4 {
                    prepared.push(crate::engine::Prepared {
                        version: previous.versions[&fixture.retained.version_id].clone(),
                        placement: previous.placements[&fixture.retained.version_id].clone(),
                        source: crate::engine::ReadSource::external(
                            fixture.temp.path().join("payload-must-not-open"),
                        ),
                    });
                }
                Ok(result)
            },
        );
        if attack == 0 {
            let Ok(MutationPlan::Prepared {
                next,
                prepared,
                result,
            }) = plan
            else {
                panic!("exact metadata-only archival must leave planning")
            };
            assert!(prepared.is_empty());
            view.verify_new_branch_archive(&previous, &next, &result)
                .unwrap();
            let advanced = view.advance(&previous, &next).unwrap();
            assert!(Arc::ptr_eq(&view.branches, &advanced.branches));
            assert!(Arc::ptr_eq(&view.directories, &advanced.directories));
        } else {
            let Err(error) = plan else {
                panic!("unproven archive candidate {attack} left planning")
            };
            assert_eq!(
                error.code(),
                ErrorCode::PublicationInvalid,
                "attack {attack}"
            );
        }
    }
    context.current_namespaces = None;
    let pending = context.plan_branch_archive_mutation(
        "admin",
        operation,
        receipt.request_digest,
        |_, _, _| panic!("pending registry must reject before archival preparation"),
    );
    let Err(error) = pending else {
        panic!("unadmitted registry must remain fenced")
    };
    assert_eq!(error.code(), ErrorCode::RecoveryRequired);
    assert!(Arc::ptr_eq(&published, &fixture.engine.state));
    assert_eq!(
        canonical_json(fixture.engine.state()).unwrap(),
        published_bytes
    );
}

#[test]
fn later_unchanged_head_cannot_rewrite_archive_actor_event_or_reactivate_branch() {
    let mut fixture = Fixture::new();
    fixture.exit(0, MembershipExitAction::Deregister);
    let operation = op();
    fixture.archive(operation).unwrap();
    let view = fixture.engine.current_namespaces.clone().unwrap();
    let previous = Arc::clone(&fixture.engine.state);
    let cold_previous = decoded(&previous);
    for attack in 0..8 {
        let mut next = previous.as_ref().clone();
        next.begin_tracking();
        match attack {
            0 => {
                next.operation_principals
                    .insert(operation, "replacement actor".into());
            }
            1 => {
                next.operation_principals.remove(&operation);
            }
            2 => {
                next.events.last_mut().unwrap().kind = "auth.fake".into();
            }
            3 => {
                next.events.last_mut().unwrap().object_id = "different history".into();
            }
            4 => {
                next.events.last_mut().unwrap().operation_id = op();
            }
            5 => {
                next.events.push(Event {
                    sequence: next.sequence + 1,
                    operation_id: operation,
                    kind: KIND.into(),
                    object_id: fixture.repository.history_tree_id.clone(),
                });
            }
            6 => {
                next.branches
                    .get_mut(&fixture.branch.branch_id)
                    .unwrap()
                    .state = BranchState::Active;
            }
            7 => {
                next.branches.remove(&fixture.branch.branch_id);
            }
            _ => unreachable!(),
        }
        assert!(
            next.operations
                .changes_since(&previous.operations)
                .is_empty()
        );
        if attack < 6 {
            assert!(next.branches.changes_since(&previous.branches).is_empty());
        }
        assert_eq!(
            view.advance(&previous, &next).err().unwrap().code(),
            ErrorCode::PublicationInvalid,
            "tracked {attack}"
        );
        assert_eq!(
            view.advance(&cold_previous, &decoded(&next))
                .err()
                .unwrap()
                .code(),
            ErrorCode::PublicationInvalid,
            "cold {attack}"
        );
    }
    assert!(Arc::ptr_eq(&previous, &fixture.engine.state));
}

#[test]
fn archive_source_rejects_mismatched_historical_identity_before_mutation() {
    let mut fixture = Fixture::new();
    fixture.exit(0, MembershipExitAction::Deregister);
    let view = fixture.engine.current_namespaces.clone().unwrap();
    let before = Arc::clone(&fixture.engine.state);
    for attack in 0..8 {
        let mut state = before.as_ref().clone();
        match attack {
            0 => {
                state
                    .branches
                    .get_mut(&fixture.branch.branch_id)
                    .unwrap()
                    .branch_id = fixture.alias.branch_id.clone();
            }
            1 => {
                state
                    .commits
                    .get_mut(&fixture.branch.head_commit_id)
                    .unwrap()
                    .commit_id = fixture.source.clone();
            }
            2 => {
                state
                    .commits
                    .get_mut(&fixture.branch.fork_commit_id)
                    .unwrap()
                    .history_tree_id = new_id("hist").unwrap();
            }
            3 => {
                state
                    .histories
                    .get_mut(&fixture.repository.history_tree_id)
                    .unwrap()
                    .history_tree_id = new_id("hist").unwrap();
            }
            4 => {
                state
                    .repositories
                    .get_mut(&fixture.repository.repository_id)
                    .unwrap()
                    .repository_id = new_id("repo").unwrap();
            }
            5 => {
                state
                    .repositories
                    .get_mut(&fixture.repository.repository_id)
                    .unwrap()
                    .history_tree_id = new_id("hist").unwrap();
            }
            6 => {
                state.commits.remove(&fixture.branch.fork_commit_id);
            }
            7 => {
                state
                    .branches
                    .get_mut(&fixture.branch.branch_id)
                    .unwrap()
                    .head_commit_id = fixture.source.clone();
            }
            _ => unreachable!(),
        }
        assert_eq!(
            view.branch_archive_source(&state, &fixture.branch.branch_id)
                .unwrap_err()
                .code(),
            ErrorCode::PublicationInvalid,
            "identity {attack}"
        );
    }
    assert!(Arc::ptr_eq(&before, &fixture.engine.state));
}

#[cfg(feature = "fault-injection")]
#[test]
fn decided_archive_recovers_once_without_unmasking_or_opening_stopped_roots() {
    for point in [
        "after_finalize",
        "after_container_names",
        "after_publication_1",
    ] {
        let mut fixture = Fixture::new();
        let target_bytes = fixture.exit(0, MembershipExitAction::Deregister);
        let parked = fixture.park_root(0);
        let previous = Arc::clone(&fixture.engine.state);
        let view = fixture.engine.current_namespaces.clone().unwrap();
        let operation = op();
        fixture.engine.inject_fault_once(point);
        let error = fixture.archive(operation).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InDoubt, "{point}");
        let Error::Context { source, .. } = error else {
            panic!("indeterminate archival must retain its cause")
        };
        assert_eq!(source.code(), ErrorCode::InjectedFailure);
        assert!(fixture.engine.is_read_only());
        assert_eq!(*fixture.engine.state, *previous);
        assert!(Arc::ptr_eq(
            &view.branches,
            &fixture.engine.current_namespaces.as_ref().unwrap().branches
        ));
        let system = fixture.temp.path().join("system");
        drop(fixture.engine);
        let mut engine = Engine::open(&system).unwrap();
        let published = Arc::clone(&engine.state);
        assert_eq!(
            engine.state.branches[&fixture.branch.branch_id].generation,
            fixture.branch.generation + 1
        );
        assert_eq!(
            engine.state.branches[&fixture.branch.branch_id].state,
            BranchState::Archived
        );
        let result = engine
            .archive_branch(&fixture.branch.branch_id, "admin", operation)
            .unwrap();
        assert_eq!(result.operation_id, operation);
        assert!(Arc::ptr_eq(&published, &engine.state));
        assert_eq!(published.commits, previous.commits);
        assert_eq!(published.versions, previous.versions);
        assert_eq!(published.placements, previous.placements);
        assert!(
            engine
                .namespace_tree(&fixture.branch.branch_id)
                .unwrap()
                .get("dir/gone")
                .is_none()
        );
        assert_eq!(files(&parked), target_bytes);
        assert_eq!(
            fs::read(&fixture.config.disks[0].root).unwrap(),
            b"stopped root must not be opened"
        );
        drop(engine);
        let mut engine = Engine::open(&system).unwrap();
        assert_eq!(
            engine
                .archive_branch(&fixture.branch.branch_id, "admin", operation)
                .unwrap(),
            result
        );
        assert_eq!(files(&parked), target_bytes);
    }
}

#[test]
fn archival_preserves_merged_alias_state_and_its_unselected_active_owner() {
    let mut fixture = Fixture::new();
    let root = fixture
        .engine
        .create_disk_root("D04", "admin", op())
        .unwrap();
    save(
        &mut fixture.engine,
        &root.main_branch_id,
        "shared",
        b"merged body",
    );
    let entry = fixture
        .engine
        .namespace_tree(&root.main_branch_id)
        .unwrap()
        .get("shared")
        .unwrap()
        .clone();
    let owner = fixture.engine.state.file_selections[&entry.file_id].clone();
    let mut create = |label: &str| {
        fixture
            .engine
            .create_file_branch(
                &entry.file_id,
                &entry.version_id,
                &name(label),
                "admin",
                op(),
            )
            .unwrap()
            .branch_id
            .unwrap()
    };
    let alias = create("merged historical name");
    let replacement = create("separate selected name");
    let inputs = [&owner, &alias].map(|branch| FileMergeInput {
        branch_id: branch.clone(),
        expected_node_id: entry.version_id.clone(),
    });
    fixture
        .engine
        .merge_file_branches(FileBranchMergeRequest {
            file_id: &entry.file_id,
            inputs: &inputs,
            chosen_node_id: &entry.version_id,
            surviving_branch_id: &owner,
            location: None,
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    fixture
        .engine
        .select_file_branch(&entry.file_id, &replacement, "admin", op())
        .unwrap();
    assert_eq!(fixture.engine.state.file_branch_aliases[&alias], owner);
    assert_eq!(
        fixture.engine.state.branches[&alias].state,
        BranchState::Merged
    );
    assert_eq!(
        fixture.engine.state.file_selections[&entry.file_id],
        replacement
    );
    fixture.exit(0, MembershipExitAction::Deregister);
    let before = canonical_json(fixture.engine.state()).unwrap();
    for branch in [&alias, &owner] {
        assert!(
            !fixture
                .engine
                .state
                .file_contexts
                .values()
                .any(|context| &context.branch_id == branch)
        );
        let operation = op();
        assert_eq!(
            fixture
                .engine
                .archive_branch(branch, "admin", operation)
                .unwrap_err()
                .code(),
            ErrorCode::PublicationInvalid
        );
        assert!(!fixture.engine.state.operations.contains_key(&operation));
        assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    }
    // The source history and its immutable alias edges are valid after rejection.
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    let engine = Engine::open(&system).unwrap();
    assert_eq!(engine.state.file_branch_aliases[&alias], owner);
    assert_eq!(engine.state.branches[&alias].state, BranchState::Merged);
    assert_eq!(engine.state.branches[&owner].state, BranchState::Active);
}

/// Persist a real hash-bound decision using test-owned journal writes. This
/// deliberately does not pass an invalid request through production planning.
/// Hashes, fixed members, history closure and publication receipts remain valid;
/// only the archive's metadata-only proof is varied by the caller.
fn install_archive_decision(
    engine: &mut Engine,
    next: &State,
    result: &OperationResult,
    published: bool,
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
            versions: Vec::new(),
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
            finalize,
            finalize_receipts,
            payload_receipts: Vec::new(),
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
fn cold_archive_rejects_rehashed_candidate_before_replica_repair_and_projection() {
    use crate::engine::MutationPlan;
    use crate::journal::object_path;

    for published in [false, true] {
        for attack in ["valid", "source", "location"] {
            let mut fixture = Fixture::new();
            let stopped_bytes = fixture.exit(0, MembershipExitAction::Deregister);
            let parked = fixture.park_root(0);
            let view = fixture.engine.current_namespaces.clone().unwrap();
            let before = Arc::clone(&fixture.engine.state);
            let operation = op();
            let branch = &fixture.branch.branch_id;
            let plan = fixture
                .engine
                .plan_mutation_digest(
                    KIND,
                    "admin",
                    operation,
                    json!({"branch_id": branch}),
                    None,
                    |_, next, _| {
                        let source = view.branch_archive_source(next, branch)?;
                        let branch = next.branches.get_mut(branch).unwrap();
                        branch.state = BranchState::Archived;
                        branch.generation += 1;
                        Ok(json!({
                            "branch_id": branch.branch_id,
                            "history_tree_id": branch.history_tree_id,
                            "namespace_archive": source,
                        }))
                    },
                )
                .unwrap();
            let MutationPlan::Prepared {
                mut next,
                prepared,
                mut result,
            } = plan
            else {
                panic!("fresh archive must prepare a native candidate")
            };
            assert!(prepared.is_empty());
            view.verify_new_branch_archive(&before, &next, &result)
                .unwrap();
            match attack {
                "valid" => (),
                "source" => {
                    result.result["namespace_archive"]["mask_hash"] = Value::Null;
                    next.operations.insert(operation, result.clone());
                }
                "location" => {
                    // Keep the real history name/path invariant valid. Only
                    // the archive's closed write set must reject this move.
                    let location =
                        format!("unrequested/{}", next.file_names[&fixture.retained.file_id]);
                    next.file_locations
                        .insert(fixture.retained.file_id.clone(), location);
                }
                _ => unreachable!(),
            }
            let candidate =
                install_archive_decision(&mut fixture.engine, &next, &result, published);
            let members = fixture.engine.members();
            let live_roots = fixture
                .engine
                .config
                .io_disks()
                .map(|disk| disk.root.clone())
                .collect::<Vec<_>>();
            let missing = object_path(&members[0], "snapshot", &candidate.snapshot_hash);
            let snapshot_bytes = fs::read(&missing).unwrap();
            let system = fixture.temp.path().join("system");
            drop(fixture.engine);
            // This candidate-only object can be repaired from another member.
            // A rejected proof must fail before that first observable repair.
            fs::remove_file(&missing).unwrap();
            let system_bytes = files(&system);
            let live_bytes = live_roots
                .iter()
                .map(|root| files(root))
                .collect::<Vec<_>>();
            let opened = Engine::open(&system);
            if attack == "valid" {
                let mut engine = opened.unwrap();
                assert_eq!(fs::read(&missing).unwrap(), snapshot_bytes);
                assert_eq!(engine.state.branches[branch].state, BranchState::Archived);
                assert_eq!(
                    engine.archive_branch(branch, "admin", operation).unwrap(),
                    result
                );
                assert!(
                    engine
                        .namespace_tree(branch)
                        .unwrap()
                        .get("dir/gone")
                        .is_none()
                );
            } else {
                let error = opened
                    .err()
                    .expect("cold replay must reject the archive proof");
                assert_eq!(
                    error.code(),
                    ErrorCode::PublicationInvalid,
                    "{published} {attack}"
                );
                assert_eq!(
                    fs::symlink_metadata(&missing).unwrap_err().kind(),
                    std::io::ErrorKind::NotFound
                );
                assert_eq!(
                    files(&system),
                    system_bytes,
                    "system changed: {published} {attack}"
                );
                for (root, bytes) in live_roots.iter().zip(live_bytes) {
                    assert_eq!(
                        files(root),
                        bytes,
                        "retained root changed: {published} {attack}"
                    );
                }
            }
            assert_eq!(files(&parked), stopped_bytes);
            assert_eq!(
                fs::read(&fixture.config.disks[0].root).unwrap(),
                b"stopped root must not be opened"
            );
        }
    }
}
