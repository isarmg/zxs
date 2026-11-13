//! Public-engine regressions for branch-scoped aliases and namespace batches.
//! All storage, physical-container and restart checks use disposable pools.
use std::{io::Read, path::PathBuf};
use storage_engine::{
    CommitRequest, Config, DirectoryBranch, DirectoryCommitRequest, DirectoryNamespaceBatch,
    DirectoryNamespaceItem, DirectoryNamespaceSource, DirectoryNamespaceTarget, Engine, Entry,
    NamespaceOperation, OperationResult, Repository, Workspace, new_id,
};
#[path = "branch_alias_operations/batch_planning_scenarios.rs"]
mod batch_planning_scenarios;
#[path = "branch_alias_operations/copy_alias_scenarios.rs"]
mod copy_alias_scenarios;
#[path = "branch_alias_operations/current_name_scenarios.rs"]
mod current_name_scenarios;
#[path = "branch_alias_operations/deletion_rebase_scenarios.rs"]
mod deletion_rebase_scenarios;
#[path = "../support/directory_git_fixture.rs"]
mod directory_fixture;
#[path = "branch_alias_operations/file_history_merge_scenarios.rs"]
mod file_history_merge_scenarios;
#[path = "branch_alias_operations/file_merge_scenarios.rs"]
mod file_merge_scenarios;
#[path = "branch_alias_operations/object_move_scenarios.rs"]
mod object_move_scenarios;

#[path = "branch_alias_operations/directory_history_scenarios.rs"]
mod directory_history_scenarios;

fn unchanged_body_path_fact(
    receipt: &OperationResult,
    repository: &str,
    entry: &Entry,
    before: &str,
    after: &str,
) -> storage_contract::FilePathEventId {
    let facts = receipt.result["path_changes"].as_array().unwrap();
    let matching = facts.iter().filter(|fact| {
        fact["before"]["path"] == before && fact["after"]["path"] == after
    }).collect::<Vec<_>>();
    assert_eq!(matching.len(), 1);
    let fact = matching[0];
    assert_eq!(fact["operation_id"], receipt.operation_id.to_string());
    assert_eq!(fact["sequence"], receipt.sequence.to_string());
    assert_eq!(fact["file_id"], entry.file_id);
    assert_eq!(fact["entry_id"], entry.entry_id);
    assert_eq!(fact["before_version_id"], entry.version_id);
    assert_eq!(fact["after_version_id"], entry.version_id);
    assert_eq!(fact["before"]["repository_id"], repository);
    assert_eq!(fact["after"]["repository_id"], repository);
    fact["event_id"].as_str().unwrap().parse().unwrap()
}

struct Pool {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    engine: Engine,
    repo: Repository,
}
impl Pool {
    fn new() -> Self {
        Self::with_disk_root(false)
    }
    fn disk_root() -> Self {
        Self::with_disk_root(true)
    }
    fn with_disk_root(file_manager_root: bool) -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("system");
        let config = Config::development([
            temporary.path().join("primary"),
            temporary.path().join("backup"),
            temporary.path().join("tertiary"),
        ])
        .unwrap();
        let mut engine = Engine::initialize(&root, config).unwrap();
        let operation = storage_contract::OperationId::generate().unwrap();
        let repo = if file_manager_root {
            engine.create_disk_root("P01", "admin", operation).unwrap()
        } else {
            engine
                .create_repository(
                    &storage_contract::RepositoryName::parse("Alias fixtures").unwrap(),
                    "P01",
                    3,
                    "admin",
                    operation,
                )
                .unwrap()
        };
        Self {
            _temporary: temporary,
            root,
            engine,
            repo,
        }
    }
    fn save(&mut self, path: &str, bytes: &[u8]) {
        let workspace = self
            .engine
            .create_workspace(&self.repo.main_branch_id, "admin")
            .unwrap();
        self.engine
            .stage_bytes(&workspace.workspace_id, path, bytes)
            .unwrap();
        self.commit(workspace);
    }
    fn commit(&mut self, workspace: Workspace) -> OperationResult {
        self.engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: storage_contract::OperationId::generate().unwrap(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "Alias integration regression".into(),
                request_digest: None,
            })
            .unwrap()
    }
    fn checkpoint(&mut self, path: &str) -> storage_contract::directory_git::DirectoryGitStatus {
        directory_fixture::checkpoint(&mut self.engine, &self.repo, path)
    }
    fn register(&mut self, path: &str, name: &str) -> String {
        self.checkpoint(path);
        self.engine
            .create_directory_branches(
                &self.repo.repository_id,
                (if (path).is_empty() {
                    None
                } else {
                    Some(name_policy::LookupPath::parse(path).unwrap())
                })
                .as_ref(),
                &storage_contract::BranchName::parse(name).unwrap(),
                "admin",
                storage_contract::OperationId::generate().unwrap(),
            )
            .unwrap()
            .result["directory_branch_id"]
            .as_str()
            .unwrap()
            .to_owned()
    }
    fn select(&mut self, path: &str, name: &str) {
        self.checkpoint(path);
        self.engine
            .select_directory_branches(
                &self.repo.repository_id,
                (if (path).is_empty() {
                    None
                } else {
                    Some(name_policy::LookupPath::parse(path).unwrap())
                })
                .as_ref(),
                &storage_contract::BranchName::parse(name).unwrap(),
                "admin",
                storage_contract::OperationId::generate().unwrap(),
            )
            .unwrap();
    }
    fn branch(&self, id: &str) -> DirectoryBranch {
        self.engine.state().directory_branches[id].clone()
    }
    fn entry(&self, branch: &str, path: &str) -> Entry {
        self.engine.state().directory_branches[branch]
            .entries
            .iter()
            .find(|entry| entry.path.as_str() == path)
            .unwrap_or_else(|| panic!("missing {path} in branch {branch}"))
            .clone()
    }
    fn source(&self, branch: &str, path: &str) -> DirectoryNamespaceSource {
        let entry = self.entry(branch, path);
        DirectoryNamespaceSource {
            directory_branch_id: branch.into(),
            expected_tree_id: self.branch(branch).head_tree_id,
            path: path.into(),
            entry_id: entry.entry_id,
        }
    }
    fn target(&self, branch: &str, path: &str) -> DirectoryNamespaceTarget {
        DirectoryNamespaceTarget {
            directory_branch_id: branch.into(),
            expected_tree_id: self.branch(branch).head_tree_id,
            path: path.into(),
        }
    }
    fn mapping(
        &self,
        source: &str,
        path: &str,
        target: &str,
        destination: &str,
    ) -> DirectoryNamespaceItem {
        DirectoryNamespaceItem {
            source: self.source(source, path),
            target: Some(self.target(target, destination)),
        }
    }
    fn batch(
        &mut self,
        kind: storage_contract::DirectoryBatchKind,
        items: Vec<DirectoryNamespaceItem>,
    ) -> OperationResult {
        self.engine
            .directory_namespace_batch(
                &self.repo.repository_id,
                &DirectoryNamespaceBatch { kind, items },
                "admin",
                storage_contract::OperationId::generate().unwrap(),
            )
            .unwrap()
    }
    fn bytes(&self, version: &str) -> Vec<u8> {
        let mut content = self.engine.open_content(version).unwrap();
        let mut bytes = Vec::new();
        content.file.read_to_end(&mut bytes).unwrap();
        bytes
    }
    fn payload(&self, file: &str, version: &str) -> PathBuf {
        let state = self.engine.state();
        let version = &state.versions[version];
        let primary = self
            .engine
            .config()
            .disks
            .iter()
            .find(|disk| disk.slot_id == "P01")
            .unwrap();
        primary
            .root
            .join("histories")
            .join(&self.repo.history_tree_id)
            .join("file-containers")
            .join(file)
            .join("namespace")
            .join(&state.file_locations[file])
            .join("versions")
            .join(format!(
                "{}-{}",
                version.storage_id,
                std::path::Path::new(&state.file_locations[file])
                    .file_name().unwrap().to_str().unwrap(),
            ))
    }
    fn current_entry(&self, path: &str) -> Entry {
        self.engine
            .list_tree(&self.repo.main_branch_id)
            .unwrap()
            .into_iter()
            .find(|entry| entry.path.as_str() == path)
            .unwrap()
    }
    fn reopen(self, inspect: impl FnOnce(&Engine)) {
        let Self {
            _temporary,
            root,
            engine,
            repo: _,
        } = self;
        drop(engine);
        let recovered = Engine::open(&root).unwrap();
        inspect(&recovered);
        drop(recovered);
        drop(_temporary);
    }
}

#[test]
fn folder_copy_shares_one_new_folder_and_file_identity_across_target_branches() {
    let mut p = Pool::new();
    p.save("a/folder/nested/report.txt", b"source content");
    p.save("b/stay.txt", b"sibling");
    let m = p.register("a", "m");
    let l = p.register("b", "l");
    let target_p = p.register("b", "p");
    let q = p.register("b", "q");
    let source_folder = p.entry(&m, "a/folder");
    let source_nested = p.entry(&m, "a/folder/nested");
    let source = p.entry(&m, "a/folder/nested/report.txt");
    let source_head = p.branch(&m).head_tree_id;
    let untouched = p.branch(&q).head_tree_id;
    let result = p.batch(
        storage_contract::DirectoryBatchKind::Copy,
        vec![
            p.mapping(&m, "a/folder", &l, "b/copied"),
            p.mapping(&m, "a/folder", &target_p, "b/copied"),
        ],
    );
    let folder_l = p.entry(&l, "b/copied");
    let folder_p = p.entry(&target_p, "b/copied");
    assert_eq!(folder_l.entry_id, folder_p.entry_id);
    assert_ne!(folder_l.entry_id, source_folder.entry_id);
    let nested_l = p.entry(&l, "b/copied/nested");
    let nested_p = p.entry(&target_p, "b/copied/nested");
    assert_eq!(nested_l.entry_id, nested_p.entry_id);
    assert_ne!(nested_l.entry_id, source_nested.entry_id);
    let copy_l = p.entry(&l, "b/copied/nested/report.txt");
    let copy_p = p.entry(&target_p, "b/copied/nested/report.txt");
    assert_eq!(copy_l.file_id, copy_p.file_id);
    assert_eq!(copy_l.entry_id, copy_p.entry_id);
    assert_eq!(copy_l.version_id, copy_p.version_id);
    assert_ne!(copy_l.file_id, source.file_id);
    assert_ne!(copy_l.version_id, source.version_id);
    assert!(
        p.engine.state().versions[&copy_l.version_id]
            .source_version_ids
            .is_empty(),
        "different file identities must not fake same-file ancestry"
    );
    assert_eq!(result.result["copies"].as_array().unwrap().len(), 1);
    assert_eq!(result.result["copies"][0]["source_file_id"], source.file_id);
    assert_eq!(
        result.result["copies"][0]["source_version_id"],
        source.version_id
    );
    assert_eq!(
        p.engine.state().file_locations[&copy_l.file_id],
        "b/copied/nested/report.txt"
    );
    assert_eq!(p.bytes(&copy_l.version_id), b"source content");
    assert!(p.payload(&copy_l.file_id, &copy_l.version_id).is_file());
    assert_eq!(p.engine.file_history(&copy_l.file_id).unwrap().len(), 1);
    assert_eq!(
        p.branch(&m).head_tree_id,
        source_head,
        "copy does not create a source subtree"
    );
    assert_eq!(p.branch(&q).head_tree_id, untouched);
    assert!(
        p.branch(&q)
            .entries
            .iter()
            .all(|entry| !entry.path.as_str().starts_with("b/copied"))
    );
    p.reopen(|recovered| {
        assert_eq!(
            recovered.state().file_locations[&copy_l.file_id],
            "b/copied/nested/report.txt"
        )
    });
}

#[test]
fn batch_alias_rename_keeps_one_canonical_owner_and_no_op_has_no_subtree() {
    let mut p = Pool::new();
    p.save("a/report.txt", b"original");
    p.save("b/stay.txt", b"sibling");
    let m = p.register("", "m");
    let n = p.register("", "n");
    let retained = p.register("", "retained");
    let l = p.register("", "l");
    let target_p = p.register("", "p");
    let original = p.entry(&m, "a/report.txt");
    p.batch(
        storage_contract::DirectoryBatchKind::Move,
        vec![p.mapping(&m, "a/report.txt", &l, "b/report.txt")],
    );
    let old_n = p.branch(&n);
    let old_p = p.branch(&target_p);
    p.batch(
        storage_contract::DirectoryBatchKind::Rename,
        vec![
            p.mapping(&retained, "a/report.txt", &retained, "a/new.txt"),
            p.mapping(&l, "b/report.txt", &l, "b/new.txt"),
        ],
    );
    assert_eq!(p.entry(&retained, "a/new.txt").file_id, original.file_id);
    assert_eq!(p.entry(&l, "b/new.txt").version_id, original.version_id);
    assert_eq!(
        p.engine.state().file_locations[&original.file_id],
        "b/new.txt"
    );
    assert_eq!(p.engine.state().file_names[&original.file_id], "new.txt");
    assert_eq!(p.branch(&n).head_tree_id, old_n.head_tree_id);
    assert_eq!(p.entry(&n, "a/report.txt").version_id, original.version_id);
    assert_eq!(p.branch(&target_p).head_tree_id, old_p.head_tree_id);
    assert!(p.payload(&original.file_id, &original.version_id).is_file());
    assert_eq!(p.bytes(&original.version_id), b"original");
    let trees = p.engine.state().directory_trees.len();
    let head = p.branch(&retained).head_tree_id;
    let no_op = p.batch(
        storage_contract::DirectoryBatchKind::Rename,
        vec![p.mapping(&retained, "a/new.txt", &retained, "a/new.txt")],
    );
    assert_eq!(no_op.result["path_changes"].as_array().unwrap().len(), 0);
    assert_eq!(no_op.result["transfers"].as_array().unwrap().len(), 1);
    assert_eq!(no_op.result["count"], "1");
    assert_eq!(no_op.result["action"], "rename");
    assert_eq!(p.engine.state().directory_trees.len(), trees);
    assert_eq!(p.branch(&retained).head_tree_id, head);
    assert_eq!(
        p.engine.state().file_locations[&original.file_id],
        "b/new.txt"
    );
    p.reopen(|recovered| {
        assert_eq!(
            recovered.state().file_locations[&original.file_id],
            "b/new.txt"
        )
    });
}

// Stable aliases share one initialized root owner. Keep a separate frozen
// branch so tests distinguish retained history from the selected working view.
fn selected_aliases(p: &mut Pool) -> (String, String, Entry) {
    p.save("a/report.txt", b"original");
    p.save("b/stay.txt", b"sibling");
    let source = p.register("", "source");
    let frozen = p.register("", "retained");
    let selected = p.register("", "aliases");
    let original = p.entry(&source, "a/report.txt");
    p.batch(
        storage_contract::DirectoryBatchKind::Move,
        vec![p.mapping(&source, "a/report.txt", &selected, "b/report.txt")],
    );
    p.select("", "aliases");
    assert_ne!(frozen, selected);
    assert_eq!(
        p.branch(&frozen).directory_id,
        p.branch(&selected).directory_id
    );
    assert_eq!(p.current_entry("a/report.txt").file_id, original.file_id);
    assert_eq!(p.current_entry("b/report.txt").file_id, original.file_id);
    assert_eq!(
        p.entry(&frozen, "a/report.txt").version_id,
        original.version_id
    );
    select_alias_draft(p, &original);
    (frozen, selected, original)
}

fn selected_root_aliases(p: &mut Pool) -> Entry {
    selected_aliases(p).2
}

fn select_alias_draft(p: &mut Pool, original: &Entry) {
    let draft = p
        .engine
        .create_file_branch(
            &original.file_id,
            &original.version_id,
            &storage_contract::BranchName::parse("explicit draft").unwrap(),
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap()
        .result["branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    p.engine
        .select_file_branch_at(
            &p.repo.repository_id,
            &name_policy::PortablePath::parse("b/report.txt").unwrap(),
            &original.file_id,
            &draft,
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
}

#[test]
fn archive_respects_each_selected_alias_and_preserves_closed_branch_head() {
    let mut p = Pool::new();
    let (_, _, original) = selected_aliases(&mut p);
    p.save("a/report.txt", b"owned a content");
    let a = p
        .engine
        .file_context(&p.repo.repository_id, "a/report.txt")
        .unwrap();
    let b = p
        .engine
        .file_context(&p.repo.repository_id, "b/report.txt")
        .unwrap();
    assert_ne!(a.branch_id, b.branch_id);
    assert_eq!(
        p.engine.state().file_selections[&original.file_id],
        a.branch_id
    );
    let before = p.engine.state().branches[&b.branch_id].clone();
    let operation = storage_contract::OperationId::generate().unwrap();
    assert_eq!(
        p.engine
            .archive_branch(&b.branch_id, "admin", operation)
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::FileBranchSelected
    );
    assert!(!p.engine.state().operations.contains_key(&operation));
    assert_eq!(
        p.engine.state().branches[&b.branch_id].state.as_str(),
        "active"
    );
    assert_eq!(
        p.engine.state().branches[&b.branch_id].head_commit_id,
        before.head_commit_id
    );

    p.engine
        .select_file_branch_at(
            &p.repo.repository_id,
            &name_policy::PortablePath::parse("b/report.txt").unwrap(),
            &original.file_id,
            &a.branch_id,
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    p.engine
        .archive_branch(
            &b.branch_id,
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    p.save("b/report.txt", b"new active branch content");
    assert_eq!(
        p.engine.state().branches[&b.branch_id].state.as_str(),
        "archived"
    );
    assert_eq!(
        p.engine.state().branches[&b.branch_id].head_commit_id,
        before.head_commit_id
    );
    p.reopen(|engine| {
        assert_eq!(
            engine.state().branches[&b.branch_id].state.as_str(),
            "archived"
        );
        assert_eq!(
            engine.state().branches[&b.branch_id].head_commit_id,
            before.head_commit_id
        );
    });
}

#[test]
fn workspace_namespace_renames_two_aliases_atomically_with_distinct_write_contexts() {
    let mut p = Pool::new();
    let (n, l, original) = selected_aliases(&mut p);
    let a_context = p
        .engine
        .file_context(&p.repo.repository_id, "a/report.txt")
        .unwrap();
    let b_context = p
        .engine
        .file_context(&p.repo.repository_id, "b/report.txt")
        .unwrap();
    assert_ne!(a_context.branch_id, b_context.branch_id);
    assert!(a_context.inherited_from.is_some());
    assert!(b_context.inherited_from.is_none());
    let a_head = p.branch(&n).head_tree_id;
    let b_head = p.branch(&l).head_tree_id;
    let workspace = p
        .engine
        .create_workspace(&p.repo.main_branch_id, "admin")
        .unwrap();
    for (path, destination) in [
        ("a/report.txt", "a/renamed.txt"),
    ] {
        p.engine
            .namespace_operation(
                &workspace.workspace_id,
                NamespaceOperation::parse(
                    storage_contract::NamespaceOperationKind::Rename,
                    path,
                    Some(destination),
                )
                .unwrap(),
            )
            .unwrap();
    }
    let content_branches = p.engine.state().branches.values()
        .filter(|branch| branch.file_id.is_some()).cloned().collect::<Vec<_>>();
    let receipt = p.commit(workspace);
    assert_eq!(p.engine.state().branches.values()
        .filter(|branch| branch.file_id.is_some()).cloned().collect::<Vec<_>>(), content_branches);
    let renamed_a = p
        .engine
        .file_context(&p.repo.repository_id, "a/renamed.txt")
        .unwrap();
    let renamed_b = p
        .engine
        .file_context(&p.repo.repository_id, "b/renamed.txt")
        .unwrap();
    for (renamed, before, path, old_path) in [
        (&renamed_a, &a_context, "a/renamed.txt", "a/report.txt"),
        (&renamed_b, &b_context, "b/renamed.txt", "b/report.txt"),
    ] {
        assert_eq!(renamed.branch_id, before.branch_id);
        assert_eq!(renamed.inherited_from, before.inherited_from);
        assert_eq!(renamed.selection_generation, before.selection_generation);
        assert_eq!(renamed.node_id, before.node_id);
        unchanged_body_path_fact(&receipt, &p.repo.repository_id, &original, old_path, path);
    }
    let facts = receipt.result["path_changes"].as_array().unwrap();
    assert_eq!(facts.len(), 2);
    assert_ne!(facts[0]["event_id"], facts[1]["event_id"]);
    assert!(p.engine.state().file_path_nodes.is_empty());
    assert_eq!(p.current_entry("a/renamed.txt").file_id, original.file_id);
    assert_eq!(p.current_entry("b/renamed.txt").file_id, original.file_id);
    assert_eq!(p.branch(&n).head_tree_id, a_head);
    assert_eq!(p.entry(&n, "a/report.txt").version_id, original.version_id);
    assert_ne!(p.branch(&l).head_tree_id, b_head);
    assert_eq!(
        p.engine.state().file_locations[&original.file_id],
        "b/renamed.txt"
    );
    assert_eq!(
        p.engine.file_history(&original.file_id).unwrap().len(),
        1,
        "namespace edits create no file content versions"
    );
    let a_key = format!("{}/{}", p.repo.repository_id, "a/renamed.txt");
    let b_key = format!("{}/{}", p.repo.repository_id, "b/renamed.txt");
    p.reopen(|recovered| {
        assert_eq!(recovered.state().file_contexts[&a_key], renamed_a);
        assert_eq!(recovered.state().file_contexts[&b_key], renamed_b);
    });
}

#[test]
fn deleting_an_inactive_root_alias_is_rejected_without_changing_any_context() {
    let mut p = Pool::disk_root();
    let original = selected_root_aliases(&mut p);
    let root = p.register("", "archive");
    let before = p.engine.state().clone();
    let request = DirectoryNamespaceBatch {
        kind: storage_contract::DirectoryBatchKind::Delete,
        items: vec![DirectoryNamespaceItem {
            source: p.source(&root, "a/report.txt"),
            target: None,
        }],
    };
    let operation = storage_contract::OperationId::generate().unwrap();
    assert_eq!(
        p.engine
            .directory_namespace_batch(&p.repo.repository_id, &request, "admin", operation)
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::DirectoryBranchConflict
    );
    assert_eq!(p.engine.state(), &before);
    assert_eq!(p.entry(&root, "a/report.txt").file_id, original.file_id);
    assert_eq!(p.entry(&root, "b/report.txt").file_id, original.file_id);
    assert_eq!(p.current_entry("a/report.txt").file_id, original.file_id);
    assert_eq!(p.current_entry("b/report.txt").file_id, original.file_id);
    assert_eq!(p.bytes(&original.version_id), b"original");
    p.reopen(|recovered| assert_eq!(recovered.state(), &before));
}

#[test]
fn public_file_branch_rename_is_bounded_authorized_metadata_without_content_nodes() {
    let mut p = Pool::new();
    let (_, _, original) = selected_aliases(&mut p);
    // Actual writing of the inherited a view creates the public P2 branch.
    p.save("a/report.txt", b"own branch content");
    let context = p
        .engine
        .file_context(&p.repo.repository_id, "a/report.txt")
        .unwrap();
    let branch = p.engine.state().branches[&context.branch_id].clone();
    let versions = p.engine.state().versions.len();
    let commits = p.engine.state().commits.len();
    let trees = p.engine.state().directory_trees.len();
    let heads = p
        .engine
        .state()
        .directory_branches
        .iter()
        .map(|(id, branch)| (id.clone(), branch.head_tree_id.clone()))
        .collect::<std::collections::BTreeMap<_, _>>();
    let operation = storage_contract::OperationId::generate().unwrap();
    let renamed = p
        .engine
        .rename_file_branch(
            &original.file_id,
            &branch.branch_id,
            &storage_contract::BranchName::parse("客户主稿").unwrap(),
            "admin",
            operation,
        )
        .unwrap();
    assert_eq!(renamed.result["previous_name"], branch.name);
    assert_eq!(renamed.result["name"], "客户主稿");
    assert_eq!(
        p.engine
            .rename_file_branch(
                &original.file_id,
                &branch.branch_id,
                &storage_contract::BranchName::parse("客户主稿").unwrap(),
                "admin",
                operation
            )
            .unwrap()
            .result,
        renamed.result
    );
    assert_eq!(p.engine.state().versions.len(), versions);
    assert_eq!(p.engine.state().commits.len(), commits);
    assert_eq!(p.engine.state().directory_trees.len(), trees);
    let current = &p.engine.state().branches[&branch.branch_id];
    assert_eq!(current.head_commit_id, branch.head_commit_id);
    assert_eq!(current.generation, branch.generation);
    assert_eq!(
        p.engine
            .file_context(&p.repo.repository_id, "a/report.txt")
            .unwrap(),
        context
    );
    for (id, head) in &heads {
        assert_eq!(&p.engine.state().directory_branches[id].head_tree_id, head);
    }
    let collision = p
        .engine
        .rename_file_branch(
            &original.file_id,
            &branch.branch_id,
            &storage_contract::BranchName::parse("EXPLICIT DRAFT").unwrap(),
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap_err();
    assert_eq!(collision.code(), storage_contract::ErrorCode::NameCollision);
    let denied = p
        .engine
        .rename_file_branch(
            &original.file_id,
            &branch.branch_id,
            &storage_contract::BranchName::parse("Missing actor").unwrap(),
            "",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap_err();
    assert_eq!(denied.code(), storage_contract::ErrorCode::Unauthenticated);
    let foreign = p
        .engine
        .rename_file_branch(
            &new_id("file").unwrap(),
            &branch.branch_id,
            &storage_contract::BranchName::parse("Wrong file").unwrap(),
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap_err();
    assert_eq!(
        foreign.code(),
        storage_contract::ErrorCode::FileBranchNotFound
    );
    let boundary = format!("{}ab", "文".repeat(42));
    assert_eq!(boundary.len(), 128);
    p.engine
        .rename_file_branch(
            &original.file_id,
            &branch.branch_id,
            &storage_contract::BranchName::parse(&boundary).unwrap(),
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    assert_eq!(p.engine.state().directory_trees.len(), trees);
    p.reopen(|recovered| assert_eq!(recovered.state().branches[&branch.branch_id].name, boundary));
}

#[test]
fn workspace_stale_alias_source_rejects_the_complete_namespace_decision() {
    let mut p = Pool::new();
    let (n, l, original) = selected_aliases(&mut p);
    let before = p.engine.list_tree(&p.repo.main_branch_id).unwrap();
    let a_context = p
        .engine
        .file_context(&p.repo.repository_id, "a/report.txt")
        .unwrap();
    let b_context = p
        .engine
        .file_context(&p.repo.repository_id, "b/report.txt")
        .unwrap();
    let a_head = p.branch(&n).head_tree_id;
    let b_head = p.branch(&l).head_tree_id;
    let versions = p.engine.state().versions.len();
    let workspace = p
        .engine
        .create_workspace(&p.repo.main_branch_id, "admin")
        .unwrap();
    for (path, destination) in [
        ("a/report.txt", "a/left.txt"),
        ("b/report.txt", "b/right.txt"),
    ] {
        p.engine
            .namespace_operation(
                &workspace.workspace_id,
                NamespaceOperation::parse(
                    storage_contract::NamespaceOperationKind::Rename,
                    path,
                    Some(destination),
                )
                .unwrap(),
            )
            .unwrap();
    }
    let error = p
        .engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id.clone(),
            operation_id: storage_contract::OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "Stale source after whole-container rename".into(),
            request_digest: None,
        })
        .unwrap_err();
    assert_eq!(
        error.code(),
        storage_contract::ErrorCode::EntryNotFound
    );
    assert_eq!(
        json_codec::to_value(p.engine.list_tree(&p.repo.main_branch_id).unwrap()).unwrap(),
        json_codec::to_value(before).unwrap()
    );
    assert_eq!(
        p.engine
            .file_context(&p.repo.repository_id, "a/report.txt")
            .unwrap(),
        a_context
    );
    assert_eq!(
        p.engine
            .file_context(&p.repo.repository_id, "b/report.txt")
            .unwrap(),
        b_context
    );
    assert_eq!(p.branch(&n).head_tree_id, a_head);
    assert_eq!(p.branch(&l).head_tree_id, b_head);
    assert_eq!(p.engine.state().versions.len(), versions);
    assert_eq!(
        p.engine.state().file_locations[&original.file_id],
        "b/report.txt"
    );
    assert!(p.payload(&original.file_id, &original.version_id).is_file());
    p.engine
        .abort_workspace(&workspace.workspace_id, "admin")
        .unwrap();
}

#[test]
fn chained_workspace_directory_renames_keep_context_and_physical_owner() {
    let mut p = Pool::new();
    p.save("a/child/report.txt", b"original");
    let draft = p.register("a", "draft");
    p.select("a", "draft");
    let original = p.current_entry("a/child/report.txt");
    let context = p
        .engine
        .file_context(&p.repo.repository_id, original.path.as_str())
        .unwrap();
    let workspace = p
        .engine
        .create_workspace(&p.repo.main_branch_id, "admin")
        .unwrap();
    for (path, destination) in [("a", "temporary"), ("temporary", "final")] {
        p.engine
            .namespace_operation(
                &workspace.workspace_id,
                NamespaceOperation::parse(
                    storage_contract::NamespaceOperationKind::Rename,
                    path,
                    Some(destination),
                )
                .unwrap(),
            )
            .unwrap();
    }
    let content_branches = p.engine.state().branches.values()
        .filter(|branch| branch.file_id.is_some()).cloned().collect::<Vec<_>>();
    let receipt = p.commit(workspace);
    assert_eq!(p.engine.state().branches.values()
        .filter(|branch| branch.file_id.is_some()).cloned().collect::<Vec<_>>(), content_branches);
    let renamed = p.current_entry("final/child/report.txt");
    assert_eq!(renamed.entry_id, original.entry_id);
    let updated = p
        .engine
        .file_context(&p.repo.repository_id, renamed.path.as_str())
        .unwrap();
    assert_eq!(updated.branch_id, context.branch_id);
    assert_eq!(updated.inherited_from, context.inherited_from);
    assert_eq!(
        p.engine.state().file_locations[&original.file_id],
        renamed.path()
    );
    assert_eq!(p.branch(&draft).path, "final");
    assert_eq!(updated.node_id, context.node_id);
    unchanged_body_path_fact(
        &receipt, &p.repo.repository_id, &original, "a/child/report.txt", "final/child/report.txt",
    );
    assert_eq!(receipt.result["path_changes"].as_array().unwrap().len(), 1);
    assert!(p.engine.state().file_path_nodes.is_empty());
    assert_eq!(p.bytes(&original.version_id), b"original");
    let repo = p.repo.repository_id.clone();
    p.reopen(|engine| {
        assert_eq!(engine.file_context(&repo, renamed.path.as_str()).unwrap(), updated);
        assert_eq!(
            engine.state().file_locations[&original.file_id],
            renamed.path()
        );
    });
}

#[test]
fn workspace_path_swap_preserves_each_files_exact_branch_context() {
    let mut p = Pool::new();
    p.save("a.txt", b"a");
    p.save("b.txt", b"b");
    p.register("", "draft");
    p.select("", "draft");
    let a = p.current_entry("a.txt");
    let b = p.current_entry("b.txt");
    let ca = p
        .engine
        .file_context(&p.repo.repository_id, "a.txt")
        .unwrap();
    let cb = p
        .engine
        .file_context(&p.repo.repository_id, "b.txt")
        .unwrap();
    let workspace = p
        .engine
        .create_workspace(&p.repo.main_branch_id, "admin")
        .unwrap();
    for (path, destination) in [
        ("a.txt", "temporary.txt"),
        ("b.txt", "a.txt"),
        ("temporary.txt", "b.txt"),
    ] {
        p.engine
            .namespace_operation(
                &workspace.workspace_id,
                NamespaceOperation::parse(
                    storage_contract::NamespaceOperationKind::Rename,
                    path,
                    Some(destination),
                )
                .unwrap(),
            )
            .unwrap();
    }
    let content_branches = p.engine.state().branches.values()
        .filter(|branch| branch.file_id.is_some()).cloned().collect::<Vec<_>>();
    let receipt = p.commit(workspace);
    assert_eq!(p.engine.state().branches.values()
        .filter(|branch| branch.file_id.is_some()).cloned().collect::<Vec<_>>(), content_branches);
    for (original, context, path) in [(a, ca, "b.txt"), (b, cb, "a.txt")] {
        let current = p.current_entry(path);
        assert_eq!(current.file_id, original.file_id);
        let updated = p.engine.file_context(&p.repo.repository_id, path).unwrap();
        assert_eq!(updated.branch_id, context.branch_id);
        assert_eq!(updated.inherited_from, context.inherited_from);
        assert_eq!(p.engine.state().file_locations[&original.file_id], path);
        assert_eq!(updated.node_id, context.node_id);
        unchanged_body_path_fact(
            &receipt, &p.repo.repository_id, &original, original.path(), path,
        );
    }
    assert_eq!(receipt.result["path_changes"].as_array().unwrap().len(), 2);
    assert!(p.engine.state().file_path_nodes.is_empty());
    p.reopen(|_| {});
}

#[test]
fn workspace_delete_then_replace_in_initialized_repository_is_atomic_rejection() {
    let mut p = Pool::new();
    p.save("a.txt", b"retired");
    p.save("b.txt", b"surviving");
    p.register("", "draft");
    p.select("", "draft");
    let retired = p.current_entry("a.txt");
    let survivor = p.current_entry("b.txt");
    let context = p
        .engine
        .file_context(&p.repo.repository_id, "b.txt")
        .unwrap();
    let workspace = p
        .engine
        .create_workspace(&p.repo.main_branch_id, "admin")
        .unwrap();
    for change in [
        NamespaceOperation::parse(
            storage_contract::NamespaceOperationKind::Delete,
            "a.txt",
            None,
        )
        .unwrap(),
        NamespaceOperation::parse(
            storage_contract::NamespaceOperationKind::Rename,
            "b.txt",
            Some("a.txt"),
        )
        .unwrap(),
    ] {
        p.engine
            .namespace_operation(&workspace.workspace_id, change)
            .unwrap();
    }
    let before = p.engine.state().clone();
    let operation = storage_contract::OperationId::generate().unwrap();
    assert_eq!(
        p.engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id.clone(),
                operation_id: operation,
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "Reject raw deletion inside initialized owner".into(),
                request_digest: None,
            })
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::UnsupportedOperation
    );
    assert_eq!(p.engine.state(), &before);
    p.engine
        .abort_workspace(&workspace.workspace_id, "admin")
        .unwrap();
    assert_eq!(p.current_entry("a.txt").file_id, retired.file_id);
    assert_eq!(p.current_entry("b.txt").file_id, survivor.file_id);
    let updated = p
        .engine
        .file_context(&p.repo.repository_id, "b.txt")
        .unwrap();
    assert_eq!(updated, context);
    assert_eq!(p.bytes(&retired.version_id), b"retired");
    assert_eq!(p.bytes(&survivor.version_id), b"surviving");
    p.reopen(|engine| assert_eq!(engine.state(), &before));
}

#[test]
fn chained_alias_renames_preserve_distinct_contexts_and_one_physical_owner() {
    let mut p = Pool::new();
    let (_, _, original) = selected_aliases(&mut p);
    let contexts = ["a/report.txt", "b/report.txt"]
        .map(|path| p.engine.file_context(&p.repo.repository_id, path).unwrap());
    let workspace = p
        .engine
        .create_workspace(&p.repo.main_branch_id, "admin")
        .unwrap();
    for (path, destination) in [
        ("a/report.txt", "a/temporary.txt"),
        ("b/temporary.txt", "b/final.txt"),
    ] {
        p.engine
            .namespace_operation(
                &workspace.workspace_id,
                NamespaceOperation::parse(
                    storage_contract::NamespaceOperationKind::Rename,
                    path,
                    Some(destination),
                )
                .unwrap(),
            )
            .unwrap();
    }
    let content_branches = p.engine.state().branches.values()
        .filter(|branch| branch.file_id.is_some()).cloned().collect::<Vec<_>>();
    let receipt = p.commit(workspace);
    assert_eq!(p.engine.state().branches.values()
        .filter(|branch| branch.file_id.is_some()).cloned().collect::<Vec<_>>(), content_branches);
    for (path, before) in ["a/final.txt", "b/final.txt"].into_iter().zip(contexts) {
        let context = p.engine.file_context(&p.repo.repository_id, path).unwrap();
        assert_eq!(context.branch_id, before.branch_id);
        assert_eq!(context.inherited_from, before.inherited_from);
        assert_eq!(p.current_entry(path).file_id, original.file_id);
        assert_eq!(context.node_id, before.node_id);
        let old_path = if path.starts_with("a/") { "a/report.txt" } else { "b/report.txt" };
        unchanged_body_path_fact(&receipt, &p.repo.repository_id, &original, old_path, path);
    }
    assert_eq!(
        p.engine.state().file_locations[&original.file_id],
        "b/final.txt"
    );
    let facts = receipt.result["path_changes"].as_array().unwrap();
    assert_eq!(facts.len(), 2);
    assert_ne!(facts[0]["event_id"], facts[1]["event_id"]);
    assert!(p.engine.state().file_path_nodes.is_empty());
    assert!(p.payload(&original.file_id, &original.version_id).is_file());
    assert_eq!(p.bytes(&original.version_id), b"original");
    p.reopen(|engine| {
        assert_eq!(
            engine.state().file_locations[&original.file_id],
            "b/final.txt"
        )
    });
}

#[test]
fn renaming_both_alias_directories_moves_only_the_actual_physical_owner() {
    for (source, owner) in [("a", "b"), ("b", "a")] {
        let mut p = Pool::new();
        p.save(&format!("{source}/report.txt"), b"original");
        p.save(&format!("{owner}/stay.txt"), b"sibling");
        let moved = p.register("", "moved");
        p.register("", "retained");
        let target = p.register("", "target");
        let original = p.current_entry(&format!("{source}/report.txt"));
        p.batch(
            storage_contract::DirectoryBatchKind::Move,
            vec![p.mapping(
                &moved,
                original.path.as_str(),
                &target,
                &format!("{owner}/report.txt"),
            )],
        );
        p.select("", "target");
        let workspace = p
            .engine
            .create_workspace(&p.repo.main_branch_id, "admin")
            .unwrap();
        for (path, destination) in [("a", "aa"), ("b", "bb")] {
            p.engine
                .namespace_operation(
                    &workspace.workspace_id,
                    NamespaceOperation::parse(
                        storage_contract::NamespaceOperationKind::Rename,
                        path,
                        Some(destination),
                    )
                    .unwrap(),
                )
                .unwrap();
        }
        p.commit(workspace);
        assert_eq!(p.current_entry("aa/report.txt").file_id, original.file_id);
        assert_eq!(p.current_entry("bb/report.txt").file_id, original.file_id);
        assert_eq!(
            p.engine.state().file_locations[&original.file_id],
            format!("{owner}{owner}/report.txt")
        );
        assert_eq!(p.bytes(&original.version_id), b"original");
        p.reopen(|engine| {
            assert_eq!(
                engine.state().file_locations[&original.file_id],
                format!("{owner}{owner}/report.txt")
            )
        });
    }
}

#[test]
fn exact_no_op_rename_uses_latest_alias_name_and_history_selection_keeps_it() {
    let mut p = Pool::new();
    let (frozen, _, original) = selected_aliases(&mut p);
    let frozen_head = p.branch(&frozen).head_tree_id;
    p.engine
        .file_namespace(
            &p.repo.repository_id,
            NamespaceOperation::parse(
                storage_contract::NamespaceOperationKind::Rename,
                "a/report.txt",
                Some("a/new.txt"),
            )
            .unwrap(),
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    assert_eq!(
        p.engine.state().file_locations[&original.file_id],
        "b/new.txt"
    );
    assert_eq!(p.current_entry("a/new.txt").file_id, original.file_id);
    assert_eq!(p.current_entry("b/new.txt").file_id, original.file_id);
    let current = p.engine.list_tree(&p.repo.main_branch_id).unwrap();
    assert!(current.iter().filter(|entry| entry.file_id == original.file_id)
        .all(|entry| entry.name() == "new.txt"));
    assert_eq!(p.branch(&frozen).head_tree_id, frozen_head);
    assert_eq!(p.entry(&frozen, "a/report.txt").version_id, original.version_id);
    let before = p
        .engine
        .file_context(&p.repo.repository_id, "b/new.txt")
        .unwrap();
    let workspace = p
        .engine
        .create_workspace(&p.repo.main_branch_id, "admin")
        .unwrap();
    p.engine
        .namespace_operation(
            &workspace.workspace_id,
            NamespaceOperation::parse(
                storage_contract::NamespaceOperationKind::Rename,
                "b/new.txt",
                Some("b/new.txt"),
            )
            .unwrap(),
        )
        .unwrap();
    let operation = storage_contract::OperationId::generate().unwrap();
    let request = CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: operation,
        principal: "admin".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "Latest-name no-op".into(),
        request_digest: None,
    };
    let before_unauthorized = storage_engine::canonical_json(p.engine.state()).unwrap();
    let mut unauthorized = request.clone();
    unauthorized.principal = "other-actor".into();
    unauthorized.operation_id = storage_contract::OperationId::generate().unwrap();
    assert_eq!(
        p.engine.commit_workspace(unauthorized).unwrap_err().code(),
        storage_contract::ErrorCode::PermissionDenied,
    );
    assert_eq!(
        storage_engine::canonical_json(p.engine.state()).unwrap(),
        before_unauthorized,
    );
    let receipt = p.engine.commit_workspace(request.clone()).unwrap();
    let published = storage_engine::canonical_json(p.engine.state()).unwrap();
    assert_eq!(p.engine.commit_workspace(request).unwrap(), receipt);
    assert_eq!(storage_engine::canonical_json(p.engine.state()).unwrap(), published);
    assert_eq!(
        p.engine.state().file_locations[&original.file_id],
        "b/new.txt"
    );
    assert_eq!(
        p.engine
            .file_context(&p.repo.repository_id, "b/new.txt")
            .unwrap(),
        before
    );
    assert_eq!(p.bytes(&original.version_id), b"original");
    // The retained branch still records the old path for audit. Selecting it
    // projects its content under the one current container name.
    p.select("", "retained");
    assert_eq!(p.current_entry("a/new.txt").version_id, original.version_id);
    assert!(p.engine.list_tree(&p.repo.main_branch_id).unwrap().iter()
        .filter(|entry| entry.file_id == original.file_id)
        .all(|entry| entry.name() == "new.txt"));
    let branch = p.repo.main_branch_id.clone();
    p.reopen(|engine| {
        assert_eq!(engine.state().file_names[&original.file_id], "new.txt");
        let entries = engine.list_tree(&branch).unwrap();
        assert!(entries.iter().any(|entry| entry.file_id == original.file_id));
        assert!(entries.iter().filter(|entry| entry.file_id == original.file_id)
            .all(|entry| entry.name() == "new.txt"));
    });
}

#[test]
fn batch_file_deletion_requires_selected_scope_and_preserves_alias_compositions() {
    for active in [false, true] {
        let mut p = Pool::disk_root();
        let original = selected_root_aliases(&mut p);
        let root = p.register("", "deletion scope");
        if active {
            p.select("", "deletion scope");
        }
        let before = p.engine.state().clone();
        let request = DirectoryNamespaceBatch {
            kind: storage_contract::DirectoryBatchKind::Delete,
            items: vec![DirectoryNamespaceItem {
                source: p.source(&root, "a/report.txt"),
                target: None,
            }],
        };
        let operation = storage_contract::OperationId::generate().unwrap();
        let result =
            p.engine
                .directory_namespace_batch(&p.repo.repository_id, &request, "admin", operation);
        if active {
            let receipt = result.unwrap();
            assert_eq!(receipt.kind, "file.delete.batch");
            assert_eq!(
                receipt.result["deletion_nodes"].as_array().unwrap().len(),
                1
            );
            let node = p.engine.file_deletion(&original.file_id).unwrap();
            assert_eq!(node.operation_id, operation);
            assert_eq!(node.path.as_str(), "a/report.txt");
            assert_eq!(p.engine.state().commits, before.commits);
            assert_eq!(
                p.engine.state().directory_branches,
                before.directory_branches
            );
            assert_eq!(p.engine.state().file_contexts, before.file_contexts);
            assert_eq!(p.engine.state().file_selections, before.file_selections);
            let visible = p.engine.namespace_tree(&p.repo.main_branch_id).unwrap();
            assert!(visible.get("a/report.txt").is_none());
            assert!(visible.get("b/report.txt").is_none());
            assert!(visible.get("b/stay.txt").is_some());
            assert_eq!(
                p.engine
                    .directory_namespace_batch(&p.repo.repository_id, &request, "admin", operation)
                    .unwrap()
                    .result,
                receipt.result
            );
        } else {
            assert_eq!(
                result.unwrap_err().code(),
                storage_contract::ErrorCode::DirectoryBranchConflict
            );
            assert_eq!(p.engine.state(), &before);
        }
        assert_eq!(p.entry(&root, "a/report.txt").file_id, original.file_id);
        assert_eq!(p.entry(&root, "b/report.txt").file_id, original.file_id);
        assert_eq!(p.bytes(&original.version_id), b"original");
        let after = p.engine.state().clone();
        p.reopen(|recovered| assert_eq!(recovered.state(), &after));
    }
}

#[test]
fn inherited_first_write_truncates_a_multibyte_collision_label_and_reopens() {
    let mut p = Pool::new();
    p.save("docs/report.txt", b"source");
    let source = p.current_entry("docs/report.txt");
    let origin = format!("{}ab", "文".repeat(42));
    p.register("docs", &origin);
    p.engine
        .create_file_branch(
            &source.file_id,
            &source.version_id,
            &storage_contract::BranchName::parse(&origin).unwrap(),
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    p.select("docs", &origin);
    let branches = p.engine.state().branches.clone();
    let versions = p.engine.state().versions.clone();
    let inherited = p
        .engine
        .file_context(&p.repo.repository_id, "docs/report.txt")
        .unwrap();
    assert!(inherited.inherited_from.is_some());
    p.save("docs/report.txt", b"source");
    assert_eq!(p.engine.state().branches, branches);
    assert_eq!(p.engine.state().versions, versions);
    assert_eq!(
        p.engine.file_context(&p.repo.repository_id, "docs/report.txt"),
        Some(inherited)
    );
    p.save("docs/report.txt", b"first own body");
    let context = p
        .engine
        .file_context(&p.repo.repository_id, "docs/report.txt")
        .unwrap();
    let expected = format!("{}-1", "文".repeat(42));
    assert_eq!(expected.len(), 128);
    assert_eq!(p.engine.state().branches[&context.branch_id].name, expected);
    let own = p.current_entry("docs/report.txt");
    assert_ne!(own.version_id, source.version_id);
    assert_eq!(p.bytes(&own.version_id), b"first own body");
    p.reopen(|recovered| {
        assert_eq!(
            recovered.state().branches[&context.branch_id].name,
            expected
        );
        storage_contract::BranchName::parse(&recovered.state().branches[&context.branch_id].name)
            .unwrap();
    });
}

#[test]
fn shared_file_branch_workspace_uses_explicit_alias_and_refuses_second_destination() {
    let mut pool = Pool::new();
    let (_, _, original) = selected_aliases(&mut pool);
    let branch = pool.engine.file_context(&pool.repo.repository_id, "b/report.txt")
        .unwrap().branch_id;
    pool.engine.select_file_branch_at(
        &pool.repo.repository_id,
        &name_policy::PortablePath::parse("a/report.txt").unwrap(),
        &original.file_id,
        &branch,
        "admin",
        storage_contract::OperationId::generate().unwrap(),
    ).unwrap();
    let a = pool.current_entry("a/report.txt");
    let b = pool.current_entry("b/report.txt");
    assert_eq!(pool.engine.file_context(&pool.repo.repository_id, a.path()).unwrap().branch_id, branch);
    assert_eq!(pool.engine.file_context(&pool.repo.repository_id, b.path()).unwrap().branch_id, branch);
    let workspace = pool.engine.create_workspace(&branch, "admin").unwrap();
    pool.engine.stage_bytes(&workspace.workspace_id, b.path(), b"explicit alias body").unwrap();
    let staged = pool.engine.workspace(&workspace.workspace_id).unwrap();
    assert_eq!(staged.files.len(), 1);
    assert_eq!(
        pool.engine.stage_bytes(&workspace.workspace_id, a.path(), b"wrong second alias")
            .unwrap_err().code(),
        storage_contract::ErrorCode::FileBranchConflict,
    );
    let after_rejection = pool.engine.workspace(&workspace.workspace_id).unwrap();
    assert_eq!(after_rejection.files.len(), 1);
    assert_eq!(
        storage_engine::canonical_json(&after_rejection.files).unwrap(),
        storage_engine::canonical_json(&staged.files).unwrap(),
    );
    pool.commit(workspace);
    let updated = pool.current_entry(b.path());
    assert_ne!(updated.version_id, b.version_id);
    assert_eq!(pool.current_entry(a.path()), a);
    assert_eq!(pool.bytes(&updated.version_id), b"explicit alias body");
    assert!(pool.engine.state().file_path_nodes.is_empty());
    let repository = pool.repo.clone();
    pool.reopen(|engine| {
        let tree = engine.namespace_tree(&repository.main_branch_id).unwrap();
        assert_eq!(tree.get(b.path()).unwrap(), &updated);
        assert_eq!(tree.get(a.path()).unwrap(), &a);
    });
}
