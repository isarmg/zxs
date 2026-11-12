//! Real temporary-root Engine/D3 publications, plus cold decoded-state proofs.
//! These are regression sources, not physical-device or HTTP acceptance evidence.
use super::*;
use crate::*;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

fn op() -> OperationId {
    OperationId::generate().unwrap()
}
fn name(value: &str) -> BranchName {
    BranchName::parse(value).unwrap()
}
fn save(engine: &mut Engine, branch: &str, path: &str, bytes: &[u8]) -> Entry {
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
            message: "current file tags fixture".into(),
            request_digest: None,
        })
        .unwrap();
    engine
        .namespace_tree(branch)
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
fn cold(state: &State) -> State {
    json_codec::from_slice(&canonical_json(state).unwrap()).unwrap()
}

struct Fixture {
    temp: tempfile::TempDir,
    engine: Engine,
    config: Config,
    repository: Repository,
    hidden: Entry,
    first: Entry,
    current: Entry,
    branch: String,
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
        let hidden_rule = new_id("rule").unwrap();
        let live_rule = new_id("rule").unwrap();
        for (rule, path) in [(&hidden_rule, "dir/gone"), (&live_rule, "dir/live")] {
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
        let hidden = save(
            &mut engine,
            &repository.main_branch_id,
            "dir/gone",
            b"target only",
        );
        let first = save(
            &mut engine,
            &repository.main_branch_id,
            "dir/live",
            b"historical source",
        );
        for (rule, path) in [(&hidden_rule, "dir/gone"), (&live_rule, "dir/live")] {
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
        let current = save(
            &mut engine,
            &repository.main_branch_id,
            "dir/live",
            b"retained body",
        );
        let branch = engine
            .file_context(&repository.repository_id, "dir/live")
            .unwrap()
            .branch_id;
        Self {
            temp,
            engine,
            config,
            repository,
            hidden,
            first,
            current,
            branch,
        }
    }
    fn exit(&mut self, action: MembershipExitAction) -> BTreeMap<PathBuf, Vec<u8>> {
        let disk = self.config.disks[0].clone();
        let held = Arc::clone(self.engine.disk_resources.get(&disk).unwrap());
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
        let physical = files(&disk.root);
        self.engine.activate_pending_disk_exit().unwrap();
        held.root.io_binding().unwrap().closed_proof().unwrap();
        assert!(self.engine.disk_resources.available(&disk).is_none());
        assert_eq!(files(&disk.root), physical);
        physical
    }
    fn hidden_absent(&self) {
        assert!(
            self.engine
                .namespace_tree(&self.repository.main_branch_id)
                .unwrap()
                .get("dir/gone")
                .is_none()
        );
        let view = self.engine.current_namespaces.as_ref().unwrap();
        for scope in view.branches.values().chain(view.directories.values()) {
            assert!(
                scope
                    .entries
                    .iter()
                    .all(|entry| entry.file_id != self.hidden.file_id)
            );
        }
    }
}

#[test]
fn current_container_tags_live_cold_replay_rejects_extra_writes_and_forged_requests() {
    for action in [
        MembershipExitAction::Deregister,
        MembershipExitAction::PlannedOffline,
    ] {
        let mut fixture = Fixture::new();
        fixture.exit(action);
        fixture.hidden_absent();
        let request = storage_contract::file_container_tags::ReplaceCommand {
            file_id: fixture.current.file_id.parse().unwrap(),
            context: storage_contract::file_container_tags::Context {
                repository_id: fixture.repository.repository_id.parse().unwrap(),
                path: name_policy::PortablePath::parse("dir/live").unwrap(),
            },
            expected_generation: storage_contract::Generation::new(0),
            tags: storage_contract::file_container_tags::TagSet::new([
                storage_contract::file_container_tags::Tag::parse("review".into()).unwrap(),
            ])
            .unwrap(),
        };
        let previous = cold(fixture.engine.state());
        let view = fixture.engine.current_namespaces.clone().unwrap();
        let receipt = fixture
            .engine
            .replace_container_tags(&request, &"admin".parse().unwrap(), op())
            .unwrap();
        let next = cold(fixture.engine.state());
        view.verify_new_file_tags(&previous, &next, &receipt).unwrap();
        assert_eq!(previous.branches, next.branches);
        assert_eq!(previous.directory_trees, next.directory_trees);
        assert_eq!(previous.file_metadata_nodes, next.file_metadata_nodes);
        for corruption in 0..5 {
            let mut bad = cold(&next);
            match corruption {
                0 => {
                    bad.file_notes
                        .insert(fixture.current.file_id.clone(), "forged".into());
                }
                1 => {
                    bad.container_tags.insert(
                        request.file_id,
                        storage_contract::file_container_tags::ContainerTags::empty(),
                    );
                }
                2 => {
                    bad.operations.get_mut(&receipt.operation_id).unwrap().result["request"]
                        ["expected_generation"] = json_codec::json!("1");
                }
                3 => {
                    bad.events.pop();
                }
                4 => {
                    bad.operation_principals
                        .insert(receipt.operation_id, "other".into());
                }
                _ => unreachable!(),
            }
            let supplied = bad.operations.get(&receipt.operation_id).unwrap();
            assert!(view.verify_new_file_tags(&previous, &bad, supplied).is_err());
        }
        drop(fixture.engine);
        let engine = Engine::open(&fixture.temp.path().join("system")).unwrap();
        let snapshot = engine
            .file_container_tags(&storage_contract::file_container_tags::ReadRequest {
                file_id: request.file_id,
                context: request.context.clone(),
            })
            .unwrap();
        assert_eq!(snapshot.tags, request.tags);
        assert_eq!(snapshot.generation, storage_contract::Generation::new(1));
    }
}
