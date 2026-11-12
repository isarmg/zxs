//! Request-local ancestry over one admitted immutable State. No cross-generation
//! cache: nodes and distinct edges are indexed once, including merge provenance.
use crate::{Error, Result, State};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub(crate) const MAX_ANCESTRY_NODES: usize = 65_536;
pub(crate) const MAX_ANCESTRY_EDGES: usize = 262_144;
pub(crate) const MAX_ANCESTRY_HEADERS: usize = 1_048_576;
const MAX_INDEX_PARENT_RECORDS: usize = 1_048_576;

pub(crate) fn exhausted() -> Error {
    Error::new(
        storage_contract::ErrorCode::ResourceLimit,
        "historical merge ancestry exceeds its bounded lookup budget",
    )
}

/// A shared request work budget. Each search charges a vertex once and each
/// distinct outgoing edge once; targets sharing a head share the same search.
pub(crate) struct SearchBudget {
    nodes: usize,
    edges: usize,
}
impl SearchBudget {
    pub(crate) fn read() -> Self {
        Self {
            nodes: MAX_ANCESTRY_NODES,
            edges: MAX_ANCESTRY_EDGES,
        }
    }
    pub(crate) fn write() -> Self {
        Self {
            nodes: MAX_ANCESTRY_NODES * 4,
            edges: MAX_ANCESTRY_EDGES * 4,
        }
    }
    fn node(&mut self) -> Result<()> {
        if self.nodes == 0 {
            return Err(exhausted());
        }
        self.nodes -= 1;
        Ok(())
    }
    fn edge(&mut self) -> Result<()> {
        if self.edges == 0 {
            return Err(exhausted());
        }
        self.edges -= 1;
        Ok(())
    }
}

pub(crate) struct FileAncestry<'a> {
    parents: BTreeMap<&'a str, BTreeSet<&'a str>>,
}
struct Builder<'a> {
    parents: BTreeMap<&'a str, BTreeSet<&'a str>>,
    edges: usize,
    headers: usize,
    parent_records: usize,
}
impl<'a> Builder<'a> {
    fn header(&mut self) -> Result<()> {
        if self.headers == MAX_ANCESTRY_HEADERS {
            return Err(exhausted());
        }
        self.headers += 1;
        Ok(())
    }
    fn node(&mut self, id: &'a str) -> Result<()> {
        if self.parents.len() == MAX_ANCESTRY_NODES {
            return Err(exhausted());
        }
        self.parents.insert(id, BTreeSet::new());
        Ok(())
    }
    fn parent_record(&mut self) -> Result<()> {
        if self.parent_records == MAX_INDEX_PARENT_RECORDS {
            return Err(exhausted());
        }
        self.parent_records += 1;
        Ok(())
    }
    fn edge(&mut self, child: &'a str, parent: &'a str) -> Result<()> {
        self.parent_record()?;
        // Scope selection matches public history projection. The admitted graph
        // owns these edges; cross-file fixture members are never source proofs.
        if !self.parents.contains_key(parent) {
            return Ok(());
        }
        // HistoryManifest admission resolves each merge result and verifies
        // its file/history against the event (history_package/manifest.rs).
        // Base-edge children come directly from this index's admitted keys.
        let parents = self
            .parents
            .get_mut(child)
            .expect("invariant: indexed edge child belongs to this file");
        if !parents.contains(parent) {
            if self.edges == MAX_ANCESTRY_EDGES {
                return Err(exhausted());
            }
            parents.insert(parent);
            self.edges += 1;
        }
        Ok(())
    }
}
impl<'a> FileAncestry<'a> {
    pub(crate) fn new(
        state: &'a State,
        file: &str,
        history: &str,
        mut checkpoint: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        checkpoint()?;
        let mut builder = Builder {
            parents: BTreeMap::new(),
            edges: 0,
            headers: 0,
            parent_records: 0,
        };
        for version in state.versions.values() {
            checkpoint()?;
            builder.header()?;
            if version.file_id == file && version.history_tree_id == history {
                builder.node(&version.version_id)?;
            }
        }
        for node in state.file_path_nodes.values() {
            checkpoint()?;
            builder.header()?;
            if node.file_id == file && node.history_tree_id == history {
                builder.node(&node.node_id)?;
            }
        }
        for node in state.file_metadata_nodes.values() {
            checkpoint()?;
            builder.header()?;
            if node.file_id == file && node.history_tree_id == history {
                builder.node(&node.node_id)?;
            }
        }
        // Borrow only scoped node IDs. Their count was admitted above, before
        // allocating this finite lookup list. No per-node merge-table scan.
        let nodes = builder.parents.keys().copied().collect::<Vec<_>>();
        for id in nodes {
            checkpoint()?;
            let parents = if let Some(node) = state.file_metadata_nodes.get(id) {
                &node.parent_node_ids
            } else if let Some(node) = state.file_path_nodes.get(id) {
                &node.parent_node_ids
            } else {
                let version = &state.versions[id];
                state
                    .file_node_parents
                    .get(id)
                    .unwrap_or(&version.source_version_ids)
            };
            for parent in parents {
                checkpoint()?;
                builder.edge(id, parent)?;
            }
        }
        for event in state.file_branch_merges.values() {
            checkpoint()?;
            builder.header()?;
            if event.file_id != file || event.history_tree_id != history {
                continue;
            }
            for parent in &event.parent_node_ids {
                checkpoint()?;
                if parent == &event.result_node_id {
                    // Reused result self-members are not graph edges, but still
                    // consume scan work even if a malformed fixture repeats one.
                    builder.parent_record()?;
                } else {
                    builder.edge(&event.result_node_id, parent)?;
                }
            }
        }
        Ok(Self {
            parents: builder.parents,
        })
    }
    pub(crate) fn node(&self, id: &str) -> Option<&'a str> {
        self.parents.get_key_value(id).map(|(id, _)| *id)
    }

    /// All candidate heads enter one queue before any ancestry is expanded.
    /// Shared ancestors have one origin proof, so diamonds never replay and an
    /// early candidate cannot consume an independent per-branch search budget.
    pub(crate) fn source_branch<'b>(
        &self,
        source: &str,
        heads: impl IntoIterator<Item = (&'b str, &'b str)>,
        budget: &mut SearchBudget,
        mut checkpoint: impl FnMut() -> Result<()>,
    ) -> Result<Option<&'b str>> {
        let mut seen = BTreeSet::new();
        let mut queue = VecDeque::new();
        for (head, branch) in heads {
            checkpoint()?;
            let Some(head) = self.node(head) else {
                continue;
            };
            if head == source {
                return Ok(Some(branch));
            }
            if seen.insert(head) {
                budget.node()?;
                queue.push_back((head, branch));
            }
        }
        while let Some((node, branch)) = queue.pop_front() {
            checkpoint()?;
            for &parent in &self.parents[node] {
                checkpoint()?;
                budget.edge()?;
                if parent == source {
                    return Ok(Some(branch));
                }
                if seen.insert(parent) {
                    budget.node()?;
                    queue.push_back((parent, branch));
                }
            }
        }
        Ok(None)
    }

    /// One traversal for all sources requested against this exact reviewed head.
    pub(crate) fn contains_all(
        &self,
        head: &str,
        targets: &BTreeSet<&str>,
        budget: &mut SearchBudget,
        mut checkpoint: impl FnMut() -> Result<()>,
    ) -> Result<bool> {
        let Some(head) = self.node(head) else {
            return Ok(false);
        };
        let mut remaining = targets.clone();
        remaining.remove(head);
        if remaining.is_empty() {
            return Ok(true);
        }
        let mut seen = BTreeSet::from([head]);
        let mut queue = VecDeque::from([head]);
        budget.node()?;
        while let Some(node) = queue.pop_front() {
            checkpoint()?;
            for &parent in &self.parents[node] {
                checkpoint()?;
                budget.edge()?;
                remaining.remove(parent);
                if remaining.is_empty() {
                    return Ok(true);
                }
                if seen.insert(parent) {
                    budget.node()?;
                    queue.push_back(parent);
                }
            }
        }
        Ok(false)
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/business/file_ancestry.rs"]
mod tests;
