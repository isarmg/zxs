//! Actual D3 exit publications followed by ordinary file-branch consumers.
//! These tests use temporary roots, not physical-device or power-loss evidence.
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
fn name(value: &str) -> BranchName {
    BranchName::parse(value).unwrap()
}

fn save(engine: &mut Engine, branch: &str, path: &str, bytes: &[u8]) {
    let workspace = engine.create_workspace(branch, "admin").unwrap();
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
            message: "current file branch fixture".into(),
            request_digest: None,
        })
        .unwrap();
}

fn entry(engine: &Engine, repository: &Repository, path: &str) -> Entry {
    engine
        .namespace_tree(&repository.main_branch_id)
        .unwrap()
        .get(path)
        .unwrap()
        .clone()
}

fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut result = BTreeMap::new();
    for child in fs::read_dir(root).unwrap() {
        let path = child.unwrap().path();
        if path.is_dir() {
            result.extend(files(&path));
        } else {
            result.insert(path.clone(), fs::read(path).unwrap());
        }
    }
    result
}

fn branch(
    engine: &mut Engine,
    source: &Entry,
    label: &str,
    operation: OperationId,
) -> OperationResult {
    engine
        .create_file_branch(
            &source.file_id,
            &source.version_id,
            &name(label),
            "admin",
            operation,
        )
        .unwrap()
}

struct Fixture {
    temp: tempfile::TempDir,
    engine: Engine,
    config: Config,
    repository: Repository,
    hidden: Entry,
    first: Entry,
    current: Entry,
    file_main: String,
    alternate: String,
    old_creation: OperationResult,
}

impl Fixture {
    fn new(shared_copies: u32) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let mut config =
            Config::development((0..4).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
        config.disks[3].level = 1;
        config.disks[3].upstream_slot_id = None;
        config.disks[3].metadata_member = false;
        let mut engine = Engine::initialize(&temp.path().join("system"), config.clone()).unwrap();
        let repository = engine.create_disk_root("D04", "admin", op()).unwrap();
        let hidden_rule = new_id("rule").unwrap();
        let shared_rule = new_id("rule").unwrap();
        for (rule, path) in [(&hidden_rule, "dir/gone"), (&shared_rule, "dir/live")] {
            engine
                .update_placement_rule(PlacementRuleUpdate {
                    rule_id: rule,
                    repository_id: &repository.repository_id,
                    path_prefix: path,
                    primary_slot_id: "P01",
                    expected_generation: 0,
                    principal: "admin",
                    operation_id: op(),
                })
                .unwrap();
        }
        let policy = new_id("policy").unwrap();
        engine
            .update_protection_policy(ProtectionPolicyUpdate {
                policy_id: &policy,
                repository_id: &repository.repository_id,
                path_prefix: "dir/live",
                target_copies: shared_copies,
                expected_generation: 0,
                principal: "admin",
                operation_id: op(),
            })
            .unwrap();
        save(
            &mut engine,
            &repository.main_branch_id,
            "dir/gone",
            b"target only",
        );
        save(
            &mut engine,
            &repository.main_branch_id,
            "dir/live",
            b"original source",
        );
        let hidden = entry(&engine, &repository, "dir/gone");
        let first = entry(&engine, &repository, "dir/live");
        let file_main = engine.state.file_selections[&first.file_id].clone();
        let old_creation = branch(&mut engine, &first, "alternate", op());
        let alternate = old_creation.branch_id.clone().unwrap();
        engine
            .select_file_branch(&first.file_id, &alternate, "admin", op())
            .unwrap();
        save(&mut engine, &alternate, "dir/live", b"alternate source");
        engine
            .select_file_branch(&first.file_id, &file_main, "admin", op())
            .unwrap();
        let old_placements = engine.state.placements.clone();
        assert_eq!(
            old_placements[&first.version_id].target_copies,
            shared_copies
        );
        // D04 has no backup successor. Change only the future copy policy while
        // P01 can still satisfy it, then move the future route to D04.
        engine
            .update_protection_policy(ProtectionPolicyUpdate {
                policy_id: &policy,
                repository_id: &repository.repository_id,
                path_prefix: "dir/live",
                target_copies: 1,
                expected_generation: 1,
                principal: "admin",
                operation_id: op(),
            })
            .unwrap();
        for (rule, path) in [(&hidden_rule, "dir/gone"), (&shared_rule, "dir/live")] {
            engine
                .update_placement_rule(PlacementRuleUpdate {
                    rule_id: rule,
                    repository_id: &repository.repository_id,
                    path_prefix: path,
                    primary_slot_id: "D04",
                    expected_generation: 1,
                    principal: "admin",
                    operation_id: op(),
                })
                .unwrap();
        }
        assert_eq!(
            engine.state.placements, old_placements,
            "policy and route updates preserve every historical placement"
        );
        save(
            &mut engine,
            &repository.main_branch_id,
            "dir/live",
            b"current retained",
        );
        let current = entry(&engine, &repository, "dir/live");
        let current_placement = &engine.state.placements[&current.version_id];
        assert_eq!(current_placement.target_copies, 1);
        assert_eq!(current_placement.route_slot_ids, ["D04"]);
        for (version, placement) in old_placements.iter() {
            assert_eq!(&engine.state.placements[version], placement);
        }
        Self {
            temp,
            engine,
            config,
            repository,
            hidden,
            first,
            current,
            file_main,
            alternate,
            old_creation,
        }
    }

    fn exit(&mut self, target: usize, action: MembershipExitAction) -> BTreeMap<PathBuf, Vec<u8>> {
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
        self.engine.resume_membership_joint_decision().unwrap();
        let before = files(&disk.root);
        let receipt = self.engine.activate_pending_disk_exit().unwrap();
        assert_eq!(receipt.result["target_io"], "closed");
        resource.root.io_binding().unwrap().closed_proof().unwrap();
        assert!(self.engine.disk_resources.available(&disk).is_none());
        assert_eq!(
            files(&disk.root),
            before,
            "exit does not create a target backup or erase bytes"
        );
        before
    }

    fn inputs(&self) -> Vec<FileMergeInput> {
        [&self.file_main, &self.alternate]
            .into_iter()
            .map(|branch| FileMergeInput {
                branch_id: branch.clone(),
                expected_node_id: crate::business::file_nodes::branch_head(
                    self.engine.state(),
                    branch,
                )
                .unwrap(),
            })
            .collect()
    }

    fn assert_hidden(&self) {
        assert!(
            self.engine
                .namespace_tree(&self.repository.main_branch_id)
                .unwrap()
                .get("dir/gone")
                .is_none()
        );
        for branch in self
            .engine
            .state
            .directory_branches
            .values()
            .filter(|branch| branch.repository_id == self.repository.repository_id)
        {
            assert!(
                self.engine
                    .current_directory_entries(&branch.directory_branch_id)
                    .unwrap()
                    .iter()
                    .all(|entry| entry.file_id != self.hidden.file_id)
            );
        }
    }
}

#[test]
fn disk_exit_current_file_create_select_default_merge_and_restart_preserve_history() {
    let mut fixture = Fixture::new(2);
    let hidden_directory = fixture
        .engine
        .create_directory_branches(
            &fixture.repository.repository_id,
            Some(&name_policy::LookupPath::parse("dir").unwrap()),
            &name("unselected current directory"),
            "admin",
            op(),
        )
        .unwrap();
    let hidden_directory = hidden_directory.result["directory_branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let old_commits = fixture.engine.state.commits.clone();
    let old_trees = fixture.engine.state.directory_trees.clone();
    let versions = canonical_json(&fixture.engine.state.versions).unwrap();
    let target = fixture.exit(0, MembershipExitAction::Deregister);
    fixture.assert_hidden();
    let created = branch(&mut fixture.engine, &fixture.first, "historical body", op());
    assert_eq!(created.result["materialized"], false);
    assert_eq!(created.result["selected"], false);
    let pending = created.branch_id.as_ref().unwrap();
    let pending_entries = fixture.engine.namespace_snapshot(pending).unwrap();
    assert_eq!(
        pending_entries
            .entries()
            .iter()
            .find(|entry| entry.kind == NamespaceEntryKind::File)
            .unwrap()
            .version_id,
        fixture.first.version_id
    );
    fixture
        .engine
        .select_file_branch_at(
            &fixture.repository.repository_id,
            fixture.current.location(),
            &fixture.current.file_id,
            &fixture.alternate,
            "admin",
            op(),
        )
        .unwrap();
    let expected_head = fixture.engine.state.branches[&fixture.repository.main_branch_id]
        .head_commit_id
        .clone();
    let node = crate::business::file_nodes::branch_head(fixture.engine.state(), &fixture.file_main)
        .unwrap();
    let default_operation = op();
    let default_receipt = fixture
        .engine
        .set_file_default(FileDefaultSelection {
            file_id: &fixture.current.file_id,
            branch_id: &fixture.file_main,
            repository_id: &fixture.repository.repository_id,
            path: fixture.current.location(),
            expected_node_id: &node,
            expected_head: &expected_head,
            principal: "admin",
            operation_id: default_operation,
        })
        .unwrap();
    let inputs = fixture.inputs();
    let operation = op();
    let merge = |engine: &mut Engine| {
        engine.merge_file_branches(FileBranchMergeRequest {
            file_id: &fixture.current.file_id,
            inputs: &inputs,
            chosen_node_id: &fixture.first.version_id,
            surviving_branch_id: &fixture.file_main,
            location: None,
            principal: "admin",
            operation_id: operation,
        })
    };
    let result = merge(&mut fixture.engine).unwrap();
    assert_eq!(result.result["version_id"], fixture.first.version_id);
    assert_eq!(result.result["reused_node"], false);
    assert!(
        result.result["directory_branch_ids"]
            .as_array()
            .unwrap()
            .iter()
            .any(|branch| branch.as_str() == Some(hidden_directory.as_str())),
        "unselected current directories participate too"
    );
    assert_eq!(merge(&mut fixture.engine).unwrap(), result);
    assert_eq!(
        fixture
            .engine
            .set_file_default(FileDefaultSelection {
                file_id: &fixture.current.file_id,
                branch_id: &fixture.file_main,
                repository_id: &fixture.repository.repository_id,
                path: fixture.current.location(),
                expected_node_id: &node,
                expected_head: &expected_head,
                principal: "admin",
                operation_id: default_operation,
            })
            .unwrap(),
        default_receipt,
        "default receipt survives a later changed head"
    );
    assert_eq!(
        fixture.engine.state.branches[&fixture.alternate].state,
        BranchState::Merged
    );
    fixture.assert_hidden();
    assert_eq!(
        canonical_json(&fixture.engine.state.versions).unwrap(),
        versions
    );
    for (id, commit) in old_commits.iter() {
        assert_eq!(fixture.engine.state.commits.get(id), Some(commit));
    }
    for (id, tree) in old_trees.iter() {
        assert_eq!(fixture.engine.state.directory_trees.get(id), Some(tree));
    }
    let mut bytes = Vec::new();
    fixture
        .engine
        .open_content(&fixture.first.version_id)
        .unwrap()
        .file
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(
        bytes, b"original source",
        "merge reads a retained backup after the original primary exited"
    );
    assert_eq!(files(&fixture.config.disks[0].root), target);
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    let mut engine = Engine::open(&system).unwrap();
    assert_eq!(merge(&mut engine).unwrap(), result);
    assert!(
        engine
            .namespace_tree(&fixture.repository.main_branch_id)
            .unwrap()
            .get("dir/gone")
            .is_none()
    );
    assert_eq!(
        engine
            .namespace_tree(&fixture.repository.main_branch_id)
            .unwrap()
            .get("dir/live")
            .unwrap()
            .version_id,
        fixture.first.version_id
    );
    assert_eq!(files(&fixture.config.disks[0].root), target);
}

#[test]
fn disk_exit_current_file_same_name_replacement_never_adopts_old_identity() {
    let mut fixture = Fixture::new(2);
    let old_branch = fixture.engine.state.file_selections[&fixture.hidden.file_id].clone();
    let historical_head = fixture.engine.state.branches[&fixture.repository.main_branch_id]
        .head_commit_id
        .clone();
    let target = fixture.exit(0, MembershipExitAction::Deregister);
    save(
        &mut fixture.engine,
        &fixture.repository.main_branch_id,
        "dir/gone",
        b"new identity",
    );
    let replacement = entry(&fixture.engine, &fixture.repository, "dir/gone");
    assert_ne!(replacement.file_id, fixture.hidden.file_id);
    assert_ne!(replacement.entry_id, fixture.hidden.entry_id);
    let before = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(
        fixture
            .engine
            .create_file_branch(
                &fixture.hidden.file_id,
                &fixture.hidden.version_id,
                &name("forbidden old identity"),
                "admin",
                op()
            )
            .unwrap_err()
            .code(),
        ErrorCode::FileNotFound
    );
    assert_eq!(
        fixture
            .engine
            .select_file_branch_at(
                &fixture.repository.repository_id,
                replacement.location(),
                &fixture.hidden.file_id,
                &old_branch,
                "admin",
                op()
            )
            .unwrap_err()
            .code(),
        ErrorCode::FileNotFound
    );
    let head = fixture.engine.state.branches[&fixture.repository.main_branch_id]
        .head_commit_id
        .clone();
    assert_eq!(
        fixture
            .engine
            .set_file_default(FileDefaultSelection {
                file_id: &fixture.hidden.file_id,
                branch_id: &old_branch,
                repository_id: &fixture.repository.repository_id,
                path: replacement.location(),
                expected_node_id: &fixture.hidden.version_id,
                expected_head: &head,
                principal: "admin",
                operation_id: op(),
            })
            .unwrap_err()
            .code(),
        ErrorCode::FileNotFound
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    assert_eq!(
        fixture
            .engine
            .namespace_tree_at(&fixture.repository.main_branch_id, &historical_head)
            .unwrap()
            .get("dir/gone")
            .unwrap()
            .file_id,
        fixture.hidden.file_id
    );
    assert_eq!(files(&fixture.config.disks[0].root), target);
}

#[test]
fn disk_disabled_current_file_metadata_choices_do_not_grant_body_writes() {
    let mut fixture = Fixture::new(2);
    let target = fixture.exit(3, MembershipExitAction::PlannedOffline);
    assert!(
        fixture
            .engine
            .registered_disk(&fixture.config.disks[3].disk_id)
            .is_some()
    );
    assert!(
        fixture
            .engine
            .namespace_tree(&fixture.repository.main_branch_id)
            .unwrap()
            .get("dir/live")
            .is_some()
    );
    assert_eq!(
        fixture
            .engine
            .create_file_branch(
                &fixture.current.file_id,
                &fixture.current.version_id,
                &name("disabled creation"),
                "admin",
                op()
            )
            .unwrap_err()
            .code(),
        ErrorCode::BranchNotActive
    );
    assert_eq!(
        fixture
            .engine
            .select_file_branch(&fixture.current.file_id, &fixture.alternate, "admin", op())
            .unwrap_err()
            .code(),
        ErrorCode::BranchNotActive
    );
    let head = fixture.engine.state.branches[&fixture.repository.main_branch_id]
        .head_commit_id
        .clone();
    let node = crate::business::file_nodes::branch_head(fixture.engine.state(), &fixture.alternate)
        .unwrap();
    assert_eq!(
        fixture
            .engine
            .set_file_default(FileDefaultSelection {
                file_id: &fixture.current.file_id,
                branch_id: &fixture.alternate,
                repository_id: &fixture.repository.repository_id,
                path: fixture.current.location(),
                expected_node_id: &node,
                expected_head: &head,
                principal: "admin",
                operation_id: op(),
            })
            .unwrap_err()
            .code(),
        ErrorCode::BranchNotActive
    );
    let inputs = fixture.inputs();
    assert_eq!(
        fixture
            .engine
            .merge_file_branches(FileBranchMergeRequest {
                file_id: &fixture.current.file_id,
                inputs: &inputs,
                chosen_node_id: &fixture.first.version_id,
                surviving_branch_id: &fixture.file_main,
                location: None,
                principal: "admin",
                operation_id: op(),
            })
            .unwrap_err()
            .code(),
        ErrorCode::BranchNotActive
    );
    assert!(
        fixture
            .engine
            .create_workspace(&fixture.repository.main_branch_id, "admin")
            .is_err()
    );
    assert_eq!(
        fixture
            .engine
            .create_file_branch(
                &fixture.first.file_id,
                &fixture.first.version_id,
                &name("alternate"),
                "admin",
                fixture.old_creation.operation_id
            )
            .unwrap(),
        fixture.old_creation,
        "successful permanent receipts replay before current registry checks"
    );
    assert_eq!(files(&fixture.config.disks[3].root), target);
}

#[test]
fn disk_exit_current_file_merge_rejects_unavailable_chosen_historical_body() {
    let mut fixture = Fixture::new(1);
    let target = fixture.exit(0, MembershipExitAction::Deregister);
    let before_creation = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(
        fixture
            .engine
            .create_file_branch(
                &fixture.current.file_id,
                &fixture.first.version_id,
                &name("unavailable source"),
                "admin",
                op()
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkspaceChanged
    );
    assert_eq!(
        canonical_json(fixture.engine.state()).unwrap(),
        before_creation
    );

    let other = branch(
        &mut fixture.engine,
        &fixture.current,
        "retained input",
        op(),
    )
    .branch_id
    .unwrap();
    let inputs = [&fixture.file_main, &other]
        .into_iter()
        .map(|branch| FileMergeInput {
            branch_id: branch.clone(),
            expected_node_id: crate::business::file_nodes::branch_head(
                fixture.engine.state(),
                branch,
            )
            .unwrap(),
        })
        .collect::<Vec<_>>();
    let before = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(
        fixture
            .engine
            .merge_file_branches(FileBranchMergeRequest {
                file_id: &fixture.current.file_id,
                inputs: &inputs,
                chosen_node_id: &fixture.first.version_id,
                surviving_branch_id: &fixture.file_main,
                location: None,
                principal: "admin",
                operation_id: op(),
            })
            .unwrap_err()
            .code(),
        ErrorCode::ContentUnavailable
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    assert_eq!(files(&fixture.config.disks[0].root), target);
}

#[test]
fn disk_exit_current_file_merge_rejects_unclosed_frozen_child_references() {
    let mut fixture = Fixture::new(2);
    fixture
        .engine
        .create_directory_branches(
            &fixture.repository.repository_id,
            None,
            &name("frozen parent"),
            "admin",
            op(),
        )
        .unwrap();
    save(
        &mut fixture.engine,
        &fixture.repository.main_branch_id,
        "dir/live",
        b"newer named child",
    );
    let target = fixture.exit(0, MembershipExitAction::Deregister);
    let inputs = fixture.inputs();
    let before = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(
        fixture
            .engine
            .merge_file_branches(FileBranchMergeRequest {
                file_id: &fixture.current.file_id,
                inputs: &inputs,
                chosen_node_id: &fixture.first.version_id,
                surviving_branch_id: &fixture.file_main,
                location: None,
                principal: "admin",
                operation_id: op(),
            })
            .unwrap_err()
            .code(),
        ErrorCode::RecoveryRequired
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    assert_eq!(files(&fixture.config.disks[0].root), target);
}

#[path = "current_history_merge_tests.rs"]
mod history_merge_tests;

#[path = "current_selected_file_merge_tests.rs"]
mod selected_merge_tests;
