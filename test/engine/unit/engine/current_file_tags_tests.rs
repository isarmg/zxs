//! Real temporary-root Engine/D3 publications, plus cold decoded-state proofs.
//! These are regression sources, not physical-device or HTTP acceptance evidence.
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
fn tag_target(engine: &Engine, branch: &str, source: &str) -> FileTagTarget {
    FileTagTarget {
        branch_id: branch.into(),
        source_node_id: source.into(),
        expected_node_id: engine.file_branch_head(branch).unwrap(),
    }
}
fn edit(
    engine: &mut Engine,
    file: &str,
    targets: &[FileTagTarget],
    add: &[String],
    operation: OperationId,
) -> Result<OperationResult> {
    engine.update_file_node_tags(FileNodeTagsUpdateRequest {
        file_id: file,
        targets,
        add,
        remove: &[],
        location: None,
        principal: "admin",
        operation_id: operation,
    })
}
fn node(receipt: &OperationResult) -> String {
    receipt.result["nodes"][0]["node_id"]
        .as_str()
        .unwrap()
        .into()
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
fn disk_exit_current_file_tags_references_query_retry_noop_and_cold_open() {
    for action in [
        MembershipExitAction::Deregister,
        MembershipExitAction::PlannedOffline,
    ] {
        let mut fixture = Fixture::new();
        crate::directory_git_fixture::checkpoint(&mut fixture.engine, &fixture.repository, "dir");
        let unselected = fixture
            .engine
            .create_directory_branches(
                &fixture.repository.repository_id,
                Some(&name_policy::LookupPath::parse("dir").unwrap()),
                &name("unselected tags"),
                "admin",
                op(),
            )
            .unwrap()
            .result["directory_branch_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let retained = fixture.current.clone();
        let branch = fixture.branch.clone();
        let target = tag_target(&fixture.engine, &branch, &retained.version_id);
        let previous_versions = canonical_json(&fixture.engine.state.versions).unwrap();
        let old_commits = fixture.engine.state.commits.clone();
        let old_trees = fixture.engine.state.directory_trees.clone();
        let physical = fixture.exit(action);
        let original = fixture.engine.state.clone();
        let view = fixture.engine.current_namespaces.as_ref().unwrap().clone();
        let operation = op();
        let add = vec!["review".into(), "发布".into()];
        let receipt = edit(
            &mut fixture.engine,
            &retained.file_id,
            std::slice::from_ref(&target),
            &add,
            operation,
        )
        .unwrap();
        let tagged = node(&receipt);
        assert_eq!(fixture.engine.file_branch_head(&branch).unwrap(), tagged);
        assert_eq!(
            fixture
                .engine
                .file_node_tags(&retained.file_id, &tagged)
                .unwrap(),
            BTreeSet::from(["review".into(), "发布".into()])
        );
        assert!(
            fixture
                .engine
                .file_node_tags(&retained.file_id, &retained.version_id)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            fixture.engine.state.file_metadata_nodes[&tagged].parent_node_ids,
            vec![retained.version_id.clone()]
        );
        assert_eq!(
            fixture
                .engine
                .file_context(&fixture.repository.repository_id, "dir/live")
                .unwrap()
                .node_id
                .as_deref(),
            Some(tagged.as_str())
        );
        let impacted = receipt.result["nodes"][0]["directory_branch_ids"]
            .as_array()
            .unwrap();
        assert!(
            impacted
                .iter()
                .any(|id| id.as_str() == Some(unselected.as_str()))
        );
        assert!(
            impacted.len() >= 3,
            "root, child and unselected current composition are preserved"
        );
        for id in impacted {
            let directory = &fixture.engine.state.directory_branches[id.as_str().unwrap()];
            for entry in directory
                .entries
                .iter()
                .filter(|entry| entry.file_id == retained.file_id)
            {
                assert_eq!(entry.version_id, retained.version_id);
                assert_eq!(
                    directory.file_contexts[entry.lookup_key()]
                        .node_id
                        .as_deref(),
                    Some(tagged.as_str())
                );
            }
            for reference in directory.directory_refs.values() {
                let tree = &fixture.engine.state.directory_trees[&reference.tree_id];
                if action == MembershipExitAction::Deregister {
                    assert!(
                        tree.entries
                            .iter()
                            .all(|entry| entry.file_id != fixture.hidden.file_id)
                    );
                }
            }
        }
        assert_eq!(
            canonical_json(&fixture.engine.state.versions).unwrap(),
            previous_versions
        );
        for (id, commit) in old_commits.iter() {
            assert_eq!(fixture.engine.state.commits.get(id), Some(commit));
        }
        for (id, tree) in old_trees.iter() {
            assert_eq!(fixture.engine.state.directory_trees.get(id), Some(tree));
        }
        view.advance(&cold(&original), &cold(fixture.engine.state()))
            .unwrap();
        assert_eq!(
            edit(
                &mut fixture.engine,
                &retained.file_id,
                std::slice::from_ref(&target),
                &add,
                operation
            )
            .unwrap(),
            receipt
        );
        let noop_target = tag_target(&fixture.engine, &branch, &tagged);
        let metadata_count = fixture.engine.state.file_metadata_nodes.len();
        let noop_operation = op();
        let noop = edit(
            &mut fixture.engine,
            &retained.file_id,
            std::slice::from_ref(&noop_target),
            &add,
            noop_operation,
        )
        .unwrap();
        assert_eq!(noop.result["nodes"][0]["no_change"], true);
        assert!(
            noop.commit_id.is_none() && noop.history_tree_id.is_none() && noop.branch_id.is_none()
        );
        assert_eq!(
            fixture.engine.state.file_metadata_nodes.len(),
            metadata_count
        );
        let mut bytes = Vec::new();
        fixture
            .engine
            .open_content(&retained.version_id)
            .unwrap()
            .file
            .read_to_end(&mut bytes)
            .unwrap();
        assert_eq!(bytes, b"retained body");
        if action == MembershipExitAction::Deregister {
            fixture.hidden_absent();
        } else {
            assert!(
                fixture
                    .engine
                    .namespace_tree(&fixture.repository.main_branch_id)
                    .unwrap()
                    .get("dir/gone")
                    .is_some()
            );
        }
        assert_eq!(files(&fixture.config.disks[0].root), physical);
        let system = fixture.temp.path().join("system");
        drop(fixture.engine);
        let mut engine = Engine::open(&system).unwrap();
        assert_eq!(
            edit(&mut engine, &retained.file_id, &[target], &add, operation).unwrap(),
            receipt
        );
        assert_eq!(
            edit(
                &mut engine,
                &retained.file_id,
                &[noop_target],
                &add,
                noop_operation
            )
            .unwrap(),
            noop
        );
        assert_eq!(engine.file_branch_head(&branch).unwrap(), tagged);
        assert_eq!(files(&fixture.config.disks[0].root), physical);
    }
}

#[test]
fn disk_exit_historical_tags_do_not_read_unavailable_body_or_rewind_current() {
    let mut fixture = Fixture::new();
    let historical = fixture.first.clone();
    let physical = fixture.exit(MembershipExitAction::Deregister);
    let target = tag_target(&fixture.engine, &fixture.branch, &historical.version_id);
    let previous = fixture.engine.state.clone();
    let view = fixture.engine.current_namespaces.as_ref().unwrap().clone();
    let operation = op();
    let receipt = edit(
        &mut fixture.engine,
        &historical.file_id,
        std::slice::from_ref(&target),
        &["historical".into()],
        operation,
    )
    .unwrap();
    let tagged = node(&receipt);
    assert_eq!(receipt.result["nodes"][0]["advanced"], false);
    assert!(
        receipt.commit_id.is_none()
            && receipt.history_tree_id.is_none()
            && receipt.branch_id.is_none()
    );
    assert_eq!(fixture.engine.state.branches, previous.branches);
    assert_eq!(
        fixture.engine.state.directory_branches,
        previous.directory_branches
    );
    assert_eq!(
        fixture.engine.state.directory_trees,
        previous.directory_trees
    );
    assert_eq!(fixture.engine.state.commits, previous.commits);
    assert_eq!(fixture.engine.state.file_contexts, previous.file_contexts);
    assert_eq!(
        fixture.engine.state.file_metadata_nodes[&tagged].version_id,
        historical.version_id
    );
    view.advance(&cold(&previous), &cold(fixture.engine.state()))
        .unwrap();
    assert_eq!(
        fixture.engine.file_branch_head(&fixture.branch).unwrap(),
        fixture.current.version_id
    );
    fixture.hidden_absent();
    assert_eq!(files(&fixture.config.disks[0].root), physical);
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    let mut engine = Engine::open(&system).unwrap();
    assert_eq!(
        edit(
            &mut engine,
            &historical.file_id,
            &[target],
            &["historical".into()],
            operation
        )
        .unwrap(),
        receipt
    );
    assert_eq!(
        engine.file_node_tags(&historical.file_id, &tagged).unwrap(),
        BTreeSet::from(["historical".into()])
    );
}

#[test]
fn disk_exit_tags_reject_hidden_head_wrong_location_and_old_workspace_atomically() {
    let mut fixture = Fixture::new();
    let hidden_branch = fixture
        .engine
        .file_context(&fixture.repository.repository_id, "dir/gone")
        .unwrap()
        .branch_id;
    let hidden_target = tag_target(&fixture.engine, &hidden_branch, &fixture.hidden.version_id);
    let workspace = fixture
        .engine
        .create_workspace(&fixture.repository.main_branch_id, "admin")
        .unwrap();
    fixture
        .engine
        .stage_bytes(&workspace.workspace_id, "dir/old-workspace", b"staged")
        .unwrap();
    fixture.exit(MembershipExitAction::Deregister);
    let before = canonical_json(fixture.engine.state()).unwrap();
    assert!(
        edit(
            &mut fixture.engine,
            &fixture.hidden.file_id,
            &[hidden_target],
            &["hidden".into()],
            op()
        )
        .is_err()
    );
    let live_target = tag_target(
        &fixture.engine,
        &fixture.branch,
        &fixture.current.version_id,
    );
    let wrong = FileLocation {
        repository_id: fixture.repository.repository_id.clone(),
        path: "dir/gone".into(),
    };
    assert_eq!(
        fixture
            .engine
            .update_file_node_tags(FileNodeTagsUpdateRequest {
                file_id: &fixture.current.file_id,
                targets: std::slice::from_ref(&live_target),
                add: &["wrong".into()],
                remove: &[],
                location: Some(&wrong),
                principal: "admin",
                operation_id: op(),
            })
            .unwrap_err()
            .code(),
        ErrorCode::FileNotFound
    );
    assert_eq!(
        edit(
            &mut fixture.engine,
            &fixture.current.file_id,
            &[live_target.clone(), live_target],
            &["duplicate".into()],
            op()
        )
        .unwrap_err()
        .code(),
        ErrorCode::InvalidRequest
    );
    assert_eq!(
        fixture
            .engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id.clone(),
                operation_id: op(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "must not revive old namespace".into(),
                request_digest: None,
            })
            .unwrap_err()
            .code(),
        ErrorCode::WorkspaceChanged
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    assert!(
        fixture
            .engine
            .workspaces
            .contains_key(&workspace.workspace_id)
    );
    fixture.hidden_absent();
}

#[test]
fn disk_exit_pending_tag_child_keeps_inheritance_and_merged_alias_is_read_only() {
    let mut fixture = Fixture::new();
    fixture.exit(MembershipExitAction::Deregister);
    let created = fixture
        .engine
        .create_file_branch(
            &fixture.current.file_id,
            &fixture.current.version_id,
            &name("pending tags"),
            "admin",
            op(),
        )
        .unwrap();
    let pending = created.branch_id.unwrap();
    let inherited = fixture.engine.state.branches[&pending]
        .pending_file_versions
        .clone();
    let source = tag_target(&fixture.engine, &pending, &fixture.current.version_id);
    let receipt = edit(
        &mut fixture.engine,
        &fixture.current.file_id,
        &[source],
        &["pending".into()],
        op(),
    )
    .unwrap();
    assert_eq!(
        fixture.engine.state.branches[&pending].pending_file_versions,
        inherited
    );
    assert_eq!(
        fixture.engine.state.file_metadata_nodes[&node(&receipt)].version_id,
        fixture.current.version_id
    );
    let inputs = vec![
        FileMergeInput {
            branch_id: fixture.branch.clone(),
            expected_node_id: fixture.engine.file_branch_head(&fixture.branch).unwrap(),
        },
        FileMergeInput {
            branch_id: pending.clone(),
            expected_node_id: fixture.engine.file_branch_head(&pending).unwrap(),
        },
    ];
    fixture
        .engine
        .merge_file_branches(FileBranchMergeRequest {
            file_id: &fixture.current.file_id,
            inputs: &inputs,
            chosen_node_id: &fixture.current.version_id,
            surviving_branch_id: &fixture.branch,
            location: None,
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    let before = canonical_json(fixture.engine.state()).unwrap();
    let source = tag_target(&fixture.engine, &pending, &node(&receipt));
    assert_eq!(
        edit(
            &mut fixture.engine,
            &fixture.current.file_id,
            &[source],
            &["forbidden".into()],
            op()
        )
        .unwrap_err()
        .code(),
        ErrorCode::BranchNotActive
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
}

#[test]
fn disk_exit_tag_replay_authenticates_metadata_actor_digest_context_and_all_write_sets() {
    let mut fixture = Fixture::new();
    fixture.exit(MembershipExitAction::Deregister);
    let before = fixture.engine.state.clone();
    let view = fixture.engine.current_namespaces.as_ref().unwrap().clone();
    let source = tag_target(
        &fixture.engine,
        &fixture.branch,
        &fixture.current.version_id,
    );
    let operation = op();
    let receipt = edit(
        &mut fixture.engine,
        &fixture.current.file_id,
        &[source],
        &["proof".into()],
        operation,
    )
    .unwrap();
    let published = cold(fixture.engine.state());
    let tagged = node(&receipt);
    view.advance(&cold(&before), &published).unwrap();
    for case in 0..11 {
        let mut forged = cold(&published);
        match case {
            0 => forged.file_metadata_nodes.get_mut(&tagged).unwrap().author = "other".into(),
            1 => {
                forged
                    .file_metadata_nodes
                    .get_mut(&tagged)
                    .unwrap()
                    .parent_node_ids = vec![fixture.hidden.version_id.clone()]
            }
            2 => forged
                .file_metadata_nodes
                .get_mut(&tagged)
                .unwrap()
                .tags
                .insert("injected".into())
                .then_some(())
                .unwrap(),
            3 => {
                forged
                    .operation_principals
                    .insert(operation, "other".into());
            }
            4 => {
                forged
                    .operations
                    .get_mut(&operation)
                    .unwrap()
                    .request_digest = "0".repeat(64)
            }
            5 => {
                forged.version_notes.insert(
                    fixture.current.version_id.clone(),
                    "unrequested note".into(),
                );
            }
            6 => {
                let key = crate::business::context_key_from_key(
                    &fixture.repository.repository_id,
                    "dir/live",
                );
                forged.file_contexts.get_mut(&key).unwrap().node_id =
                    Some(fixture.first.version_id.clone());
            }
            7 => {
                forged.file_metadata_nodes.remove(&tagged);
            }
            8 => {
                let id = forged
                    .directory_branches
                    .values()
                    .find(|branch| branch.repository_id == fixture.repository.repository_id)
                    .unwrap()
                    .head_tree_id
                    .clone();
                forged
                    .directory_trees
                    .get_mut(&id)
                    .unwrap()
                    .entries
                    .push(fixture.hidden.clone());
            }
            9 => {
                forged.branches.get_mut(&fixture.branch).unwrap().generation += 1;
            }
            10 => {
                forged.operations.get_mut(&operation).unwrap().result["source_request"]["targets"]
                    [0]["expected_node_id"] = json!(fixture.first.version_id)
            }
            _ => unreachable!(),
        }
        assert!(
            view.advance(&cold(&before), &forged).is_err(),
            "forged tag case {case}"
        );
    }
}

#[test]
fn disk_exit_tag_noop_replay_still_rejects_unrequested_rows_and_actor_reuse() {
    let mut fixture = Fixture::new();
    fixture.exit(MembershipExitAction::Deregister);
    let source = tag_target(
        &fixture.engine,
        &fixture.branch,
        &fixture.current.version_id,
    );
    let before = fixture.engine.state.clone();
    let view = fixture.engine.current_namespaces.as_ref().unwrap().clone();
    let operation = op();
    let receipt = edit(
        &mut fixture.engine,
        &fixture.current.file_id,
        std::slice::from_ref(&source),
        &[],
        operation,
    )
    .unwrap();
    assert_eq!(receipt.result["nodes"][0]["no_change"], true);
    let published = cold(fixture.engine.state());
    view.advance(&cold(&before), &published).unwrap();
    let mut forged = published.clone();
    forged
        .file_notes
        .insert(fixture.current.file_id.clone(), "not a tag change".into());
    assert!(view.advance(&cold(&before), &forged).is_err());
    let mut forged = published;
    forged.operations.get_mut(&operation).unwrap().result["nodes"][0]["no_change"] = json!(false);
    assert!(view.advance(&cold(&before), &forged).is_err());
    assert_eq!(
        fixture
            .engine
            .update_file_node_tags(FileNodeTagsUpdateRequest {
                file_id: &fixture.current.file_id,
                targets: &[source],
                add: &[],
                remove: &[],
                location: None,
                principal: "other",
                operation_id: operation,
            })
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyKeyReused
    );
}

#[test]
fn disk_exit_tag_source_cannot_borrow_another_retained_occurrence_or_stale_head() {
    let mut fixture = Fixture::new();
    fixture.exit(MembershipExitAction::Deregister);
    let source = tag_target(
        &fixture.engine,
        &fixture.branch,
        &fixture.current.version_id,
    );
    let mut view = fixture
        .engine
        .current_namespaces
        .as_ref()
        .unwrap()
        .as_ref()
        .clone();
    let scopes = Arc::make_mut(&mut view.branches);
    let scope = Arc::make_mut(scopes.get_mut(&fixture.branch).unwrap());
    scope.entries = Arc::new(Vec::new());
    let input = Input {
        file_id: fixture.current.file_id.clone(),
        targets: vec![source.clone()],
        add: vec!["source proof".into()],
        remove: Vec::new(),
        location: None,
    };
    let tag_edit = TagEdit::new(
        &input.targets,
        &input.add,
        &input.remove,
        fixture.engine.config.max_entries,
    )
    .unwrap();
    assert!(
        view.file_tag_plan(
            fixture.engine.state(),
            &input,
            "admin",
            TagPublication {
                operation: op(),
                sequence: fixture.engine.state.sequence + 1,
                created_at: crate::business::now().unwrap(),
            },
            &tag_edit,
            fixture.engine.config.max_entries
        )
        .is_err(),
        "the owner still retains this exact file, but cannot replace the missing source branch occurrence"
    );
    let before = canonical_json(fixture.engine.state()).unwrap();
    let mut stale = source.clone();
    stale.expected_node_id = fixture.first.version_id.clone();
    assert_eq!(
        edit(
            &mut fixture.engine,
            &fixture.current.file_id,
            &[stale],
            &["stale".into()],
            op()
        )
        .unwrap_err()
        .code(),
        ErrorCode::FileBranchConflict
    );
    let too_many = (0..=MAX_FILE_TAGS)
        .map(|n| format!("tag-{n}"))
        .collect::<Vec<_>>();
    assert_eq!(
        edit(
            &mut fixture.engine,
            &fixture.current.file_id,
            &[source],
            &too_many,
            op()
        )
        .unwrap_err()
        .code(),
        ErrorCode::InvalidRequest
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
}

#[test]
fn disk_exit_tag_main_receipt_binds_the_real_historical_source_after_restart() {
    let mut fixture = Fixture::new();
    fixture.exit(MembershipExitAction::Deregister);
    let source = tag_target(
        &fixture.engine,
        &fixture.branch,
        &fixture.current.version_id,
    );
    let operation = op();
    let receipt = edit(
        &mut fixture.engine,
        &fixture.current.file_id,
        std::slice::from_ref(&source),
        &["historical source proof".into()],
        operation,
    )
    .unwrap();
    let commit = receipt.commit_id.clone().unwrap();
    assert_eq!(
        receipt.branch_id.as_deref(),
        Some(fixture.repository.main_branch_id.as_str())
    );
    assert_eq!(
        receipt.history_tree_id.as_deref(),
        Some(fixture.repository.history_tree_id.as_str())
    );
    assert_eq!(
        fixture.engine.state.branches[&fixture.repository.main_branch_id].head_commit_id,
        commit
    );
    let source_entries = fixture
        .engine
        .namespace_tree_at(&fixture.repository.main_branch_id, &commit)
        .unwrap()
        .entries()
        .to_vec();
    assert_eq!(
        source_entries,
        fixture
            .engine
            .namespace_tree(&fixture.repository.main_branch_id)
            .unwrap()
            .entries()
    );
    assert_eq!(
        fixture
            .engine
            .create_branch(
                &commit,
                &name("independent history remains out of scope"),
                "admin",
                op()
            )
            .unwrap_err()
            .code(),
        ErrorCode::RecoveryRequired,
        "a valid receipt source does not grant an independent-history branch import"
    );
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    let mut engine = Engine::open(&system).unwrap();
    assert_eq!(
        edit(
            &mut engine,
            &fixture.current.file_id,
            &[source],
            &["historical source proof".into()],
            operation
        )
        .unwrap(),
        receipt
    );
    assert_eq!(
        engine
            .namespace_tree_at(&fixture.repository.main_branch_id, &commit)
            .unwrap()
            .entries(),
        source_entries
    );
}

fn prepared_tags(
    fixture: &Fixture,
    source: &str,
    add: &[String],
    operation: OperationId,
) -> crate::engine::MutationPlan {
    let targets = [tag_target(&fixture.engine, &fixture.branch, source)];
    let request = FileNodeTagsUpdateRequest {
        file_id: &fixture.current.file_id,
        targets: &targets,
        add,
        remove: &[],
        location: None,
        principal: "admin",
        operation_id: operation,
    };
    let body =
        json!({"file_id":request.file_id,"targets":targets,"add":add,"remove":[],"location":null});
    let tag_edit = TagEdit::new(
        request.targets,
        request.add,
        request.remove,
        fixture.engine.config.max_entries,
    )
    .unwrap();
    fixture
        .engine
        .plan_mutation_digest(KIND, "admin", operation, body, None, |engine, next, _| {
            engine
                .current_namespaces
                .as_ref()
                .unwrap()
                .update_current_file_tags(
                    engine.state(),
                    next,
                    &request,
                    &tag_edit,
                    engine.config.max_entries,
                )
        })
        .unwrap()
}

#[test]
fn current_file_tags_closed_planner_rejects_payload_and_unproven_metadata() {
    use crate::engine::MutationPlan;
    let mut fixture = Fixture::new();
    fixture.exit(MembershipExitAction::Deregister);
    let previous = Arc::clone(&fixture.engine.state);
    let view = fixture.engine.current_namespaces.clone().unwrap();
    let operation = op();
    let add = vec!["planner proof".into()];
    let targets = [tag_target(
        &fixture.engine,
        &fixture.branch,
        &fixture.current.version_id,
    )];
    let request = FileNodeTagsUpdateRequest {
        file_id: &fixture.current.file_id,
        targets: &targets,
        add: &add,
        remove: &[],
        location: None,
        principal: "admin",
        operation_id: operation,
    };
    let input =
        json!({"file_id":request.file_id,"targets":targets,"add":add,"remove":[],"location":null});
    let tag_edit = TagEdit::new(
        request.targets,
        request.add,
        request.remove,
        fixture.engine.config.max_entries,
    )
    .unwrap();
    let digest = Engine::native_request_digest(KIND, "admin", &input).unwrap();
    let mut context = fixture.engine.planning_context();
    assert_eq!(
        view.require_operation(KIND).unwrap_err().code(),
        ErrorCode::RecoveryRequired
    );
    let rejected =
        context.plan_new_mutation(KIND, "admin", operation, digest.clone(), |_, _, _| {
            panic!("no generic tag permit")
        });
    assert!(rejected.is_err());
    for attack in 0..6 {
        let digest = if attack == 1 {
            "e".repeat(64)
        } else {
            digest.clone()
        };
        let planned =
            context.plan_file_tags_mutation("admin", operation, digest, |_, next, prepared| {
                let mut result = view.update_current_file_tags(
                    &previous,
                    next,
                    &request,
                    &tag_edit,
                    fixture.engine.config.max_entries,
                )?;
                match attack {
                    0 | 1 => (),
                    2 => {
                        result["namespace_tags"]["registry"] = Value::Null;
                    }
                    3 => {
                        next.file_names
                            .insert(fixture.current.file_id.clone(), "unrequested".into());
                    }
                    4 => {
                        prepared.push(crate::engine::Prepared {
                            version: previous.versions[&fixture.current.version_id].clone(),
                            placement: previous.placements[&fixture.current.version_id].clone(),
                            source: crate::engine::ReadSource::external(
                                fixture.temp.path().join("not-a-payload"),
                            ),
                        });
                    }
                    5 => {
                        next.file_metadata_nodes.clear();
                    }
                    _ => unreachable!(),
                }
                Ok(result)
            });
        if attack == 0 {
            let Ok(MutationPlan::Prepared {
                next,
                prepared,
                result,
            }) = planned
            else {
                panic!("exact plan must be admitted")
            };
            assert!(prepared.is_empty());
            view.verify_new_file_tags(&previous, &next, &result)
                .unwrap();
            view.advance(&previous, &next).unwrap();
            let mut result = result.clone();
            result.request_digest = "f".repeat(64);
            assert!(
                view.verify_new_file_tags(&previous, &next, &result)
                    .is_err()
            );
        } else {
            let Err(error) = planned else {
                panic!("unproven tag plan {attack} escaped")
            };
            assert_eq!(error.code(), ErrorCode::PublicationInvalid, "{attack}");
        }
    }
    context.current_namespaces = None;
    let pending = context.plan_file_tags_mutation("admin", operation, digest, |_, _, _| {
        panic!("pending registry must reject before preparing tags")
    });
    let Err(error) = pending else {
        panic!("pending registry admitted tag plan")
    };
    assert_eq!(error.code(), ErrorCode::RecoveryRequired);
    assert!(Arc::ptr_eq(&previous, &fixture.engine.state));
}

#[test]
fn current_file_tags_prior_actor_event_and_node_cannot_change_without_new_receipt() {
    let mut fixture = Fixture::new();
    fixture.exit(MembershipExitAction::Deregister);
    let target = tag_target(
        &fixture.engine,
        &fixture.branch,
        &fixture.current.version_id,
    );
    let operation = op();
    let receipt = edit(
        &mut fixture.engine,
        &fixture.current.file_id,
        &[target],
        &["immutable".into()],
        operation,
    )
    .unwrap();
    let id = node(&receipt);
    let previous = Arc::clone(&fixture.engine.state);
    let view = fixture.engine.current_namespaces.clone().unwrap();
    for attack in 0..7 {
        let mut forged = previous.as_ref().clone();
        forged.begin_tracking();
        forged.sequence += 1;
        match attack {
            0 => {
                forged
                    .operation_principals
                    .insert(operation, "other".into());
            }
            1 => {
                forged.events.last_mut().unwrap().kind = "auth.other".into();
            }
            2 => {
                forged.events.last_mut().unwrap().operation_id = op();
            }
            3 => {
                forged.operations.get_mut(&operation).unwrap().kind = "auth.other".into();
            }
            4 => {
                forged.operations.remove(&operation);
            }
            5 => {
                forged.file_metadata_nodes.remove(&id);
            }
            6 => {
                forged.file_metadata_nodes.get_mut(&id).unwrap().kind = FileMetadataKind::Merge;
            }
            _ => unreachable!(),
        }
        assert!(
            view.advance(&previous, &forged).is_err(),
            "tracked {attack}"
        );
        assert!(
            view.advance(&cold(&previous), &cold(&forged)).is_err(),
            "cold {attack}"
        );
    }
}

#[test]
fn current_file_tags_receipt_envelope_and_disguised_child_are_rejected() {
    use crate::engine::MutationPlan;
    let mut fixture = Fixture::new();
    fixture.exit(MembershipExitAction::Deregister);
    let previous = Arc::clone(&fixture.engine.state);
    let view = fixture.engine.current_namespaces.clone().unwrap();
    for source in [&fixture.current.version_id, &fixture.first.version_id] {
        let operation = op();
        let MutationPlan::Prepared { next, result, .. } =
            prepared_tags(&fixture, source, &["bound".into()], operation)
        else {
            panic!("expected new tag plan")
        };
        for attack in 0..13 {
            let mut forged = next.as_ref().clone();
            let record = forged.operations.get_mut(&operation).unwrap();
            match attack {
                0 => {
                    record.current_health = CurrentHealth::Unavailable;
                }
                1 => {
                    record.commit_state = CommitState::InDoubt;
                }
                2 => {
                    record.protection_at_publish = ProtectionAtPublish::TargetMet;
                }
                3 => {
                    record.result["timestamp"] = json!("01");
                }
                4 => {
                    record.result["membership_epoch"] = json!(true);
                }
                5 => {
                    record.result["namespace_tags"]["mask_hash"] = Value::Null;
                }
                6 => {
                    record.result["extra"] = json!(true);
                }
                7 => {
                    record.result["metadata_created_at"] = json!("01");
                }
                8 => {
                    record.result["source_request"]["unexpected"] = json!(true);
                }
                9 => {
                    record.result["source_request"]
                        .as_object_mut()
                        .unwrap()
                        .remove("location");
                }
                10 => {
                    forged.topology_generation += 1;
                }
                11 => {
                    forged.control_auth = Some(json!({"unrequested":true}));
                }
                12 => {
                    record.kind = "auth.unrelated".into();
                    record.result = json!({});
                    record.request_digest =
                        Engine::native_request_digest("auth.unrelated", "admin", &json!({}))
                            .unwrap();
                    forged.events.last_mut().unwrap().kind = "auth.unrelated".into();
                    forged
                        .file_metadata_nodes
                        .get_mut(&node(&result))
                        .unwrap()
                        .kind = FileMetadataKind::Merge;
                }
                _ => unreachable!(),
            }
            assert!(
                view.advance(&previous, &forged).is_err(),
                "tracked {source} {attack}"
            );
            assert!(
                view.advance(&cold(&previous), &cold(&forged)).is_err(),
                "cold {source} {attack}"
            );
        }
    }
}

#[test]
fn current_file_tags_disguised_merge_cannot_borrow_an_unadvanced_branch() {
    use crate::engine::MutationPlan;
    let mut fixture = Fixture::new();
    let helper = fixture
        .engine
        .create_file_branch(
            &fixture.current.file_id,
            &fixture.current.version_id,
            &name("unadvanced merge input"),
            "admin",
            op(),
        )
        .unwrap()
        .branch_id
        .unwrap();
    fixture.exit(MembershipExitAction::Deregister);
    let previous = Arc::clone(&fixture.engine.state);
    let view = fixture.engine.current_namespaces.clone().unwrap();
    let operation = op();
    let MutationPlan::Prepared { next, result, .. } = prepared_tags(
        &fixture,
        &fixture.first.version_id,
        &["not adopted by a merge".into()],
        operation,
    ) else {
        panic!("expected historical tag plan")
    };
    let id = node(&result);
    for kind in ["file.branches.merge", "file.history.merge"] {
        let mut forged = next.as_ref().clone();
        forged.file_metadata_nodes.get_mut(&id).unwrap().kind = FileMetadataKind::Merge;
        let merge_id = new_id("fmerge").unwrap();
        forged.file_branch_merges.insert(
            merge_id.clone(),
            FileBranchMerge {
                merge_id,
                history_tree_id: fixture.repository.history_tree_id.clone(),
                file_id: fixture.current.file_id.clone(),
                input_branch_ids: vec![fixture.branch.clone(), helper.clone()],
                input_node_ids: vec![
                    fixture.current.version_id.clone(),
                    fixture.current.version_id.clone(),
                ],
                chosen_node_id: fixture.first.version_id.clone(),
                result_node_id: id.clone(),
                surviving_branch_id: fixture.branch.clone(),
                reused_node: false,
                parent_node_ids: vec![fixture.first.version_id.clone()],
                operation_id: operation,
                sequence: forged.sequence,
            },
        );
        let receipt = forged.operations.get_mut(&operation).unwrap();
        receipt.kind = kind.into();
        receipt.result = json!({});
        receipt.request_digest = Engine::native_request_digest(kind, "admin", &json!({})).unwrap();
        forged.events.last_mut().unwrap().kind = kind.into();
        assert_eq!(forged.branches, previous.branches);
        assert_eq!(forged.directory_branches, previous.directory_branches);
        assert!(
            view.advance(&previous, &forged).is_err(),
            "{kind} must adopt its new metadata node before an unchanged-head shortcut"
        );
        assert!(
            view.advance(&cold(&previous), &cold(&forged)).is_err(),
            "cold {kind} must prove the same adoption"
        );
    }
}

#[test]
fn current_file_tags_merge_child_keeps_its_chosen_nodes_tags() {
    let mut fixture = Fixture::new();
    let newer = save(
        &mut fixture.engine,
        &fixture.repository.main_branch_id,
        "dir/live",
        b"newer merge input",
    );
    let helper = fixture
        .engine
        .create_file_branch(
            &fixture.current.file_id,
            &newer.version_id,
            &name("merge tag source proof"),
            "admin",
            op(),
        )
        .unwrap()
        .branch_id
        .unwrap();
    fixture.exit(MembershipExitAction::Deregister);
    let previous = Arc::clone(&fixture.engine.state);
    let view = fixture.engine.current_namespaces.clone().unwrap();
    let inputs = vec![
        FileMergeInput {
            branch_id: fixture.branch.clone(),
            expected_node_id: fixture.engine.file_branch_head(&fixture.branch).unwrap(),
        },
        FileMergeInput {
            branch_id: helper.clone(),
            expected_node_id: fixture.engine.file_branch_head(&helper).unwrap(),
        },
    ];
    let receipt = fixture
        .engine
        .merge_file_branches(FileBranchMergeRequest {
            file_id: &fixture.current.file_id,
            inputs: &inputs,
            chosen_node_id: &fixture.current.version_id,
            surviving_branch_id: &fixture.branch,
            location: None,
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    assert_eq!(receipt.result["reused_node"], false);
    let id = receipt.result["node_id"].as_str().unwrap();
    view.advance(&cold(&previous), &cold(fixture.engine.state()))
        .unwrap();
    let mut forged = fixture.engine.state.as_ref().clone();
    forged
        .file_metadata_nodes
        .get_mut(id)
        .unwrap()
        .tags
        .insert("not from the chosen node".into());
    assert!(view.advance(&previous, &forged).is_err());
    assert!(view.advance(&cold(&previous), &cold(&forged)).is_err());
}

// Real hash-bound journal decisions bypass live planning only in this fixture.
// Fixed membership, snapshot/history closure and receipts stay authenticated so
// cold recovery must reach the operation-specific proof before repairing them.
fn install_tag_decision(
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

fn relative_files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    files(root)
        .into_iter()
        .map(|(path, bytes)| (path.strip_prefix(root).unwrap().to_owned(), bytes))
        .collect()
}

#[test]
fn current_file_tags_cold_proof_precedes_replica_repair_and_name_projection() {
    use crate::engine::MutationPlan;
    use crate::journal::object_path;
    for published in [false, true] {
        for historical in [false, true] {
            for attack in [
                "valid",
                "actor",
                "source",
                "context",
                "location",
                "payload",
                "disguised_payload",
                "disguised_noop_payload",
            ] {
                let mut fixture = Fixture::new();
                fixture.exit(MembershipExitAction::Deregister);
                let stopped = relative_files(&fixture.config.disks[0].root);
                let parked = fixture.temp.path().join("parked-source");
                fs::rename(&fixture.config.disks[0].root, &parked).unwrap();
                fs::write(
                    &fixture.config.disks[0].root,
                    b"stopped root must not be opened",
                )
                .unwrap();
                let source = if historical {
                    fixture.first.version_id.clone()
                } else {
                    fixture.current.version_id.clone()
                };
                let target = tag_target(&fixture.engine, &fixture.branch, &source);
                let operation = op();
                let add = if attack == "disguised_noop_payload" {
                    Vec::new()
                } else {
                    vec!["cold source proof".into()]
                };
                let MutationPlan::Prepared {
                    mut next,
                    prepared,
                    mut result,
                } = prepared_tags(&fixture, &source, &add, operation)
                else {
                    panic!("native tags must prepare")
                };
                assert!(prepared.is_empty());
                let tagged = node(&result);
                match attack {
                    "valid" | "payload" => (),
                    "actor" => {
                        next.operation_principals.insert(operation, "other".into());
                    }
                    "source" => {
                        result.result["namespace_tags"]["mask_hash"] = Value::Null;
                        next.operations.insert(operation, result.clone());
                    }
                    "context" => {
                        // Same file and same immutable body, so history structure
                        // remains valid. Only this transition's exact occurrence
                        // proof can reject the invented context generation.
                        let key = crate::business::context_key_from_key(
                            &fixture.repository.repository_id,
                            "dir/live",
                        );
                        next.file_contexts
                            .get_mut(&key)
                            .unwrap()
                            .selection_generation += 1;
                    }
                    "location" => {
                        let name = next.file_names[&fixture.current.file_id].clone();
                        next.file_locations.insert(
                            fixture.current.file_id.clone(),
                            format!("unrequested/{name}"),
                        );
                    }
                    "disguised_payload" | "disguised_noop_payload" => {
                        // Neither the operation label nor its removable marker
                        // can authorize payload I/O. The no-op case has no new
                        // tag node, so the prefix's exact version delta must
                        // reject it independently of metadata-child detection.
                        result.kind = "auth.unrelated".into();
                        result.result = json!({});
                        result.request_digest =
                            Engine::native_request_digest("auth.unrelated", "admin", &json!({}))
                                .unwrap();
                        next.operations.insert(operation, result.clone());
                        next.events.last_mut().unwrap().kind = result.kind.clone();
                    }
                    _ => unreachable!(),
                }
                let versions = if matches!(
                    attack,
                    "payload" | "disguised_payload" | "disguised_noop_payload"
                ) {
                    if attack == "payload" {
                        fixture
                            .engine
                            .current_namespaces
                            .as_ref()
                            .unwrap()
                            .verify_new_file_tags(&fixture.engine.state, &next, &result)
                            .unwrap();
                    }
                    vec![crate::journal::VersionEvidence {
                        version: next.versions[&fixture.current.version_id].clone(),
                        placement: next.placements[&fixture.current.version_id].clone(),
                    }]
                } else {
                    Vec::new()
                };
                let candidate =
                    install_tag_decision(&mut fixture.engine, &next, &result, published, versions);
                let payload_path = candidate.versions.first().map(|evidence| {
                    let disk = fixture
                        .engine
                        .disk(&evidence.placement.route_slot_ids[0])
                        .unwrap();
                    assert!(!disk.metadata_member);
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
                let before_system = relative_files(&system);
                let before_roots = live_roots
                    .iter()
                    .map(|root| relative_files(root))
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
                    assert_eq!(
                        edit(
                            &mut engine,
                            &fixture.current.file_id,
                            &[target],
                            &add,
                            operation
                        )
                        .unwrap(),
                        result
                    );
                    assert_eq!(
                        engine
                            .file_node_version(&fixture.current.file_id, &tagged)
                            .unwrap(),
                        source
                    );
                    assert_eq!(
                        engine.file_branch_head(&fixture.branch).unwrap(),
                        if historical {
                            fixture.current.version_id.clone()
                        } else {
                            tagged
                        }
                    );
                    assert!(
                        engine
                            .namespace_tree(&fixture.repository.main_branch_id)
                            .unwrap()
                            .get("dir/gone")
                            .is_none()
                    );
                } else {
                    let error = opened
                        .err()
                        .expect("invalid tag proof must refuse cold recovery");
                    assert_eq!(
                        error.code(),
                        ErrorCode::PublicationInvalid,
                        "{published} {historical} {attack}"
                    );
                    if attack == "payload" {
                        assert!(
                            error
                                .to_string()
                                .contains("metadata-only current-registry candidate"),
                            "{error}"
                        );
                    } else if attack == "disguised_noop_payload" {
                        assert!(error.to_string().contains("payload"), "{error}");
                    } else {
                        assert!(error.to_string().contains("file tag"), "{error}");
                    }
                    assert_eq!(
                        fs::symlink_metadata(&missing).unwrap_err().kind(),
                        std::io::ErrorKind::NotFound
                    );
                    assert_eq!(relative_files(&system), before_system, "SYSTEM changed");
                    for (root, bytes) in live_roots.iter().zip(before_roots) {
                        assert_eq!(
                            relative_files(root),
                            bytes,
                            "root changed: {published} {historical} {attack}"
                        );
                    }
                }
                assert_eq!(relative_files(&parked), stopped);
                assert_eq!(
                    fs::read(&fixture.config.disks[0].root).unwrap(),
                    b"stopped root must not be opened"
                );
            }
        }
    }
}

#[test]
fn current_file_tags_pre_registry_receipt_remains_permanent_after_activation() {
    let mut fixture = Fixture::new();
    let target = tag_target(
        &fixture.engine,
        &fixture.branch,
        &fixture.current.version_id,
    );
    let operation = op();
    let add = vec!["before registry".into()];
    let original = edit(
        &mut fixture.engine,
        &fixture.current.file_id,
        std::slice::from_ref(&target),
        &add,
        operation,
    )
    .unwrap();
    assert!(original.result.get("namespace_tags").is_none());
    fixture.exit(MembershipExitAction::Deregister);
    let before = Arc::clone(&fixture.engine.state);
    assert_eq!(
        edit(
            &mut fixture.engine,
            &fixture.current.file_id,
            std::slice::from_ref(&target),
            &add,
            operation
        )
        .unwrap(),
        original
    );
    assert!(Arc::ptr_eq(&before, &fixture.engine.state));
    let latest = tag_target(&fixture.engine, &fixture.branch, &node(&original));
    edit(
        &mut fixture.engine,
        &fixture.current.file_id,
        &[latest],
        &["after registry".into()],
        op(),
    )
    .unwrap();
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    let mut engine = Engine::open(&system).unwrap();
    assert_eq!(
        edit(
            &mut engine,
            &fixture.current.file_id,
            &[target],
            &add,
            operation
        )
        .unwrap(),
        original
    );
}

#[test]
fn current_file_tags_source_node_and_body_identities_remain_exact() {
    let mut fixture = Fixture::new();
    fixture.exit(MembershipExitAction::Deregister);
    let previous = Arc::clone(&fixture.engine.state);
    let view = fixture.engine.current_namespaces.clone().unwrap();
    let target = tag_target(
        &fixture.engine,
        &fixture.branch,
        &fixture.current.version_id,
    );
    let input = Input {
        file_id: fixture.current.file_id.clone(),
        targets: vec![target],
        add: vec!["identity proof".into()],
        remove: vec![],
        location: None,
    };
    let tag_edit = TagEdit::new(
        &input.targets,
        &input.add,
        &input.remove,
        fixture.engine.config.max_entries,
    )
    .unwrap();
    for attack in 0..8 {
        let mut source = previous.as_ref().clone();
        match attack {
            0 => {
                source
                    .versions
                    .get_mut(&fixture.current.version_id)
                    .unwrap()
                    .version_id = fixture.first.version_id.clone();
            }
            1 => {
                source
                    .versions
                    .get_mut(&fixture.current.version_id)
                    .unwrap()
                    .file_id = fixture.hidden.file_id.clone();
            }
            2 => {
                source
                    .versions
                    .get_mut(&fixture.current.version_id)
                    .unwrap()
                    .history_tree_id = new_id("history").unwrap();
            }
            3 => {
                source.branches.get_mut(&fixture.branch).unwrap().branch_id =
                    fixture.repository.main_branch_id.clone();
            }
            4 => {
                source
                    .histories
                    .get_mut(&fixture.repository.history_tree_id)
                    .unwrap()
                    .repository_id = new_id("repo").unwrap();
            }
            5 => {
                source
                    .commits
                    .get_mut(&fixture.engine.state.branches[&fixture.branch].head_commit_id)
                    .unwrap()
                    .commit_id = new_id("commit").unwrap();
            }
            6 => {
                source
                    .versions
                    .get_mut(&fixture.current.version_id)
                    .unwrap()
                    .logical_size += 1;
            }
            7 => {
                source.versions.remove(&fixture.current.version_id);
            }
            _ => unreachable!(),
        }
        assert!(
            view.file_tag_plan(
                &source,
                &input,
                "admin",
                TagPublication {
                    operation: op(),
                    sequence: source.sequence + 1,
                    created_at: 0,
                },
                &tag_edit,
                fixture.engine.config.max_entries
            )
            .is_err(),
            "identity {attack}"
        );
    }
    assert!(Arc::ptr_eq(&previous, &fixture.engine.state));
}

#[test]
fn current_file_tags_keep_selected_historical_content_apart_from_newer_file_heads() {
    let mut fixture = Fixture::new();
    let original = fixture.current.clone();
    crate::directory_git_fixture::checkpoint(&mut fixture.engine, &fixture.repository, "");
    let root = fixture
        .engine
        .create_directory_branches(
            &fixture.repository.repository_id,
            None,
            &name("frozen tag source"),
            "admin",
            op(),
        )
        .unwrap()
        .result["directory_branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let later = save(
        &mut fixture.engine,
        &fixture.repository.main_branch_id,
        "dir/live",
        b"newer named body must remain",
    );
    assert_ne!(original.version_id, later.version_id);
    let stopped = fixture.exit(MembershipExitAction::Deregister);
    crate::directory_git_fixture::checkpoint(&mut fixture.engine, &fixture.repository, "");
    let tree = fixture.engine.state.directory_branches[&root]
        .head_tree_id
        .clone();
    let head = fixture.engine.state.branches[&fixture.repository.main_branch_id]
        .head_commit_id
        .clone();
    fixture
        .engine
        .set_directory_default(DirectoryDefaultSelection {
            repository_id: &fixture.repository.repository_id,
            directory_branch_id: &root,
            expected_tree_id: &tree,
            expected_head: &head,
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    let namespace = fixture
        .engine
        .namespace_tree(&fixture.repository.main_branch_id)
        .unwrap();
    assert_eq!(namespace.get("dir/live"), Some(&original));
    let directory = namespace.get("dir").unwrap().entry_id.clone();
    let child = fixture.engine.state.directory_selections[&directory].clone();
    let named = fixture.engine.state.directory_branches[&child].clone();
    assert_eq!(
        fixture.engine.state.directory_tree_selections[&directory],
        named.head_tree_id
    );
    assert!(named.entries.iter().any(|entry| entry == &original));
    assert!(!named.entries.iter().any(|entry| entry == &later));
    let frozen_working = fixture.engine.state.directory_trees[&named.head_tree_id].clone();
    let git_head = fixture.engine.state.directory_git_repositories
        [&fixture.repository.repository_id]
        .branches[&root]
        .head_tree_id
        .clone();
    let previous = Arc::clone(&fixture.engine.state);
    let view = fixture.engine.current_namespaces.clone().unwrap();
    let target = tag_target(&fixture.engine, &fixture.branch, &original.version_id);
    assert_eq!(target.expected_node_id, later.version_id);
    let operation = op();
    let add = vec!["selected history".into()];
    let receipt = edit(
        &mut fixture.engine,
        &original.file_id,
        std::slice::from_ref(&target),
        &add,
        operation,
    )
    .unwrap();
    let tagged = node(&receipt);
    assert_eq!(receipt.result["nodes"][0]["advanced"], false);
    assert_eq!(
        fixture.engine.state.directory_trees[&named.head_tree_id],
        frozen_working
    );
    assert_eq!(
        fixture.engine.state.directory_git_repositories[&fixture.repository.repository_id].branches
            [&root]
            .head_tree_id,
        git_head
    );
    assert_eq!(
        fixture.engine.state.branches[&fixture.branch],
        previous.branches[&fixture.branch]
    );
    assert_eq!(
        fixture.engine.file_branch_head(&fixture.branch).unwrap(),
        later.version_id
    );
    let selected = &fixture.engine.state.directory_tree_selections[&directory];
    let selected = &fixture.engine.state.directory_trees[selected];
    assert!(selected.entries.iter().any(|entry| entry == &original));
    assert!(!selected.entries.iter().any(|entry| entry == &later));
    assert_eq!(
        selected.file_contexts["dir/live"].node_id.as_deref(),
        Some(tagged.as_str())
    );
    assert_eq!(
        fixture
            .engine
            .file_context(&fixture.repository.repository_id, "dir/live")
            .unwrap()
            .node_id
            .as_deref(),
        Some(tagged.as_str())
    );
    for (id, commit) in previous.commits.iter() {
        assert_eq!(fixture.engine.state.commits.get(id), Some(commit));
    }
    for (id, tree) in previous.directory_trees.iter() {
        assert_eq!(fixture.engine.state.directory_trees.get(id), Some(tree));
    }
    assert_eq!(fixture.engine.state.versions, previous.versions);
    assert_eq!(fixture.engine.state.placements, previous.placements);
    assert_eq!(fixture.engine.state.file_names, previous.file_names);
    assert_eq!(fixture.engine.state.file_locations, previous.file_locations);
    view.advance(&cold(&previous), &cold(fixture.engine.state()))
        .unwrap();
    fixture.hidden_absent();
    assert_eq!(files(&fixture.config.disks[0].root), stopped);
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    let mut engine = Engine::open(&system).unwrap();
    assert_eq!(
        engine.state.directory_trees[&named.head_tree_id],
        frozen_working
    );
    assert_eq!(
        engine.state.directory_git_repositories[&fixture.repository.repository_id].branches[&root]
            .head_tree_id,
        git_head
    );
    assert_eq!(
        engine
            .namespace_tree(&fixture.repository.main_branch_id)
            .unwrap()
            .get("dir/live"),
        Some(&original)
    );
    assert_eq!(
        edit(&mut engine, &original.file_id, &[target], &add, operation).unwrap(),
        receipt
    );
    let next_target = tag_target(&engine, &fixture.branch, &tagged);
    let next = edit(
        &mut engine,
        &original.file_id,
        &[next_target],
        &["second selected child".into()],
        op(),
    )
    .unwrap();
    assert_eq!(next.result["nodes"][0]["advanced"], false);
    assert_eq!(
        engine.state.directory_trees[&named.head_tree_id],
        frozen_working
    );
    assert_eq!(
        engine.state.directory_git_repositories[&fixture.repository.repository_id].branches[&root]
            .head_tree_id,
        git_head
    );
    drop(engine);
    let engine = Engine::open(&system).unwrap();
    assert_eq!(
        engine.state.directory_trees[&named.head_tree_id],
        frozen_working
    );
    assert_eq!(
        engine.state.directory_git_repositories[&fixture.repository.repository_id].branches[&root]
            .head_tree_id,
        git_head
    );
    assert_eq!(
        engine.file_branch_head(&fixture.branch).unwrap(),
        later.version_id
    );
    assert_eq!(files(&fixture.config.disks[0].root), stopped);
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
        "a metadata-only tag candidate must not open or read its smuggled local proof or payload"
    );
}

#[test]
fn current_file_tags_batch_keeps_noop_and_changed_sources_exact() {
    let mut fixture = Fixture::new();
    fixture.exit(MembershipExitAction::Deregister);
    let historical_target = tag_target(&fixture.engine, &fixture.branch, &fixture.first.version_id);
    let historical = edit(
        &mut fixture.engine,
        &fixture.current.file_id,
        &[historical_target],
        &["review".into()],
        op(),
    )
    .unwrap();
    let historical_node = node(&historical);
    let targets = [
        tag_target(&fixture.engine, &fixture.branch, &historical_node),
        tag_target(
            &fixture.engine,
            &fixture.branch,
            &fixture.current.version_id,
        ),
    ];
    let previous = Arc::clone(&fixture.engine.state);
    let operation = op();
    let receipt = edit(
        &mut fixture.engine,
        &fixture.current.file_id,
        &targets,
        &["review".into()],
        operation,
    )
    .unwrap();
    assert_eq!(receipt.result["nodes"][0]["no_change"], true);
    assert_eq!(receipt.result["nodes"][0]["node_id"], historical_node);
    assert_eq!(receipt.result["nodes"][0]["advanced"], false);
    assert_eq!(receipt.result["nodes"][1]["no_change"], false);
    assert_eq!(receipt.result["nodes"][1]["advanced"], true);
    let current_node = receipt.result["nodes"][1]["node_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        fixture.engine.file_branch_head(&fixture.branch).unwrap(),
        current_node
    );
    assert_eq!(
        fixture.engine.state.file_metadata_nodes.len(),
        previous.file_metadata_nodes.len() + 1
    );
    assert_eq!(
        fixture.engine.state.file_metadata_nodes[&historical_node],
        previous.file_metadata_nodes[&historical_node]
    );
    assert_eq!(fixture.engine.state.versions, previous.versions);
    let remove_targets = [
        tag_target(&fixture.engine, &fixture.branch, &historical_node),
        tag_target(&fixture.engine, &fixture.branch, &current_node),
    ];
    let remove = fixture
        .engine
        .update_file_node_tags(FileNodeTagsUpdateRequest {
            file_id: &fixture.current.file_id,
            targets: &remove_targets,
            add: &[],
            remove: &["review".into()],
            location: None,
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    for (index, source) in [historical_node, current_node].iter().enumerate() {
        let id = remove.result["nodes"][index]["node_id"].as_str().unwrap();
        let child = &fixture.engine.state.file_metadata_nodes[id];
        assert_eq!(child.parent_node_ids, vec![source.clone()]);
        assert!(child.tags.is_empty());
        assert_eq!(remove.result["nodes"][index]["advanced"], index == 1);
    }
    let before = Arc::clone(&fixture.engine.state);
    let mut invalid = remove_targets.clone();
    invalid[0].expected_node_id = fixture.engine.file_branch_head(&fixture.branch).unwrap();
    invalid[1].expected_node_id = "stale".into();
    assert!(
        edit(
            &mut fixture.engine,
            &fixture.current.file_id,
            &invalid,
            &["atomic".into()],
            op()
        )
        .is_err()
    );
    assert!(Arc::ptr_eq(&before, &fixture.engine.state));
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    let mut engine = Engine::open(&system).unwrap();
    assert_eq!(
        edit(
            &mut engine,
            &fixture.current.file_id,
            &targets,
            &["review".into()],
            operation
        )
        .unwrap(),
        receipt
    );
}
