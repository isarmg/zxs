//! A detached file may retain branch HEADs as immutable terminal evidence.
//! This does not detach a current namespace, directory composition, or D node.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct RetainedFrontier {
    pub(super) branch_ids: Vec<String>,
    digest: String,
}

fn owns_node(state: &State, file: &str, node: &str) -> bool {
    node_version(state, node).and_then(|id| state.versions.get(id))
        .is_some_and(|version| version.file_id == file)
}

fn blocked() -> Error {
    Error::new(ErrorCode::ContentPinned,
        "terminal branch archival requires the file to be detached from namespace, directory, path contexts and deletion frontiers")
}

impl RetainedFrontier {
    pub(super) fn capture(state: &State, file: &str, history: &str) -> Result<Self> {
        if state.file_deletions.contains_key(file) { return Err(blocked()); }
        let mut branches = Vec::new();
        let mut heads = BTreeMap::new();
        let mut head_entries = 0;
        for branch in state.branches.values() {
            let commit = state.commits.get(&branch.head_commit_id).ok_or_else(invalid)?;
            if branch.file_id.as_deref() != Some(file) {
                if commit.entries.iter().any(|entry| entry.file_id == file)
                    || commit.pending_file_versions.contains_key(file)
                    || branch.pending_file_versions.contains_key(file) { return Err(blocked()); }
                continue;
            }
            if branches.len() == storage_contract::shared_history::MAX_SHARED_HISTORY_BRANCHES {
                return Err(Error::new(ErrorCode::ResourceLimit, "terminal branch frontier exceeds its complete branch budget"));
            }
            let head = branch.file_node_id.as_deref().or_else(|| {
                commit.entries.iter().find(|entry| entry.kind == crate::NamespaceEntryKind::File && entry.file_id == file)
                    .map(|entry| entry.version_id.as_str())
            });
            if branch.branch_kind != crate::BranchKind::File || branch.history_tree_id != history
                || commit.history_tree_id != history
                || !head.is_some_and(|node| owns_node(state, file, node))
                || commit.entries.iter().filter(|entry| entry.kind == crate::NamespaceEntryKind::File)
                    .any(|entry| entry.file_id != file || !owns_node(state, file, &entry.version_id))
                || commit.pending_file_versions.iter().chain(branch.pending_file_versions.iter())
                    .any(|(owner, version)| owner != file || !owns_node(state, file, version)) {
                return Err(invalid());
            }
            head_entries += commit.entries.len();
            if head_entries > 65_536 {
                return Err(Error::new(ErrorCode::ResourceLimit, "terminal branch HEAD records exceed their complete budget"));
            }
            heads.insert(&commit.commit_id, commit);
            branches.push(branch);
        }
        branches.sort_by(|left, right| left.branch_id.cmp(&right.branch_id));
        let ids = branches.iter().map(|branch| branch.branch_id.as_str()).collect::<BTreeSet<_>>();
        if state.file_selections.get(file).is_some_and(|branch| !ids.contains(branch.as_str()))
            || state.file_write_origins.get(file).is_some_and(|branch| !ids.contains(branch.as_str())) {
            return Err(invalid());
        }
        // A surviving path-local context can reselect a HEAD without changing
        // its body ID. It is not an archival branch record and must be detached.
        if state.file_contexts.values().any(|context| ids.contains(context.branch_id.as_str())
            || context.node_id.as_deref().is_some_and(|node| owns_node(state, file, node)))
            || state.directory_branches.values().any(|branch| {
                branch.entries.iter().any(|entry| entry.file_id == file)
                    || branch.file_branches.contains_key(file) || branch.inherited_files.contains_key(file)
            }) { return Err(blocked()); }
        let aliases = state.file_branch_aliases.iter().filter(|(source, target)| {
            ids.contains(source.as_str()) || ids.contains(target.as_str())
        }).collect::<BTreeMap<_, _>>();
        if aliases.iter().any(|(source, target)| !ids.contains(source.as_str()) || !ids.contains(target.as_str())) { return Err(invalid()); }
        let mut merges = Vec::new();
        for merge in state.file_branch_merges.values().filter(|merge| merge.file_id == file) {
            if merges.len() == MAX_SELECTED {
                return Err(Error::new(ErrorCode::ResourceLimit, "terminal merge frontier exceeds its complete event budget"));
            }
            if merge.history_tree_id != history || !ids.contains(merge.surviving_branch_id.as_str())
                || merge.input_branch_ids.iter().any(|branch| !ids.contains(branch.as_str()))
                || merge.parent_node_ids.iter().chain(&merge.input_node_ids)
                    .chain([&merge.chosen_node_id, &merge.result_node_id])
                    .any(|node| !owns_node(state, file, node)) { return Err(invalid()); }
            merges.push(merge);
        }
        Ok(Self {
            branch_ids: branches.iter().map(|branch| branch.branch_id.clone()).collect(),
            digest: crate::object_hash("terminal-file-retained-frontier", &json!({
                "file_id": file, "history_tree_id": history, "branches": branches,
                "head_commits": heads, "aliases": aliases, "merges": merges,
                "selection": state.file_selections.get(file),
                "write_origin": state.file_write_origins.get(file),
                "selection_generation": state.file_selection_generations.get(file),
            }))?,
        })
    }

    pub(super) fn validate(&self, state: &State, file: &str, history: &str) -> Result<()> {
        if self.branch_ids.len() > storage_contract::shared_history::MAX_SHARED_HISTORY_BRANCHES
            || self.branch_ids.windows(2).any(|pair| pair[0] >= pair[1]) { return Err(invalid()); }
        for id in &self.branch_ids { storage_contract::history_window::HistoryBranchId::parse(id)?; }
        HistoryRoot::parse(&self.digest)?;
        if *self != Self::capture(state, file, history)? { return Err(invalid()); }
        Ok(())
    }
}
