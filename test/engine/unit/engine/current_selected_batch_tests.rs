//! Real temporary registered-directory D3, selected projections, batch, reopen.
//! These source assertions do not certify physical-device or power-loss behavior.
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
            message: "selected directory batch fixture".into(),
            request_digest: None,
        })
        .unwrap();
}

fn entry(engine: &Engine, repo: &Repository, path: &str) -> Entry {
    engine
        .namespace_tree(&repo.main_branch_id)
        .unwrap()
        .get(path)
        .unwrap()
        .clone()
}

fn body(engine: &Engine, entry: &Entry) -> Vec<u8> {
    let mut bytes = Vec::new();
    engine
        .open_content(&entry.version_id)
        .unwrap()
        .file
        .read_to_end(&mut bytes)
        .unwrap();
    bytes
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

fn selected_branch(engine: &Engine, repo: &Repository, path: &str) -> String {
    let directory = if path.is_empty() {
        repo.repository_id.clone()
    } else {
        entry(engine, repo, path).entry_id
    };
    engine.state.directory_selections[&directory].clone()
}

fn mapping(
    engine: &Engine,
    repo: &Repository,
    source: &str,
    path: &str,
    target: Option<(&str, &str)>,
) -> DirectoryNamespaceItem {
    DirectoryNamespaceItem {
        source: DirectoryNamespaceSource {
            directory_branch_id: source.into(),
            expected_tree_id: engine.state.directory_branches[source].head_tree_id.clone(),
            path: path.into(),
            entry_id: entry(engine, repo, path).entry_id,
        },
        target: target.map(|(id, path)| DirectoryNamespaceTarget {
            directory_branch_id: id.into(),
            expected_tree_id: engine.state.directory_branches[id].head_tree_id.clone(),
            path: path.into(),
        }),
    }
}

fn apply(
    engine: &mut Engine,
    repo: &Repository,
    request: &DirectoryNamespaceBatch,
    operation: OperationId,
) -> Result<OperationResult> {
    engine.directory_namespace_batch(&repo.repository_id, request, "admin", operation)
}

struct Fixture {
    temp: tempfile::TempDir,
    engine: Engine,
    repo: Repository,
    target: DiskConfig,
    root: String,
    inactive: String,
    hidden: Vec<Entry>,
    original_child: Entry,
    later_child: Entry,
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
        let mut hidden = Vec::new();
        for path in ["dir/a", "dir/child/hidden"] {
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
            save(&mut engine, &repo, path, b"deregistered occurrence");
            hidden.push(entry(&engine, &repo, path));
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
        save(&mut engine, &repo, "dir/b", b"retained sibling");
        save(
            &mut engine,
            &repo,
            "dir/child/kept",
            b"selected original body",
        );
        save(&mut engine, &repo, "dest/anchor", b"retained destination");
        let original_child = entry(&engine, &repo, "dir/child/kept");
        let root = engine
            .create_directory_branches(
                &repo.repository_id,
                None,
                &BranchName::parse("frozen root").unwrap(),
                "admin",
                op(),
            )
            .unwrap()
            .result["directory_branch_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let inactive = engine
            .create_directory_branches(
                &repo.repository_id,
                Some(&name_policy::LookupPath::parse("dir").unwrap()),
                &BranchName::parse("unselected dir").unwrap(),
                "admin",
                op(),
            )
            .unwrap()
            .result["directory_branch_id"]
            .as_str()
            .unwrap()
            .to_owned();
        // The frozen root keeps this child's previous tree. Selecting the root
        // later must not make the child's newer named head the batch source.
        save(
            &mut engine,
            &repo,
            "dir/child/kept",
            b"unselected later body",
        );
        let later_child = entry(&engine, &repo, "dir/child/kept");
        Self {
            temp,
            engine,
            repo,
            target,
            root,
            inactive,
            hidden,
            original_child,
            later_child,
        }
    }

    fn exit_and_select(&mut self, action: MembershipExitAction) {
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
        let tree = self.engine.state.directory_branches[&self.root]
            .head_tree_id
            .clone();
        let head = self.engine.state.branches[&self.repo.main_branch_id]
            .head_commit_id
            .clone();
        self.engine
            .set_directory_default(DirectoryDefaultSelection {
                repository_id: &self.repo.repository_id,
                directory_branch_id: &self.root,
                expected_tree_id: &tree,
                expected_head: &head,
                principal: "admin",
                operation_id: op(),
            })
            .unwrap();
        assert_ne!(
            self.engine.state.directory_tree_selections[&self.repo.repository_id],
            tree
        );
        assert_eq!(
            entry(&self.engine, &self.repo, "dir/child/kept"),
            self.original_child
        );
    }
}

fn assert_history(
    engine: &Engine,
    commits: &storage_contract::TrackedRecords<Commit>,
    trees: &storage_contract::TrackedRecords<DirectoryTree>,
) {
    for (id, original) in commits.iter() {
        assert_eq!(engine.state.commits.get(id), Some(original));
    }
    for (id, original) in trees.iter() {
        assert_eq!(engine.state.directory_trees.get(id), Some(original));
    }
}

fn assert_clean_graph(engine: &Engine, directory: &str, hidden: &[Entry]) {
    let selected = &engine.state.directory_tree_selections[directory];
    let tree = &engine.state.directory_trees[selected];
    assert!(tree.entries.iter().all(|entry| {
        hidden
            .iter()
            .all(|old| entry.entry_id != old.entry_id && entry.file_id != old.file_id)
    }));
    for (id, reference) in &tree.directory_refs {
        assert_eq!(
            engine.state.directory_selections[id],
            reference.directory_branch_id
        );
        assert_eq!(
            engine.state.directory_tree_selections[id],
            reference.tree_id
        );
        assert_clean_graph(engine, id, hidden);
    }
}

#[test]
fn disk_exit_selected_projection_batch_copies_exact_selected_versions_and_reopens() {
    let mut fixture = Fixture::new();
    fixture.exit_and_select(MembershipExitAction::Deregister);
    let commits = fixture.engine.state.commits.clone();
    let trees = fixture.engine.state.directory_trees.clone();
    let target_bytes = files(&fixture.target.root);
    let request = DirectoryNamespaceBatch {
        kind: DirectoryBatchKind::Copy,
        items: vec![mapping(
            &fixture.engine,
            &fixture.repo,
            &fixture.root,
            "dir",
            Some((&fixture.root, "copied")),
        )],
    };
    let operation = op();
    let receipt = apply(&mut fixture.engine, &fixture.repo, &request, operation).unwrap();
    let copied = entry(&fixture.engine, &fixture.repo, "copied/child/kept");
    assert_eq!(
        body(&fixture.engine, &copied),
        body(&fixture.engine, &fixture.original_child)
    );
    assert_ne!(
        body(&fixture.engine, &copied),
        body(&fixture.engine, &fixture.later_child)
    );
    assert_ne!(copied.entry_id, fixture.original_child.entry_id);
    assert_ne!(copied.file_id, fixture.original_child.file_id);
    assert_ne!(copied.version_id, fixture.original_child.version_id);
    assert!(
        fixture
            .engine
            .namespace_tree(&fixture.repo.main_branch_id)
            .unwrap()
            .get("copied/a")
            .is_none()
    );
    assert!(
        fixture
            .engine
            .namespace_tree(&fixture.repo.main_branch_id)
            .unwrap()
            .get("copied/child/hidden")
            .is_none()
    );
    assert_clean_graph(
        &fixture.engine,
        &fixture.repo.repository_id,
        &fixture.hidden,
    );
    assert_history(&fixture.engine, &commits, &trees);
    drop(fixture.engine);
    let mut engine = Engine::open(&fixture.temp.path().join("system")).unwrap();
    assert_eq!(
        apply(&mut engine, &fixture.repo, &request, operation).unwrap(),
        receipt
    );
    assert_eq!(entry(&engine, &fixture.repo, "copied/child/kept"), copied);
    assert_eq!(body(&engine, &copied), b"selected original body");
    assert_history(&engine, &commits, &trees);
    assert_clean_graph(&engine, &fixture.repo.repository_id, &fixture.hidden);
    assert_eq!(files(&fixture.target.root), target_bytes);
}

#[test]
fn disk_exit_selected_projection_batch_moves_child_selection_not_its_newer_head() {
    let mut fixture = Fixture::new();
    fixture.exit_and_select(MembershipExitAction::Deregister);
    let child = selected_branch(&fixture.engine, &fixture.repo, "dir/child");
    let destination = selected_branch(&fixture.engine, &fixture.repo, "dest");
    let directory = &fixture.engine.state.directory_branches[&child];
    assert_ne!(
        fixture.engine.state.directory_tree_selections[&directory.directory_id],
        directory.head_tree_id
    );
    let context = fixture
        .engine
        .file_context(&fixture.repo.repository_id, "dir/child/kept")
        .unwrap();
    assert_eq!(
        context.inherited_from.as_deref(),
        Some(fixture.root.as_str())
    );
    let request = DirectoryNamespaceBatch {
        kind: DirectoryBatchKind::Move,
        items: vec![mapping(
            &fixture.engine,
            &fixture.repo,
            &child,
            "dir/child/kept",
            Some((&destination, "dest/moved")),
        )],
    };
    apply(&mut fixture.engine, &fixture.repo, &request, op()).unwrap();
    let moved = entry(&fixture.engine, &fixture.repo, "dest/moved");
    assert_eq!(moved.entry_id, fixture.original_child.entry_id);
    assert_eq!(moved.file_id, fixture.original_child.file_id);
    assert_eq!(moved.version_id, fixture.original_child.version_id);
    assert_ne!(moved.version_id, fixture.later_child.version_id);
    assert_eq!(
        fixture
            .engine
            .file_context(&fixture.repo.repository_id, "dest/moved")
            .unwrap()
            .inherited_from,
        context.inherited_from
    );
    assert!(
        fixture
            .engine
            .namespace_tree(&fixture.repo.main_branch_id)
            .unwrap()
            .get("dir/child/kept")
            .is_none()
    );
    assert_clean_graph(
        &fixture.engine,
        &fixture.repo.repository_id,
        &fixture.hidden,
    );
    drop(fixture.engine);
    let engine = Engine::open(&fixture.temp.path().join("system")).unwrap();
    assert_eq!(entry(&engine, &fixture.repo, "dest/moved"), moved);
    assert_eq!(body(&engine, &moved), b"selected original body");
}

#[test]
fn disk_exit_selected_projection_batch_rename_reuses_hidden_name_not_identity_and_keeps_cas() {
    let mut fixture = Fixture::new();
    fixture.exit_and_select(MembershipExitAction::Deregister);
    let retained = entry(&fixture.engine, &fixture.repo, "dir/b");
    let request = DirectoryNamespaceBatch {
        kind: DirectoryBatchKind::Rename,
        items: vec![mapping(
            &fixture.engine,
            &fixture.repo,
            &fixture.root,
            "dir/b",
            Some((&fixture.root, "dir/a")),
        )],
    };
    let operation = op();
    let receipt = apply(&mut fixture.engine, &fixture.repo, &request, operation).unwrap();
    let moved = entry(&fixture.engine, &fixture.repo, "dir/a");
    assert_eq!(moved.entry_id, retained.entry_id);
    assert_eq!(moved.file_id, retained.file_id);
    assert_ne!(moved.entry_id, fixture.hidden[0].entry_id);
    let before = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(
        apply(&mut fixture.engine, &fixture.repo, &request, op())
            .unwrap_err()
            .code(),
        ErrorCode::DirectoryBranchConflict
    );
    assert_eq!(
        apply(&mut fixture.engine, &fixture.repo, &request, operation).unwrap(),
        receipt
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    save(
        &mut fixture.engine,
        &fixture.repo,
        "dir/b",
        b"fresh same-name occurrence",
    );
    let fresh = entry(&fixture.engine, &fixture.repo, "dir/b");
    assert_ne!(fresh.entry_id, retained.entry_id);
    assert_ne!(fresh.file_id, retained.file_id);
    assert_ne!(fresh.version_id, retained.version_id);
    assert_clean_graph(
        &fixture.engine,
        &fixture.repo.repository_id,
        &fixture.hidden,
    );
}

#[test]
fn disk_exit_selected_projection_batch_delete_tree_and_recreate_never_readmits_old_directory() {
    let mut fixture = Fixture::new();
    fixture.exit_and_select(MembershipExitAction::Deregister);
    let old_directory = entry(&fixture.engine, &fixture.repo, "dir/child");
    let old_branch = selected_branch(&fixture.engine, &fixture.repo, "dir/child");
    let request = DirectoryNamespaceBatch {
        kind: DirectoryBatchKind::Delete,
        items: vec![mapping(
            &fixture.engine,
            &fixture.repo,
            &fixture.root,
            "dir/child",
            None,
        )],
    };
    let receipt = apply(&mut fixture.engine, &fixture.repo, &request, op()).unwrap();
    let commit = &fixture.engine.state.commits[receipt.result["commit_id"].as_str().unwrap()];
    let deleted = &commit.namespace_deletions.as_ref().unwrap().paths;
    assert!(deleted.iter().all(|entry| entry.path != "dir/child/hidden"));
    save(
        &mut fixture.engine,
        &fixture.repo,
        "dir/child/new",
        b"new directory identity",
    );
    assert_ne!(
        entry(&fixture.engine, &fixture.repo, "dir/child").entry_id,
        old_directory.entry_id
    );
    let request = DirectoryNamespaceBatch {
        kind: DirectoryBatchKind::Delete,
        items: vec![DirectoryNamespaceItem {
            source: DirectoryNamespaceSource {
                directory_branch_id: old_branch.clone(),
                expected_tree_id: fixture.engine.state.directory_branches[&old_branch]
                    .head_tree_id
                    .clone(),
                path: "dir/child/kept".into(),
                entry_id: fixture.original_child.entry_id.clone(),
            },
            target: None,
        }],
    };
    let before = canonical_json(fixture.engine.state()).unwrap();
    assert!(apply(&mut fixture.engine, &fixture.repo, &request, op()).is_err());
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    assert_clean_graph(
        &fixture.engine,
        &fixture.repo.repository_id,
        &fixture.hidden,
    );
    drop(fixture.engine);
    let engine = Engine::open(&fixture.temp.path().join("system")).unwrap();
    assert_ne!(
        entry(&engine, &fixture.repo, "dir/child").entry_id,
        old_directory.entry_id
    );
}

#[test]
fn disk_exit_selected_projection_batch_rejects_inactive_hidden_and_unselected_tree_inputs_atomically()
 {
    let mut fixture = Fixture::new();
    fixture.exit_and_select(MembershipExitAction::Deregister);
    let mut inactive = mapping(
        &fixture.engine,
        &fixture.repo,
        &fixture.inactive,
        "dir/b",
        None,
    );
    let before = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(
        apply(
            &mut fixture.engine,
            &fixture.repo,
            &DirectoryNamespaceBatch {
                kind: DirectoryBatchKind::Delete,
                items: vec![inactive.clone()]
            },
            op()
        )
        .unwrap_err()
        .code(),
        ErrorCode::RecoveryRequired
    );
    inactive.source.directory_branch_id = fixture.root.clone();
    inactive.source.expected_tree_id = fixture.engine.state.directory_branches[&fixture.root]
        .head_tree_id
        .clone();
    inactive.source.path = fixture.hidden[0].path().into();
    inactive.source.entry_id = fixture.hidden[0].entry_id.clone();
    assert_eq!(
        apply(
            &mut fixture.engine,
            &fixture.repo,
            &DirectoryNamespaceBatch {
                kind: DirectoryBatchKind::Delete,
                items: vec![inactive]
            },
            op()
        )
        .unwrap_err()
        .code(),
        ErrorCode::EntryNotFound
    );
    let mut wrong_baseline = mapping(&fixture.engine, &fixture.repo, &fixture.root, "dir/b", None);
    wrong_baseline.source.expected_tree_id =
        fixture.engine.state.directory_tree_selections[&fixture.repo.repository_id].clone();
    assert_eq!(
        apply(
            &mut fixture.engine,
            &fixture.repo,
            &DirectoryNamespaceBatch {
                kind: DirectoryBatchKind::Delete,
                items: vec![wrong_baseline]
            },
            op()
        )
        .unwrap_err()
        .code(),
        ErrorCode::DirectoryBranchConflict
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
}

#[test]
fn disk_exit_selected_projection_batch_rejects_selected_graph_and_occurrence_tampering() {
    let mut fixture = Fixture::new();
    fixture.exit_and_select(MembershipExitAction::Deregister);
    let state = fixture.engine.state.as_ref().clone();
    let view = fixture.engine.current_namespaces.as_ref().unwrap();
    let root_tree = state.directory_tree_selections[&fixture.repo.repository_id].clone();
    let child_id = entry(&fixture.engine, &fixture.repo, "dir/child").entry_id;
    let child_branch = state.directory_selections[&child_id].clone();
    for corruption in [
        "hidden",
        "version",
        "refs",
        "context",
        "owner_context",
        "identity",
    ] {
        let mut altered = state.clone();
        match corruption {
            "hidden" => {
                let tree = altered.directory_trees.get_mut(&root_tree).unwrap();
                tree.entries.push(fixture.hidden[0].clone());
                tree.entries
                    .sort_by(|left, right| left.lookup_key().cmp(right.lookup_key()));
            }
            "version" => {
                altered.directory_tree_selections.insert(
                    child_id.clone(),
                    state.directory_branches[&child_branch].head_tree_id.clone(),
                );
            }
            "refs" => {
                altered
                    .directory_trees
                    .get_mut(&root_tree)
                    .unwrap()
                    .directory_refs
                    .get_mut(&child_id)
                    .unwrap()
                    .tree_id = state.directory_branches[&child_branch].head_tree_id.clone();
            }
            "context" => {
                altered
                    .directory_trees
                    .get_mut(&root_tree)
                    .unwrap()
                    .file_contexts
                    .remove("dir/b");
            }
            "owner_context" => {
                altered
                    .file_contexts
                    .remove(&crate::business::context_key_from_key(
                        &fixture.repo.repository_id,
                        "dir/b",
                    ));
            }
            "identity" => {
                altered
                    .directory_trees
                    .get_mut(&root_tree)
                    .unwrap()
                    .directory_branch_id = child_branch.clone();
            }
            _ => unreachable!(),
        }
        assert!(
            view.batch_scope(&altered, &fixture.root).is_err(),
            "{corruption}"
        );
        if corruption == "version" {
            assert!(view.batch_scope(&altered, &child_branch).is_err());
        }
    }
    assert_eq!(
        canonical_json(fixture.engine.state()).unwrap(),
        canonical_json(&state).unwrap()
    );
}
