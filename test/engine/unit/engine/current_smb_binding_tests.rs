//! Real lifecycle D3, permanent native SMB results and metadata-only replay.
//! Temporary roots and closed-binding proofs do not replace physical I/O tracing.
use super::*;
use crate::*;
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

fn op() -> OperationId {
    OperationId::generate().unwrap()
}

fn request(workspace: &Workspace) -> CommitRequest {
    CommitRequest {
        workspace_id: workspace.workspace_id.clone(),
        operation_id: op(),
        principal: workspace.principal.clone(),
        expected_head: workspace.base_head.clone(),
        fencing_token: workspace.fencing_token,
        message: "SMB binding fixture".into(),
        request_digest: None,
    }
}

fn save(engine: &mut Engine, branch: &str, path: &str, bytes: &[u8]) {
    let workspace = engine.create_workspace(branch, "admin").unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, path, bytes)
        .unwrap();
    engine.commit_workspace(request(&workspace)).unwrap();
}

fn entry(engine: &Engine, branch: &str, path: &str) -> Entry {
    engine
        .namespace_tree(branch)
        .unwrap()
        .get(path)
        .unwrap()
        .clone()
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
    alias: Branch,
    hidden: Entry,
    original: Entry,
    current: Entry,
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
        let repository = engine.create_disk_root("D04", "admin", op()).unwrap();
        assert!(repository.smb_enabled);
        assert_eq!(repository.smb_binding_generation, 1);
        engine
            .update_placement_rule(PlacementRuleUpdate {
                rule_id: &new_id("rule").unwrap(),
                repository_id: &repository.repository_id,
                path_prefix: "dir/gone",
                primary_slot_id: "P01",
                expected_generation: 0,
                principal: "admin",
                operation_id: op(),
            })
            .unwrap();
        save(
            &mut engine,
            &repository.main_branch_id,
            "dir/gone",
            b"target only body",
        );
        save(
            &mut engine,
            &repository.main_branch_id,
            "dir/live",
            b"alias old body",
        );
        let hidden = entry(&engine, &repository.main_branch_id, "dir/gone");
        let original = entry(&engine, &repository.main_branch_id, "dir/live");
        let head = engine.state.branches[&repository.main_branch_id]
            .head_commit_id
            .clone();
        let alias = engine
            .create_branch(
                &head,
                &BranchName::parse("share alias").unwrap(),
                "admin",
                op(),
            )
            .unwrap();
        save(
            &mut engine,
            &repository.main_branch_id,
            "dir/live",
            b"current main body",
        );
        let current = entry(&engine, &repository.main_branch_id, "dir/live");
        Self {
            temp,
            engine,
            config,
            repository,
            alias,
            hidden,
            original,
            current,
        }
    }

    fn exit(&mut self, target: usize, action: MembershipExitAction) {
        let disk = self.config.disks[target].clone();
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
        assert!(
            self.engine
                .resume_membership_joint_decision()
                .unwrap()
                .joint_quorum_accepted()
        );
        let before = files(&disk.root);
        let result = self.engine.activate_pending_disk_exit().unwrap();
        assert_eq!(result.result["target_io"], "closed");
        resource.root.io_binding().unwrap().closed_proof().unwrap();
        assert!(self.engine.disk_resources.available(&disk).is_none());
        assert_eq!(files(&disk.root), before);
    }

    fn replace_stopped_root(&self, target: usize) -> (PathBuf, BTreeMap<PathBuf, Vec<u8>>) {
        let disk = &self.config.disks[target];
        let original = files(&disk.root);
        let saved = self.temp.path().join(format!("closed-{target}"));
        fs::rename(&disk.root, &saved).unwrap();
        fs::write(&disk.root, b"stopped root must not be opened").unwrap();
        (saved, original)
    }

    fn generation(&self) -> u64 {
        self.engine.state.histories[&self.repository.history_tree_id].generation
    }

    fn bind(&mut self, enabled: bool, operation: OperationId) -> Result<OperationResult> {
        let generation = self.generation();
        self.engine.set_smb_binding(
            &self.repository.repository_id,
            &self.alias.branch_id,
            enabled,
            generation,
            "admin",
            operation,
        )
    }
}

fn assert_same_scopes(before: &CurrentNamespaces, after: &CurrentNamespaces) {
    assert!(Arc::ptr_eq(&before.branches, &after.branches));
    assert!(Arc::ptr_eq(&before.directories, &after.directories));
    assert!(Arc::ptr_eq(&before.known_entries, &after.known_entries));
    assert!(Arc::ptr_eq(&before.known_files, &after.known_files));
    assert!(Arc::ptr_eq(
        &before.unavailable_versions,
        &after.unavailable_versions
    ));
    assert_eq!(before.registry, after.registry);
    assert_eq!(before.mask_hash, after.mask_hash);
}

#[test]
fn current_smb_binding_after_d3_keeps_alias_history_and_old_workspace_then_cold_replays() {
    let mut fixture = Fixture::new();
    let other = fixture
        .engine
        .create_repository(
            &RepositoryName::parse("unrelated workspace").unwrap(),
            "D04",
            1,
            "admin",
            op(),
        )
        .unwrap();
    let old = fixture
        .engine
        .create_workspace(&other.main_branch_id, "admin")
        .unwrap();
    let staged = fixture
        .engine
        .stage_bytes(&old.workspace_id, "staged", b"preserve unrelated old work")
        .unwrap();
    fixture.exit(0, MembershipExitAction::Deregister);
    let (saved, target_bytes) = fixture.replace_stopped_root(0);
    let previous = fixture.engine.state.clone();
    let view = fixture.engine.current_namespaces.clone().unwrap();
    let old_workspace =
        canonical_json(&fixture.engine.workspace(&old.workspace_id).unwrap()).unwrap();
    let generation = fixture.generation();
    let operation = op();
    let result = fixture.bind(false, operation).unwrap();
    let next = fixture.engine.state.clone();
    assert!(unchanged_binding_state(&previous, &next));
    assert_eq!(next.branches, previous.branches);
    assert_eq!(next.commits, previous.commits);
    assert_eq!(next.directory_branches, previous.directory_branches);
    assert_eq!(next.directory_trees, previous.directory_trees);
    assert_eq!(next.versions, previous.versions);
    assert_eq!(next.placements, previous.placements);
    assert_eq!(next.file_branch_aliases, previous.file_branch_aliases);
    assert_eq!(next.file_contexts, previous.file_contexts);
    assert_eq!(next.file_locations, previous.file_locations);
    assert_eq!(next.file_names, previous.file_names);
    assert_same_scopes(&view, fixture.engine.current_namespaces.as_ref().unwrap());
    assert_eq!(next.branches[&fixture.alias.branch_id], fixture.alias);
    assert_eq!(
        next.repositories[&fixture.repository.repository_id].main_branch_id,
        fixture.alias.branch_id
    );
    assert!(!next.repositories[&fixture.repository.repository_id].smb_enabled);
    assert_eq!(
        next.histories[&fixture.repository.history_tree_id].generation,
        generation + 1
    );
    assert_eq!(
        entry(&fixture.engine, &fixture.alias.branch_id, "dir/live").version_id,
        fixture.original.version_id
    );
    assert_eq!(
        entry(
            &fixture.engine,
            &fixture.repository.main_branch_id,
            "dir/live"
        )
        .version_id,
        fixture.current.version_id
    );
    assert!(
        fixture
            .engine
            .namespace_tree(&fixture.alias.branch_id)
            .unwrap()
            .get("dir/gone")
            .is_none()
    );
    assert!(
        fixture
            .engine
            .namespace_tree_at(&fixture.alias.branch_id, &fixture.alias.head_commit_id)
            .unwrap()
            .get("dir/gone")
            .is_some()
    );
    assert_eq!(
        canonical_json(&fixture.engine.workspace(&old.workspace_id).unwrap()).unwrap(),
        old_workspace
    );
    assert_eq!(
        fs::read(&staged.source_path).unwrap(),
        b"preserve unrelated old work"
    );
    assert_eq!(
        fixture
            .engine
            .stage_bytes(&old.workspace_id, "staged", b"forbidden stale update")
            .unwrap_err()
            .code(),
        ErrorCode::WorkspaceChanged
    );
    assert_eq!(
        fixture
            .engine
            .set_smb_binding(
                &fixture.repository.repository_id,
                &fixture.alias.branch_id,
                false,
                generation,
                "admin",
                operation
            )
            .unwrap(),
        result
    );
    for (enabled, actor) in [(true, "admin"), (false, "other")] {
        assert_eq!(
            fixture
                .engine
                .set_smb_binding(
                    &fixture.repository.repository_id,
                    &fixture.alias.branch_id,
                    enabled,
                    generation,
                    actor,
                    operation
                )
                .unwrap_err()
                .code(),
            ErrorCode::IdempotencyKeyReused
        );
    }
    let enable_generation = fixture.generation();
    let enable_operation = op();
    let enabled = fixture.bind(true, enable_operation).unwrap();
    let current_repository = &fixture.engine.state.repositories[&fixture.repository.repository_id];
    assert!(current_repository.smb_enabled);
    assert_eq!(
        current_repository.smb_binding_generation,
        fixture.repository.smb_binding_generation + 2
    );
    let mut body = Vec::new();
    fixture
        .engine
        .open_content(&fixture.original.version_id)
        .unwrap()
        .file
        .read_to_end(&mut body)
        .unwrap();
    assert_eq!(body, b"alias old body");
    assert_eq!(files(&saved), target_bytes);
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    let mut engine = Engine::open(&system).unwrap();
    assert_eq!(
        engine
            .set_smb_binding(
                &fixture.repository.repository_id,
                &fixture.alias.branch_id,
                false,
                generation,
                "admin",
                operation
            )
            .unwrap(),
        result
    );
    assert_eq!(
        engine
            .set_smb_binding(
                &fixture.repository.repository_id,
                &fixture.alias.branch_id,
                true,
                enable_generation,
                "admin",
                enable_operation
            )
            .unwrap(),
        enabled
    );
    assert!(unchanged_binding_state(&previous, engine.state()));
    assert!(
        engine
            .namespace_tree(&fixture.alias.branch_id)
            .unwrap()
            .get("dir/gone")
            .is_none()
    );
    assert_eq!(
        entry(&engine, &fixture.alias.branch_id, "dir/live").version_id,
        fixture.original.version_id
    );
    assert_eq!(
        canonical_json(&engine.workspace(&old.workspace_id).unwrap()).unwrap(),
        old_workspace
    );
    assert_eq!(
        engine
            .stage_bytes(&old.workspace_id, "staged", b"still stale")
            .unwrap_err()
            .code(),
        ErrorCode::WorkspaceChanged
    );
    assert_eq!(
        fs::read(&staged.source_path).unwrap(),
        b"preserve unrelated old work"
    );
    assert_eq!(
        fs::read(&fixture.config.disks[0].root).unwrap(),
        b"stopped root must not be opened"
    );
    assert_eq!(files(&saved), target_bytes);
}

#[test]
fn current_smb_binding_on_disabled_or_detached_owner_never_grants_body_io_or_write() {
    for action in [
        MembershipExitAction::PlannedOffline,
        MembershipExitAction::Deregister,
    ] {
        let mut fixture = Fixture::new();
        fixture.exit(3, action);
        let (saved, target_bytes) = fixture.replace_stopped_root(3);
        let before = fixture.engine.state.clone();
        let view = fixture.engine.current_namespaces.clone().unwrap();
        let generation = fixture.generation();
        let operation = op();
        let result = fixture.bind(true, operation).unwrap();
        assert!(unchanged_binding_state(&before, fixture.engine.state()));
        assert_same_scopes(&view, fixture.engine.current_namespaces.as_ref().unwrap());
        assert!(
            !fixture
                .engine
                .current_namespaces
                .as_ref()
                .unwrap()
                .branch_writable(fixture.engine.state(), &fixture.alias.branch_id)
                .unwrap()
        );
        assert_eq!(
            fixture
                .engine
                .create_workspace(&fixture.alias.branch_id, "admin")
                .unwrap_err()
                .code(),
            ErrorCode::BranchNotActive
        );
        assert!(
            fixture
                .engine
                .open_content(&fixture.original.version_id)
                .is_err()
        );
        match action {
            MembershipExitAction::PlannedOffline => assert!(
                fixture
                    .engine
                    .registered_disk(&fixture.config.disks[3].disk_id)
                    .is_some()
            ),
            MembershipExitAction::Deregister => assert!(
                fixture
                    .engine
                    .registered_disk(&fixture.config.disks[3].disk_id)
                    .is_none()
            ),
        }
        let entries = fixture
            .engine
            .namespace_tree(&fixture.alias.branch_id)
            .unwrap()
            .entries()
            .to_vec();
        let system = fixture.temp.path().join("system");
        drop(fixture.engine);
        let mut engine = Engine::open(&system).unwrap();
        assert_eq!(
            engine
                .set_smb_binding(
                    &fixture.repository.repository_id,
                    &fixture.alias.branch_id,
                    true,
                    generation,
                    "admin",
                    operation
                )
                .unwrap(),
            result
        );
        assert_eq!(
            engine
                .namespace_tree(&fixture.alias.branch_id)
                .unwrap()
                .entries(),
            entries
        );
        assert_eq!(
            engine
                .create_workspace(&fixture.alias.branch_id, "admin")
                .unwrap_err()
                .code(),
            ErrorCode::BranchNotActive
        );
        assert!(engine.open_content(&fixture.original.version_id).is_err());
        assert_eq!(
            fs::read(&fixture.config.disks[3].root).unwrap(),
            b"stopped root must not be opened"
        );
        assert_eq!(files(&saved), target_bytes);
    }
}

#[test]
fn current_smb_binding_rejects_affected_old_workspace_without_altering_its_evidence() {
    let mut fixture = Fixture::new();
    let old = fixture
        .engine
        .create_workspace(&fixture.repository.main_branch_id, "admin")
        .unwrap();
    let staged = fixture
        .engine
        .stage_bytes(&old.workspace_id, "held", b"held workspace evidence")
        .unwrap();
    fixture.exit(0, MembershipExitAction::Deregister);
    let (saved, target_bytes) = fixture.replace_stopped_root(0);
    let before = canonical_json(fixture.engine.state()).unwrap();
    let workspace = canonical_json(&fixture.engine.workspace(&old.workspace_id).unwrap()).unwrap();
    let operation = op();
    assert_eq!(
        fixture.bind(false, operation).unwrap_err().code(),
        ErrorCode::WorkspaceBusy
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    assert_eq!(
        canonical_json(&fixture.engine.workspace(&old.workspace_id).unwrap()).unwrap(),
        workspace
    );
    assert_eq!(
        fs::read(&staged.source_path).unwrap(),
        b"held workspace evidence"
    );
    assert!(!fixture.engine.state.operations.contains_key(&operation));
    assert_eq!(
        fixture
            .engine
            .stage_bytes(&old.workspace_id, "held", b"not admitted")
            .unwrap_err()
            .code(),
        ErrorCode::WorkspaceChanged
    );
    fixture
        .engine
        .abort_workspace(&old.workspace_id, "admin")
        .unwrap();
    fixture.bind(false, operation).unwrap();
    assert_eq!(
        fs::read(&fixture.config.disks[0].root).unwrap(),
        b"stopped root must not be opened"
    );
    assert_eq!(files(&saved), target_bytes);
}

#[test]
fn current_smb_binding_preserves_business_generation_scope_and_actor_validation() {
    let mut fixture = Fixture::new();
    let other = fixture
        .engine
        .create_repository(
            &RepositoryName::parse("foreign namespace").unwrap(),
            "D04",
            1,
            "admin",
            op(),
        )
        .unwrap();
    let file = fixture
        .engine
        .create_file_branch(
            &fixture.original.file_id,
            &fixture.original.version_id,
            &BranchName::parse("file is not share").unwrap(),
            "admin",
            op(),
        )
        .unwrap()
        .branch_id
        .unwrap();
    fixture.exit(0, MembershipExitAction::Deregister);
    let before = canonical_json(fixture.engine.state()).unwrap();
    let generation = fixture.generation();
    for (repository, branch, expected, actor, code) in [
        (
            fixture.repository.repository_id.as_str(),
            fixture.alias.branch_id.as_str(),
            generation + 1,
            "admin",
            ErrorCode::GenerationMismatch,
        ),
        (
            fixture.repository.repository_id.as_str(),
            other.main_branch_id.as_str(),
            generation,
            "admin",
            ErrorCode::BranchNotActive,
        ),
        (
            fixture.repository.repository_id.as_str(),
            file.as_str(),
            generation,
            "admin",
            ErrorCode::FileBranchScope,
        ),
        (
            fixture.repository.repository_id.as_str(),
            "missing-branch",
            generation,
            "admin",
            ErrorCode::BranchNotFound,
        ),
        (
            "missing-repository",
            fixture.alias.branch_id.as_str(),
            generation,
            "admin",
            ErrorCode::RepositoryNotFound,
        ),
        (
            fixture.repository.repository_id.as_str(),
            fixture.alias.branch_id.as_str(),
            generation,
            "",
            ErrorCode::Unauthenticated,
        ),
    ] {
        assert_eq!(
            fixture
                .engine
                .set_smb_binding(repository, branch, true, expected, actor, op())
                .unwrap_err()
                .code(),
            code
        );
        assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    }
}

#[test]
fn current_smb_binding_receipt_and_closed_write_set_reject_fast_path_and_replay_forgery() {
    let mut fixture = Fixture::new();
    fixture.exit(0, MembershipExitAction::Deregister);
    let first = op();
    fixture.bind(false, first).unwrap();
    let previous = fixture.engine.state.clone();
    let view = fixture.engine.current_namespaces.clone().unwrap();
    let operation = op();
    fixture.bind(true, operation).unwrap();
    let published = fixture.engine.state.clone();
    assert!(
        published
            .branches
            .changes_since(&previous.branches)
            .is_empty()
    );
    assert!(
        published
            .directory_branches
            .changes_since(&previous.directory_branches)
            .is_empty()
    );
    assert_same_scopes(&view, &view.advance(&previous, &published).unwrap());
    let recovered_previous: State =
        json_codec::from_slice(&canonical_json(previous.as_ref()).unwrap()).unwrap();
    let recovered_next: State =
        json_codec::from_slice(&canonical_json(published.as_ref()).unwrap()).unwrap();
    assert_same_scopes(
        &view,
        &view.advance(&recovered_previous, &recovered_next).unwrap(),
    );
    for attack in 0..30 {
        let mut forged = published.as_ref().clone();
        match attack {
            0 => {
                forged
                    .operation_principals
                    .insert(operation, "different actor".into());
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
                forged.operations.get_mut(&operation).unwrap().kind = "auth.fake".into();
            }
            4 => {
                forged.operations.remove(&operation);
            }
            5 => {
                forged.operations.get_mut(&operation).unwrap().result["namespace_binding"]["registry"] =
                    Value::Null;
            }
            6 => {
                forged.operations.get_mut(&operation).unwrap().result["namespace_binding"]["mask_hash"] =
                    Value::Null;
            }
            7 => {
                forged.operations.get_mut(&operation).unwrap().result["namespace_binding"]["target_head_commit_id"] = json!(
                    fixture.engine.state.branches[&fixture.repository.main_branch_id]
                        .head_commit_id
                );
            }
            8 => {
                forged.operations.get_mut(&operation).unwrap().result["smb_enabled"] = json!(false);
            }
            9 => {
                forged.operations.get_mut(&operation).unwrap().result["generation"] = json!("999");
            }
            10 => {
                forged.operations.get_mut(&operation).unwrap().sequence += 1;
            }
            11 => {
                forged.operations.get_mut(&operation).unwrap().commit_state = CommitState::InDoubt;
            }
            12 => {
                forged
                    .repositories
                    .get_mut(&fixture.repository.repository_id)
                    .unwrap()
                    .primary_slot_id = "P01".into();
            }
            13 => {
                forged
                    .histories
                    .get_mut(&fixture.repository.history_tree_id)
                    .unwrap()
                    .root_commit_id = fixture.alias.head_commit_id.clone();
            }
            14 => {
                forged
                    .branches
                    .get_mut(&fixture.alias.branch_id)
                    .unwrap()
                    .name = "unrequested name".into();
            }
            15 => {
                forged
                    .commits
                    .get_mut(&fixture.alias.head_commit_id)
                    .unwrap()
                    .message = "rewritten history".into();
            }
            16 => {
                forged
                    .commits
                    .get_mut(&fixture.alias.head_commit_id)
                    .unwrap()
                    .entries
                    .push(fixture.hidden.clone());
            }
            17 => {
                forged.file_locations.insert(
                    fixture.original.file_id.clone(),
                    "restored/physical-owner".into(),
                );
            }
            18 => {
                forged.file_names.insert(
                    fixture.hidden.file_id.clone(),
                    "recreated-hidden-name".into(),
                );
            }
            19 => {
                forged
                    .placements
                    .get_mut(&fixture.original.version_id)
                    .unwrap()
                    .primary_slot_id = "P01".into();
            }
            20 => {
                forged
                    .file_branch_aliases
                    .insert("forged-alias".into(), fixture.alias.branch_id.clone());
            }
            21 => {
                forged.operations.get_mut(&first).unwrap().request_digest = "e".repeat(64);
            }
            22 => {
                forged
                    .operation_principals
                    .insert(first, "replacement old owner".into());
            }
            23 => {
                forged.events.last_mut().unwrap().object_id =
                    fixture.repository.repository_id.clone();
            }
            24 => {
                forged.events[0].kind = "rewritten audit".into();
            }
            25 => {
                forged.control_auth = Some(json!({"unrequested":true}));
            }
            26 => {
                forged.topology_generation += 1;
            }
            27 => {
                forged.business_generation += 1;
            }
            28 => {
                forged.operations.get_mut(&operation).unwrap().result["namespace_binding"]["unexpected"] =
                    json!(true);
            }
            29 => {
                forged.operations.remove(&operation);
                forged.repositories.insert(
                    fixture.repository.repository_id.clone(),
                    previous.repositories[&fixture.repository.repository_id].clone(),
                );
            }
            _ => unreachable!(),
        }
        assert!(
            view.advance(&previous, &forged).is_err(),
            "tracked attack {attack}"
        );
        let restored: State = json_codec::from_slice(&canonical_json(&forged).unwrap()).unwrap();
        assert!(
            view.advance(&recovered_previous, &restored).is_err(),
            "restored attack {attack}"
        );
    }
}

#[test]
fn current_smb_binding_replays_pre_registry_receipt_without_new_permission_or_publication() {
    let mut fixture = Fixture::new();
    let generation = fixture.generation();
    let operation = op();
    let original = fixture.bind(false, operation).unwrap();
    assert!(original.result.get("namespace_binding").is_none());
    fixture.exit(0, MembershipExitAction::Deregister);
    let before = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(
        fixture
            .engine
            .set_smb_binding(
                &fixture.repository.repository_id,
                &fixture.alias.branch_id,
                false,
                generation,
                "admin",
                operation
            )
            .unwrap(),
        original
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    let mut engine = Engine::open(&system).unwrap();
    assert_eq!(
        engine
            .set_smb_binding(
                &fixture.repository.repository_id,
                &fixture.alias.branch_id,
                false,
                generation,
                "admin",
                operation
            )
            .unwrap(),
        original
    );
}

#[test]
fn local_ready_plans_do_not_leak_into_metadata_publications_or_cold_replay() {
    let mut fixture = Fixture::new();
    fixture.exit(0, MembershipExitAction::Deregister);
    let (stopped, original_bytes) = fixture.replace_stopped_root(0);
    let source = fixture.engine.state.branches[&fixture.repository.main_branch_id]
        .head_commit_id
        .clone();
    let published = Arc::clone(&fixture.engine.state);
    let published_bytes = canonical_json(published.as_ref()).unwrap();
    let plans: Vec<_> = ["first local preview", "second local preview"]
        .into_iter()
        .map(|name| {
            fixture
                .engine
                .create_historical_save_plan(
                    &fixture.repository.history_tree_id,
                    HistoricalSaveOptions {
                        source_commit_id: source.clone(),
                        name: name.into(),
                        workspace_id: None,
                        primary_slot_id: None,
                        target_copies: None,
                    },
                    "admin",
                )
                .unwrap()
        })
        .collect();
    assert!(Arc::ptr_eq(&published, &fixture.engine.state));
    assert_eq!(
        canonical_json(fixture.engine.state()).unwrap(),
        published_bytes
    );
    let plan_bytes: Vec<_> = plans
        .iter()
        .map(|plan| {
            assert_eq!(fixture.engine.plan(&plan.plan_id).unwrap(), *plan);
            assert!(!fixture.engine.state.plans.contains_key(&plan.plan_id));
            fs::read(
                fixture
                    .temp
                    .path()
                    .join("system/plans")
                    .join(format!("{}.json", plan.plan_id)),
            )
            .unwrap()
        })
        .collect();
    let operation = op();
    let generation = fixture.generation();
    let result = fixture.bind(false, operation).unwrap();
    for plan in &plans {
        assert!(!fixture.engine.state.plans.contains_key(&plan.plan_id));
        assert_eq!(fixture.engine.plan(&plan.plan_id).unwrap(), *plan);
    }
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    let mut engine = Engine::open(&system).unwrap();
    assert_eq!(
        engine
            .set_smb_binding(
                &fixture.repository.repository_id,
                &fixture.alias.branch_id,
                false,
                generation,
                "admin",
                operation,
            )
            .unwrap(),
        result
    );
    for (plan, bytes) in plans.iter().zip(plan_bytes) {
        assert!(!engine.state.plans.contains_key(&plan.plan_id));
        assert_eq!(engine.plan(&plan.plan_id).unwrap(), *plan);
        assert_eq!(
            fs::read(system.join("plans").join(format!("{}.json", plan.plan_id))).unwrap(),
            bytes
        );
    }
    assert_eq!(files(&stopped), original_bytes);
    assert_eq!(
        fs::read(&fixture.config.disks[0].root).unwrap(),
        b"stopped root must not be opened"
    );
}

#[test]
fn current_smb_binding_requires_its_closed_proof_without_widening_kind_admission() {
    use crate::engine::MutationPlan;

    let mut fixture = Fixture::new();
    fixture.exit(0, MembershipExitAction::Deregister);
    let view = fixture.engine.current_namespaces.clone().unwrap();
    assert_eq!(
        view.require_operation(KIND).unwrap_err().code(),
        ErrorCode::RecoveryRequired
    );
    let previous = Arc::clone(&fixture.engine.state);
    let operation = op();
    let receipt = fixture.bind(false, operation).unwrap();
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
        |_, _, _| panic!("general kind admission must reject before preparing a binding"),
    );
    let Err(error) = generic else {
        panic!("SMB binding must not become a generic allowed kind");
    };
    assert_eq!(error.code(), ErrorCode::RecoveryRequired);
    assert!(
        view.verify_new_smb_binding(&previous, &previous, &receipt)
            .is_err()
    );

    for attack in 0..6 {
        let digest = if attack == 1 {
            "f".repeat(64)
        } else {
            receipt.request_digest.clone()
        };
        let candidate =
            context.plan_smb_binding_mutation("admin", operation, digest, |_, next, prepared| {
                if attack != 5 {
                    next.repositories.insert(
                        fixture.repository.repository_id.clone(),
                        published.repositories[&fixture.repository.repository_id].clone(),
                    );
                    next.histories.insert(
                        fixture.repository.history_tree_id.clone(),
                        published.histories[&fixture.repository.history_tree_id].clone(),
                    );
                }
                let mut result = receipt.result.clone();
                if attack == 2 {
                    result["namespace_binding"]["registry"] = Value::Null;
                }
                if attack == 3 {
                    next.file_notes
                        .insert(fixture.original.file_id.clone(), "unrequested".into());
                }
                if attack == 4 {
                    prepared.push(crate::engine::Prepared {
                        version: previous.versions[&fixture.original.version_id].clone(),
                        placement: previous.placements[&fixture.original.version_id].clone(),
                        source: crate::engine::ReadSource::external(
                            fixture.temp.path().join("must-not-open-payload"),
                        ),
                    });
                }
                Ok(result)
            });
        if attack == 0 {
            let Ok(MutationPlan::Prepared {
                next,
                prepared,
                result,
            }) = candidate
            else {
                panic!("exact metadata-only native candidate must be admitted");
            };
            assert!(prepared.is_empty());
            view.verify_new_smb_binding(&previous, &next, &result)
                .unwrap();
            assert_same_scopes(&view, &view.advance(&previous, &next).unwrap());
        } else {
            let Err(error) = candidate else {
                panic!("unproven binding candidate {attack} left planning");
            };
            assert_eq!(
                error.code(),
                ErrorCode::PublicationInvalid,
                "attack {attack}"
            );
        }
    }

    context.current_namespaces = None;
    let pending =
        context.plan_smb_binding_mutation("admin", operation, receipt.request_digest, |_, _, _| {
            panic!("missing registry admission must reject before preparation")
        });
    let Err(error) = pending else {
        panic!("pending current namespace must not admit a binding");
    };
    assert_eq!(error.code(), ErrorCode::RecoveryRequired);
    assert!(Arc::ptr_eq(&fixture.engine.state, &published));
    assert_eq!(
        canonical_json(fixture.engine.state()).unwrap(),
        published_bytes
    );
}

#[test]
fn current_smb_binding_rejects_principal_only_rewrites_without_a_new_receipt() {
    let mut fixture = Fixture::new();
    fixture.exit(0, MembershipExitAction::Deregister);
    let operation = op();
    fixture.bind(false, operation).unwrap();
    let previous = Arc::clone(&fixture.engine.state);
    let view = fixture.engine.current_namespaces.as_ref().unwrap();
    let recovered_previous: State =
        json_codec::from_slice(&canonical_json(previous.as_ref()).unwrap()).unwrap();
    for remove in [false, true] {
        let mut next = previous.as_ref().clone();
        next.begin_tracking();
        if remove {
            next.operation_principals.remove(&operation);
        } else {
            next.operation_principals
                .insert(operation, "replacement historical owner".into());
        }
        assert!(
            next.operations
                .changes_since(&previous.operations)
                .is_empty()
        );
        assert!(next.branches.changes_since(&previous.branches).is_empty());
        assert_eq!(
            view.advance(&previous, &next).err().unwrap().code(),
            ErrorCode::PublicationInvalid
        );
        let restored: State = json_codec::from_slice(&canonical_json(&next).unwrap()).unwrap();
        assert_eq!(
            view.advance(&recovered_previous, &restored)
                .err()
                .unwrap()
                .code(),
            ErrorCode::PublicationInvalid
        );
    }
    assert!(Arc::ptr_eq(&fixture.engine.state, &previous));
}
