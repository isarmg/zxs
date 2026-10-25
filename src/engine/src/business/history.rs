//! History queries, lazy branches and metadata-only relations.
use super::*;
use json_codec::json;
fn branch_contains_commit(state: &State, branch: &Branch, target: &str) -> Result<bool> {
    if branch.state == "migrated" {
        return Err(Error::new("TREE_MOVED", "branch moved to another history"));
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut cursor = branch.head_commit_id.as_str();
    loop {
        if !seen.insert(cursor) {
            return Err(Error::new("STATE_INVALID", "history parent cycle"));
        }
        let commit = state.commits.get(cursor).ok_or_else(|| {
            Error::new("STATE_INVALID", "branch ancestry contains an absent commit")
        })?;
        if commit.history_tree_id != branch.history_tree_id || commit.parent_commit_ids.len() > 1 {
            return Err(Error::new(
                "STATE_INVALID",
                "branch ancestry violates single history and parent invariants",
            ));
        }
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

    pub fn list_tree(&self, branch_id: &str) -> Result<Vec<Entry>> {
        Ok(self.list_tree_ref(branch_id)?.to_vec())
    }

    /// Borrow the current frozen namespace without copying unrelated entries.
    pub fn list_tree_ref(&self, branch_id: &str) -> Result<&[Entry]> {
        let branch = self
            .state
            .branches
            .get(branch_id)
            .ok_or_else(|| Error::new("BRANCH_NOT_FOUND", "branch not found"))?;
        self.list_tree_at_ref(branch_id, &branch.head_commit_id)
    }

    /// An unwritten snapshot reads its frozen parent versions. Reservation
    /// markers separately distinguish inherited content from owned versions.
    pub fn list_tree_at(&self, branch_id: &str, commit_id: &str) -> Result<Vec<Entry>> {
        Ok(self.list_tree_at_ref(branch_id, commit_id)?.to_vec())
    }

    fn list_tree_at_ref(&self, branch_id: &str, commit_id: &str) -> Result<&[Entry]> {
        let branch = self
            .state
            .branches
            .get(branch_id)
            .ok_or_else(|| Error::new("BRANCH_NOT_FOUND", "branch not found"))?;
        if branch.state == "migrated" {
            return Err(Error::new(
                "TREE_MOVED",
                "this branch moved during a history split; use the published mapping",
            ));
        }
        let commit = self
            .state
            .commits
            .get(commit_id)
            .filter(|commit| commit.history_tree_id == branch.history_tree_id)
            .ok_or_else(|| {
                Error::new(
                    "COMMIT_NOT_FOUND",
                    "selected snapshot is outside this branch's history",
                )
            })?;
        Ok(&commit.entries)
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
    pub fn graph(&self, history_id: &str) -> Result<HistoryGraph> {
        let history = self
            .state
            .histories
            .get(history_id)
            .cloned()
            .ok_or_else(|| Error::new("HISTORY_NOT_FOUND", "history tree not found"))?;
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
        name: &str,
        principal: &str,
        operation: &str,
    ) -> Result<Branch> {
        super::validate_branch_name(name)?;
        let op = self.mutate(
            "branch.create",
            principal,
            operation,
            json!({"source_commit_id":source_commit,"name":name}),
            |engine, state, _| {
                let source = state
                    .commits
                    .get(source_commit)
                    .cloned()
                    .ok_or_else(|| Error::new("COMMIT_NOT_FOUND", "source commit not found"))?;
                engine.repository_for_history(&source.history_tree_id)?;

                if state.branches.values().any(|branch| {
                    branch.history_tree_id == source.history_tree_id && branch.file_id.is_some()
                }) {
                    return Err(Error::new(
                        "FILE_BRANCH_SCOPE",
                        "use independent file branches in this history",
                    ));
                }
                if state.branches.values().any(|branch| {
                    branch.history_tree_id == source.history_tree_id
                        && name_policy::lookup_key(&branch.name) == name_policy::lookup_key(name)
                }) {
                    return Err(Error::new("NAME_COLLISION", "branch name already exists"));
                }
                let id = new_id("br");
                let commit_id = new_id("commit");
                let mut entries = source.entries.clone();
                let pending_file_versions: std::collections::BTreeMap<String, String> = entries
                    .iter()
                    .filter(|entry| entry.kind == "file")
                    .map(|entry| (entry.file_id.clone(), entry.version_id.clone()))
                    .collect();
                for entry in &mut entries {
                    entry.entry_id = new_id("entry");
                }
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
                        operation_id: operation.into(),
                        author: principal.into(),
                        message: format!("Create pending branch {name}"),
                        entries,
                        pending_file_versions: pending_file_versions.clone(),
                        provenance: Some(source_commit.into()),
                    },
                );
                state.branches.insert(
                    id.clone(),
                    Branch {
                        file_node_id: None,
                        branch_id: id.clone(),
                        history_tree_id: source.history_tree_id.clone(),
                        name: name.into(),
                        head_commit_id: commit_id.clone(),
                        fork_commit_id: source_commit.into(),
                        branch_kind: "full".into(),
                        file_id: None,
                        pending_file_versions,
                        generation: 1,
                        state: "active".into(),
                    },
                );
                Ok(json!({
                    "branch_id": id,
                    "commit_id": commit_id,
                    "history_tree_id": source.history_tree_id,
                    "copied_files": "0",
                    "placeholder_files": source.entries.iter()
                        .filter(|entry| entry.kind == "file").count().to_string()
                }))
            },
        )?;
        self.state
            .branches
            .get(op.branch_id.as_deref().unwrap_or(""))
            .cloned()
            .ok_or_else(|| Error::new("STATE_INVALID", "branch result missing"))
    }
    pub fn add_merge_link(
        &mut self,
        source_branch: &str,
        source_commit: &str,
        target_branch: &str,
        target_commit: &str,
        principal: &str,
        operation: &str,
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
                let sb = state
                    .branches
                    .get(source_branch)
                    .ok_or_else(|| Error::new("BRANCH_NOT_FOUND", "source branch missing"))?;
                let tb = state
                    .branches
                    .get(target_branch)
                    .ok_or_else(|| Error::new("BRANCH_NOT_FOUND", "target branch missing"))?;
                if sb.history_tree_id != tb.history_tree_id {
                    return Err(Error::new(
                        "HISTORY_MISMATCH",
                        "merge endpoints must belong to one history",
                    ));
                }
                if sb.file_id != tb.file_id && (sb.file_id.is_some() || tb.file_id.is_some()) {
                    return Err(Error::new(
                        "FILE_BRANCH_SCOPE",
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
                            "COMMIT_NOT_FOUND",
                            "selected endpoint is not in the history",
                        ));
                    }
                }
                if source_branch == target_branch
                    || !branch_contains_commit(state, sb, source_commit)?
                    || !branch_contains_commit(state, tb, target_commit)?
                {
                    return Err(Error::new(
                        "MERGE_ENDPOINT_MISMATCH",
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
                    operation_id: operation.into(),
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
                .ok_or_else(|| Error::new("STATE_INVALID", "merge-link receipt identity missing"))?
                .into(),
            history_tree_id: op
                .history_tree_id
                .ok_or_else(|| Error::new("STATE_INVALID", "merge-link receipt history missing"))?,
            source_branch_id: source_branch.into(),
            source_commit_id: source_commit.into(),
            target_branch_id: target_branch.into(),
            target_commit_id: target_commit.into(),
            author: principal.into(),
            operation_id: operation.into(),
        })
    }
    pub fn archive_branch(
        &mut self,
        id: &str,
        principal: &str,
        operation: &str,
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
                        "MAIN_BRANCH_REQUIRED",
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
                        "FILE_BRANCH_SELECTED",
                        "select another file branch before archiving this branch",
                    ));
                }
                if engine
                    .workspaces
                    .values()
                    .any(|w| w.branch_id == id && matches!(w.state.as_str(), "open" | "sealed"))
                {
                    return Err(Error::new("WORKSPACE_BUSY", "branch has an open workspace"));
                }
                let b = state
                    .branches
                    .get_mut(id)
                    .ok_or_else(|| Error::new("BRANCH_NOT_FOUND", "branch missing"))?;
                b.state = "archived".into();
                b.generation = b
                    .generation
                    .checked_add(1)
                    .ok_or_else(|| Error::new("RESOURCE_LIMIT", "branch generation exhausted"))?;
                Ok(json!({"branch_id":id,"history_tree_id":b.history_tree_id}))
            },
        )
    }

    pub fn compare_commits(&self, left: &str, right: &str) -> Result<json_codec::Value> {
        let a = self
            .state
            .commits
            .get(left)
            .ok_or_else(|| Error::new("COMMIT_NOT_FOUND", "left commit missing"))?;
        let b = self
            .state
            .commits
            .get(right)
            .ok_or_else(|| Error::new("COMMIT_NOT_FOUND", "right commit missing"))?;
        fn index(entries: &[Entry]) -> std::collections::BTreeMap<&str, &Entry> {
            entries
                .iter()
                .map(|entry| (entry.lookup_key.as_str(), entry))
                .collect()
        }
        let left_entries = index(&a.entries);
        let right_entries = index(&b.entries);
        let mut changes = Vec::new();
        for e in &a.entries {
            match right_entries.get(e.lookup_key.as_str()) {
                None => changes.push(json!({"path":e.path,"change":"removed","before":e})),
                Some(other) if e.version_id != other.version_id || e.path != other.path => changes
                    .push(json!({"path":other.path,"change":"modified","before":e,"after":other})),
                _ => {}
            }
        }
        for e in &b.entries {
            if !left_entries.contains_key(e.lookup_key.as_str()) {
                changes.push(json!({"path":e.path,"change":"added","after":e}));
            }
        }
        Ok(json!({"left":left,"right":right,"changes":changes}))
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/business/history/replay_tests.rs"]
mod replay_tests;
