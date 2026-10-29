//! Bounded graph inspection over one admitted immutable State. The request
//! owner supplies cancellation and performs current authorization/root admission.
use crate::{Engine, Error, ErrorCode, HistoryRoot, Result, State};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Bound,
    sync::Arc,
};
use storage_contract::history_graph::*;
use storage_contract::{Branch, Commit, DeletedPath, HistoryBoundary, HistoryTree, MergeLink};

/// Consumed by one worker. It retains one old State only until projection ends,
/// never through response encoding or transport drain. It grants no authority.
pub struct HistoryGraphReadPlan {
    state: Arc<State>,
    history_id: String,
    root: HistoryRoot,
    membership: Option<crate::state_projection::Rows>,
}

pub struct GraphSelection {
    pub history: HistorySummary,
    pub items: GraphItems,
    pub next: Option<GraphKey>,
}
pub struct TextSlice {
    pub total_bytes: usize,
    pub offset: usize,
    pub text: Option<String>,
    pub next_offset: Option<usize>,
}
pub struct PendingSelection {
    pub items: Vec<PendingRecord>,
    pub next: Option<String>,
}
pub struct DeletionSelection {
    pub timestamp: Option<u64>,
    pub total: usize,
    pub offset: usize,
    pub items: Vec<DeletedPath>,
    pub next_offset: Option<usize>,
}

impl Engine {
    /// Capture the exact root and admitted State after the caller's current
    /// authorization. No owner index is built on a cold read.
    pub fn history_graph_read_plan(&self, history_id: &str) -> Result<HistoryGraphReadPlan> {
        if !self.state.histories.contains_key(history_id) {
            return Err(Error::new(
                ErrorCode::HistoryNotFound,
                "history tree not found",
            ));
        }
        let root = self.history_root(history_id)?;
        let membership = self.available_history_owners().map(|owners| {
            owners
                .rows(history_id)
                .expect("invariant: admitted history owns its root commit")
                .clone()
        });
        Ok(HistoryGraphReadPlan {
            state: Arc::clone(&self.state),
            history_id: history_id.to_owned(),
            root,
            membership,
        })
    }
}

impl HistoryGraphReadPlan {
    pub fn root(&self) -> &HistoryRoot {
        &self.root
    }

    /// Select at most limit + 1 keys, then project only the returned page.
    /// Cold scans are O(all records); captured membership limits warm scans to
    /// this history. Selection memory is O(limit), not O(history length).
    pub fn read_page(
        self,
        collection: GraphCollection,
        after: Option<&GraphKey>,
        limit: PageLimit,
        mut observe: impl FnMut() -> Result<()>,
    ) -> Result<GraphSelection> {
        observe()?;
        if let Some(key) = after {
            self.admit_key(collection, key)?;
        }
        let keys = self.select(collection, after, limit, &mut observe)?;
        let more = keys.len() > limit.get();
        let selected: Vec<_> = keys.into_iter().take(limit.get()).collect();
        let next = more.then(|| {
            selected
                .last()
                .expect("invariant: positive page limit has a last selected key")
                .clone()
        });
        observe()?;
        let items = self.project(collection, &selected, &mut observe)?;
        let history = history_summary(&self.state.histories[&self.history_id]);
        Ok(GraphSelection {
            history,
            items,
            next,
        })
    }

    fn admit_key(&self, collection: GraphCollection, key: &GraphKey) -> Result<()> {
        let valid = match (collection, key) {
            (
                GraphCollection::Commits,
                GraphKey::Commit {
                    sequence,
                    commit_id,
                },
            ) => self.state.commits.get(commit_id).is_some_and(|record| {
                record.history_tree_id == self.history_id && record.sequence == *sequence
            }),
            (GraphCollection::Branches, GraphKey::Id { id }) => self
                .state
                .branches
                .get(id)
                .is_some_and(|record| record.history_tree_id == self.history_id),
            (GraphCollection::MergeLinks, GraphKey::Id { id }) => self
                .state
                .merge_links
                .get(id)
                .is_some_and(|record| record.history_tree_id == self.history_id),
            (GraphCollection::BoundaryMarks, GraphKey::Id { id }) => self
                .state
                .history_boundaries
                .get(id)
                .is_some_and(|record| record.history_tree_id == self.history_id),
            (GraphCollection::Commits, GraphKey::Id { .. })
            | (
                GraphCollection::Branches
                | GraphCollection::MergeLinks
                | GraphCollection::BoundaryMarks,
                GraphKey::Commit { .. },
            ) => false,
        };
        if !valid {
            return Err(Error::new(
                ErrorCode::InvalidCursor,
                "graph continuation does not name a record in this history",
            ));
        }
        Ok(())
    }

    fn select<'a>(
        &'a self,
        collection: GraphCollection,
        after: Option<&GraphKey>,
        limit: PageLimit,
        observe: &mut impl FnMut() -> Result<()>,
    ) -> Result<BTreeSet<GraphKey>> {
        let after = after.map(|key| match key {
            GraphKey::Commit {
                sequence,
                commit_id,
            } => (*sequence, commit_id.as_str()),
            GraphKey::Id { id } => (0, id.as_str()),
        });
        let mut selected = BTreeSet::new();
        let mut consider = |sequence, id: &'a str| {
            if after.is_none_or(|after| (sequence, id) > after) {
                selected.insert((sequence, id));
                if selected.len() > limit.get() + 1 {
                    selected.pop_last();
                }
            }
        };
        if let Some(rows) = &self.membership {
            let field = match collection {
                GraphCollection::Commits => crate::state_projection::Field::Commits,
                GraphCollection::Branches => crate::state_projection::Field::Branches,
                GraphCollection::MergeLinks => crate::state_projection::Field::MergeLinks,
                GraphCollection::BoundaryMarks => crate::state_projection::Field::BoundaryMarks,
            };
            for id in rows.keys(field) {
                observe()?;
                let sequence = match collection {
                    GraphCollection::Commits => self.state.commits[id].sequence,
                    GraphCollection::Branches
                    | GraphCollection::MergeLinks
                    | GraphCollection::BoundaryMarks => 0,
                };
                consider(sequence, id);
            }
        } else {
            match collection {
                GraphCollection::Commits => {
                    for (id, record) in self.state.commits.iter() {
                        observe()?;
                        if record.history_tree_id == self.history_id {
                            consider(record.sequence, id);
                        }
                    }
                }
                GraphCollection::Branches => {
                    for (id, record) in &self.state.branches {
                        observe()?;
                        if record.history_tree_id == self.history_id {
                            consider(0, id);
                        }
                    }
                }
                GraphCollection::MergeLinks => {
                    for (id, record) in &self.state.merge_links {
                        observe()?;
                        if record.history_tree_id == self.history_id {
                            consider(0, id);
                        }
                    }
                }
                GraphCollection::BoundaryMarks => {
                    for (id, record) in &self.state.history_boundaries {
                        observe()?;
                        if record.history_tree_id == self.history_id {
                            consider(0, id);
                        }
                    }
                }
            }
        }
        Ok(selected
            .into_iter()
            .map(|(sequence, id)| match collection {
                GraphCollection::Commits => GraphKey::Commit {
                    sequence,
                    commit_id: id.to_owned(),
                },
                GraphCollection::Branches
                | GraphCollection::MergeLinks
                | GraphCollection::BoundaryMarks => GraphKey::Id { id: id.to_owned() },
            })
            .collect())
    }

    fn project(
        &self,
        collection: GraphCollection,
        keys: &[GraphKey],
        observe: &mut impl FnMut() -> Result<()>,
    ) -> Result<GraphItems> {
        let ids = keys.iter().map(|key| match key {
            GraphKey::Commit { commit_id, .. } => commit_id.as_str(),
            GraphKey::Id { id } => id.as_str(),
        });
        match collection {
            GraphCollection::Commits => Ok(GraphItems::Commits(
                ids.map(|id| {
                    observe()?;
                    Ok(commit_summary(&self.state.commits[id]))
                })
                .collect::<Result<_>>()?,
            )),
            GraphCollection::Branches => Ok(GraphItems::Branches(
                ids.map(|id| {
                    observe()?;
                    Ok(branch_summary(&self.state.branches[id]))
                })
                .collect::<Result<_>>()?,
            )),
            GraphCollection::MergeLinks => Ok(GraphItems::MergeLinks(
                ids.map(|id| {
                    observe()?;
                    Ok(link_summary(&self.state.merge_links[id]))
                })
                .collect::<Result<_>>()?,
            )),
            GraphCollection::BoundaryMarks => Ok(GraphItems::BoundaryMarks(
                ids.map(|id| {
                    observe()?;
                    Ok(boundary_summary(&self.state.history_boundaries[id]))
                })
                .collect::<Result<_>>()?,
            )),
        }
    }

    pub fn read_text(
        self,
        target: &TextTarget,
        offset: usize,
        limit: TextLimit,
        mut observe: impl FnMut() -> Result<()>,
    ) -> Result<TextSlice> {
        observe()?;
        let text = match target {
            TextTarget::History {
                field: HistoryField::Provenance,
            } => self.state.histories[&self.history_id].provenance.as_deref(),
            TextTarget::Commit { id, field } => {
                let record = self.commit(id)?;
                match field {
                    CommitField::Kind => Some(record.kind.as_str()),
                    CommitField::TreeId => Some(record.tree_id.as_str()),
                    CommitField::Author => Some(record.author.as_str()),
                    CommitField::Message => Some(record.message.as_str()),
                    CommitField::Provenance => record.provenance.as_deref(),
                }
            }
            TextTarget::MergeLink {
                id,
                field: MergeLinkField::Author,
            } => Some(
                self.state
                    .merge_links
                    .get(id)
                    .filter(|record| record.history_tree_id == self.history_id)
                    .ok_or_else(record_not_found)?
                    .author
                    .as_str(),
            ),
            TextTarget::Boundary { id, field } => {
                let record = self
                    .state
                    .history_boundaries
                    .get(id)
                    .filter(|record| record.history_tree_id == self.history_id)
                    .ok_or_else(record_not_found)?;
                match field {
                    BoundaryField::Relation => Some(record.relation.as_str()),
                    BoundaryField::SourceMergeLinkId => record.source_merge_link_id.as_deref(),
                }
            }
        };
        observe()?;
        text_slice(text, offset, limit)
    }

    pub fn read_pending(
        self,
        target: &PendingTarget,
        after: Option<&str>,
        limit: PageLimit,
        mut observe: impl FnMut() -> Result<()>,
    ) -> Result<PendingSelection> {
        observe()?;
        let records = match target {
            PendingTarget::Commit { id } => &self.commit(id)?.pending_file_versions,
            PendingTarget::Branch { id } => {
                &self
                    .state
                    .branches
                    .get(id)
                    .filter(|record| record.history_tree_id == self.history_id)
                    .ok_or_else(record_not_found)?
                    .pending_file_versions
            }
        };
        pending_slice(records, after, limit, &mut observe)
    }

    pub fn read_deletions(
        self,
        commit_id: &str,
        offset: usize,
        limit: PageLimit,
        mut observe: impl FnMut() -> Result<()>,
    ) -> Result<DeletionSelection> {
        observe()?;
        let deletions = self.commit(commit_id)?.namespace_deletions.as_ref();
        let total = deletions.map_or(0, |record| record.paths.len());
        if offset > total {
            return Err(invalid_offset());
        }
        let end = offset + (total - offset).min(limit.get());
        let mut items = Vec::with_capacity(end - offset);
        if let Some(record) = deletions {
            for item in &record.paths[offset..end] {
                observe()?;
                items.push(item.clone());
            }
        }
        Ok(DeletionSelection {
            timestamp: deletions.map(|record| record.timestamp),
            total,
            offset,
            items,
            next_offset: (end < total).then_some(end),
        })
    }

    fn commit(&self, id: &str) -> Result<&Commit> {
        self.state
            .commits
            .get(id)
            .filter(|record| record.history_tree_id == self.history_id)
            .ok_or_else(record_not_found)
    }
}

fn record_not_found() -> Error {
    Error::new(
        ErrorCode::NotFound,
        "record is outside the requested history",
    )
}
fn invalid_offset() -> Error {
    Error::new(
        ErrorCode::InvalidCursor,
        "graph field offset is outside its value",
    )
}
fn text_slice(text: Option<&str>, offset: usize, limit: TextLimit) -> Result<TextSlice> {
    let Some(text) = text else {
        if offset != 0 {
            return Err(invalid_offset());
        }
        return Ok(TextSlice {
            total_bytes: 0,
            offset,
            text: None,
            next_offset: None,
        });
    };
    if offset > text.len() || !text.is_char_boundary(offset) {
        return Err(invalid_offset());
    }
    let mut end = offset + (text.len() - offset).min(limit.get());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Ok(TextSlice {
        total_bytes: text.len(),
        offset,
        text: Some(text[offset..end].to_owned()),
        next_offset: (end < text.len()).then_some(end),
    })
}
fn pending_slice(
    records: &BTreeMap<String, String>,
    after: Option<&str>,
    limit: PageLimit,
    observe: &mut impl FnMut() -> Result<()>,
) -> Result<PendingSelection> {
    if let Some(after) = after
        && !records.contains_key(after)
    {
        return Err(Error::new(
            ErrorCode::InvalidCursor,
            "pending continuation is not in this record",
        ));
    }
    let mut rows = records.range::<str, _>((
        after.map_or(Bound::Unbounded, Bound::Excluded),
        Bound::Unbounded,
    ));
    let mut items = Vec::with_capacity(limit.get());
    for (file_id, version_id) in rows.by_ref().take(limit.get()) {
        observe()?;
        items.push(PendingRecord {
            file_id: file_id.clone(),
            version_id: version_id.clone(),
        });
    }
    observe()?;
    let more = rows.next().is_some();
    let next = more.then(|| {
        items
            .last()
            .expect("invariant: positive pending limit has last record")
            .file_id
            .clone()
    });
    Ok(PendingSelection { items, next })
}

fn history_summary(record: &HistoryTree) -> HistorySummary {
    let HistoryTree {
        history_tree_id,
        repository_id,
        root_commit_id,
        main_branch_id,
        generation,
        state,
        provenance,
    } = record;
    HistorySummary {
        history_tree_id: history_tree_id.clone(),
        repository_id: repository_id.clone(),
        root_commit_id: root_commit_id.clone(),
        main_branch_id: main_branch_id.clone(),
        generation: *generation,
        state: *state,
        provenance: provenance.as_deref().map(TextSummary::from_text),
    }
}
fn commit_summary(record: &Commit) -> CommitSummary {
    let Commit {
        commit_id,
        history_tree_id,
        kind,
        parent_commit_ids,
        tree_id,
        sequence,
        operation_id,
        author,
        message,
        namespace,
        namespace_deletions,
        pending_file_versions,
        provenance,
    } = record;
    CommitSummary {
        commit_id: commit_id.clone(),
        history_tree_id: history_tree_id.clone(),
        kind: TextSummary::from_text(kind),
        parent_commit_ids: parent_commit_ids.clone(),
        tree_id: TextSummary::from_text(tree_id),
        sequence: *sequence,
        operation_id: *operation_id,
        author: TextSummary::from_text(author),
        message: TextSummary::from_text(message),
        namespace: namespace.clone(),
        namespace_deletions: namespace_deletions.as_ref().map(|record| DeletionSummary {
            timestamp: record.timestamp,
            paths: record.paths.len() as u64,
        }),
        pending_file_versions: pending_file_versions.len() as u64,
        provenance: provenance.as_deref().map(TextSummary::from_text),
    }
}
fn branch_summary(record: &Branch) -> BranchSummary {
    let Branch {
        branch_id,
        history_tree_id,
        name,
        head_commit_id,
        fork_commit_id,
        branch_kind,
        file_id,
        file_node_id,
        pending_file_versions,
        generation,
        state,
    } = record;
    BranchSummary {
        branch_id: branch_id.clone(),
        history_tree_id: history_tree_id.clone(),
        name: name.clone(),
        head_commit_id: head_commit_id.clone(),
        fork_commit_id: fork_commit_id.clone(),
        branch_kind: *branch_kind,
        file_id: file_id.clone(),
        file_node_id: file_node_id.clone(),
        pending_file_versions: pending_file_versions.len() as u64,
        generation: *generation,
        state: *state,
    }
}
fn link_summary(record: &MergeLink) -> MergeLinkSummary {
    let MergeLink {
        merge_link_id,
        history_tree_id,
        source_branch_id,
        source_commit_id,
        target_branch_id,
        target_commit_id,
        author,
        operation_id,
    } = record;
    MergeLinkSummary {
        merge_link_id: merge_link_id.clone(),
        history_tree_id: history_tree_id.clone(),
        source_branch_id: source_branch_id.clone(),
        source_commit_id: source_commit_id.clone(),
        target_branch_id: target_branch_id.clone(),
        target_commit_id: target_commit_id.clone(),
        author: TextSummary::from_text(author),
        operation_id: *operation_id,
    }
}
fn boundary_summary(record: &HistoryBoundary) -> BoundarySummary {
    let HistoryBoundary {
        boundary_id,
        history_tree_id,
        local_commit_id,
        local_branch_id,
        relation,
        external_history_tree_id,
        external_commit_id,
        source_merge_link_id,
    } = record;
    BoundarySummary {
        boundary_id: boundary_id.clone(),
        history_tree_id: history_tree_id.clone(),
        local_commit_id: local_commit_id.clone(),
        local_branch_id: local_branch_id.clone(),
        relation: TextSummary::from_text(relation),
        external_history_tree_id: external_history_tree_id.clone(),
        external_commit_id: external_commit_id.clone(),
        source_merge_link_id: source_merge_link_id.as_deref().map(TextSummary::from_text),
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/business/history_graph_pages.rs"]
mod tests;
