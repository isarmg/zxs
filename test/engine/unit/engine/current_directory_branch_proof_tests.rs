//! Actual retained D3 publication followed by directory business operations.
//! All storage roots are temporary registered directories, never real devices.
use super::*;
use crate::*;
use std::{
    fs,
    path::{Path, PathBuf},
};

fn op() -> OperationId {
    OperationId::generate().unwrap()
}

fn save(engine: &mut Engine, repo: &Repository, path: &str, bytes: &[u8]) {
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
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
            message: "current directory source".into(),
            request_digest: None,
        })
        .unwrap();
}

fn create(
    engine: &mut Engine,
    repo: &Repository,
    path: Option<&str>,
    name: &str,
    operation: OperationId,
) -> Result<OperationResult> {
    let path = path.map(|path| name_policy::LookupPath::parse(path).unwrap());
    engine.create_directory_branches(
        &repo.repository_id,
        path.as_ref(),
        &BranchName::parse(name).unwrap(),
        "admin",
        operation,
    )
}

fn selected(
    engine: &mut Engine,
    repo: &Repository,
    branch: &str,
    operation: OperationId,
) -> Result<OperationResult> {
    let tree = engine.state.directory_branches[branch].head_tree_id.clone();
    let head = engine.state.branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    engine.set_directory_default(DirectoryDefaultSelection {
        repository_id: &repo.repository_id,
        directory_branch_id: branch,
        expected_tree_id: &tree,
        expected_head: &head,
        principal: "admin",
        operation_id: operation,
    })
}

fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut result = BTreeMap::new();
    for item in fs::read_dir(root).unwrap() {
        let path = item.unwrap().path();
        if path.is_dir() {
            result.extend(files(&path));
        } else {
            result.insert(path.clone(), fs::read(path).unwrap());
        }
    }
    result
}

struct Fixture {
    temp: tempfile::TempDir,
    engine: Engine,
    repo: Repository,
    target: DiskConfig,
    root_branch: String,
    directory_branch: String,
    lost_branch: String,
    root_operation: OperationId,
    root_receipt: OperationResult,
    hidden: Vec<Entry>,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let mut config =
            Config::development((0..4).map(|index| temp.path().join(format!("disk-{index}"))))
                .unwrap();
        config.disks[3].level = 1;
        config.disks[3].upstream_slot_id = None;
        config.disks[3].metadata_member = false;
        let target = config.disks[0].clone();
        let mut engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
        let repo = engine.create_disk_root("D04", "admin", op()).unwrap();
        for path in ["dir/a", "dir/child/hidden", "only/hidden"] {
            let rule = new_id("rule").unwrap();
            engine
                .update_placement_rule(PlacementRuleUpdate {
                    rule_id: &rule,
                    repository_id: &repo.repository_id,
                    path_prefix: path,
                    primary_slot_id: "P01",
                    expected_generation: 0,
                    principal: "admin",
                    operation_id: op(),
                })
                .unwrap();
            save(&mut engine, &repo, path, b"target-only original");
            engine
                .update_placement_rule(PlacementRuleUpdate {
                    rule_id: &rule,
                    repository_id: &repo.repository_id,
                    path_prefix: path,
                    primary_slot_id: "D04",
                    expected_generation: 1,
                    principal: "admin",
                    operation_id: op(),
                })
                .unwrap();
        }
        save(&mut engine, &repo, "dir/b", b"retained original");
        save(&mut engine, &repo, "dir/child/kept", b"retained child");
        save(&mut engine, &repo, "outside", b"retained sibling");
        let root_operation = op();
        let root_receipt = create(&mut engine, &repo, None, "frozen root", root_operation).unwrap();
        let root_branch = root_receipt.result["directory_branch_id"]
            .as_str()
            .unwrap()
            .into();
        let directory_branch = create(&mut engine, &repo, Some("dir"), "frozen dir", op())
            .unwrap()
            .result["directory_branch_id"]
            .as_str()
            .unwrap()
            .into();
        let lost_branch = create(&mut engine, &repo, Some("only"), "lost", op())
            .unwrap()
            .result["directory_branch_id"]
            .as_str()
            .unwrap()
            .into();
        let hidden = engine
            .list_tree(&repo.main_branch_id)
            .unwrap()
            .into_iter()
            .filter(|entry| matches!(entry.path(), "dir/a" | "dir/child/hidden" | "only/hidden"))
            .collect();
        Self {
            temp,
            engine,
            repo,
            target,
            root_branch,
            directory_branch,
            lost_branch,
            root_operation,
            root_receipt,
            hidden,
        }
    }

    fn exit(&mut self, action: MembershipExitAction) {
        self.engine
            .begin_disk_exit(&self.target.disk_id, action, "admin", op())
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
                .all_certificates_synchronized()
        );
        self.engine.activate_pending_disk_exit().unwrap();
        assert_eq!(
            self.engine.disk_exit_status().unwrap().unwrap().phase,
            DiskExitPhase::Published
        );
    }
}

fn assert_projected_graph(engine: &Engine, directory: &str, hidden: &[Entry]) {
    let tree = &engine.state.directory_trees[&engine.state.directory_tree_selections[directory]];
    assert!(
        tree.entries
            .iter()
            .all(|entry| !hidden.iter().any(|old| old.entry_id == entry.entry_id))
    );
    let files = tree
        .entries
        .iter()
        .filter(|entry| entry.kind == NamespaceEntryKind::File)
        .collect::<Vec<_>>();
    assert_eq!(tree.file_contexts.len(), files.len());
    for entry in files {
        assert!(tree.file_contexts.contains_key(entry.lookup_key()));
        assert!(tree.file_branches.contains_key(&entry.file_id));
    }
    for (directory, reference) in &tree.directory_refs {
        assert_eq!(
            engine.state.directory_selections[directory],
            reference.directory_branch_id
        );
        assert_eq!(
            engine.state.directory_tree_selections[directory],
            reference.tree_id
        );
        assert_projected_graph(engine, directory, hidden);
    }
}

#[test]
fn disk_exit_current_directory_choices_project_entries_contexts_and_refs_and_reopen() {
    let mut fixture = Fixture::new();
    let original_trees = fixture.engine.state.directory_trees.clone();
    let original_commits = fixture.engine.state.commits.clone();
    let source = fixture.engine.state.directory_branches[&fixture.root_branch].clone();
    fixture.exit(MembershipExitAction::Deregister);
    let target_bytes = files(&fixture.target.root);
    let before = canonical_json(fixture.engine.state()).unwrap();
    let preview = fixture
        .engine
        .preview_directory_branch(
            &fixture.repo.repository_id,
            &fixture.root_branch,
            &source.head_tree_id,
        )
        .unwrap();
    assert!(
        preview
            .branch()
            .entries
            .iter()
            .all(|entry| !fixture.hidden.contains(entry))
    );
    assert_eq!(
        preview.branch().file_contexts.len(),
        preview
            .branch()
            .entries
            .iter()
            .filter(|entry| entry.kind == NamespaceEntryKind::File)
            .count()
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    let operation = op();
    let head = fixture.engine.state.branches[&fixture.repo.main_branch_id]
        .head_commit_id
        .clone();
    let result = selected(
        &mut fixture.engine,
        &fixture.repo,
        &fixture.root_branch,
        operation,
    )
    .unwrap();
    assert_eq!(
        fixture.engine.state.directory_branches[&fixture.root_branch],
        source
    );
    assert_projected_graph(
        &fixture.engine,
        &fixture.repo.repository_id,
        &fixture.hidden,
    );
    for entry in &fixture.hidden {
        assert!(
            fixture
                .engine
                .namespace_tree(&fixture.repo.main_branch_id)
                .unwrap()
                .get(entry.path())
                .is_none()
        );
        assert!(
            fixture
                .engine
                .file_context(&fixture.repo.repository_id, entry.path())
                .is_none()
        );
    }
    assert_eq!(
        create(
            &mut fixture.engine,
            &fixture.repo,
            None,
            "frozen root",
            fixture.root_operation
        )
        .unwrap(),
        fixture.root_receipt
    );
    let fresh = create(
        &mut fixture.engine,
        &fixture.repo,
        Some("DIR"),
        "after exit",
        op(),
    )
    .unwrap();
    let fresh_id = fresh.result["directory_branch_id"].as_str().unwrap();
    assert!(
        fixture.engine.state.directory_branches[fresh_id]
            .entries
            .iter()
            .all(|entry| !fixture.hidden.contains(entry))
    );
    assert!(!fixture.engine.state.directory_branches[fresh_id].materialized);
    for (id, tree) in original_trees.iter() {
        assert_eq!(fixture.engine.state.directory_trees.get(id), Some(tree));
    }
    for (id, commit) in original_commits.iter() {
        assert_eq!(fixture.engine.state.commits.get(id), Some(commit));
    }
    drop(preview);
    drop(fixture.engine);
    let mut engine = Engine::open(&fixture.temp.path().join("system")).unwrap();
    assert_eq!(
        engine
            .set_directory_default(DirectoryDefaultSelection {
                repository_id: &fixture.repo.repository_id,
                directory_branch_id: &fixture.root_branch,
                expected_tree_id: &source.head_tree_id,
                expected_head: &head,
                principal: "admin",
                operation_id: operation,
            })
            .unwrap(),
        result
    );
    assert_projected_graph(&engine, &fixture.repo.repository_id, &fixture.hidden);
    assert_eq!(files(&fixture.target.root), target_bytes);
}

#[test]
fn disk_exit_current_directory_selection_preserves_siblings_and_replays_exact_receipts() {
    let mut fixture = Fixture::new();
    fixture.exit(MembershipExitAction::Deregister);
    let outside = fixture
        .engine
        .namespace_tree(&fixture.repo.main_branch_id)
        .unwrap()
        .get("outside")
        .unwrap()
        .clone();
    let source = fixture.engine.state.directory_branches[&fixture.directory_branch].clone();
    let operation = op();
    let request = |engine: &mut Engine| {
        engine.select_directory_branches(
            &fixture.repo.repository_id,
            Some(&name_policy::LookupPath::parse("DIR").unwrap()),
            &BranchName::parse("frozen dir").unwrap(),
            "admin",
            operation,
        )
    };
    let result = request(&mut fixture.engine).unwrap();
    assert_eq!(request(&mut fixture.engine).unwrap(), result);
    assert_eq!(
        fixture
            .engine
            .namespace_tree(&fixture.repo.main_branch_id)
            .unwrap()
            .get("outside"),
        Some(&outside)
    );
    assert_eq!(
        fixture.engine.state.directory_branches[&fixture.directory_branch],
        source
    );
    assert_projected_graph(&fixture.engine, &source.directory_id, &fixture.hidden);
    let tree_count = fixture.engine.state.directory_trees.len();
    let unchanged = selected(
        &mut fixture.engine,
        &fixture.repo,
        &fixture.directory_branch,
        op(),
    )
    .unwrap();
    assert_eq!(unchanged.result["no_change"], true);
    assert_eq!(fixture.engine.state.directory_trees.len(), tree_count);
}

#[test]
fn disk_exit_new_same_name_directory_never_inherits_lost_identity_or_old_branch() {
    let mut fixture = Fixture::new();
    let lost = fixture.engine.state.directory_branches[&fixture.lost_branch].clone();
    fixture.exit(MembershipExitAction::Deregister);
    // D04 still owns this empty directory after its only file is removed.
    // Deleting and recreating the path must not recover its old identity.
    fixture
        .engine
        .file_namespace(
            &fixture.repo.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::DeleteTree, "only", None).unwrap(),
            "admin",
            op(),
        )
        .unwrap();
    assert_eq!(
        fixture
            .engine
            .directory_branches(&fixture.repo.repository_id, "only")
            .unwrap_err()
            .code(),
        ErrorCode::DirectoryNotFound
    );
    save(
        &mut fixture.engine,
        &fixture.repo,
        "only/new",
        b"fresh directory identity",
    );
    let created = create(
        &mut fixture.engine,
        &fixture.repo,
        Some("only"),
        "new scope",
        op(),
    )
    .unwrap();
    let id = created.result["directory_branch_id"].as_str().unwrap();
    assert_ne!(
        fixture.engine.state.directory_branches[id].directory_id,
        lost.directory_id
    );
    let before = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(
        selected(
            &mut fixture.engine,
            &fixture.repo,
            &fixture.lost_branch,
            op()
        )
        .unwrap_err()
        .code(),
        ErrorCode::DirectoryNotFound
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    assert_eq!(
        fixture.engine.state.directory_branches[&fixture.lost_branch],
        lost
    );
}

#[test]
fn disk_exit_directory_replay_rejects_hidden_splice_context_and_ref_graph_tampering() {
    let mut fixture = Fixture::new();
    fixture.exit(MembershipExitAction::Deregister);
    let previous = Arc::clone(&fixture.engine.state);
    let view = Arc::clone(fixture.engine.current_namespaces.as_ref().unwrap());
    selected(
        &mut fixture.engine,
        &fixture.repo,
        &fixture.root_branch,
        op(),
    )
    .unwrap();
    let published = fixture.engine.state.as_ref().clone();
    view.advance(&previous, &published).unwrap();
    for corruption in ["entry", "context", "tree", "old_tree"] {
        let mut next = published.clone();
        match corruption {
            "entry" => {
                let head = next.branches[&fixture.repo.main_branch_id]
                    .head_commit_id
                    .clone();
                let commit = next.commits.get_mut(&head).unwrap();
                commit.entries.push(
                    fixture
                        .hidden
                        .iter()
                        .find(|entry| entry.path() == "dir/a")
                        .unwrap()
                        .clone(),
                );
                commit
                    .entries
                    .sort_by(|a, b| a.lookup_key().cmp(b.lookup_key()));
            }
            "context" => {
                let key =
                    crate::business::context_key_from_key(&fixture.repo.repository_id, "dir/b");
                next.file_contexts.get_mut(&key).unwrap().branch_id =
                    fixture.repo.main_branch_id.clone();
            }
            "tree" => {
                let id = next.directory_tree_selections[&fixture.repo.repository_id].clone();
                next.directory_trees
                    .get_mut(&id)
                    .unwrap()
                    .file_contexts
                    .clear();
            }
            "old_tree" => {
                let id = previous.directory_branches[&fixture.root_branch]
                    .head_tree_id
                    .clone();
                next.directory_trees.get_mut(&id).unwrap().entries.clear();
            }
            _ => unreachable!(),
        }
        assert!(view.advance(&previous, &next).is_err(), "{corruption}");
    }
}

#[test]
fn disk_exit_directory_fork_replay_rejects_cross_scope_identity_and_context_imports() {
    let mut fixture = Fixture::new();
    fixture.exit(MembershipExitAction::Deregister);
    let previous = Arc::clone(&fixture.engine.state);
    let view = Arc::clone(fixture.engine.current_namespaces.as_ref().unwrap());
    let result = create(
        &mut fixture.engine,
        &fixture.repo,
        Some("dir"),
        "after exit",
        op(),
    )
    .unwrap();
    let branch = result.result["directory_branch_id"].as_str().unwrap();
    let published = fixture.engine.state.as_ref().clone();
    view.advance(&previous, &published).unwrap();
    for corruption in ["directory", "entry", "context", "materialized"] {
        let mut next = published.clone();
        let target = next.directory_branches.get_mut(branch).unwrap();
        match corruption {
            "directory" => target.directory_id = fixture.repo.repository_id.clone(),
            "entry" => {
                target.entries.push(
                    fixture
                        .hidden
                        .iter()
                        .find(|entry| entry.path() == "dir/a")
                        .unwrap()
                        .clone(),
                );
                target
                    .entries
                    .sort_by(|a, b| a.lookup_key().cmp(b.lookup_key()));
            }
            "context" => {
                target.file_contexts.get_mut("dir/b").unwrap().branch_id =
                    fixture.repo.main_branch_id.clone()
            }
            "materialized" => target.materialized = true,
            _ => unreachable!(),
        }
        assert!(view.advance(&previous, &next).is_err(), "{corruption}");
    }
}

#[test]
fn disk_exit_directory_default_rejects_stale_heads_without_publishing_a_receipt() {
    let mut fixture = Fixture::new();
    fixture.exit(MembershipExitAction::Deregister);
    let tree = fixture.engine.state.directory_branches[&fixture.directory_branch]
        .head_tree_id
        .clone();
    let head = fixture.engine.state.branches[&fixture.repo.main_branch_id]
        .head_commit_id
        .clone();
    for (expected_tree, expected_head) in [("old-tree", head.as_str()), (tree.as_str(), "old-head")]
    {
        let before = canonical_json(fixture.engine.state()).unwrap();
        let operation = op();
        assert_eq!(
            fixture
                .engine
                .set_directory_default(DirectoryDefaultSelection {
                    repository_id: &fixture.repo.repository_id,
                    directory_branch_id: &fixture.directory_branch,
                    expected_tree_id: expected_tree,
                    expected_head,
                    principal: "admin",
                    operation_id: operation,
                })
                .unwrap_err()
                .code(),
            ErrorCode::ListingChanged
        );
        assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
        assert!(!fixture.engine.state.operations.contains_key(&operation));
    }
}

#[test]
fn disk_exit_directory_failed_publication_keeps_selection_and_retry_reopens() {
    let mut fixture = Fixture::new();
    fixture.exit(MembershipExitAction::Deregister);
    let target_bytes = files(&fixture.target.root);
    let before = fixture.engine.state.directory_selections.clone();
    let trees = fixture.engine.state.directory_tree_selections.clone();
    let member = fixture.engine.members()[0].disk_id.clone();
    fixture.engine.journals.get_mut(&member).unwrap().fault =
        Some(crate::journal::JournalFault::Sync);
    let operation = op();
    assert!(
        selected(
            &mut fixture.engine,
            &fixture.repo,
            &fixture.root_branch,
            operation
        )
        .is_err()
    );
    assert_eq!(fixture.engine.state.directory_selections, before);
    assert_eq!(fixture.engine.state.directory_tree_selections, trees);
    drop(fixture.engine);
    let mut engine = Engine::open(&fixture.temp.path().join("system")).unwrap();
    assert_eq!(engine.state.directory_selections, before);
    assert_eq!(engine.state.directory_tree_selections, trees);
    selected(&mut engine, &fixture.repo, &fixture.root_branch, operation).unwrap();
    assert_eq!(files(&fixture.target.root), target_bytes);
}

#[test]
fn disk_exit_disabled_target_keeps_directory_data_and_choices_without_target_writes() {
    let mut fixture = Fixture::new();
    fixture.exit(MembershipExitAction::PlannedOffline);
    let target_bytes = files(&fixture.target.root);
    selected(
        &mut fixture.engine,
        &fixture.repo,
        &fixture.root_branch,
        op(),
    )
    .unwrap();
    assert!(
        fixture
            .engine
            .namespace_tree(&fixture.repo.main_branch_id)
            .unwrap()
            .get("dir/a")
            .is_some()
    );
    assert!(
        fixture
            .engine
            .registered_disks()
            .any(|disk| disk.slot_id == fixture.target.slot_id)
    );
    assert_eq!(files(&fixture.target.root), target_bytes);
}
