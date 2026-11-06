//! Public D3 regressions for staged owned content and fixed child history.
//! Every disk is a registered directory inside a fresh temporary pool.
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
};
use storage_engine::{
    CommitRequest, Config, ContentState, DirectoryCommitRequest, DiskConfig, DiskExitPhase, Engine,
    Entry, ErrorCode, MembershipExitAction, OperationId, OperationResult, PlacementRuleUpdate,
    Repository, Result, canonical_json, directory_git::*, new_id,
};

fn op() -> OperationId {
    OperationId::generate().unwrap()
}

struct Fixture {
    temp: tempfile::TempDir,
    engine: Engine,
    repo: Repository,
    retired: DiskConfig,
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
        let retired = config.disks[0].clone();
        let mut engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
        let repo = engine.create_disk_root("D04", "admin", op()).unwrap();
        assert_eq!(repo.target_copies, 1);
        Self {
            temp,
            engine,
            repo,
            retired,
        }
    }

    fn reopen(mut self) -> Self {
        drop(self.engine);
        self.engine = Engine::open(&self.temp.path().join("system")).unwrap();
        self
    }

    fn place(&mut self, rule: &str, path: &str, slot: &str, generation: u64) {
        self.engine
            .update_placement_rule(PlacementRuleUpdate {
                rule_id: rule,
                repository_id: &self.repo.repository_id,
                path_prefix: path,
                primary_slot_id: slot,
                expected_generation: generation,
                principal: "admin",
                operation_id: op(),
            })
            .unwrap();
    }

    fn save(&mut self, path: &str, bytes: &[u8]) -> Entry {
        let workspace = self
            .engine
            .create_workspace(&self.repo.main_branch_id, "admin")
            .unwrap();
        self.engine
            .stage_bytes(&workspace.workspace_id, path, bytes)
            .unwrap();
        self.engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: op(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "save staged-source fixture".into(),
                request_digest: None,
            })
            .unwrap();
        self.entry(path)
    }

    fn entry(&self, path: &str) -> Entry {
        self.engine
            .list_tree(&self.repo.main_branch_id)
            .unwrap()
            .into_iter()
            .find(|entry| entry.path() == path)
            .unwrap()
    }

    fn status(&self, path: &str) -> DirectoryGitStatus {
        self.engine
            .directory_git_status(&self.repo.repository_id, path)
            .unwrap()
    }

    fn index(&self, status: &DirectoryGitStatus) -> &DirectoryGitBranch {
        &self.engine.state().directory_git_repositories[&status.directory_id].branches
            [status.directory_branch_id.as_ref().unwrap()]
    }

    fn initialize(&mut self, path: &str) {
        let head = self.engine.state().branches[&self.repo.main_branch_id]
            .head_commit_id
            .clone();
        self.engine
            .initialize_directory_git(
                &self.repo.repository_id,
                &DirectoryGitInitializeBody {
                    path: path.into(),
                    expected_namespace_head: head,
                    operation_id: op(),
                },
                "admin",
            )
            .unwrap();
    }

    fn stage_owned(&mut self, path: &str) -> DirectoryGitStatus {
        let status = self.status(path);
        self.engine
            .stage_directory_git_owned(
                &self.repo.repository_id,
                &DirectoryGitStageOwnedBody {
                    directory_id: status.directory_id,
                    directory_branch_id: status.directory_branch_id.unwrap(),
                    expected_tree_id: status.working_tree_id.unwrap(),
                    expected_index_generation: status.index_generation,
                    operation_id: op(),
                },
                "admin",
            )
            .unwrap();
        self.status(path)
    }

    fn stage_child(&mut self, parent: &str, child: &str) {
        let parent = self.status(parent);
        let child = self.status(child);
        self.engine
            .stage_directory_git_child(
                &self.repo.repository_id,
                &DirectoryGitStageChildBody {
                    directory_id: parent.directory_id,
                    directory_branch_id: parent.directory_branch_id.unwrap(),
                    expected_index_generation: parent.index_generation,
                    child_directory_id: child.directory_id,
                    expected_child_head_tree_id: child.head_tree_id.unwrap(),
                    operation_id: op(),
                },
                "admin",
            )
            .unwrap();
    }

    fn commit(
        &mut self,
        status: &DirectoryGitStatus,
        operation: OperationId,
    ) -> Result<OperationResult> {
        self.engine
            .commit_directory_changes(DirectoryCommitRequest {
                repository_id: &self.repo.repository_id,
                directory_branch_id: status.directory_branch_id.as_ref().unwrap(),
                expected_tree_id: status.working_tree_id.as_ref().unwrap(),
                expected_index_generation: status.index_generation,
                message: "commit explicitly staged source",
                principal: "admin",
                operation_id: operation,
            })
    }

    fn exit_p01(&mut self) {
        self.engine
            .begin_disk_exit(
                &self.retired.disk_id,
                MembershipExitAction::Deregister,
                "admin",
                op(),
            )
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
        // Deliberately do not stage or commit any directory after publication.
    }
}

fn assert_placement(engine: &Engine, entry: &Entry, slot: &str) {
    assert_eq!(
        engine.state().versions[&entry.version_id].creation_primary_slot_id,
        slot
    );
    let placement = &engine.state().placements[&entry.version_id];
    assert_eq!(placement.primary_slot_id, slot);
    assert_eq!(placement.target_copies, 1);
    assert_eq!(placement.route_slot_ids, vec![slot.to_owned()]);
}

fn bytes(engine: &Engine, entry: &Entry) -> Vec<u8> {
    let mut result = Vec::new();
    engine
        .open_content(&entry.version_id)
        .unwrap()
        .file
        .read_to_end(&mut result)
        .unwrap();
    result
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

fn assert_staged_owned_rejected(
    fixture: &mut Fixture,
    staged: &Entry,
    working: &Entry,
    original_index: &DirectoryGitBranch,
) {
    let ready = fixture.status("");
    assert_eq!(fixture.index(&ready), original_index);
    assert_ne!(
        ready.working_tree_id.as_ref().unwrap(),
        &original_index.index_tree_id
    );
    assert!(ready.staged);
    assert!(ready.unstaged);
    assert!(!ready.needs_restage);
    assert_eq!(fixture.entry(working.path()), *working);
    assert_eq!(bytes(&fixture.engine, working), b"working T2");
    // D3 masks the old replica; no purge or forged ContentState is involved.
    assert_eq!(
        fixture.engine.state().content_states[&staged.version_id],
        ContentState::Available
    );
    assert!(fixture.engine.open_content(&staged.version_id).is_err());
    let metadata = canonical_json(fixture.engine.state()).unwrap();
    let work = fixture
        .engine
        .list_tree(&fixture.repo.main_branch_id)
        .unwrap();
    let disk_bytes = files(fixture.temp.path());
    let operation = op();
    assert_eq!(
        fixture.commit(&ready, operation).unwrap_err().code(),
        ErrorCode::DirectoryBranchConflict
    );
    assert!(!fixture.engine.state().operations.contains_key(&operation));
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), metadata);
    assert_eq!(
        fixture
            .engine
            .list_tree(&fixture.repo.main_branch_id)
            .unwrap(),
        work
    );
    assert_eq!(files(fixture.temp.path()), disk_bytes);
    assert_eq!(bytes(&fixture.engine, working), b"working T2");
}

#[test]
fn disk_exit_rejects_unavailable_staged_owned_version_without_mutation_hot_and_cold() {
    let mut fixture = Fixture::new();
    let rule = new_id("rule").unwrap();
    fixture.place(&rule, "file", "P01", 0);
    let t1 = fixture.save("file", b"staged T1");
    assert_placement(&fixture.engine, &t1, "P01");
    fixture.initialize("");
    let staged = fixture.stage_owned("");
    let original_index = fixture.index(&staged).clone();
    assert!(
        fixture.engine.state().directory_trees[&original_index.index_tree_id]
            .entries
            .contains(&t1)
    );

    fixture.place(&rule, "file", "D04", 1);
    let t2 = fixture.save("file", b"working T2");
    assert_eq!(t1.file_id, t2.file_id);
    assert_ne!(t1.version_id, t2.version_id);
    assert_placement(&fixture.engine, &t2, "D04");
    assert_eq!(fixture.index(&fixture.status("")), &original_index);
    fixture.exit_p01();

    assert_staged_owned_rejected(&mut fixture, &t1, &t2, &original_index);
    fixture = fixture.reopen();
    assert_staged_owned_rejected(&mut fixture, &t1, &t2, &original_index);

    // A new explicit review repairs the source; the retained working file stays T2.
    let ready = fixture.stage_owned("");
    let receipt = fixture.commit(&ready, op()).unwrap();
    let head = receipt.result["head_tree_id"].as_str().unwrap().to_owned();
    assert!(
        fixture.engine.state().directory_trees[&head]
            .entries
            .contains(&t2)
    );
    assert_eq!(fixture.entry("file"), t2);
    fixture = fixture.reopen();
    assert_eq!(
        fixture.status("").head_tree_id.as_deref(),
        Some(head.as_str())
    );
    assert_eq!(
        bytes(&fixture.engine, &fixture.entry("file")),
        b"working T2"
    );
}

#[test]
fn disk_exit_allows_fixed_child_history_and_replays_after_reopen() {
    let mut fixture = Fixture::new();
    let rule = new_id("rule").unwrap();
    fixture.place(&rule, "child/file", "P01", 0);
    let child_t1 = fixture.save("child/file", b"child C1");
    assert_placement(&fixture.engine, &child_t1, "P01");
    fixture.initialize("child");
    let child_ready = fixture.stage_owned("child");
    fixture.commit(&child_ready, op()).unwrap();
    let child_c1 = fixture.status("child");

    let own = fixture.save("own", b"parent owned content");
    fixture.initialize("");
    fixture.stage_owned("");
    fixture.stage_child("", "child");
    let original_index = fixture.index(&fixture.status("")).clone();
    assert!(
        original_index
            .index_boundaries
            .contains_key(&child_c1.directory_id)
    );
    assert_eq!(
        original_index.index_children[&child_c1.directory_id].tree_id,
        child_c1.head_tree_id
    );
    // The raw index contains child T1, but that entry is outside the parent's ownership.
    assert!(
        fixture.engine.state().directory_trees[&original_index.index_tree_id]
            .entries
            .contains(&child_t1)
    );

    fixture.place(&rule, "child/file", "D04", 1);
    let child_t2 = fixture.save("child/file", b"child C2");
    assert_eq!(child_t1.file_id, child_t2.file_id);
    assert_placement(&fixture.engine, &child_t2, "D04");
    let child_ready = fixture.stage_owned("child");
    fixture.commit(&child_ready, op()).unwrap();
    let child_c2 = fixture.status("child");
    assert_ne!(child_c1.head_tree_id, child_c2.head_tree_id);
    fixture.exit_p01();

    assert!(fixture.engine.open_content(&child_t1.version_id).is_err());
    assert_eq!(fixture.index(&fixture.status("")), &original_index);
    let ready = fixture.status("");
    assert!(!ready.needs_restage);
    let operation = op();
    let receipt = fixture.commit(&ready, operation).unwrap();
    let parent_head = receipt.result["head_tree_id"].as_str().unwrap().to_owned();
    let tree = &fixture.engine.state().directory_trees[&parent_head];
    assert_eq!(
        tree.directory_refs[&child_c1.directory_id].tree_id,
        child_c1.head_tree_id.as_ref().unwrap().as_str()
    );
    assert!(tree.entries.contains(&child_t1));
    assert!(tree.entries.contains(&own));
    assert_eq!(fixture.entry("child/file"), child_t2);
    assert_eq!(fixture.status("child").head_tree_id, child_c2.head_tree_id);
    assert_eq!(
        fixture.commit(&ready, operation).unwrap().result,
        receipt.result
    );

    fixture = fixture.reopen();
    let metadata = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(
        fixture.commit(&ready, operation).unwrap().result,
        receipt.result
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), metadata);
    assert_eq!(
        fixture.status("").head_tree_id.as_deref(),
        Some(parent_head.as_str())
    );
    assert_eq!(fixture.status("child").head_tree_id, child_c2.head_tree_id);
    assert_eq!(fixture.entry("child/file"), child_t2);
    assert_eq!(bytes(&fixture.engine, &child_t2), b"child C2");
}
