//! Real Engine/D3 node-note consumers and exact ordinary-publication replay.
//! These temporary roots are not physical-media or power-loss acceptance.
use super::*;
use crate::*;
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};
use storage_contract::history_window::{
    HistoryCollection, HistoryFileId, HistoryPageLimit, HistoryWindowPage, HistoryWindowRequest,
};

fn op() -> OperationId {
    OperationId::generate().unwrap()
}

fn note(value: &str) -> Note {
    Note::parse(value.into()).unwrap()
}

fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(base: &Path, root: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(root).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(base, &path, files);
            } else {
                files.insert(
                    path.strip_prefix(base).unwrap().into(),
                    fs::read(&path).unwrap(),
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
    file: String,
    version: String,
    path_node: String,
    metadata_node: String,
    other_node: String,
    alias: Branch,
}

impl Fixture {
    fn new(copies: u32) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let config =
            Config::development((0..3).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
        let mut engine = Engine::initialize(&temp.path().join("system"), config.clone()).unwrap();
        let repository = engine.create_disk_root("P01", "admin", op()).unwrap();
        if copies != 1 {
            engine
                .update_protection_policy(ProtectionPolicyUpdate {
                    policy_id: &new_id("policy").unwrap(),
                    repository_id: &repository.repository_id,
                    path_prefix: "",
                    target_copies: copies,
                    expected_generation: 0,
                    principal: "admin",
                    operation_id: op(),
                })
                .unwrap();
        }
        let workspace = engine
            .create_workspace(&repository.main_branch_id, "admin")
            .unwrap();
        engine
            .stage_bytes(
                &workspace.workspace_id,
                "original.txt",
                b"immutable note body",
            )
            .unwrap();
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: op(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "note source".into(),
                request_digest: None,
            })
            .unwrap();
        let entry = engine
            .namespace_tree(&repository.main_branch_id)
            .unwrap()
            .get("original.txt")
            .unwrap()
            .clone();
        let context = engine
            .file_context(&repository.repository_id, "original.txt")
            .unwrap();
        for (from, to) in [("original.txt", "merge-input.txt"),
            ("merge-input.txt", "original.txt")]
        {
            engine.file_namespace(
                &repository.repository_id,
                NamespaceOperation::parse(NamespaceOperationKind::Rename, from, Some(to))
                    .unwrap(),
                "admin",
                op(),
            ).unwrap();
        }
        let head = engine.file_branch_head(&context.branch_id).unwrap();
        let inputs = [&entry.version_id, &head].map(|source| FileHistoryMergeInput {
            branch_id: context.branch_id.clone(),
            source_node_id: source.clone(),
            expected_node_id: head.clone(),
        });
        let metadata_node = engine.merge_file_history(FileHistoryMergeRequest {
            file_id: &entry.file_id,
            inputs: &inputs,
            chosen_node_id: &entry.version_id,
            name: &BranchName::parse("note metadata merge").unwrap(),
            location: None,
            principal: "admin",
            operation_id: op(),
        }).unwrap().result["node_id"].as_str().unwrap().to_owned();
        let rename = |engine: &mut Engine, from: &str, to: &str| {
            engine
                .file_namespace(
                    &repository.repository_id,
                    NamespaceOperation::parse(NamespaceOperationKind::Rename, from, Some(to))
                        .unwrap(),
                    "admin",
                    op(),
                )
                .unwrap()
                .result["file_node_ids"][0]
                .as_str()
                .unwrap()
                .to_owned()
        };
        let path_node = rename(&mut engine, "original.txt", "past.txt");
        // This history already has independent file branches. Build a real
        // consumed file alias rather than using the forbidden full-tree API.
        let survivor = engine
            .file_context(&repository.repository_id, "past.txt")
            .unwrap()
            .branch_id;
        let alias_id = engine
            .create_file_branch_from_node(
                &entry.file_id,
                &entry.version_id,
                &path_node,
                &BranchName::parse("unchanged alias").unwrap(),
                "admin",
                op(),
            )
            .unwrap()
            .branch_id
            .unwrap();
        let inputs = [&survivor, &alias_id].map(|branch| FileMergeInput {
            branch_id: branch.clone(),
            expected_node_id: path_node.clone(),
        });
        engine
            .merge_file_branches(FileBranchMergeRequest {
                file_id: &entry.file_id,
                inputs: &inputs,
                chosen_node_id: &path_node,
                surviving_branch_id: &survivor,
                location: None,
                principal: "admin",
                operation_id: op(),
            })
            .unwrap();
        let alias = engine.state.branches[&alias_id].clone();
        assert_eq!(alias.state, BranchState::Merged);
        assert_eq!(engine.state.file_branch_aliases[&alias_id], survivor);
        let other_node = rename(&mut engine, "past.txt", "current.txt");
        engine
            .set_file_node_note(
                &entry.file_id,
                &other_node,
                &note("leave this note"),
                "admin",
                op(),
            )
            .unwrap();
        Self {
            temp,
            engine,
            config,
            repository,
            file: entry.file_id,
            version: entry.version_id,
            path_node,
            metadata_node,
            other_node,
            alias,
        }
    }

    fn exit(&mut self, action: MembershipExitAction) -> (PathBuf, BTreeMap<PathBuf, Vec<u8>>) {
        let target = &self.config.disks[0];
        let held = Arc::clone(self.engine.disk_resources.get(target).unwrap());
        self.engine
            .begin_disk_exit(&target.disk_id, action, "admin", op())
            .unwrap();
        let archive = self
            .engine
            .build_managed_membership_control_archive()
            .unwrap();
        self.engine
            .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
            .unwrap();
        let joint = self.engine.resume_membership_joint_decision().unwrap();
        assert_eq!((joint.original_members, joint.original_quorum), (3, 2));
        let before = files(&target.root);
        let receipt = self.engine.activate_pending_disk_exit().unwrap();
        assert_eq!(receipt.result["target_io"], "closed");
        held.root.io_binding().unwrap().closed_proof().unwrap();
        assert!(self.engine.disk_resources.available(target).is_none());
        assert_eq!(files(&target.root), before);
        let parked = self.temp.path().join("stopped-root");
        fs::rename(&target.root, &parked).unwrap();
        // Opening this original path as a directory now fails immediately.
        fs::write(&target.root, b"closed target must remain an ordinary file").unwrap();
        (parked, before)
    }
}

fn assert_only_note_publication(before: &State, after: &State) {
    let mut normalized = after.clone();
    normalized.sequence = before.sequence;
    normalized.business_generation = before.business_generation;
    normalized.file_path_notes = before.file_path_notes.clone();
    normalized.operations = before.operations.clone();
    normalized.operation_principals = before.operation_principals.clone();
    normalized.events = before.events.clone();
    assert_eq!(
        canonical_json(&normalized).unwrap(),
        canonical_json(before).unwrap()
    );
    for (id, receipt) in before.operations.iter() {
        assert_eq!(after.operations.get(id), Some(receipt));
        assert_eq!(
            after.operation_principals.get(id),
            before.operation_principals.get(id)
        );
    }
}

fn history_notes(engine: &Engine, file: &str) -> BTreeMap<String, String> {
    let read = engine
        .capture_history_window(
            HistoryWindowRequest {
                file_id: HistoryFileId::parse(file).unwrap(),
                collection: HistoryCollection::Nodes,
                location: None,
                history_root: None,
                cursor: None,
                anchor_node_id: None,
                limit: HistoryPageLimit::for_collection(32, &HistoryCollection::Nodes).unwrap(),
            },
            HistoryWindowCancellation::new(),
        )
        .unwrap();
    let HistoryWindowPage::Nodes { nodes, .. } = read.page().unwrap() else {
        panic!("node page required")
    };
    engine.recheck_history_window(&read).unwrap();
    nodes
        .into_iter()
        .map(|node| (node.node_id.to_string(), node.note))
        .collect()
}

#[test]
fn membership_registry_current_path_note_real_d3_preserves_history_aliases_and_stopped_roots() {
    for action in [
        MembershipExitAction::PlannedOffline,
        MembershipExitAction::Deregister,
    ] {
        for copies in [1, 2] {
            let mut fixture = Fixture::new(copies);
            let (parked, stopped) = fixture.exit(action);
            let before = Arc::clone(&fixture.engine.state);
            let current = fixture
                .engine
                .list_tree(&fixture.repository.main_branch_id)
                .unwrap();
            let alias = fixture.engine.list_tree(&fixture.alias.branch_id).unwrap();
            let old_view = Arc::clone(fixture.engine.current_namespaces.as_ref().unwrap());
            let visible = action == MembershipExitAction::PlannedOffline || copies == 2;
            assert_eq!(current.len(), usize::from(visible));
            assert_eq!(alias.len(), usize::from(visible));
            let mut receipts = Vec::new();
            for node in [&fixture.path_node, &fixture.metadata_node] {
                let operation = op();
                let receipt = fixture
                    .engine
                    .set_file_node_note(
                        &fixture.file,
                        node,
                        &note("historical note"),
                        "admin",
                        operation,
                    )
                    .unwrap();
                assert_eq!(
                    receipt.request_digest,
                    Engine::native_request_digest(
                        KIND,
                        "admin",
                        &json!({"file_id":fixture.file,"node_id":node,"note":"historical note"})
                    )
                    .unwrap()
                );
                assert_eq!(
                    fixture.engine.state.file_path_notes[node],
                    "historical note"
                );
                assert_eq!(
                    fixture
                        .engine
                        .set_file_node_note(
                            &fixture.file,
                            node,
                            &note("historical note"),
                            "admin",
                            operation
                        )
                        .unwrap(),
                    receipt
                );
                let same = fixture
                    .engine
                    .set_file_node_note(
                        &fixture.file,
                        node,
                        &note("historical note"),
                        "admin",
                        op(),
                    )
                    .unwrap();
                assert!(
                    same.sequence > receipt.sequence,
                    "a no-change request keeps its own permanent receipt"
                );
                receipts.push((node.clone(), operation, receipt));
            }
            assert_eq!(
                fixture.engine.state.file_path_notes[&fixture.other_node],
                "leave this note"
            );
            let notes = history_notes(&fixture.engine, &fixture.file);
            assert_eq!(notes[&fixture.path_node], "historical note");
            assert_eq!(notes[&fixture.metadata_node], "historical note");
            assert_eq!(
                fixture
                    .engine
                    .list_tree(&fixture.repository.main_branch_id)
                    .unwrap(),
                current
            );
            assert_eq!(
                fixture.engine.list_tree(&fixture.alias.branch_id).unwrap(),
                alias
            );
            assert!(Arc::ptr_eq(
                &old_view.branches,
                &fixture.engine.current_namespaces.as_ref().unwrap().branches
            ));
            assert!(Arc::ptr_eq(
                &old_view.directories,
                &fixture
                    .engine
                    .current_namespaces
                    .as_ref()
                    .unwrap()
                    .directories
            ));
            assert_only_note_publication(&before, fixture.engine.state());
            assert_eq!(files(&parked), stopped);
            assert_eq!(
                fs::read(&fixture.config.disks[0].root).unwrap(),
                b"closed target must remain an ordinary file"
            );
            if copies == 2 {
                let mut body = Vec::new();
                fixture
                    .engine
                    .open_content(&fixture.version)
                    .unwrap()
                    .file
                    .read_to_end(&mut body)
                    .unwrap();
                assert_eq!(body, b"immutable note body");
            }
            let system = fixture.temp.path().join("system");
            drop(fixture.engine);
            let mut reopened = Engine::open(&system).unwrap();
            for (node, operation, receipt) in &receipts {
                let state = canonical_json(reopened.state()).unwrap();
                assert_eq!(
                    reopened
                        .set_file_node_note(
                            &fixture.file,
                            node,
                            &note("historical note"),
                            "admin",
                            *operation
                        )
                        .unwrap(),
                    *receipt
                );
                assert_eq!(canonical_json(reopened.state()).unwrap(), state);
                // A new clear removes the annotation, not its immutable node.
                reopened
                    .set_file_node_note(&fixture.file, node, &note(""), "admin", op())
                    .unwrap();
                assert!(!reopened.state.file_path_notes.contains_key(node));
                reopened
                    .set_file_node_note(&fixture.file, node, &note(""), "admin", op())
                    .unwrap();
                assert_eq!(
                    reopened
                        .set_file_node_note(
                            &fixture.file,
                            node,
                            &note("historical note"),
                            "admin",
                            *operation
                        )
                        .unwrap(),
                    *receipt
                );
                assert!(
                    !reopened.state.file_path_notes.contains_key(node),
                    "replay cannot undo a later clear"
                );
            }
            assert_only_note_publication(&before, reopened.state());
            drop(reopened);
            let reopened = Engine::open(&system).unwrap();
            let notes = history_notes(&reopened, &fixture.file);
            assert_eq!(notes[&fixture.path_node], "");
            assert_eq!(notes[&fixture.metadata_node], "");
            assert_eq!(notes[&fixture.other_node], "leave this note");
            assert_eq!(
                reopened
                    .list_tree(&fixture.repository.main_branch_id)
                    .unwrap(),
                current
            );
            assert_eq!(reopened.list_tree(&fixture.alias.branch_id).unwrap(), alias);
            assert_only_note_publication(&before, reopened.state());
            assert_eq!(files(&parked), stopped);
            assert_eq!(
                fs::read(&fixture.config.disks[0].root).unwrap(),
                b"closed target must remain an ordinary file"
            );
        }
    }
}

#[test]
fn membership_registry_current_path_note_retains_target_actor_and_idempotency_boundaries() {
    let mut fixture = Fixture::new(1);
    fixture.exit(MembershipExitAction::Deregister);
    let operation = op();
    let receipt = fixture
        .engine
        .set_file_node_note(
            &fixture.file,
            &fixture.path_node,
            &note("accepted"),
            "admin",
            operation,
        )
        .unwrap();
    let unchanged = canonical_json(fixture.engine.state()).unwrap();
    for (file, node, actor, expected) in [
        (
            "file_00000000000000000000000000000000",
            fixture.path_node.as_str(),
            "admin",
            ErrorCode::FileNodeNotFound,
        ),
        (
            fixture.file.as_str(),
            fixture.version.as_str(),
            "admin",
            ErrorCode::FileNodeNotFound,
        ),
        (
            fixture.file.as_str(),
            "fnode_00000000000000000000000000000000",
            "admin",
            ErrorCode::FileNodeNotFound,
        ),
        (
            fixture.file.as_str(),
            fixture.path_node.as_str(),
            "",
            ErrorCode::Unauthenticated,
        ),
    ] {
        assert_eq!(
            fixture
                .engine
                .set_file_node_note(file, node, &note("refused"), actor, op())
                .unwrap_err()
                .code(),
            expected
        );
        assert_eq!(canonical_json(fixture.engine.state()).unwrap(), unchanged);
    }
    for (value, actor) in [("other note", "admin"), ("accepted", "other actor")] {
        assert_eq!(
            fixture
                .engine
                .set_file_node_note(
                    &fixture.file,
                    &fixture.path_node,
                    &note(value),
                    actor,
                    operation
                )
                .unwrap_err()
                .code(),
            ErrorCode::IdempotencyKeyReused
        );
    }
    assert_eq!(
        fixture
            .engine
            .set_file_node_note(
                &fixture.file,
                &fixture.path_node,
                &note("accepted"),
                "admin",
                operation
            )
            .unwrap(),
        receipt
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), unchanged);
}

#[test]
fn membership_registry_current_path_note_replay_checks_exact_writes_before_unchanged_heads() {
    let mut fixture = Fixture::new(1);
    fixture.exit(MembershipExitAction::Deregister);
    let previous = Arc::clone(&fixture.engine.state);
    let view = Arc::clone(fixture.engine.current_namespaces.as_ref().unwrap());
    let operation = op();
    fixture
        .engine
        .set_file_node_note(
            &fixture.file,
            &fixture.path_node,
            &note("accepted"),
            "admin",
            operation,
        )
        .unwrap();
    let next = fixture.engine.state().clone();
    let decoded: State = json_codec::from_slice(&canonical_json(&next).unwrap()).unwrap();
    assert!(matches!(
        decoded.branches.changes_since(&previous.branches),
        MapChanges::All
    ));
    for candidate in [&next, &decoded] {
        let recovered = view.advance(&previous, candidate).unwrap();
        assert!(Arc::ptr_eq(&view.branches, &recovered.branches));
        assert!(Arc::ptr_eq(&view.directories, &recovered.directories));
    }
    let historical_operation = previous.operations.keys().next().copied().unwrap();
    for corruption in 0..24 {
        let mut changed = next.clone();
        match corruption {
            0 => {
                changed
                    .file_path_nodes
                    .get_mut(&fixture.path_node)
                    .unwrap()
                    .path = "injected.txt".into()
            }
            1 => {
                changed
                    .file_path_nodes
                    .get_mut(&fixture.path_node)
                    .unwrap()
                    .file_id = "different-file".into()
            }
            2 => {
                changed
                    .file_metadata_nodes
                    .get_mut(&fixture.metadata_node)
                    .unwrap()
                    .author = "injected".into();
            }
            3 => {
                changed
                    .commits
                    .get_mut(&fixture.alias.head_commit_id)
                    .unwrap()
                    .entries
                    .clear();
            }
            4 => {
                changed
                    .branches
                    .get_mut(&fixture.alias.branch_id)
                    .unwrap()
                    .name = "extra branch edit".into()
            }
            5 => {
                changed.file_contexts.insert(
                    format!("{}/current.txt", fixture.repository.repository_id),
                    FileContext {
                        branch_id: fixture.alias.branch_id.clone(),
                        node_id: Some(fixture.path_node.clone()),
                        selection_generation: u64::MAX,
                        inherited_from: None,
                    },
                );
            }
            6 => {
                changed
                    .file_path_notes
                    .insert(fixture.other_node.clone(), "extra note".into());
            }
            7 => {
                changed
                    .operations
                    .get_mut(&operation)
                    .unwrap()
                    .request_digest = "0".repeat(64)
            }
            8 => {
                changed
                    .operation_principals
                    .insert(operation, "other actor".into());
            }
            9 => {
                changed.operations.get_mut(&operation).unwrap().result["note"] =
                    json!("different request")
            }
            10 => {
                changed.operations.remove(&operation);
            }
            11 => {
                changed
                    .operations
                    .get_mut(&historical_operation)
                    .unwrap()
                    .result["injected"] = json!(true)
            }
            12 => changed.events[0].kind = "rewritten history".into(),
            13 => {
                changed
                    .file_locations
                    .insert(fixture.file.clone(), "injected.txt".into());
            }
            14 => {
                changed
                    .versions
                    .get_mut(&fixture.version)
                    .unwrap()
                    .history_tree_id = "different-history".into()
            }
            15 => {
                changed.file_metadata_nodes.remove(&fixture.metadata_node);
            }
            16 => {
                let id = new_id("commit").unwrap();
                let mut commit = changed.commits[&fixture.alias.head_commit_id].clone();
                commit.commit_id = id.clone();
                commit.sequence = changed.sequence;
                commit.operation_id = operation;
                changed.commits.insert(id.clone(), commit);
                changed
                    .branches
                    .get_mut(&fixture.alias.branch_id)
                    .unwrap()
                    .head_commit_id = id;
            }
            17 => {
                changed
                    .operations
                    .get_mut(&operation)
                    .unwrap()
                    .result
                    .as_object_mut()
                    .unwrap()
                    .remove("file_id");
            }
            18 => {
                changed.operations.get_mut(&operation).unwrap().result["note"] =
                    json!("invalid\nnote")
            }
            19 => {
                changed
                    .file_names
                    .insert(fixture.file.clone(), "injected.txt".into());
            }
            20 => {
                let id = new_id("fnode").unwrap();
                let mut forged = changed.file_path_nodes[&fixture.path_node].clone();
                forged.node_id = id.clone();
                changed.file_path_nodes.insert(id.clone(), forged);
                changed.file_path_notes.remove(&fixture.path_node);
                changed
                    .file_path_notes
                    .insert(id.clone(), "accepted".into());
                let receipt = changed.operations.get_mut(&operation).unwrap();
                receipt.result["node_id"] = json!(id);
                receipt.request_digest = Engine::native_request_digest(
                    KIND,
                    "admin",
                    &json!({"file_id":fixture.file,"node_id":id,"note":"accepted"}),
                )
                .unwrap();
            }
            21 => {
                let file = new_id("file").unwrap();
                let receipt = changed.operations.get_mut(&operation).unwrap();
                receipt.result["file_id"] = json!(file);
                receipt.request_digest = Engine::native_request_digest(
                    KIND,
                    "admin",
                    &json!({"file_id":file,"node_id":fixture.path_node,"note":"accepted"}),
                )
                .unwrap();
            }
            22 => {
                changed
                    .histories
                    .get_mut(&fixture.repository.history_tree_id)
                    .unwrap()
                    .repository_id = "different-repository".into();
            }
            23 => {
                let id = op();
                let mut extra = changed.operations[&operation].clone();
                extra.operation_id = id;
                changed.operations.insert(id, extra);
                changed.operation_principals.insert(id, "admin".into());
            }
            _ => unreachable!(),
        }
        for cold in [false, true] {
            let candidate = if cold {
                json_codec::from_slice::<State>(&canonical_json(&changed).unwrap()).unwrap()
            } else {
                changed.clone()
            };
            assert_eq!(
                view.advance(&previous, &candidate).err().unwrap().code(),
                ErrorCode::PublicationInvalid,
                "corruption {corruption}, cold {cold}"
            );
        }
    }
}

#[test]
fn membership_registry_current_path_note_target_requires_unique_node_and_exact_body_history() {
    let fixture = Fixture::new(1);
    let previous = fixture.engine.state();
    for target in [&fixture.path_node, &fixture.metadata_node] {
        assert_eq!(
            Engine::file_node_note_history(previous, &fixture.file, target).unwrap(),
            fixture.repository.history_tree_id
        );
    }
    for corruption in 0..8 {
        let mut changed = previous.clone();
        let target = match corruption {
            0 => {
                changed
                    .file_path_nodes
                    .get_mut(&fixture.path_node)
                    .unwrap()
                    .node_id = new_id("fnode").unwrap();
                &fixture.path_node
            }
            1 => {
                changed
                    .file_metadata_nodes
                    .get_mut(&fixture.metadata_node)
                    .unwrap()
                    .node_id = new_id("fmeta").unwrap();
                &fixture.metadata_node
            }
            2 => {
                let mut duplicate = changed.file_metadata_nodes[&fixture.metadata_node].clone();
                duplicate.node_id = fixture.path_node.clone();
                changed
                    .file_metadata_nodes
                    .insert(fixture.path_node.clone(), duplicate);
                &fixture.path_node
            }
            3 => {
                changed
                    .versions
                    .get_mut(&fixture.version)
                    .unwrap()
                    .version_id = new_id("ver").unwrap();
                &fixture.path_node
            }
            4 => {
                changed.versions.get_mut(&fixture.version).unwrap().file_id =
                    new_id("file").unwrap();
                &fixture.metadata_node
            }
            5 => {
                changed
                    .versions
                    .get_mut(&fixture.version)
                    .unwrap()
                    .history_tree_id = new_id("history").unwrap();
                &fixture.path_node
            }
            6 => {
                changed
                    .file_metadata_nodes
                    .get_mut(&fixture.metadata_node)
                    .unwrap()
                    .version_id = new_id("ver").unwrap();
                &fixture.metadata_node
            }
            7 => {
                changed
                    .file_path_nodes
                    .get_mut(&fixture.path_node)
                    .unwrap()
                    .file_id = new_id("file").unwrap();
                &fixture.path_node
            }
            _ => unreachable!(),
        };
        assert_eq!(
            Engine::file_node_note_history(&changed, &fixture.file, target)
                .unwrap_err()
                .code(),
            ErrorCode::FileNodeNotFound,
            "corruption {corruption}"
        );
    }
}

#[test]
fn membership_registry_current_path_note_no_change_requires_its_receipt_and_preserves_old_events() {
    let mut fixture = Fixture::new(1);
    fixture.exit(MembershipExitAction::Deregister);
    let previous = Arc::clone(&fixture.engine.state);
    let view = Arc::clone(fixture.engine.current_namespaces.as_ref().unwrap());
    let operation = op();
    fixture
        .engine
        .set_file_node_note(
            &fixture.file,
            &fixture.other_node,
            &note("leave this note"),
            "admin",
            operation,
        )
        .unwrap();
    let next = fixture.engine.state().clone();
    assert_eq!(next.file_path_notes, previous.file_path_notes);
    view.advance(&previous, &next).unwrap();
    for corruption in 0..4 {
        let mut changed = next.clone();
        match corruption {
            0 => {
                changed.operations.remove(&operation);
            }
            1 => {
                changed.operations.get_mut(&operation).unwrap().kind = "file.metadata".into();
            }
            2 => {
                changed.operations.remove(&operation);
                changed
                    .branches
                    .get_mut(&fixture.alias.branch_id)
                    .unwrap()
                    .name = "smuggled label".into();
            }
            3 => {
                // Rewriting an older note event is forbidden even in a state
                // without a new note receipt or changed annotation value.
                changed = previous.as_ref().clone();
                changed
                    .events
                    .iter_mut()
                    .find(|event| event.kind == KIND)
                    .unwrap()
                    .kind = "rewritten note event".into();
            }
            _ => unreachable!(),
        }
        for cold in [false, true] {
            let candidate = if cold {
                json_codec::from_slice::<State>(&canonical_json(&changed).unwrap()).unwrap()
            } else {
                changed.clone()
            };
            assert_eq!(
                view.advance(&previous, &candidate).err().unwrap().code(),
                ErrorCode::PublicationInvalid,
                "corruption {corruption}, cold {cold}"
            );
        }
    }
}
