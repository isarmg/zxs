//! Focused phase-one safety regressions using only public Engine operations.
//! One P01 plus SYSTEM; no DTO additions, checkpoint helper or actual purge.
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
};
use storage_engine::{
    BranchState, CommitRequest, Config, ContentState, DirectoryCommitRequest, Engine, Entry,
    ErrorCode, NamespaceOperation, NamespaceOperationKind, OperationId, OperationResult,
    Repository, Result, WorkspaceState, canonical_json,
    directory_git::{
        DirectoryGitBranch, DirectoryGitInitializeBody, DirectoryGitStageChildBody,
        DirectoryGitStageOwnedBody, DirectoryGitStatus,
    },
};

fn op() -> OperationId {
    OperationId::generate().unwrap()
}

fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut result = BTreeMap::new();
    for child in fs::read_dir(root).unwrap() {
        let child = child.unwrap();
        let path = child.path();
        if child.file_type().unwrap().is_dir() {
            result.extend(files(&path));
        } else {
            result.insert(path.clone(), fs::read(path).unwrap());
        }
    }
    result
}

struct Pool {
    temp: tempfile::TempDir,
    engine: Engine,
    repo: Repository,
    workspaces: Vec<String>,
}

impl Pool {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let config = Config::development([temp.path().join("primary")]).unwrap();
        assert_eq!(config.disks.len(), 1);
        assert_eq!(config.disks[0].slot_id, "P01");
        assert_eq!(config.disks[0].level, 1);
        assert!(config.disks[0].upstream_slot_id.is_none());
        assert!(config.disks[0].metadata_member);
        assert_eq!(config.metadata_quorum(), 1);
        let mut engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
        let repo = engine.create_disk_root("P01", "admin", op()).unwrap();
        assert_eq!(repo.target_copies, 1);
        Self {
            temp,
            engine,
            repo,
            workspaces: Vec::new(),
        }
    }

    fn reopen(self) -> Self {
        let Self {
            temp,
            engine,
            repo,
            workspaces,
        } = self;
        drop(engine);
        let engine = Engine::open(&temp.path().join("system")).unwrap();
        Self {
            temp,
            engine,
            repo,
            workspaces,
        }
    }

    fn save(&mut self, path: &str, body: &[u8]) -> Entry {
        let workspace = self
            .engine
            .create_workspace(&self.repo.main_branch_id, "admin")
            .unwrap();
        let id = workspace.workspace_id.clone();
        self.engine.stage_bytes(&id, path, body).unwrap();
        self.engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: op(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "Safety fixture save".into(),
                request_digest: None,
            })
            .unwrap();
        assert_eq!(
            self.engine.workspace(&id).unwrap().state,
            WorkspaceState::Committed
        );
        self.workspaces.push(id);
        self.entry(path)
    }

    fn status(&self, path: &str) -> DirectoryGitStatus {
        self.engine
            .directory_git_status(&self.repo.repository_id, path)
            .unwrap()
    }

    fn init(&mut self, path: &str) {
        let body = DirectoryGitInitializeBody {
            path: path.into(),
            expected_namespace_head: self.engine.state().branches[&self.repo.main_branch_id]
                .head_commit_id
                .clone(),
            operation_id: op(),
        };
        self.engine
            .initialize_directory_git(&self.repo.repository_id, &body, "admin")
            .unwrap();
    }

    fn stage(&mut self, path: &str) {
        let s = self.status(path);
        self.engine
            .stage_directory_git_owned(
                &self.repo.repository_id,
                &DirectoryGitStageOwnedBody {
                    directory_id: s.directory_id,
                    directory_branch_id: s.directory_branch_id.unwrap(),
                    expected_tree_id: s.working_tree_id.unwrap(),
                    expected_index_generation: s.index_generation,
                    operation_id: op(),
                },
                "admin",
            )
            .unwrap();
    }

    fn stage_child(&mut self, parent: &str, child: &str) {
        let p = self.status(parent);
        let c = self.status(child);
        self.engine
            .stage_directory_git_child(
                &self.repo.repository_id,
                &DirectoryGitStageChildBody {
                    directory_id: p.directory_id,
                    directory_branch_id: p.directory_branch_id.unwrap(),
                    expected_index_generation: p.index_generation,
                    child_directory_id: c.directory_id,
                    expected_child_head_tree_id: c.head_tree_id.unwrap(),
                    operation_id: op(),
                },
                "admin",
            )
            .unwrap();
    }

    fn commit(
        &mut self,
        reviewed: &DirectoryGitStatus,
        operation: OperationId,
    ) -> Result<OperationResult> {
        self.engine
            .commit_directory_changes(DirectoryCommitRequest {
                repository_id: &self.repo.repository_id,
                directory_branch_id: reviewed.directory_branch_id.as_deref().unwrap(),
                expected_tree_id: reviewed.working_tree_id.as_deref().unwrap(),
                expected_index_generation: reviewed.index_generation,
                message: "Safety checkpoint",
                principal: "admin",
                operation_id: operation,
            })
    }

    fn entry(&self, path: &str) -> Entry {
        self.engine
            .list_tree(&self.repo.main_branch_id)
            .unwrap()
            .into_iter()
            .find(|entry| entry.path() == path)
            .unwrap()
    }

    fn index(&self, path: &str) -> DirectoryGitBranch {
        let s = self.status(path);
        self.engine.state().directory_git_repositories[&s.directory_id].branches
            [s.directory_branch_id.as_ref().unwrap()]
        .clone()
    }

    fn snapshot(&self, tree: &str, path: &str) -> Entry {
        self.engine.state().directory_trees[tree]
            .entries
            .iter()
            .find(|entry| entry.path() == path)
            .unwrap()
            .clone()
    }

    fn bytes(&self, entry: &Entry) -> Vec<u8> {
        let mut body = Vec::new();
        self.engine
            .open_content(&entry.version_id)
            .unwrap()
            .file
            .read_to_end(&mut body)
            .unwrap();
        body
    }

    fn history(&self, entry: &Entry) -> String {
        self.engine
            .file_history(&entry.file_id)
            .unwrap()
            .into_iter()
            .find(|version| version.version_id == entry.version_id)
            .unwrap()
            .history_tree_id
    }

    fn assert_no_ordinary_pin(&self, entry: &Entry) {
        let state = self.engine.state();
        for branch in state
            .branches
            .values()
            .filter(|branch| branch.state == BranchState::Active)
        {
            assert!(
                state.commits[&branch.head_commit_id]
                    .entries
                    .iter()
                    .all(|current| current.version_id != entry.version_id),
                "ordinary HEAD {} masks the intended Git-only pin",
                branch.branch_id
            );
            assert!(
                branch
                    .pending_file_versions
                    .values()
                    .all(|version| version != &entry.version_id)
            );
        }
        for branch in state.directory_branches.values() {
            assert!(
                branch
                    .entries
                    .iter()
                    .all(|current| current.version_id != entry.version_id)
            );
            assert!(
                state.directory_trees[&branch.head_tree_id]
                    .entries
                    .iter()
                    .all(|current| current.version_id != entry.version_id)
            );
        }
        assert!(
            self.engine
                .list_tree(&self.repo.main_branch_id)
                .unwrap()
                .iter()
                .all(|current| current.version_id != entry.version_id)
        );
        for workspace in &self.workspaces {
            assert_eq!(
                self.engine.workspace(workspace).unwrap().state,
                WorkspaceState::Committed
            );
        }
        // Every content reader above returns owned bytes and drops its lease.
    }
}

fn pinned(pool: &mut Pool, entry: &Entry) {
    let history = pool.history(entry);
    let state = canonical_json(pool.engine.state()).unwrap();
    let storage = files(pool.temp.path());
    assert_eq!(
        pool.engine
            .create_purge_plan(&history, vec![entry.version_id.clone()], "admin")
            .unwrap_err()
            .code(),
        ErrorCode::ContentPinned
    );
    assert_eq!(canonical_json(pool.engine.state()).unwrap(), state);
    assert_eq!(
        files(pool.temp.path()),
        storage,
        "rejection changed source, journal or local plans"
    );
}

fn allowed_plan(pool: &mut Pool, entry: &Entry) {
    let history = pool.history(entry);
    let body = pool.bytes(entry);
    let state = canonical_json(pool.engine.state()).unwrap();
    let primary = pool.engine.config().disks[0].root.clone();
    let storage = files(&primary);
    let plan = pool
        .engine
        .create_purge_plan(&history, vec![entry.version_id.clone()], "admin")
        .unwrap();
    assert_eq!(plan.kind, "purge");
    assert_eq!(
        plan.input["history_tree_id"].as_str(),
        Some(history.as_str())
    );
    assert_eq!(plan.input["version_ids"].as_array().unwrap().len(), 1);
    assert_eq!(
        plan.input["version_ids"][0].as_str(),
        Some(entry.version_id.as_str())
    );
    assert_eq!(pool.engine.plan(&plan.plan_id).unwrap(), plan);
    assert_eq!(canonical_json(pool.engine.state()).unwrap(), state);
    assert_eq!(
        files(&primary),
        storage,
        "preparation changed data or journals"
    );
    assert_eq!(
        pool.engine.state().content_states[&entry.version_id],
        ContentState::Available
    );
    assert_eq!(pool.bytes(entry), body);
    // The sole permitted write is the local SYSTEM plan; no commit_plan call.
}

fn replaced_directory_identity_case(committed: bool) {
    let mut pool = Pool::new();
    pool.save("docs/file", b"old");
    let original = pool.entry("docs");
    pool.init("docs");
    let receipt = if committed {
        pool.stage("docs");
        let reviewed = pool.status("docs");
        let operation = op();
        let result = pool.commit(&reviewed, operation).unwrap();
        Some((reviewed, operation, result))
    } else {
        None
    };
    let old = pool.status("docs");
    assert_eq!(old.directory_id, original.entry_id);
    assert_eq!(old.head_tree_id.is_some(), committed);
    pool.engine
        .file_namespace(
            &pool.repo.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::DeleteTree, "docs", None).unwrap(),
            "admin",
            op(),
        )
        .unwrap();
    pool.save("docs/file", b"replacement");
    let replacement = pool.entry("docs");
    assert_ne!(replacement.entry_id, original.entry_id);
    for cold in [false, true] {
        if cold {
            pool = pool.reopen();
        }
        let fresh = pool.status("docs");
        assert_eq!(fresh.directory_id, replacement.entry_id);
        assert!(!fresh.initialized);
        assert!(fresh.directory_branch_id.is_none());
        let operation = op();
        let state = canonical_json(pool.engine.state()).unwrap();
        let storage = files(pool.temp.path());
        assert_eq!(
            pool.commit(&old, operation).unwrap_err().code(),
            ErrorCode::DirectoryBranchConflict,
            "committed={committed}, cold={cold}"
        );
        assert!(!pool.engine.state().operations.contains_key(&operation));
        if let Some((reviewed, operation, result)) = &receipt {
            assert_eq!(pool.commit(reviewed, *operation).unwrap(), *result);
        }
        assert_eq!(canonical_json(pool.engine.state()).unwrap(), state);
        assert_eq!(files(pool.temp.path()), storage);
        assert_eq!(pool.entry("docs"), replacement);
        assert_eq!(pool.bytes(&pool.entry("docs/file")), b"replacement");
        for workspace in &pool.workspaces {
            assert_eq!(
                pool.engine.workspace(workspace).unwrap().state,
                WorkspaceState::Committed
            );
        }
    }
}

#[test]
fn unborn_directory_replacement_rejects_fresh_commit_hot_and_cold() {
    replaced_directory_identity_case(false);
}

#[test]
fn committed_directory_replacement_rejects_fresh_commit_and_preserves_old_receipt() {
    replaced_directory_identity_case(true);
}

fn owned_git_body_pin_case(head_only: bool) {
    let mut pool = Pool::new();
    pool.save("file", b"H0");
    pool.init("");
    pool.stage("");
    let reviewed = pool.status("");
    pool.commit(&reviewed, op()).unwrap();
    let h0 = pool.status("").head_tree_id.unwrap();
    let t1 = pool.save("file", b"T1");
    pool.stage("");
    let t1_index = pool.index("").index_tree_id;
    let t2 = pool.save("file", b"T2");
    if !head_only {
        for cold in [false, true] {
            if cold {
                pool = pool.reopen();
            }
            assert_eq!(pool.status("").head_tree_id.as_ref(), Some(&h0));
            assert_eq!(pool.index("").index_tree_id, t1_index);
            assert_eq!(pool.snapshot(&t1_index, "file"), t1);
            assert_eq!(pool.entry("file"), t2);
            pool.assert_no_ordinary_pin(&t1);
            pinned(&mut pool, &t1);
        }
    }
    let reviewed = pool.status("");
    pool.commit(&reviewed, op()).unwrap();
    let t1_head = pool.status("").head_tree_id.unwrap();
    assert_eq!(pool.snapshot(&t1_head, "file"), t1);
    pool.stage("");
    for cold in [false, true] {
        if cold {
            pool = pool.reopen();
        }
        assert_eq!(pool.status("").head_tree_id.as_ref(), Some(&t1_head));
        assert_eq!(pool.snapshot(&pool.index("").index_tree_id, "file"), t2);
        pool.assert_no_ordinary_pin(&t1);
        pinned(&mut pool, &t1);
    }
    let reviewed = pool.status("");
    pool.commit(&reviewed, op()).unwrap();
    let mut pool = pool.reopen();
    pool.assert_no_ordinary_pin(&t1);
    for git in pool.engine.state().directory_git_repositories.values() {
        for index in git.branches.values() {
            assert!(index.index_boundaries.is_empty() && index.index_children.is_empty());
            for tree in index
                .head_tree_id
                .iter()
                .chain(std::iter::once(&index.index_tree_id))
            {
                assert!(
                    pool.engine.state().directory_trees[tree]
                        .entries
                        .iter()
                        .all(|entry| entry.version_id != t1.version_id)
                );
            }
        }
    }
    allowed_plan(&mut pool, &t1);
}

#[test]
fn owned_index_and_head_pin_body_hot_and_cold_until_both_advance() {
    owned_git_body_pin_case(false);
}

#[test]
fn owned_head_pins_body_hot_and_cold_until_head_advances() {
    owned_git_body_pin_case(true);
}

#[test]
fn explicitly_staged_child_head_is_a_pin_but_raw_child_work_in_parent_index_is_not() {
    let mut pool = Pool::new();
    let c0 = pool.save("child/file", b"C0");
    pool.save("own", b"parent");
    pool.init("child");
    pool.stage("child");
    let child = pool.status("child");
    pool.commit(&child, op()).unwrap();
    let child = pool.status("child");
    let child_id = child.directory_id.clone();
    let c0_head = child.head_tree_id.unwrap();
    pool.init("");
    pool.stage("");
    let parent = pool.status("");
    pool.commit(&parent, op()).unwrap();
    let parent_head = pool.status("").head_tree_id.unwrap();
    assert!(
        !pool.engine.state().directory_trees[&parent_head]
            .directory_refs
            .contains_key(&child_id)
    );
    assert!(
        pool.engine.state().directory_trees[&parent_head]
            .entries
            .iter()
            .all(|entry| entry.version_id != c0.version_id)
    );
    pool.stage_child("", "child");
    let parent_index = pool.index("");
    assert_eq!(
        parent_index.index_children[&child_id].tree_id.as_ref(),
        Some(&c0_head)
    );
    assert_eq!(parent_index.index_boundaries[&child_id], "child");
    assert_eq!(pool.snapshot(&parent_index.index_tree_id, "child/file"), c0);
    let c1 = pool.save("child/file", b"C1");
    pool.stage("child");
    let child = pool.status("child");
    pool.commit(&child, op()).unwrap();
    let c1_head = pool.status("child").head_tree_id.unwrap();
    assert_ne!(c1_head, c0_head);
    for cold in [false, true] {
        if cold {
            pool = pool.reopen();
        }
        assert_eq!(pool.entry("child/file"), c1);
        assert_eq!(pool.status("").head_tree_id.as_ref(), Some(&parent_head));
        assert_eq!(pool.index("child").head_tree_id.as_ref(), Some(&c1_head));
        assert_eq!(
            pool.snapshot(&pool.index("child").index_tree_id, "child/file"),
            c1
        );
        assert_eq!(
            pool.index("").index_children[&child_id].tree_id.as_ref(),
            Some(&c0_head)
        );
        pool.assert_no_ordinary_pin(&c0);
        pinned(&mut pool, &c0);
    }
    pool.stage_child("", "child");
    for cold in [false, true] {
        if cold {
            pool = pool.reopen();
        }
        let index = pool.index("");
        assert_eq!(index.index_tree_id, parent_index.index_tree_id);
        assert_eq!(index.index_boundaries[&child_id], "child");
        assert_eq!(
            index.index_children[&child_id].tree_id.as_ref(),
            Some(&c1_head)
        );
        assert_eq!(
            pool.snapshot(&index.index_tree_id, "child/file"),
            c0,
            "raw index retains child work; the ownership boundary must exclude it"
        );
        assert_eq!(pool.status("").head_tree_id.as_ref(), Some(&parent_head));
        assert_eq!(pool.index("child").head_tree_id.as_ref(), Some(&c1_head));
        assert_eq!(
            pool.snapshot(&pool.index("child").index_tree_id, "child/file"),
            c1
        );
        assert_eq!(pool.entry("child/file"), c1);
        pool.assert_no_ordinary_pin(&c0);
        allowed_plan(&mut pool, &c0);
    }
}
