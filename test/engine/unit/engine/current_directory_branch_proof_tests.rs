//! Actual retained D3 publication followed by directory business operations.
//! All storage roots are temporary registered directories, never real devices.
use super::*;
#[path = "current_directory_contents_tests.rs"]
mod directory_contents;
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

fn initialize_git(engine: &mut Engine, repo: &Repository, path: &str) {
    let expected = engine.state.branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    engine
        .initialize_directory_git(
            &repo.repository_id,
            &storage_contract::directory_git::DirectoryGitInitializeBody {
                path: path.into(),
                expected_namespace_head: expected,
                operation_id: op(),
            },
            "admin",
        )
        .unwrap();
}
fn stage_git(
    engine: &mut Engine,
    repo: &Repository,
    path: &str,
) -> storage_contract::directory_git::DirectoryGitStatus {
    let status = engine
        .directory_git_status(&repo.repository_id, path)
        .unwrap();
    engine
        .stage_directory_git_owned(
            &repo.repository_id,
            &storage_contract::directory_git::DirectoryGitStageOwnedBody {
                directory_id: status.directory_id,
                directory_branch_id: status.directory_branch_id.unwrap(),
                expected_tree_id: status.working_tree_id.unwrap(),
                expected_index_generation: status.index_generation,
                operation_id: op(),
            },
            "admin",
        )
        .unwrap();
    engine
        .directory_git_status(&repo.repository_id, path)
        .unwrap()
}
fn review_git(
    engine: &mut Engine,
    repo: &Repository,
    path: &str,
) -> storage_contract::directory_git::DirectoryGitStatus {
    let status = stage_git(engine, repo, path);
    engine
        .commit_directory_changes(DirectoryCommitRequest {
            repository_id: &repo.repository_id,
            directory_branch_id: status.directory_branch_id.as_ref().unwrap(),
            expected_tree_id: status.working_tree_id.as_ref().unwrap(),
            expected_index_generation: status.index_generation,
            message: "Review current registry composition",
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    engine
        .directory_git_status(&repo.repository_id, path)
        .unwrap()
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
        for path in ["dir", "only", ""] {
            initialize_git(&mut engine, &repo, path);
            review_git(&mut engine, &repo, path);
        }
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
        // The user reviews the retained working composition explicitly; exiting
        // a disk does not turn the old hidden working checkpoint into a commit.
        for path in ["dir", ""] {
            review_git(&mut self.engine, &self.repo, path);
        }
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
        fixture.engine.state.directory_trees[&source.head_tree_id].entries,
        source.entries
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
        fixture.engine.state.directory_trees[&source.head_tree_id].entries,
        source.entries
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
    initialize_git(&mut fixture.engine, &fixture.repo, "only");
    review_git(&mut fixture.engine, &fixture.repo, "only");
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
    let status = fixture
        .engine
        .directory_git_status(&fixture.repo.repository_id, "dir")
        .unwrap();
    let source = storage_contract::directory_git::DirectoryBranchCreateSource {
        repository_id: fixture.repo.repository_id.clone(),
        directory_id: status.directory_id,
        directory_branch_id: status.directory_branch_id.unwrap(),
        working_tree_id: status.working_tree_id.unwrap(),
        head_tree_id: status.head_tree_id.unwrap(),
        index_generation: status.index_generation,
        namespace_head: fixture.engine.state.branches[&fixture.repo.main_branch_id]
            .head_commit_id
            .clone(),
    };
    let result = fixture
        .engine
        .create_directory_branch_from_source(
            &source,
            Some(&name_policy::LookupPath::parse("dir").unwrap()),
            &BranchName::parse("after exit").unwrap(),
            "admin",
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
    // Recompute the forged digest too: rejection must come from source
    // re-derivation against previous State, not just a receipt hash mismatch.
    for (field, value) in [
        ("repository_id", new_id("repo").unwrap()),
        ("directory_id", new_id("entry").unwrap()),
        ("directory_branch_id", new_id("dbr").unwrap()),
        ("working_tree_id", new_id("dtree").unwrap()),
        ("head_tree_id", new_id("dtree").unwrap()),
        (
            "index_generation",
            (source.index_generation + 1).to_string(),
        ),
        ("namespace_head", new_id("commit").unwrap()),
    ] {
        let mut next = published.clone();
        let receipt = next.operations.get_mut(&result.operation_id).unwrap();
        receipt.result["input"]["source"][field] = json_codec::json!(value);
        receipt.request_digest =
            Engine::native_request_digest(&receipt.kind, "admin", &receipt.result["input"])
                .unwrap();
        assert!(view.advance(&previous, &next).is_err(), "{field}");
    }
}
#[test]
fn disk_exit_parent_checkout_preserves_child_after_its_old_file_branch_is_archived() {
    let mut fixture = Fixture::new();
    fixture.exit(MembershipExitAction::Deregister);
    save(
        &mut fixture.engine,
        &fixture.repo,
        "dir-sibling",
        b"owned between mount and child",
    );
    review_git(&mut fixture.engine, &fixture.repo, "");
    let parent = create(
        &mut fixture.engine,
        &fixture.repo,
        None,
        "before child archive",
        op(),
    )
    .unwrap()
    .result["directory_branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let entry = fixture
        .engine
        .namespace_tree(&fixture.repo.main_branch_id)
        .unwrap()
        .get("dir/b")
        .unwrap()
        .clone();
    let previous = fixture
        .engine
        .file_context(&fixture.repo.repository_id, "dir/b")
        .unwrap()
        .branch_id;
    let alternate = fixture
        .engine
        .create_file_branch(
            &entry.file_id,
            &entry.version_id,
            &BranchName::parse("independent current child").unwrap(),
            "admin",
            op(),
        )
        .unwrap()
        .branch_id
        .unwrap();
    fixture
        .engine
        .select_file_branch_at(
            &fixture.repo.repository_id,
            &entry.path,
            &entry.file_id,
            &alternate,
            "admin",
            op(),
        )
        .unwrap();
    save(
        &mut fixture.engine,
        &fixture.repo,
        "dir/b",
        b"new child body",
    );
    let child = review_git(&mut fixture.engine, &fixture.repo, "dir");
    fixture
        .engine
        .archive_branch(&previous, "admin", op())
        .unwrap();
    create(
        &mut fixture.engine,
        &fixture.repo,
        None,
        "after child archive",
        op(),
    )
    .unwrap();
    let state = fixture.engine.state.as_ref();
    let view = fixture.engine.current_namespaces.as_ref().unwrap();
    let retained = BTreeMap::from([(child.directory_id.clone(), "dir".to_owned())]);
    assert!(view.directory_projection(state, &parent).is_err());
    assert!(
        view.directory_checkout_projection(state, &parent, &BTreeMap::new())
            .is_err()
    );
    view.directory_checkout_projection(state, &parent, &retained)
        .unwrap();
    for corruption in ["child_context", "child_tree", "owned_branch"] {
        let mut forged = state.clone();
        match corruption {
            "child_context" => {
                forged
                    .directory_branches
                    .get_mut(&parent)
                    .unwrap()
                    .file_contexts
                    .get_mut("dir/b")
                    .unwrap()
                    .branch_id = fixture.repo.main_branch_id.clone();
            }
            "child_tree" => {
                let tree = forged.directory_branches[&parent].directory_refs[&child.directory_id]
                    .tree_id
                    .clone();
                forged.directory_trees.remove(&tree);
            }
            "owned_branch" => {
                let branch = forged.directory_branches[&parent].file_contexts["outside"]
                    .branch_id
                    .clone();
                forged.branches.get_mut(&branch).unwrap().state = BranchState::Archived;
            }
            _ => unreachable!(),
        }
        assert!(
            view.directory_checkout_projection(&forged, &parent, &retained)
                .is_err(),
            "{corruption}"
        );
    }
    selected(&mut fixture.engine, &fixture.repo, &parent, op()).unwrap();
    assert_eq!(
        fixture
            .engine
            .directory_git_status(&fixture.repo.repository_id, "dir")
            .unwrap(),
        child
    );
    let namespace = fixture
        .engine
        .list_tree(&fixture.repo.main_branch_id)
        .unwrap();
    assert_projected_graph(
        &fixture.engine,
        &fixture.repo.repository_id,
        &fixture.hidden,
    );
    drop(fixture.engine);
    let engine = Engine::open(&fixture.temp.path().join("system")).unwrap();
    assert_eq!(
        engine.list_tree(&fixture.repo.main_branch_id).unwrap(),
        namespace
    );
    assert_eq!(
        engine
            .directory_git_status(&fixture.repo.repository_id, "dir")
            .unwrap(),
        child
    );
}

#[test]
fn disk_exit_explicit_directory_commit_preserves_projection_and_cold_replay() {
    let mut f = Fixture::new();
    f.exit(MembershipExitAction::Deregister);
    save(&mut f.engine, &f.repo, "new-owned", b"current");
    let status = stage_git(&mut f.engine, &f.repo, "");
    let namespace = f.engine.list_tree(&f.repo.main_branch_id).unwrap();
    let versions = f.engine.state.versions.clone();
    let target = files(&f.target.root);
    let operation = op();
    let apply = |engine: &mut Engine| {
        engine.commit_directory_changes(DirectoryCommitRequest {
            repository_id: &f.repo.repository_id,
            directory_branch_id: status.directory_branch_id.as_ref().unwrap(),
            expected_tree_id: status.working_tree_id.as_ref().unwrap(),
            expected_index_generation: status.index_generation,
            message: "Commit admitted current repository",
            principal: "admin",
            operation_id: operation,
        })
    };
    let result = apply(&mut f.engine).unwrap();
    assert_eq!(
        f.engine.list_tree(&f.repo.main_branch_id).unwrap(),
        namespace
    );
    assert_eq!(f.engine.state.versions, versions);
    assert_projected_graph(&f.engine, &f.repo.repository_id, &f.hidden);
    drop(f.engine);
    let mut engine = Engine::open(&f.temp.path().join("system")).unwrap();
    assert_eq!(apply(&mut engine).unwrap(), result);
    assert_eq!(engine.list_tree(&f.repo.main_branch_id).unwrap(), namespace);
    assert_eq!(files(&f.target.root), target);
}
#[test]
fn disk_exit_directory_commit_replay_proves_closed_write_set_without_tracking() {
    let mut f = Fixture::new();
    f.exit(MembershipExitAction::Deregister);
    save(&mut f.engine, &f.repo, "new-owned", b"current");
    let status = stage_git(&mut f.engine, &f.repo, "");
    let previous = Arc::clone(&f.engine.state);
    let view = Arc::clone(f.engine.current_namespaces.as_ref().unwrap());
    let operation = op();
    let result = f
        .engine
        .commit_directory_changes(DirectoryCommitRequest {
            repository_id: &f.repo.repository_id,
            directory_branch_id: status.directory_branch_id.as_ref().unwrap(),
            expected_tree_id: status.working_tree_id.as_ref().unwrap(),
            expected_index_generation: status.index_generation,
            message: "Exact current Git commit",
            principal: "admin",
            operation_id: operation,
        })
        .unwrap();
    let previous: State =
        json_codec::from_slice(&canonical_json(previous.as_ref()).unwrap()).unwrap();
    let next: State = json_codec::from_slice(&canonical_json(f.engine.state()).unwrap()).unwrap();
    view.advance(&previous, &next).unwrap();
    let head = result.result["head_tree_id"].as_str().unwrap();
    for corruption in [
        "old_tree",
        "new_tree",
        "context",
        "child_ref",
        "commit",
        "receipt",
        "principal",
        "file_note",
        "event",
        "git_head",
        "git_index",
        "git_main",
    ] {
        let mut changed = next.clone();
        match corruption {
            "old_tree" => changed
                .directory_trees
                .get_mut(status.working_tree_id.as_ref().unwrap())
                .unwrap()
                .entries
                .clear(),
            "new_tree" => changed
                .directory_trees
                .get_mut(head)
                .unwrap()
                .entries
                .clear(),
            "context" => changed
                .directory_trees
                .get_mut(head)
                .unwrap()
                .file_contexts
                .clear(),
            "child_ref" => {
                changed
                    .directory_trees
                    .get_mut(head)
                    .unwrap()
                    .directory_refs
                    .insert(
                        f.repo.repository_id.clone(),
                        DirectoryReference {
                            directory_branch_id: status.directory_branch_id.clone().unwrap(),
                            tree_id: head.into(),
                        },
                    );
            }
            "commit" => {
                changed
                    .commits
                    .get_mut(result.commit_id.as_ref().unwrap())
                    .unwrap()
                    .author = "other author".into()
            }
            "receipt" => {
                changed.operations.get_mut(&operation).unwrap().result["input"]["message"] =
                    "other review".into()
            }
            "principal" => {
                changed
                    .operation_principals
                    .insert(operation, "other principal".into());
            }
            "file_note" => {
                changed
                    .file_notes
                    .insert(f.hidden[0].file_id.clone(), "unrelated".into());
            }
            "event" => {
                changed.events.pop();
            }
            "git_head" => {
                changed
                    .directory_git_repositories
                    .get_mut(&f.repo.repository_id)
                    .unwrap()
                    .branches
                    .get_mut(status.directory_branch_id.as_ref().unwrap())
                    .unwrap()
                    .head_tree_id = None
            }
            "git_index" => {
                changed
                    .directory_git_repositories
                    .get_mut(&f.repo.repository_id)
                    .unwrap()
                    .branches
                    .get_mut(status.directory_branch_id.as_ref().unwrap())
                    .unwrap()
                    .index_generation += 1
            }
            "git_main" => {
                changed
                    .directory_git_repositories
                    .get_mut(&f.repo.repository_id)
                    .unwrap()
                    .main_branch_id = f.root_branch.clone();
            }
            _ => unreachable!(),
        }
        assert!(view.advance(&previous, &changed).is_err(), "{corruption}");
    }
    assert!(view.require_operation("directory.changes.commit").is_err());
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

#[test]
fn detached_git_status_rechecks_current_namespace_publication_even_at_the_same_sequence() {
    let mut fixture = Fixture::new();
    fixture.exit(MembershipExitAction::Deregister);
    let read = fixture
        .engine
        .capture_directory_git_status(
            &fixture.repo.repository_id,
            "",
            HistoryWindowCancellation::new(),
        )
        .unwrap();
    assert_eq!(
        read.status().unwrap(),
        fixture
            .engine
            .directory_git_status(&fixture.repo.repository_id, "")
            .unwrap()
    );
    fixture.engine.recheck_directory_git_status(&read).unwrap();
    let sequence = fixture.engine.state.sequence;
    let publication = fixture
        .engine
        .current_namespaces
        .as_ref()
        .unwrap()
        .as_ref()
        .clone();
    fixture.engine.current_namespaces = Some(Arc::new(publication));
    assert_eq!(fixture.engine.state.sequence, sequence);
    assert_eq!(
        fixture
            .engine
            .recheck_directory_git_status(&read)
            .unwrap_err()
            .code(),
        ErrorCode::ListingChanged
    );
    fixture.engine.current_namespaces = None;
    assert_eq!(
        fixture
            .engine
            .recheck_directory_git_status(&read)
            .unwrap_err()
            .code(),
        ErrorCode::RecoveryRequired
    );
}
