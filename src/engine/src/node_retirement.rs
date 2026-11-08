//! Authenticated live-node retirement. Old descriptors are retained as proof,
//! not reusable live nodes. This is not erasure of immutable audit metadata.
use crate::{ContentState, Engine, Error, ErrorCode, OperationResult, Result, State};
use data_serialization::{Deserialize, Serialize};
use json_codec::json;
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::history_window::{HistoryFileId, HistoryRoot, HistoryTreeId, HistoryVersionId};

mod publication;
mod frontier;
mod references;
mod replicas;
pub(crate) use replicas::{CleanupRoutes, original_cleanup_bindings};
mod separation;
mod terminal;
pub use separation::PermanentNodeSeparation;
pub(crate) use publication::{admit_current, verify};
pub(crate) const KIND: &str = "file.nodes.retire";
const MAX_SELECTED: usize = storage_contract::shared_history::MAX_SHARED_HISTORY_NODES;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Request {
    pub(crate) history_tree_id: String,
    pub(crate) history_root: String,
    pub(crate) origin_file_id: String,
    pub(crate) version_ids: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Decision {
    pub(crate) request: Request,
    archive_digest: String,
    inbound_references: references::ReferenceClosure,
    separation: separation::Separation,
    pub(crate) cleanup_routes: CleanupRoutes,
    file_terminations: terminal::TerminalFiles,
}

fn invalid() -> Error {
    Error::new(ErrorCode::PublicationInvalid, "node retirement differs from its permanent authenticated decision")
}
fn pinned() -> Error {
    Error::new(ErrorCode::ContentPinned, "node retirement still has a live HEAD, merge, deletion, or directory reference")
}
pub(crate) fn is_retired(state: &State, version: &str) -> bool {
    state.content_states.get(version) == Some(&ContentState::Retired)
}
/// This is an early refusal on an already admitted State. Durable authorization
/// comes from the exact terminal receipt closure, never from this predicate.
pub(crate) fn is_terminal_file(state: &State, file: &str) -> bool {
    let mut found = false;
    for version in state.versions.values().filter(|version| version.file_id == file) {
        if !is_retired(state, &version.version_id) { return false; }
        found = true;
    }
    found
}
impl Decision {
    pub(crate) fn archived_branch_ids(&self) -> BTreeSet<String> {
        self.file_terminations.files.iter().flat_map(|file| file.retained_frontier.branch_ids.iter().cloned()).collect()
    }
}
pub(crate) fn prospective_archived_branches(state: &State, file: &str, history: &str) -> Result<Vec<String>> {
    Ok(frontier::RetainedFrontier::capture(state, file, history)?.branch_ids)
}
pub(crate) fn node_version<'a>(state: &'a State, node: &'a str) -> Option<&'a str> {
    state.versions.get(node).map(|v| v.version_id.as_str())
        .or_else(|| state.file_path_nodes.get(node).map(|n| n.version_id.as_str()))
        .or_else(|| state.file_metadata_nodes.get(node).map(|n| n.version_id.as_str()))
}
pub(crate) fn node_is_retired(state: &State, node: &str) -> bool {
    node_version(state, node).is_some_and(|version| is_retired(state, version))
}
pub(crate) fn history_has_retirements(state: &State, history: &str) -> bool {
    state.content_states.iter().any(|(id, status)| {
        *status == ContentState::Retired && state.versions.get(id).is_some_and(|v| v.history_tree_id == history)
    })
}
pub(crate) fn require_portable_history(state: &State, history: &str) -> Result<()> {
    if history_has_retirements(state, history) {
        return Err(Error::new(ErrorCode::HistoryPackageInvalid,
            "retired-node histories require full native receipts; portable copy/split/export is not supported"));
    }
    Ok(())
}

impl Request {
    fn validate(&self) -> Result<()> {
        HistoryTreeId::parse(&self.history_tree_id)?;
        HistoryRoot::parse(&self.history_root)?;
        HistoryFileId::parse(&self.origin_file_id)?;
        if self.version_ids.is_empty() || self.version_ids.len() > MAX_SELECTED
            || self.version_ids.windows(2).any(|ids| ids[0] >= ids[1]) {
            return Err(invalid());
        }
        for id in &self.version_ids { HistoryVersionId::parse(id)?; }
        Ok(())
    }
}

/// The retained records authenticate old commits/sources and tombstones. A
/// later mutation cannot edit, delete, or add coupled events behind this digest.
fn archive_digest(state: &State, request: &Request) -> Result<String> {
    let selected = request.version_ids.iter().collect::<BTreeSet<_>>();
    let versions = request.version_ids.iter().map(|id| {
        state.versions.get(id).filter(|v| v.history_tree_id == request.history_tree_id).ok_or_else(invalid)
    }).collect::<Result<Vec<_>>>()?;
    let paths = state.file_path_nodes.values().filter(|n| selected.contains(&n.version_id)).collect::<Vec<_>>();
    let metadata = state.file_metadata_nodes.values().filter(|n| selected.contains(&n.version_id)).collect::<Vec<_>>();
    if versions.len() + paths.len() + metadata.len() > MAX_SELECTED { return Err(invalid()); }
    let event_ids = paths.iter().map(|n| n.node_id.as_str()).chain(metadata.iter().map(|n| n.node_id.as_str())).collect::<BTreeSet<_>>();
    let event_notes = state.file_path_notes.iter().filter(|(id, _)| event_ids.contains(id.as_str())).collect::<BTreeMap<_, _>>();
    let parents = request.version_ids.iter().filter_map(|id| state.file_node_parents.get(id).map(|p| (id, p))).collect::<BTreeMap<_, _>>();
    let notes = request.version_ids.iter().filter_map(|id| state.version_notes.get(id).map(|n| (id, n))).collect::<BTreeMap<_, _>>();
    Ok(crate::object_hash("retired-node-archive", &json!({
        "versions": versions, "paths": paths, "metadata": metadata, "parents": parents, "notes": notes, "event_notes": event_notes
    }))?)
}

pub(crate) fn decide(state: &State, request: Request) -> Result<Decision> {
    request.validate()?;
    for id in &request.version_ids {
        let version = state.versions.get(id).ok_or_else(invalid)?;
        if version.history_tree_id != request.history_tree_id
            || !matches!(state.content_states.get(id), Some(ContentState::Available | ContentState::Purged))
            || state.file_deletions.contains_key(&version.file_id) {
            return Err(pinned());
        }
    }
    if crate::history_package::retirement_history_root(state, &request.history_tree_id)? != request.history_root {
        return Err(Error::new(ErrorCode::HistoryChanged, "retirement source history root changed"));
    }
    Ok(Decision {
        archive_digest: archive_digest(state, &request)?,
        inbound_references: references::capture(state, &request)?,
        separation: separation::Separation::capture(state, &request)?,
        cleanup_routes: CleanupRoutes::capture(state, &request)?,
        file_terminations: terminal::TerminalFiles::capture(state, &request)?,
        request,
    })
}

pub(crate) fn decision(receipt: &OperationResult) -> Result<Decision> {
    json_codec::from_value(receipt.result.get("retirement").ok_or_else(invalid)?.clone()).map_err(Into::into)
}

/// Published snapshot closure. Only exact terminal branch frontiers admitted
/// above are archival HEADs; all current namespace/directory/D roots stay live
/// and may not refer to retired bodies. Original proofs remain untouched.
fn require_live_references(state: &State, terminals: &BTreeSet<String>, archived: &BTreeSet<String>) -> Result<()> {
    for branch in state.branches.values() {
        if archived.contains(&branch.branch_id) { continue; }
        if branch.file_node_id.as_deref().is_some_and(|id| node_is_retired(state, id))
            || branch.pending_file_versions.values().any(|id| is_retired(state, id))
            || state.commits.get(&branch.head_commit_id).is_some_and(|commit| {
                commit.entries.iter().any(|e| is_retired(state, &e.version_id))
                    || commit.pending_file_versions.values().any(|id| is_retired(state, id))
            }) {
            return Err(pinned());
        }
    }
    for branch in state.directory_branches.values() {
        if branch.entries.iter().any(|e| is_retired(state, &e.version_id)) { return Err(pinned()); }
    }
    if state.file_contexts.values().any(|context| context.node_id.as_deref().is_some_and(|id| node_is_retired(state, id))) {
        return Err(pinned());
    }
    // Current directory composition can pin an older child tree even after the
    // child's own mutable HEAD advances. Follow frozen references, not today's
    // child branch, and include explicit retained tree selections.
    let mut trees = state.directory_branches.values().map(|branch| branch.head_tree_id.as_str())
        .chain(state.directory_tree_selections.values().map(String::as_str)).collect::<BTreeSet<_>>();
    let mut visited = BTreeSet::new();
    while let Some(id) = trees.pop_first() {
        if !visited.insert(id) { continue; }
        let tree = state.directory_trees.get(id).ok_or_else(invalid)?;
        if tree.entries.iter().any(|entry| is_retired(state, &entry.version_id))
            || tree.file_contexts.values().any(|context| context.node_id.as_deref().is_some_and(|node| node_is_retired(state, node))) {
            return Err(pinned());
        }
        trees.extend(tree.directory_refs.values().map(|reference| reference.tree_id.as_str()));
    }
    for node in state.file_deletions.values() {
        if node.parent_node_ids.iter().chain(node.branch_heads.values()).any(|id| node_is_retired(state, id)) { return Err(pinned()); }
    }
    for merge in state.file_branch_merges.values() {
        if terminals.contains(&merge.file_id) { continue; }
        if node_is_retired(state, &merge.result_node_id)
            || merge.parent_node_ids.iter().chain(&merge.input_node_ids).chain(std::iter::once(&merge.chosen_node_id)).any(|id| node_is_retired(state, id)) {
            return Err(pinned());
        }
    }
    let retired = state.content_states.iter().filter(|(_, s)| **s == ContentState::Retired).map(|(id, _)| id).collect::<BTreeSet<_>>();
    if !crate::business::directory_git::protected_git_versions(state, &retired).is_empty() { return Err(pinned()); }
    Ok(())
}

/// Project through retired temporal nodes, retaining the nearest live parents.
/// The caller supplies its scan-wide budget/cancellation. IDs are borrowed from
/// authenticated records, not manufactured or written back into source proofs.
pub(crate) fn live_parents<'a>(
    state: &'a State, parents: &'a [String], mut step: impl FnMut() -> Result<()>,
) -> Result<Vec<&'a str>> {
    const MAX_FRONTIER: usize = 65_536;
    let mut todo = Vec::new();
    for parent in parents {
        step()?;
        if todo.len() == MAX_FRONTIER { return Err(Error::new(ErrorCode::ResourceLimit, "retired ancestry frontier is too large")); }
        todo.push(parent.as_str());
    }
    let mut seen = BTreeSet::new();
    let mut live = BTreeSet::new();
    while let Some(id) = todo.pop() {
        step()?;
        if seen.len() == MAX_FRONTIER { return Err(Error::new(ErrorCode::ResourceLimit, "retired ancestry lookup is too large")); }
        if !seen.insert(id) { continue; }
        if !node_is_retired(state, id) { live.insert(id); continue; }
        let parents = if let Some(n) = state.file_path_nodes.get(id) { &n.parent_node_ids }
            else if let Some(n) = state.file_metadata_nodes.get(id) { &n.parent_node_ids }
            else { let v = &state.versions[id]; state.file_node_parents.get(id).unwrap_or(&v.source_version_ids) };
        for parent in parents {
            step()?;
            if todo.len() == MAX_FRONTIER { return Err(Error::new(ErrorCode::ResourceLimit, "retired ancestry frontier is too large")); }
            todo.push(parent.as_str());
        }
    }
    Ok(live.into_iter().collect())
}

#[cfg(test)]
#[path = "../../../test/engine/unit/node_retirement_tests.rs"]
mod tests;
