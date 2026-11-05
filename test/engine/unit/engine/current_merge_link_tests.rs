//! Native metadata-only relations over real D3 projections and frozen history.
//! No private synthetic registry installation or physical-media claim.
use super::*;
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
        message: "current merge link fixture".into(),
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
                &RepositoryName::parse("merge link history").unwrap(),
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
            .create_branch(&source, &name("relation source"), "admin", op())
            .unwrap();
        let alias = engine
            .create_branch(&source, &name("relation target"), "admin", op())
            .unwrap();
        save(
            &mut engine,
            &branch.branch_id,
            "dir/live",
            b"source branch successor",
        );
        save(
            &mut engine,
            &alias.branch_id,
            "target",
            b"target branch successor",
        );
        let branch = engine.state.branches[&branch.branch_id].clone();
        let alias = engine.state.branches[&alias.branch_id].clone();
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

    fn input(&self) -> Value {
        json!({
            "source_branch_id": self.branch.branch_id,
            "source_commit_id": self.source,
            "target_branch_id": self.alias.branch_id,
            "target_commit_id": self.alias.head_commit_id,
        })
    }

    fn link(&mut self, operation: OperationId) -> Result<MergeLink> {
        self.engine.add_merge_link(
            &self.branch.branch_id,
            &self.source,
            &self.alias.branch_id,
            &self.alias.head_commit_id,
            "admin",
            operation,
        )
    }

    fn plan(&self, operation: OperationId) -> crate::engine::MutationPlan {
        let link = MergeLink {
            merge_link_id: new_id("link").unwrap(),
            history_tree_id: self.repository.history_tree_id.clone(),
            source_branch_id: self.branch.branch_id.clone(),
            source_commit_id: self.source.clone(),
            target_branch_id: self.alias.branch_id.clone(),
            target_commit_id: self.alias.head_commit_id.clone(),
            author: "admin".into(),
            operation_id: operation,
        };
        self.engine
            .plan_mutation_digest(
                KIND,
                "admin",
                operation,
                self.input(),
                None,
                |engine, next, _| {
                    let source = engine
                        .current_namespaces
                        .as_ref()
                        .unwrap()
                        .merge_link_source(next, &link)?;
                    let result = json!({
                        "merge_link_id": link.merge_link_id,
                        "history_tree_id": link.history_tree_id,
                        "namespace_merge_link": source,
                    });
                    next.merge_links.insert(link.merge_link_id.clone(), link);
                    Ok(result)
                },
            )
            .unwrap()
    }
}

fn decoded(state: &State) -> State {
    json_codec::from_slice(&canonical_json(state).unwrap()).unwrap()
}

fn same_scopes(before: &CurrentNamespaces, after: &CurrentNamespaces) {
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
fn current_merge_link_preserves_projection_history_workspaces_and_permanent_results() {
    let mut fixture = Fixture::new();
    let held = fixture
        .engine
        .create_workspace(&fixture.branch.branch_id, "admin")
        .unwrap();
    fixture
        .engine
        .stage_bytes(&held.workspace_id, "staged", b"uncommitted body")
        .unwrap();
    let stopped = fixture.exit(0, MembershipExitAction::Deregister);
    let parked = fixture.park_root(0);
    let second = fixture.exit(2, MembershipExitAction::Deregister);
    let second_parked = fixture.park_root(2);
    let before = Arc::clone(&fixture.engine.state);
    let view = fixture.engine.current_namespaces.clone().unwrap();
    let operation = op();
    let link = fixture.link(operation).unwrap();
    let receipt = fixture.engine.state.operations[&operation].clone();
    verify_write_set(&before, &fixture.engine.state, &link, operation).unwrap();
    same_scopes(&view, fixture.engine.current_namespaces.as_ref().unwrap());
    assert_ne!(link.source_commit_id, fixture.branch.head_commit_id);
    assert_eq!(link.target_commit_id, fixture.alias.head_commit_id);
    assert_eq!(fixture.engine.state.branches, before.branches);
    assert_eq!(fixture.engine.state.commits, before.commits);
    assert_eq!(fixture.engine.state.versions, before.versions);
    assert_eq!(fixture.engine.state.placements, before.placements);
    assert_eq!(fixture.engine.state.file_selections, before.file_selections);
    assert!(
        fixture
            .engine
            .namespace_tree(&fixture.branch.branch_id)
            .unwrap()
            .get("dir/gone")
            .is_none()
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
        fixture.engine.state.content_states[&fixture.hidden.version_id],
        before.content_states[&fixture.hidden.version_id]
    );
    let staged = fixture.engine.workspace(&held.workspace_id).unwrap();
    assert_eq!(staged.state, WorkspaceState::Open);
    assert!(
        staged
            .files
            .values()
            .any(|file| file.path.as_str() == "staged")
    );
    assert_eq!(
        fixture
            .engine
            .commit_workspace(request(&held))
            .unwrap_err()
            .code(),
        ErrorCode::WorkspaceChanged
    );
    let published = Arc::clone(&fixture.engine.state);
    assert_eq!(fixture.link(operation).unwrap(), link);
    assert!(Arc::ptr_eq(&published, &fixture.engine.state));
    assert_eq!(
        fixture
            .engine
            .add_merge_link(
                &fixture.branch.branch_id,
                &fixture.source,
                &fixture.alias.branch_id,
                &fixture.alias.head_commit_id,
                "another actor",
                operation,
            )
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyKeyReused
    );
    assert_eq!(
        fixture
            .engine
            .add_merge_link(
                &fixture.branch.branch_id,
                &fixture.branch.head_commit_id,
                &fixture.alias.branch_id,
                &fixture.alias.head_commit_id,
                "admin",
                operation,
            )
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyKeyReused
    );
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    for _ in 0..2 {
        let mut engine = Engine::open(&system).unwrap();
        assert_eq!(
            engine
                .add_merge_link(
                    &fixture.branch.branch_id,
                    &fixture.source,
                    &fixture.alias.branch_id,
                    &fixture.alias.head_commit_id,
                    "admin",
                    operation,
                )
                .unwrap(),
            link
        );
        assert_eq!(engine.state.operations[&operation], receipt);
        assert_eq!(engine.state.merge_links[&link.merge_link_id], link);
        assert_eq!(engine.state.branches, before.branches);
        assert_eq!(engine.state.commits, before.commits);
        assert!(
            engine
                .namespace_tree(&fixture.branch.branch_id)
                .unwrap()
                .get("dir/gone")
                .is_none()
        );
        assert_eq!(
            engine.commit_workspace(request(&held)).unwrap_err().code(),
            ErrorCode::WorkspaceChanged
        );
        assert_eq!(files(&parked), stopped);
        assert_eq!(files(&second_parked), second);
        assert_eq!(
            fs::read(&fixture.config.disks[0].root).unwrap(),
            b"stopped root must not be opened"
        );
    }
}

#[test]
fn pre_registry_merge_link_receipt_remains_exact_after_two_d3_publications() {
    let mut fixture = Fixture::new();
    let operation = op();
    let original = fixture.link(operation).unwrap();
    let receipt = fixture.engine.state.operations[&operation].clone();
    assert!(receipt.result.get("namespace_merge_link").is_none());
    fixture.exit(0, MembershipExitAction::Deregister);
    fixture.exit(2, MembershipExitAction::PlannedOffline);
    assert_eq!(fixture.link(operation).unwrap(), original);
    fixture.link(op()).unwrap();
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    let mut engine = Engine::open(&system).unwrap();
    assert_eq!(
        engine
            .add_merge_link(
                &fixture.branch.branch_id,
                &fixture.source,
                &fixture.alias.branch_id,
                &fixture.alias.head_commit_id,
                "admin",
                operation,
            )
            .unwrap(),
        original
    );
    assert_eq!(engine.state.operations[&operation], receipt);
}

#[test]
fn disabled_or_detached_source_preserves_read_only_metadata_relation_semantics() {
    for action in [
        MembershipExitAction::PlannedOffline,
        MembershipExitAction::Deregister,
    ] {
        let mut fixture = Fixture::new();
        // D04 owns the repository and target-only body. A relation is still not
        // a write to this stopped owner's files or a new current namespace.
        let stopped = fixture.exit(3, action);
        let parked = fixture.park_root(3);
        let before = Arc::clone(&fixture.engine.state);
        let view = fixture.engine.current_namespaces.clone().unwrap();
        let operation = op();
        let link = fixture.link(operation).unwrap();
        verify_write_set(&before, &fixture.engine.state, &link, operation).unwrap();
        same_scopes(&view, fixture.engine.current_namespaces.as_ref().unwrap());
        assert_eq!(files(&parked), stopped);
        let system = fixture.temp.path().join("system");
        drop(fixture.engine);
        let engine = Engine::open(&system).unwrap();
        assert_eq!(engine.state.merge_links[&link.merge_link_id], link);
        assert_eq!(engine.state.branches, before.branches);
        assert_eq!(files(&parked), stopped);
        assert_eq!(
            fs::read(&fixture.config.disks[3].root).unwrap(),
            b"stopped root must not be opened"
        );
    }
}

#[test]
fn merge_link_keeps_native_actor_history_file_and_first_parent_rejections() {
    let mut fixture = Fixture::new();
    let foreign = fixture
        .engine
        .create_repository(
            &RepositoryName::parse("unrelated history").unwrap(),
            "D04",
            1,
            "admin",
            op(),
        )
        .unwrap();
    let file_sibling = fixture
        .engine
        .create_file_branch(
            &fixture.retained.file_id,
            &fixture.retained.version_id,
            &name("file relation target"),
            "admin",
            op(),
        )
        .unwrap()
        .branch_id
        .unwrap();
    // Native repository writes do not create independent file selections. The
    // first explicit file-branch request installs that file's shared main.
    let file_source = fixture.engine.state.file_selections[&fixture.retained.file_id].clone();
    fixture
        .engine
        .create_file_branch(
            &fixture.hidden.file_id,
            &fixture.hidden.version_id,
            &name("other file relation"),
            "admin",
            op(),
        )
        .unwrap();
    let other_file = fixture.engine.state.file_selections[&fixture.hidden.file_id].clone();
    fixture.exit(0, MembershipExitAction::Deregister);
    let before = Arc::clone(&fixture.engine.state);
    let cases = [
        (
            fixture.branch.branch_id.as_str(),
            fixture.source.as_str(),
            fixture.alias.branch_id.as_str(),
            fixture.alias.head_commit_id.as_str(),
            "",
            ErrorCode::Unauthenticated,
        ),
        (
            fixture.branch.branch_id.as_str(),
            fixture.source.as_str(),
            fixture.branch.branch_id.as_str(),
            fixture.source.as_str(),
            "admin",
            ErrorCode::MergeEndpointMismatch,
        ),
        (
            fixture.alias.branch_id.as_str(),
            fixture.branch.head_commit_id.as_str(),
            fixture.branch.branch_id.as_str(),
            fixture.source.as_str(),
            "admin",
            ErrorCode::MergeEndpointMismatch,
        ),
        (
            fixture.branch.branch_id.as_str(),
            fixture.source.as_str(),
            foreign.main_branch_id.as_str(),
            fixture.engine.state.branches[&foreign.main_branch_id]
                .head_commit_id
                .as_str(),
            "admin",
            ErrorCode::HistoryMismatch,
        ),
        (
            fixture.branch.branch_id.as_str(),
            fixture.source.as_str(),
            file_source.as_str(),
            fixture.engine.state.branches[&file_source]
                .head_commit_id
                .as_str(),
            "admin",
            ErrorCode::FileBranchScope,
        ),
        (
            file_source.as_str(),
            fixture.engine.state.branches[&file_source]
                .head_commit_id
                .as_str(),
            other_file.as_str(),
            fixture.engine.state.branches[&other_file]
                .head_commit_id
                .as_str(),
            "admin",
            ErrorCode::FileBranchScope,
        ),
    ];
    // Own strings so the native mutable call is independent of the prior State.
    let cases = cases.map(|(sb, sc, tb, tc, actor, code)| {
        (
            sb.to_owned(),
            sc.to_owned(),
            tb.to_owned(),
            tc.to_owned(),
            actor,
            code,
        )
    });
    for (sb, sc, tb, tc, actor, code) in cases {
        assert_eq!(
            fixture
                .engine
                .add_merge_link(&sb, &sc, &tb, &tc, actor, op())
                .unwrap_err()
                .code(),
            code
        );
        assert!(Arc::ptr_eq(&before, &fixture.engine.state));
    }
    let source_commit = fixture.engine.state.branches[&file_source]
        .head_commit_id
        .clone();
    let target_commit = fixture.engine.state.branches[&file_sibling]
        .head_commit_id
        .clone();
    let operation = op();
    let link = fixture
        .engine
        .add_merge_link(
            &file_source,
            &source_commit,
            &file_sibling,
            &target_commit,
            "admin",
            operation,
        )
        .unwrap();
    verify_write_set(&before, &fixture.engine.state, &link, operation).unwrap();
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    let engine = Engine::open(&system).unwrap();
    assert_eq!(engine.state.merge_links[&link.merge_link_id], link);
}

#[test]
fn archived_endpoint_and_sealed_workspace_do_not_gain_content_authority() {
    let mut fixture = Fixture::new();
    fixture
        .engine
        .archive_branch(&fixture.branch.branch_id, "admin", op())
        .unwrap();
    let workspace = fixture
        .engine
        .create_workspace(&fixture.alias.branch_id, "admin")
        .unwrap();
    fixture
        .engine
        .stage_bytes(&workspace.workspace_id, "staged", b"sealed body")
        .unwrap();
    fixture
        .engine
        .seal_workspace(&workspace.workspace_id, "admin")
        .unwrap();
    let stopped = fixture.exit(0, MembershipExitAction::Deregister);
    let parked = fixture.park_root(0);
    let before = Arc::clone(&fixture.engine.state);
    let operation = op();
    let link = fixture.link(operation).unwrap();
    verify_write_set(&before, &fixture.engine.state, &link, operation).unwrap();
    assert_eq!(
        fixture.engine.state.branches[&fixture.branch.branch_id].state,
        BranchState::Archived
    );
    let sealed = fixture.engine.workspace(&workspace.workspace_id).unwrap();
    assert_eq!(sealed.state, WorkspaceState::Sealed);
    assert!(
        sealed
            .files
            .values()
            .any(|file| file.path.as_str() == "staged")
    );
    assert_eq!(
        fixture
            .engine
            .create_workspace(&fixture.branch.branch_id, "admin")
            .unwrap_err()
            .code(),
        ErrorCode::BranchArchived
    );
    assert_eq!(files(&parked), stopped);
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    let engine = Engine::open(&system).unwrap();
    assert_eq!(engine.state.merge_links[&link.merge_link_id], link);
    assert_eq!(
        engine.workspace(&workspace.workspace_id).unwrap().state,
        WorkspaceState::Sealed
    );
    assert_eq!(files(&parked), stopped);
}

#[test]
fn current_link_proof_rejects_tracked_and_cold_closed_write_set_forgery() {
    let mut fixture = Fixture::new();
    fixture.exit(0, MembershipExitAction::Deregister);
    let first_operation = op();
    let first = fixture.link(first_operation).unwrap();
    let previous = Arc::clone(&fixture.engine.state);
    let view = fixture.engine.current_namespaces.clone().unwrap();
    let operation = op();
    let link = fixture.link(operation).unwrap();
    let published = Arc::clone(&fixture.engine.state);
    let cold_previous = decoded(&previous);
    assert!(
        published
            .branches
            .changes_since(&previous.branches)
            .is_empty()
    );
    same_scopes(&view, &view.advance(&previous, &published).unwrap());
    same_scopes(
        &view,
        &view.advance(&cold_previous, &decoded(&published)).unwrap(),
    );
    for attack in 0..46 {
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
                    .request_digest = "a".repeat(64);
            }
            2 => {
                forged
                    .merge_links
                    .get_mut(&link.merge_link_id)
                    .unwrap()
                    .author = "other".into();
            }
            3 => {
                forged
                    .merge_links
                    .get_mut(&link.merge_link_id)
                    .unwrap()
                    .merge_link_id = new_id("link").unwrap();
            }
            4 => {
                forged
                    .merge_links
                    .get_mut(&link.merge_link_id)
                    .unwrap()
                    .operation_id = op();
            }
            5 => {
                forged
                    .merge_links
                    .get_mut(&link.merge_link_id)
                    .unwrap()
                    .history_tree_id = new_id("history").unwrap();
            }
            6 => {
                forged
                    .merge_links
                    .get_mut(&link.merge_link_id)
                    .unwrap()
                    .source_branch_id = link.target_branch_id.clone();
            }
            7 => {
                forged
                    .merge_links
                    .get_mut(&link.merge_link_id)
                    .unwrap()
                    .source_commit_id = link.target_commit_id.clone();
            }
            8 => {
                forged
                    .merge_links
                    .get_mut(&link.merge_link_id)
                    .unwrap()
                    .target_branch_id = link.source_branch_id.clone();
            }
            9 => {
                forged.operations.get_mut(&operation).unwrap().kind = "auth.unrelated".into();
            }
            10 => {
                forged
                    .operations
                    .get_mut(&operation)
                    .unwrap()
                    .result
                    .as_object_mut()
                    .unwrap()
                    .remove("namespace_merge_link");
            }
            11 => {
                forged.operations.remove(&operation);
            }
            12 => {
                forged.merge_links.remove(&link.merge_link_id);
            }
            13 => {
                forged.merge_links.remove(&first.merge_link_id);
            }
            14 => {
                forged
                    .merge_links
                    .get_mut(&first.merge_link_id)
                    .unwrap()
                    .author = "rewritten".into();
            }
            15 => {
                let mut extra = link.clone();
                extra.merge_link_id = new_id("link").unwrap();
                forged
                    .merge_links
                    .insert(extra.merge_link_id.clone(), extra);
            }
            16 => {
                forged.operations.get_mut(&operation).unwrap().result["namespace_merge_link"]["mask_hash"] =
                    Value::Null;
            }
            17 => {
                forged.operations.get_mut(&operation).unwrap().result["namespace_merge_link"]["source"]
                    ["selected_commit_id"] = json!(fixture.branch.head_commit_id);
            }
            18 => {
                forged.operations.get_mut(&operation).unwrap().result["namespace_merge_link"]["target"]
                    ["branch_generation"] = json!("999");
            }
            19 => {
                forged.operations.get_mut(&operation).unwrap().result["namespace_merge_link"]["activation_sequence"] =
                    json!("0");
            }
            20 => {
                forged.operations.get_mut(&operation).unwrap().result["extra"] = json!(true);
            }
            21 => {
                forged.operations.get_mut(&operation).unwrap().result["timestamp"] = json!("01");
            }
            22 => {
                forged.operations.get_mut(&operation).unwrap().result["membership_epoch"] =
                    json!(0);
            }
            23 => {
                forged.operations.get_mut(&operation).unwrap().commit_id =
                    Some(fixture.source.clone());
            }
            24 => {
                forged.operations.get_mut(&operation).unwrap().branch_id =
                    Some(fixture.branch.branch_id.clone());
            }
            25 => {
                forged
                    .operations
                    .get_mut(&operation)
                    .unwrap()
                    .history_tree_id = None;
            }
            26 => {
                forged
                    .operations
                    .get_mut(&operation)
                    .unwrap()
                    .protection_at_publish = ProtectionAtPublish::TargetMet;
            }
            27 => {
                forged
                    .operations
                    .get_mut(&operation)
                    .unwrap()
                    .current_health = CurrentHealth::Unavailable;
            }
            28 => {
                forged.operations.get_mut(&operation).unwrap().sequence += 1;
            }
            29 => {
                forged.business_generation += 1;
            }
            30 => {
                forged.topology_generation += 1;
            }
            31 => {
                forged.control_auth = Some(json!({"unrequested": true}));
            }
            32 => {
                forged.events.last_mut().unwrap().object_id = link.merge_link_id.clone();
            }
            33 => {
                forged.events[0].kind = "rewritten audit".into();
            }
            34 => {
                forged
                    .branches
                    .get_mut(&fixture.branch.branch_id)
                    .unwrap()
                    .name = "renamed".into();
            }
            35 => {
                forged.commits.get_mut(&fixture.source).unwrap().message = "changed history".into();
            }
            36 => {
                forged
                    .file_names
                    .insert(fixture.retained.file_id.clone(), "changed-container".into());
            }
            37 => {
                let name = forged.file_names[&fixture.retained.file_id].clone();
                forged.file_locations.insert(
                    fixture.retained.file_id.clone(),
                    format!("unrequested/{name}"),
                );
            }
            38 => {
                forged
                    .placements
                    .get_mut(&fixture.retained.version_id)
                    .unwrap()
                    .route_slot_ids
                    .clear();
            }
            39 => {
                forged
                    .content_states
                    .insert(fixture.retained.version_id.clone(), ContentState::Purged);
            }
            40 => {
                forged.operations.get_mut(&operation).unwrap().commit_state = CommitState::InDoubt;
            }
            41 => {
                forged
                    .operations
                    .get_mut(&first_operation)
                    .unwrap()
                    .request_digest = "b".repeat(64);
            }
            42 => {
                forged
                    .operation_principals
                    .insert(first_operation, "other".into());
            }
            43 => {
                forged
                    .file_notes
                    .insert(fixture.retained.file_id.clone(), "unrequested note".into());
            }
            44 => {
                assert!(
                    forged
                        .file_selections
                        .insert(
                            fixture.retained.file_id.clone(),
                            fixture.branch.branch_id.clone(),
                        )
                        .is_none()
                );
            }
            45 => {
                forged
                    .histories
                    .get_mut(&fixture.repository.history_tree_id)
                    .unwrap()
                    .generation += 1;
            }
            _ => unreachable!(),
        }
        assert_ne!(
            &forged,
            published.as_ref(),
            "attack {attack} must change State"
        );
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
fn existing_relations_actors_receipts_and_events_cannot_change_on_the_fast_path() {
    let mut fixture = Fixture::new();
    fixture.exit(0, MembershipExitAction::Deregister);
    let operation = op();
    let link = fixture.link(operation).unwrap();
    let previous = Arc::clone(&fixture.engine.state);
    let view = fixture.engine.current_namespaces.clone().unwrap();
    let cold_previous = decoded(&previous);
    for attack in 0..8 {
        let mut next = previous.as_ref().clone();
        next.begin_tracking();
        next.sequence += 1;
        match attack {
            0 => {
                next.merge_links.remove(&link.merge_link_id);
            }
            1 => {
                next.merge_links
                    .get_mut(&link.merge_link_id)
                    .unwrap()
                    .source_commit_id = fixture.branch.head_commit_id.clone();
            }
            2 => {
                next.operation_principals.insert(operation, "other".into());
            }
            3 => {
                next.operations.get_mut(&operation).unwrap().kind = "auth.unrelated".into();
            }
            4 => {
                next.events.last_mut().unwrap().kind = "auth.unrelated".into();
            }
            5 => {
                next.events.last_mut().unwrap().operation_id = op();
            }
            6 => {
                next.operations.remove(&operation);
            }
            7 => {
                let mut extra = link.clone();
                extra.merge_link_id = new_id("link").unwrap();
                next.merge_links.insert(extra.merge_link_id.clone(), extra);
            }
            _ => unreachable!(),
        }
        assert!(view.advance(&previous, &next).is_err(), "tracked {attack}");
        assert!(
            view.advance(&cold_previous, &decoded(&next)).is_err(),
            "cold {attack}"
        );
    }
}

#[test]
fn relation_creation_cannot_be_hidden_behind_another_native_operation_kind() {
    let mut fixture = Fixture::new();
    fixture.exit(0, MembershipExitAction::Deregister);
    let previous = Arc::clone(&fixture.engine.state);
    let view = fixture.engine.current_namespaces.clone().unwrap();
    let operation = op();
    fixture.link(operation).unwrap();
    let mut next = fixture.engine.state.as_ref().clone();
    let receipt = next.operations.get_mut(&operation).unwrap();
    receipt.kind = "auth.unrelated".into();
    receipt.result = json!({"unrelated": true});
    receipt.request_digest =
        Engine::native_request_digest("auth.unrelated", "admin", &json!({})).unwrap();
    next.events.last_mut().unwrap().kind = "auth.unrelated".into();
    assert!(view.advance(&previous, &next).is_err());
    assert!(view.advance(&decoded(&previous), &decoded(&next)).is_err());
}

#[test]
fn merge_link_source_rechecks_admitted_identity_and_complete_first_parent_ancestry() {
    let mut fixture = Fixture::new();
    fixture.exit(0, MembershipExitAction::Deregister);
    let previous = Arc::clone(&fixture.engine.state);
    let view = fixture.engine.current_namespaces.clone().unwrap();
    let link = fixture.link(op()).unwrap();
    for attack in 0..12 {
        let mut state = previous.as_ref().clone();
        let mut source_link = link.clone();
        match attack {
            0 => {
                state
                    .branches
                    .get_mut(&link.source_branch_id)
                    .unwrap()
                    .branch_id = link.target_branch_id.clone();
            }
            1 => {
                state
                    .branches
                    .get_mut(&link.target_branch_id)
                    .unwrap()
                    .history_tree_id = new_id("history").unwrap();
            }
            2 => {
                state
                    .commits
                    .get_mut(&link.source_commit_id)
                    .unwrap()
                    .commit_id = new_id("commit").unwrap();
            }
            3 => {
                state
                    .commits
                    .get_mut(&fixture.branch.head_commit_id)
                    .unwrap()
                    .history_tree_id = new_id("history").unwrap();
            }
            4 => {
                state
                    .histories
                    .get_mut(&link.history_tree_id)
                    .unwrap()
                    .history_tree_id = new_id("history").unwrap();
            }
            5 => {
                state
                    .repositories
                    .get_mut(&fixture.repository.repository_id)
                    .unwrap()
                    .repository_id = new_id("repo").unwrap();
            }
            6 => {
                state.commits.remove(&fixture.branch.fork_commit_id);
            }
            7 => {
                state
                    .branches
                    .get_mut(&link.source_branch_id)
                    .unwrap()
                    .head_commit_id = fixture.source.clone();
            }
            8 => {
                state
                    .branches
                    .get_mut(&link.source_branch_id)
                    .unwrap()
                    .state = BranchState::Migrated;
            }
            9 => {
                state
                    .commits
                    .get_mut(&fixture.branch.head_commit_id)
                    .unwrap()
                    .parent_commit_ids = vec![fixture.branch.head_commit_id.clone()];
            }
            10 => {
                source_link.source_commit_id = fixture.alias.head_commit_id.clone();
            }
            11 => {
                state
                    .branches
                    .get_mut(&link.target_branch_id)
                    .unwrap()
                    .file_id = Some(fixture.retained.file_id.clone());
            }
            _ => unreachable!(),
        }
        assert!(
            view.merge_link_source(&state, &source_link).is_err(),
            "identity {attack}"
        );
    }
}

#[test]
fn link_requires_closed_planning_proof_instead_of_generic_kind_admission() {
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
    let link = fixture.link(operation).unwrap();
    let published = Arc::clone(&fixture.engine.state);
    let receipt = published.operations[&operation].clone();
    let mut context = fixture.engine.planning_context();
    context.state = &previous;
    context.next_slot = published.sequence;
    let generic = context.plan_new_mutation(
        KIND,
        "admin",
        operation,
        receipt.request_digest.clone(),
        |_, _, _| panic!("generic permit must reject before relation preparation"),
    );
    let Err(error) = generic else {
        panic!("relations must not be generally allowed")
    };
    assert_eq!(error.code(), ErrorCode::RecoveryRequired);
    assert!(
        view.verify_new_merge_link(&previous, &previous, &receipt)
            .is_err()
    );
    for attack in 0..7 {
        let digest = if attack == 1 {
            "f".repeat(64)
        } else {
            receipt.request_digest.clone()
        };
        let candidate =
            context.plan_merge_link_mutation("admin", operation, digest, |_, next, prepared| {
                if attack != 5 {
                    next.merge_links
                        .insert(link.merge_link_id.clone(), link.clone());
                }
                let mut result = receipt.result.clone();
                if attack == 2 {
                    result["namespace_merge_link"]["registry"] = Value::Null;
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
                if attack == 6 {
                    next.business_generation += 1;
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
                panic!("exact metadata-only relation must leave planning")
            };
            assert!(prepared.is_empty());
            view.verify_new_merge_link(&previous, &next, &result)
                .unwrap();
            same_scopes(&view, &view.advance(&previous, &next).unwrap());
            let mut different_result = result.clone();
            different_result.request_digest = "b".repeat(64);
            assert!(
                view.verify_new_merge_link(&previous, &next, &different_result)
                    .is_err()
            );
        } else {
            let Err(error) = candidate else {
                panic!("unproven relation {attack} left planning")
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
        context.plan_merge_link_mutation("admin", operation, receipt.request_digest, |_, _, _| {
            panic!("pending registry must reject before preparation")
        });
    let Err(error) = pending else {
        panic!("pending registry must not admit relations")
    };
    assert_eq!(error.code(), ErrorCode::RecoveryRequired);
    assert!(Arc::ptr_eq(&published, &fixture.engine.state));
}

// Real hash-bound journal decisions bypass live planning only in this fixture.
// Fixed membership, snapshot/history closure and receipts stay authenticated so
// cold recovery must reach the operation-specific proof before repairing them.
fn install_link_decision(
    engine: &mut Engine,
    next: &State,
    result: &OperationResult,
    published: bool,
    versions: Vec<crate::journal::VersionEvidence>,
) -> crate::journal::Candidate {
    use crate::journal::*;

    let members = engine.members();
    let snapshot = prepare_snapshot(next, crate::engine::MAX_METADATA).unwrap();
    snapshot
        .store_new_objects_at(&engine.disk_resources, &members)
        .unwrap();
    let catalog = crate::history_package::store_history_manifests_bounded_at(
        &engine.disk_resources,
        &members,
        next,
        crate::engine::MAX_METADATA,
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
fn cold_link_rejects_authenticated_forgery_before_replica_repair_and_projection() {
    use crate::engine::MutationPlan;
    use crate::journal::object_path;

    for published in [false, true] {
        for attack in ["valid", "source", "actor", "lineage", "location", "payload"] {
            let mut fixture = Fixture::new();
            let stopped = fixture.exit(0, MembershipExitAction::Deregister);
            let parked = fixture.park_root(0);
            let operation = op();
            let MutationPlan::Prepared {
                mut next,
                prepared,
                mut result,
            } = fixture.plan(operation)
            else {
                panic!("native relation must prepare a candidate")
            };
            assert!(prepared.is_empty());
            let link_id = result.result["merge_link_id"].as_str().unwrap().to_owned();
            match attack {
                "valid" | "payload" => (),
                "source" => {
                    result.result["namespace_merge_link"]["mask_hash"] = Value::Null;
                    next.operations.insert(operation, result.clone());
                }
                "actor" => {
                    next.operation_principals.insert(operation, "other".into());
                }
                "lineage" => {
                    // Both endpoints still name existing same-history records.
                    // Rehash their native request and matching declared source so
                    // only first-parent membership rejects the false relation.
                    let link = next.merge_links.get_mut(&link_id).unwrap();
                    link.source_commit_id = fixture.alias.head_commit_id.clone();
                    result.request_digest = Engine::native_request_digest(
                        KIND,
                        "admin",
                        &json!({
                            "source_branch_id": link.source_branch_id,
                            "source_commit_id": link.source_commit_id,
                            "target_branch_id": link.target_branch_id,
                            "target_commit_id": link.target_commit_id,
                        }),
                    )
                    .unwrap();
                    result.result["namespace_merge_link"]["source"]["selected_commit_id"] =
                        json!(link.source_commit_id);
                    next.operations.insert(operation, result.clone());
                }
                "location" => {
                    let name = next.file_names[&fixture.retained.file_id].clone();
                    next.file_locations.insert(
                        fixture.retained.file_id.clone(),
                        format!("unrequested/{name}"),
                    );
                }
                _ => unreachable!(),
            }
            let expected_link = next.merge_links[&link_id].clone();
            // The State, actor and exact metadata-only receipt remain valid.
            // Only the independently authenticated Candidate envelope is forged.
            let versions = if attack == "payload" {
                fixture
                    .engine
                    .current_namespaces
                    .as_ref()
                    .unwrap()
                    .verify_new_merge_link(&fixture.engine.state, &next, &result)
                    .unwrap();
                let target = fixture
                    .engine
                    .namespace_tree(&fixture.alias.branch_id)
                    .unwrap();
                let version = &target.get("target").unwrap().version_id;
                vec![crate::journal::VersionEvidence {
                    version: next.versions[version].clone(),
                    placement: next.placements[version].clone(),
                }]
            } else {
                Vec::new()
            };
            let candidate =
                install_link_decision(&mut fixture.engine, &next, &result, published, versions);
            let payload_path = candidate.versions.first().map(|evidence| {
                let disk = fixture
                    .engine
                    .disk(&evidence.placement.route_slot_ids[0])
                    .unwrap();
                assert!(
                    !disk.metadata_member,
                    "the attack must use live data-only D04"
                );
                assert_eq!(disk.slot_id, "D04");
                crate::disk::version_dir(
                    disk,
                    &evidence.version.history_tree_id,
                    &evidence.version.version_id,
                )
            });
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
            fs::remove_file(&missing).unwrap();
            let before_system = files(&system);
            let before_roots = live_roots
                .iter()
                .map(|root| files(root))
                .collect::<Vec<_>>();
            let payload_watch = payload_path
                .as_ref()
                .map(|path| watch_candidate_payload(path));
            let opened = Engine::open(&system);
            if let Some(watch) = payload_watch {
                assert_no_candidate_payload_io(watch);
            }
            if attack == "valid" {
                let mut engine = opened.unwrap();
                assert_eq!(fs::read(&missing).unwrap(), snapshot_bytes);
                assert_eq!(engine.state.merge_links[&link_id], expected_link);
                assert_eq!(
                    engine
                        .add_merge_link(
                            &fixture.branch.branch_id,
                            &fixture.source,
                            &fixture.alias.branch_id,
                            &fixture.alias.head_commit_id,
                            "admin",
                            operation,
                        )
                        .unwrap(),
                    expected_link
                );
                assert_eq!(engine.state.operations[&operation], result);
                assert!(
                    engine
                        .namespace_tree(&fixture.branch.branch_id)
                        .unwrap()
                        .get("dir/gone")
                        .is_none()
                );
            } else {
                let error = opened
                    .err()
                    .expect("cold replay must reject the relation proof");
                assert_eq!(
                    error.code(),
                    ErrorCode::PublicationInvalid,
                    "{published} {attack}"
                );
                if attack == "payload" {
                    assert!(
                        error
                            .to_string()
                            .contains("metadata-only current-registry candidate"),
                        "{error}"
                    );
                } else {
                    assert!(error.to_string().contains("merge link differs"), "{error}");
                }
                assert_eq!(
                    fs::symlink_metadata(&missing).unwrap_err().kind(),
                    std::io::ErrorKind::NotFound
                );
                assert_eq!(
                    files(&system),
                    before_system,
                    "system changed: {published} {attack}"
                );
                for (root, bytes) in live_roots.iter().zip(before_roots) {
                    assert_eq!(
                        files(root),
                        bytes,
                        "live root changed: {published} {attack}"
                    );
                }
            }
            assert_eq!(files(&parked), stopped);
            assert_eq!(
                fs::read(&fixture.config.disks[0].root).unwrap(),
                b"stopped root must not be opened"
            );
        }
    }
}

#[cfg(feature = "fault-injection")]
#[test]
fn decided_link_recovers_once_across_publication_boundaries() {
    for point in [
        "after_finalize",
        "after_container_names",
        "after_publication_1",
    ] {
        let mut fixture = Fixture::new();
        let stopped = fixture.exit(0, MembershipExitAction::Deregister);
        let parked = fixture.park_root(0);
        let previous = Arc::clone(&fixture.engine.state);
        let view = fixture.engine.current_namespaces.clone().unwrap();
        let operation = op();
        fixture.engine.inject_fault_once(point);
        let error = fixture.link(operation).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InDoubt, "{point}");
        let Error::Context { source, .. } = error else {
            panic!("indeterminate relation must retain its cause")
        };
        assert_eq!(source.code(), ErrorCode::InjectedFailure);
        assert!(fixture.engine.is_read_only());
        assert_eq!(*fixture.engine.state, *previous);
        same_scopes(&view, fixture.engine.current_namespaces.as_ref().unwrap());
        let system = fixture.temp.path().join("system");
        drop(fixture.engine);
        let mut engine = Engine::open(&system).unwrap();
        let published = Arc::clone(&engine.state);
        let link = engine
            .add_merge_link(
                &fixture.branch.branch_id,
                &fixture.source,
                &fixture.alias.branch_id,
                &fixture.alias.head_commit_id,
                "admin",
                operation,
            )
            .unwrap();
        verify_write_set(&previous, &published, &link, operation).unwrap();
        assert!(Arc::ptr_eq(&published, &engine.state));
        assert!(
            engine
                .namespace_tree(&fixture.branch.branch_id)
                .unwrap()
                .get("dir/gone")
                .is_none()
        );
        assert_eq!(files(&parked), stopped);
        drop(engine);
        let mut engine = Engine::open(&system).unwrap();
        assert_eq!(
            engine
                .add_merge_link(
                    &fixture.branch.branch_id,
                    &fixture.source,
                    &fixture.alias.branch_id,
                    &fixture.alias.head_commit_id,
                    "admin",
                    operation,
                )
                .unwrap(),
            link
        );
        assert_eq!(files(&parked), stopped);
    }
}

fn watch_candidate_payload(path: &Path) -> rustix::fd::OwnedFd {
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

fn assert_no_candidate_payload_io(watch: rustix::fd::OwnedFd) {
    let mut bytes = [0u8; 4096];
    assert_eq!(
        rustix::io::read(&watch, &mut bytes),
        Err(rustix::io::Errno::WOULDBLOCK),
        "a metadata-only relation must not open or read the smuggled local proof or payload"
    );
}
