//! Public-engine regressions for branch-scoped aliases and namespace batches.
//! All storage, physical-container and restart checks use disposable pools.
use std::{io::Read, path::PathBuf};
use storage_engine::{
    CommitRequest, Config, DirectoryBranch, DirectoryNamespaceBatch, DirectoryNamespaceItem,
    DirectoryNamespaceSource, DirectoryNamespaceTarget, Engine, Entry, NamespaceOperation,
    OperationResult, Repository, Workspace, new_id,
};
#[path = "branch_alias_operations/batch_planning_scenarios.rs"]
mod batch_planning_scenarios;
#[path = "branch_alias_operations/copy_alias_scenarios.rs"]
mod copy_alias_scenarios;
#[path = "branch_alias_operations/deletion_rebase_scenarios.rs"]
mod deletion_rebase_scenarios;
#[path = "branch_alias_operations/file_merge_scenarios.rs"]
mod file_merge_scenarios;

struct Pool {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    engine: Engine,
    repo: Repository,
}
impl Pool {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("system");
        let config = Config::development([
            temporary.path().join("primary"),
            temporary.path().join("backup"),
            temporary.path().join("tertiary"),
        ]);
        let mut engine = Engine::initialize(&root, config).unwrap();
        let repo = engine
            .create_repository(
                "Alias fixtures",
                "P01",
                3,
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
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
                operation_id: storage_contract::OperationId::generate(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "Alias integration regression".into(),
                request_digest: None,
            })
            .unwrap()
    }
    fn register(&mut self, path: &str, name: &str) -> String {
        self.engine
            .create_directory_branches(
                &self.repo.repository_id,
                path,
                name,
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap()
            .result["directory_branch_id"]
            .as_str()
            .unwrap()
            .to_owned()
    }
    fn select(&mut self, path: &str, name: &str) {
        self.engine
            .select_directory_branches(
                &self.repo.repository_id,
                path,
                name,
                "admin",
                storage_contract::OperationId::generate(),
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
            .find(|entry| entry.path == path)
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
    fn batch(&mut self, kind: &str, items: Vec<DirectoryNamespaceItem>) -> OperationResult {
        self.engine
            .directory_namespace_batch(
                &self.repo.repository_id,
                &DirectoryNamespaceBatch {
                    kind: kind.into(),
                    items,
                },
                "admin",
                storage_contract::OperationId::generate(),
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
            .join(&version.storage_name)
    }
    fn current_entry(&self, path: &str) -> Entry {
        self.engine
            .list_tree(&self.repo.main_branch_id)
            .unwrap()
            .into_iter()
            .find(|entry| entry.path == path)
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
        "copy",
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
            .all(|entry| !entry.path.starts_with("b/copied"))
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
    let m = p.register("a", "m");
    let n = p.register("a", "n");
    let retained = p.register("a", "retained");
    let l = p.register("b", "l");
    let target_p = p.register("b", "p");
    let original = p.entry(&m, "a/report.txt");
    p.batch(
        "move",
        vec![p.mapping(&m, "a/report.txt", &l, "b/report.txt")],
    );
    let old_n = p.branch(&n);
    let old_p = p.branch(&target_p);
    p.batch(
        "rename",
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
    p.batch(
        "rename",
        vec![p.mapping(&retained, "a/new.txt", &retained, "a/new.txt")],
    );
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

fn selected_aliases(p: &mut Pool) -> (String, String, Entry) {
    p.save("a/report.txt", b"original");
    p.save("b/stay.txt", b"sibling");
    let m = p.register("a", "m");
    let n = p.register("a", "n");
    let l = p.register("b", "l");
    let original = p.entry(&m, "a/report.txt");
    p.batch(
        "move",
        vec![p.mapping(&m, "a/report.txt", &l, "b/report.txt")],
    );
    p.select("a", "n");
    p.select("b", "l");
    let draft = p
        .engine
        .create_file_branch(
            &original.file_id,
            &original.version_id,
            "explicit draft",
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap()
        .result["branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    p.engine
        .select_file_branch_at(
            &p.repo.repository_id,
            "b/report.txt",
            &original.file_id,
            &draft,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    (n, l, original)
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
    let operation = storage_contract::OperationId::generate();
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
            "b/report.txt",
            &original.file_id,
            &a.branch_id,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    p.engine
        .archive_branch(
            &b.branch_id,
            "admin",
            storage_contract::OperationId::generate(),
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
        ("b/report.txt", "b/renamed.txt"),
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
    p.commit(workspace);
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
        let node = &p.engine.state().file_path_nodes[renamed
            .node_id
            .as_deref()
            .expect("rename creates a file path node")];
        assert_eq!(node.version_id, original.version_id);
        assert_eq!(node.path, path);
        assert_eq!(node.previous_path, old_path);
        assert_eq!(
            node.parent_node_ids,
            vec![
                before
                    .node_id
                    .clone()
                    .unwrap_or_else(|| original.version_id.clone())
            ]
        );
    }
    assert_eq!(p.current_entry("a/renamed.txt").file_id, original.file_id);
    assert_eq!(p.current_entry("b/renamed.txt").file_id, original.file_id);
    assert_ne!(p.branch(&n).head_tree_id, a_head);
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
fn deleting_one_inactive_root_alias_preserves_the_other_alias_and_its_context() {
    let mut p = Pool::new();
    let (_, _, original) = selected_aliases(&mut p);
    let root = p.register("", "archive");
    let before = p.branch(&root);
    let b_context = before.file_contexts["b/report.txt"].clone();
    let selected_before = p.engine.list_tree(&p.repo.main_branch_id).unwrap();
    let item = DirectoryNamespaceItem {
        source: p.source(&root, "a/report.txt"),
        target: None,
    };
    p.batch("delete", vec![item]);
    let after = p.branch(&root);
    assert!(
        after
            .entries
            .iter()
            .all(|entry| entry.path != "a/report.txt")
    );
    assert_eq!(
        p.entry(&root, "b/report.txt").version_id,
        original.version_id
    );
    assert_eq!(after.file_contexts["b/report.txt"], b_context);
    assert!(!after.file_contexts.contains_key("a/report.txt"));
    assert!(
        after.file_branches.contains_key(&original.file_id),
        "legacy file index still covers retained alias"
    );
    assert_eq!(
        json_codec::to_value(p.engine.list_tree(&p.repo.main_branch_id).unwrap()).unwrap(),
        json_codec::to_value(selected_before).unwrap()
    );
    assert_eq!(p.bytes(&original.version_id), b"original");
    p.reopen(|recovered| {
        assert_eq!(
            recovered.state().directory_branches[&root].file_contexts["b/report.txt"],
            b_context
        )
    });
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
    let operation = storage_contract::OperationId::generate();
    let renamed = p
        .engine
        .rename_file_branch(
            &original.file_id,
            &branch.branch_id,
            "客户主稿",
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
                "客户主稿",
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
    for invalid in ["x".repeat(129), "文".repeat(43), "bad\nname".into()] {
        assert!(
            p.engine
                .rename_file_branch(
                    &original.file_id,
                    &branch.branch_id,
                    &invalid,
                    "admin",
                    storage_contract::OperationId::generate()
                )
                .is_err()
        );
    }
    let collision = p
        .engine
        .rename_file_branch(
            &original.file_id,
            &branch.branch_id,
            "EXPLICIT DRAFT",
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap_err();
    assert_eq!(collision.code(), storage_contract::ErrorCode::NameCollision);
    let denied = p
        .engine
        .rename_file_branch(
            &original.file_id,
            &branch.branch_id,
            "Missing actor",
            "",
            storage_contract::OperationId::generate(),
        )
        .unwrap_err();
    assert_eq!(denied.code(), storage_contract::ErrorCode::Unauthenticated);
    let foreign = p
        .engine
        .rename_file_branch(
            &new_id("file"),
            &branch.branch_id,
            "Wrong file",
            "admin",
            storage_contract::OperationId::generate(),
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
            &boundary,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    assert_eq!(p.engine.state().directory_trees.len(), trees);
    p.reopen(|recovered| assert_eq!(recovered.state().branches[&branch.branch_id].name, boundary));
}

#[test]
fn workspace_conflicting_alias_names_reject_the_complete_namespace_decision() {
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
            operation_id: storage_contract::OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "Conflicting canonical names".into(),
            request_digest: None,
        })
        .unwrap_err();
    assert_eq!(
        error.code(),
        storage_contract::ErrorCode::BatchMappingConflict
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
        .file_context(&p.repo.repository_id, &original.path)
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
    p.commit(workspace);
    let renamed = p.current_entry("final/child/report.txt");
    assert_eq!(renamed.entry_id, original.entry_id);
    let updated = p
        .engine
        .file_context(&p.repo.repository_id, &renamed.path)
        .unwrap();
    assert_eq!(updated.branch_id, context.branch_id);
    assert_eq!(updated.inherited_from, context.inherited_from);
    assert_eq!(
        p.engine.state().file_locations[&original.file_id],
        renamed.path()
    );
    assert_eq!(p.branch(&draft).path, "final");
    assert!(updated.node_id.is_some());
    assert_eq!(p.bytes(&original.version_id), b"original");
    let repo = p.repo.repository_id.clone();
    p.reopen(|engine| {
        assert_eq!(engine.file_context(&repo, &renamed.path).unwrap(), updated);
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
    p.commit(workspace);
    for (original, context, path) in [(a, ca, "b.txt"), (b, cb, "a.txt")] {
        let current = p.current_entry(path);
        assert_eq!(current.file_id, original.file_id);
        let updated = p.engine.file_context(&p.repo.repository_id, path).unwrap();
        assert_eq!(updated.branch_id, context.branch_id);
        assert_eq!(updated.inherited_from, context.inherited_from);
        assert_eq!(p.engine.state().file_locations[&original.file_id], path);
        let node = &p.engine.state().file_path_nodes[updated.node_id.as_ref().unwrap()];
        assert_eq!(node.file_id, original.file_id);
        assert_eq!(node.path, path);
    }
    p.reopen(|_| {});
}

#[test]
fn workspace_delete_then_replace_path_keeps_surviving_files_context() {
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
    p.commit(workspace);
    assert_eq!(p.current_entry("a.txt").file_id, survivor.file_id);
    let updated = p
        .engine
        .file_context(&p.repo.repository_id, "a.txt")
        .unwrap();
    assert_eq!(updated.branch_id, context.branch_id);
    assert_eq!(updated.inherited_from, context.inherited_from);
    assert!(
        p.engine
            .file_context(&p.repo.repository_id, "b.txt")
            .is_none()
    );
    assert!(
        !p.engine
            .state()
            .file_selections
            .contains_key(&retired.file_id)
    );
    assert_eq!(p.bytes(&survivor.version_id), b"surviving");
    let repo = p.repo.repository_id.clone();
    p.reopen(|engine| assert_eq!(engine.file_context(&repo, "a.txt").unwrap(), updated));
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
        ("a/temporary.txt", "a/final.txt"),
        ("b/report.txt", "b/temporary.txt"),
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
    p.commit(workspace);
    for (path, before) in ["a/final.txt", "b/final.txt"].into_iter().zip(contexts) {
        let context = p.engine.file_context(&p.repo.repository_id, path).unwrap();
        assert_eq!(context.branch_id, before.branch_id);
        assert_eq!(context.inherited_from, before.inherited_from);
        assert_eq!(p.current_entry(path).file_id, original.file_id);
        assert_eq!(
            p.engine.state().file_path_nodes[context.node_id.as_ref().unwrap()].path,
            path
        );
    }
    assert_eq!(
        p.engine.state().file_locations[&original.file_id],
        "b/final.txt"
    );
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
        let moved = p.register(source, "moved");
        p.register(source, "retained");
        let target = p.register(owner, "target");
        let original = p.current_entry(&format!("{source}/report.txt"));
        p.batch(
            "move",
            vec![p.mapping(
                &moved,
                &original.path,
                &target,
                &format!("{owner}/report.txt"),
            )],
        );
        p.select(source, "retained");
        p.select(owner, "target");
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
fn exact_no_op_rename_of_stale_alias_preserves_physical_name() {
    let mut p = Pool::new();
    let (_, _, original) = selected_aliases(&mut p);
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
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    assert_eq!(
        p.engine.state().file_locations[&original.file_id],
        "b/new.txt"
    );
    let before = p
        .engine
        .file_context(&p.repo.repository_id, "b/report.txt")
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
                "b/report.txt",
                Some("b/report.txt"),
            )
            .unwrap(),
        )
        .unwrap();
    p.commit(workspace);
    assert_eq!(
        p.engine.state().file_locations[&original.file_id],
        "b/new.txt"
    );
    assert_eq!(
        p.engine
            .file_context(&p.repo.repository_id, "b/report.txt")
            .unwrap(),
        before
    );
    assert_eq!(p.bytes(&original.version_id), b"original");
    p.reopen(|engine| {
        assert_eq!(
            engine.state().file_locations[&original.file_id],
            "b/new.txt"
        )
    });
}

#[test]
fn batch_deletion_facts_cover_active_and_inactive_alias_scopes_without_move_deletions() {
    for active in [false, true] {
        let mut p = Pool::new();
        let (_, _, original) = selected_aliases(&mut p);
        assert!(
            p.engine
                .state()
                .commits
                .values()
                .all(|commit| commit.namespace_deletions.is_none()),
            "moves and selections must not manufacture deletions"
        );
        let root = p.register("", "deletion scope");
        if active {
            p.select("", "deletion scope");
        }
        let operation = p.batch(
            "delete",
            vec![DirectoryNamespaceItem {
                source: p.source(&root, "a/report.txt"),
                target: None,
            }],
        );
        let facts = p
            .engine
            .state()
            .commits
            .values()
            .filter(|commit| commit.operation_id == operation.operation_id)
            .filter_map(|commit| commit.namespace_deletions.as_ref())
            .flat_map(|facts| &facts.paths)
            .collect::<Vec<_>>();
        assert_eq!(facts.len(), 1, "scope active: {active}");
        assert_eq!(facts[0].path, "a/report.txt");
        assert_eq!(facts[0].kind, storage_engine::NamespaceEntryKind::File);
        assert_eq!(p.entry(&root, "b/report.txt").file_id, original.file_id);
        assert_eq!(p.bytes(&original.version_id), b"original");
    }
}
