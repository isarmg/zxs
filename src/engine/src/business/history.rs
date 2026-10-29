//! History queries, lazy branches and metadata-only relations.
use super::*;
use crate::engine::MutationPlan;
use json_codec::json;
fn branch_contains_commit(state: &State, branch: &Branch, target: &str) -> Result<bool> {
    if branch.state == BranchState::Migrated {
        return Err(Error::new(
            storage_contract::ErrorCode::TreeMoved,
            "branch moved to another history",
        ));
    }
    let mut cursor = branch.head_commit_id.as_str();
    loop {
        let commit = state
            .commits
            .get(cursor)
            .expect("invariant: branch ancestry references existing commits");
        if cursor == target {
            return Ok(true);
        }
        match commit.parent_commit_ids.first() {
            Some(parent) => cursor = parent,
            None => return Ok(false),
        }
    }
}

impl Engine {
    /// The frozen readable parent of a file that has no branch-owned body yet.
    pub fn pending_file_source(&self, branch_id: &str, file_id: &str) -> Option<&str> {
        super::file::pending_file_source(&self.state, branch_id, file_id)
    }

    /// The complete materialized tree at the branch head, in the legacy
    /// canonical order. Whole-tree by definition; point lookups and listing
    /// pages go through the paged namespace view instead.
    pub fn list_tree(&self, branch_id: &str) -> Result<Vec<Entry>> {
        self.namespace_entries(&self.tree_commit(branch_id)?.namespace.clone())
    }

    /// An unwritten snapshot reads its frozen parent versions. Reservation
    /// markers separately distinguish inherited content from owned versions.
    pub fn list_tree_at(&self, branch_id: &str, commit_id: &str) -> Result<Vec<Entry>> {
        self.namespace_entries(&self.tree_commit_at(branch_id, commit_id)?.namespace.clone())
    }

    /// The branch head commit that owns the current frozen namespace.
    pub(crate) fn tree_commit(&self, branch_id: &str) -> Result<&Commit> {
        let branch = self.state.branches.get(branch_id).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::BranchNotFound,
                "branch not found",
            )
        })?;
        self.tree_commit_at(branch_id, &branch.head_commit_id)
    }

    /// A selected frozen snapshot within the branch's history. Like
    /// `list_tree_at`, rejects migrated branches and foreign snapshots.
    pub(crate) fn tree_commit_at(&self, branch_id: &str, commit_id: &str) -> Result<&Commit> {
        let branch = self.state.branches.get(branch_id).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::BranchNotFound,
                "branch not found",
            )
        })?;
        if branch.state == BranchState::Migrated {
            return Err(Error::new(
                storage_contract::ErrorCode::TreeMoved,
                "this branch moved during a history split; use the published mapping",
            ));
        }
        self.state
            .commits
            .get(commit_id)
            .filter(|commit| commit.history_tree_id == branch.history_tree_id)
            .ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::CommitNotFound,
                    "selected snapshot is outside this branch's history",
                )
            })
    }
    pub fn file_history(&self, file_id: &str) -> Result<Vec<FileVersion>> {
        if self.history_index_is_selective(file_id)
            && let Some(versions) = self.indexed_file_history(file_id)
        {
            return Ok(versions);
        }
        let mut versions = self
            .state
            .versions
            .values()
            .filter(|v| v.file_id == file_id)
            .cloned()
            .collect::<Vec<_>>();
        versions.sort_by_key(|v| v.version_sequence);
        Ok(versions)
    }
    /// Share all commit records of one history in commit-ID order.
    ///
    /// Uses only an already available index bound to the current State. Cold
    /// or detached States retain the full scan without building an index or
    /// reading namespace objects. Missing histories yield an empty collection;
    /// this projection grants no authorization and retains no global map root.
    pub fn history_commit_records(&self, history_id: &str) -> Vec<std::sync::Arc<Commit>> {
        if let Some(owners) = self.available_history_owners() {
            return owners
                .rows(history_id)
                .into_iter()
                .flat_map(|rows| rows.keys(crate::state_projection::Field::Commits))
                .map(|id| {
                    self.state
                        .commits
                        .get_shared(id)
                        .expect("invariant: bound history membership names an existing commit")
                })
                .collect();
        }
        self.state
            .commits
            .shared_values()
            .filter(|commit| commit.history_tree_id == history_id)
            .map(std::sync::Arc::clone)
            .collect()
    }

    pub fn graph(&self, history_id: &str) -> Result<HistoryGraph> {
        let history = self
            .state
            .histories
            .get(history_id)
            .cloned()
            .ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::HistoryNotFound,
                    "history tree not found",
                )
            })?;
        let mut commits = self
            .state
            .commits
            .values()
            .filter(|c| c.history_tree_id == history_id)
            .cloned()
            .collect::<Vec<_>>();
        commits.sort_by_key(|c| c.sequence);
        Ok(HistoryGraph {
            history,
            boundary_marks: self
                .state
                .history_boundaries
                .values()
                .filter(|b| b.history_tree_id == history_id)
                .cloned()
                .collect(),
            commits,
            branches: self
                .state
                .branches
                .values()
                .filter(|b| b.history_tree_id == history_id)
                .cloned()
                .collect(),
            merge_links: self
                .state
                .merge_links
                .values()
                .filter(|l| l.history_tree_id == history_id)
                .cloned()
                .collect(),
        })
    }
    pub fn create_branch(
        &mut self,
        source_commit: &str,
        name: &storage_contract::BranchName,
        principal: &str,
        operation: OperationId,
    ) -> Result<Branch> {
        let plan = self.plan_mutation_digest(
            "branch.create",
            principal,
            operation,
            json!({"source_commit_id":source_commit,"name":name.as_str()}),
            None,
            |engine, state, scratch| {
                let source = state.commits.get(source_commit).cloned().ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::CommitNotFound,
                        "source commit not found",
                    )
                })?;
                engine.repository_for_history(&source.history_tree_id)?;

                if state.branches.values().any(|branch| {
                    branch.history_tree_id == source.history_tree_id && branch.file_id.is_some()
                }) {
                    return Err(Error::new(
                        storage_contract::ErrorCode::FileBranchScope,
                        "use independent file branches in this history",
                    ));
                }
                if state.branches.values().any(|branch| {
                    branch.history_tree_id == source.history_tree_id
                        && name_policy::lookup_key(&branch.name) == name.key()
                }) {
                    return Err(Error::new(
                        storage_contract::ErrorCode::NameCollision,
                        "branch name already exists",
                    ));
                }
                let id = new_id("br");
                let commit_id = new_id("commit");
                // Whole-tree by semantics: a new branch head re-identifies
                // every entry, so the source namespace materializes once and
                // the branch builds its own complete paged root.
                let mut entries = engine.namespace_entries(&source.namespace)?;
                let pending_file_versions: std::collections::BTreeMap<String, String> = entries
                    .iter()
                    .filter(|entry| entry.kind == NamespaceEntryKind::File)
                    .map(|entry| (entry.file_id.clone(), entry.version_id.clone()))
                    .collect();
                let placeholder_files = pending_file_versions.len();
                for entry in &mut entries {
                    entry.entry_id = new_id("entry");
                }
                let namespace =
                    crate::namespace::build_from_entries(&entries, &mut scratch.namespace)?;
                state.commits.insert(
                    commit_id.clone(),
                    Commit {
                        namespace_deletions: None,
                        commit_id: commit_id.clone(),
                        history_tree_id: source.history_tree_id.clone(),
                        kind: "branch_pending".into(),
                        parent_commit_ids: vec![source_commit.into()],
                        tree_id: new_id("tree"),
                        sequence: state.sequence,
                        operation_id: operation,
                        author: principal.into(),
                        message: format!("Create pending branch {}", name.as_str()),
                        namespace,
                        pending_file_versions: pending_file_versions.clone(),
                        provenance: Some(source_commit.into()),
                    },
                );
                let created_branch = Branch {
                    file_node_id: None,
                    branch_id: id.clone(),
                    history_tree_id: source.history_tree_id.clone(),
                    name: name.as_str().into(),
                    head_commit_id: commit_id.clone(),
                    fork_commit_id: source_commit.into(),
                    branch_kind: BranchKind::Full,
                    file_id: None,
                    pending_file_versions,
                    generation: 1,
                    state: BranchState::Active,
                };
                state.branches.insert(id.clone(), created_branch.clone());
                Ok(json!({
                    "branch_id": id,
                    "commit_id": commit_id,
                    "history_tree_id": source.history_tree_id,
                    "created_branch": created_branch,
                    "copied_files": "0",
                    "placeholder_files": placeholder_files.to_string()
                }))
            },
        )?;
        match plan {
            MutationPlan::Existing(op) => {
                // Exact replay returns the frozen record, not the branch's later state.
                json_codec::from_value(op.result["created_branch"].clone()).map_err(|source| {
                    Error::from(source)
                        .context(ErrorCode::StateInvalid, "created branch receipt is invalid")
                })
            }
            MutationPlan::Prepared {
                next,
                prepared,
                namespace,
                result,
            } => {
                // The closure stores this same record in the candidate and receipt.
                let created = result
                    .branch_id
                    .as_deref()
                    .and_then(|id| next.branches.get(id))
                    .expect("invariant: branch creation candidate contains its created branch")
                    .clone();
                self.persist_with_objects(*next, prepared, namespace, result)?;
                Ok(created)
            }
        }
    }
    pub fn add_merge_link(
        &mut self,
        source_branch: &str,
        source_commit: &str,
        target_branch: &str,
        target_commit: &str,
        principal: &str,
        operation: OperationId,
    ) -> Result<MergeLink> {
        let op = self.mutate(
            "merge-link.create",
            principal,
            operation,
            json!({
                "source_branch_id": source_branch,
                "source_commit_id": source_commit,
                "target_branch_id": target_branch,
                "target_commit_id": target_commit
            }),
            |engine, state, _| {
                let sb = state.branches.get(source_branch).ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::BranchNotFound,
                        "source branch missing",
                    )
                })?;
                let tb = state.branches.get(target_branch).ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::BranchNotFound,
                        "target branch missing",
                    )
                })?;
                if sb.history_tree_id != tb.history_tree_id {
                    return Err(Error::new(
                        storage_contract::ErrorCode::HistoryMismatch,
                        "merge endpoints must belong to one history",
                    ));
                }
                if sb.file_id != tb.file_id && (sb.file_id.is_some() || tb.file_id.is_some()) {
                    return Err(Error::new(
                        storage_contract::ErrorCode::FileBranchScope,
                        "file branch relations must identify the same file",
                    ));
                }
                engine.repository_for_history(&sb.history_tree_id)?;
                for id in [source_commit, target_commit] {
                    if !state
                        .commits
                        .get(id)
                        .is_some_and(|c| c.history_tree_id == sb.history_tree_id)
                    {
                        return Err(Error::new(
                            storage_contract::ErrorCode::CommitNotFound,
                            "selected endpoint is not in the history",
                        ));
                    }
                }
                if source_branch == target_branch
                    || !branch_contains_commit(state, sb, source_commit)?
                    || !branch_contains_commit(state, tb, target_commit)?
                {
                    return Err(Error::new(
                        storage_contract::ErrorCode::MergeEndpointMismatch,
                        "select one existing commit from each of two distinct branches",
                    ));
                }
                let link = MergeLink {
                    merge_link_id: new_id("link"),
                    history_tree_id: sb.history_tree_id.clone(),
                    source_branch_id: source_branch.into(),
                    source_commit_id: source_commit.into(),
                    target_branch_id: target_branch.into(),
                    target_commit_id: target_commit.into(),
                    author: principal.into(),
                    operation_id: operation,
                };
                let id = link.merge_link_id.clone();
                let history = link.history_tree_id.clone();
                state.merge_links.insert(id.clone(), link);
                Ok(json!({"merge_link_id":id,"history_tree_id":history}))
            },
        )?;
        // A later split can remap the link or replace it with audit boundaries.
        // Reconstruct the original response only after mutate has matched the
        // immutable receipt to this exact request and actor.
        Ok(MergeLink {
            merge_link_id: op.result["merge_link_id"]
                .as_str()
                .ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::StateInvalid,
                        "merge-link receipt identity missing",
                    )
                })?
                .into(),
            history_tree_id: op.history_tree_id.ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::StateInvalid,
                    "merge-link receipt history missing",
                )
            })?,
            source_branch_id: source_branch.into(),
            source_commit_id: source_commit.into(),
            target_branch_id: target_branch.into(),
            target_commit_id: target_commit.into(),
            author: principal.into(),
            operation_id: operation,
        })
    }
    pub fn archive_branch(
        &mut self,
        id: &str,
        principal: &str,
        operation: OperationId,
    ) -> Result<OperationResult> {
        self.mutate(
            "branch.archive",
            principal,
            operation,
            json!({"branch_id":id}),
            |engine, state, _| {
                let repo = engine.repository_for_branch(id)?;

                if repo.main_branch_id == id {
                    return Err(Error::new(
                        storage_contract::ErrorCode::MainBranchRequired,
                        "main branch cannot be archived while shared",
                    ));
                }
                if state
                    .file_selections
                    .values()
                    .any(|selected| selected == id)
                    || state
                        .file_contexts
                        .values()
                        .any(|context| context.branch_id == id)
                {
                    return Err(Error::new(
                        storage_contract::ErrorCode::FileBranchSelected,
                        "select another file branch before archiving this branch",
                    ));
                }
                if engine.workspaces.values().any(|w| {
                    w.branch_id == id
                        && matches!(w.state, WorkspaceState::Open | WorkspaceState::Sealed)
                }) {
                    return Err(Error::new(
                        storage_contract::ErrorCode::WorkspaceBusy,
                        "branch has an open workspace",
                    ));
                }
                let b = state.branches.get_mut(id).ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::BranchNotFound,
                        "branch missing",
                    )
                })?;
                b.state = BranchState::Archived;
                b.generation = b.generation.checked_add(1).ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::ResourceLimit,
                        "branch generation exhausted",
                    )
                })?;
                Ok(json!({"branch_id":id,"history_tree_id":b.history_tree_id}))
            },
        )
    }

    pub fn compare_commits(&self, left: &str, right: &str) -> Result<CommitComparison> {
        let left = self.state.commits.get_shared(left).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::CommitNotFound,
                "left commit missing",
            )
        })?;
        let right = self.state.commits.get_shared(right).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::CommitNotFound,
                "right commit missing",
            )
        })?;
        // Comparison is whole-namespace by definition; both sides materialize
        // once and the comparison owns the compared records.
        let left_entries = self.namespace_entries(&left.namespace)?;
        let right_entries = self.namespace_entries(&right.namespace)?;
        Ok(CommitComparison::between(
            left,
            left_entries,
            right,
            right_entries,
        ))
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/business/history/replay_tests.rs"]
mod replay_tests;
