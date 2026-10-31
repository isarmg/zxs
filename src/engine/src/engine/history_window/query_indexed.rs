//! Finite owned selection from fully verified, streamed scoped membership.
//! SQLite supplies candidates only. Every stream is consumed completely before
//! the plan is returned; QueryProof, not a database count, proves completeness.
use super::*;
use crate::engine::index::{OwnedScopeRead, ScopeNode, ScopeReader};

pub(super) struct Captured<T> {
    entries: Vec<T>,
    has_before: bool,
    has_after: bool,
}

pub(super) struct NodePick {
    id: String,
    first_commit: Option<String>,
}

pub(super) enum CapturedPage {
    Nodes(Captured<NodePick>),
    Versions(Captured<NodePick>),
}

pub(crate) struct IndexedSelection {
    owner_version: String,
    pub(super) page: CapturedPage,
}

impl MetadataView {
    pub(crate) fn history_window_selected(
        &self,
        request: &HistoryWindowRequest,
        context: &HistoryResolvedContext,
        cancellation: &HistoryWindowCancellation,
        selected: Option<&IndexedSelection>,
    ) -> Result<HistoryWindowPage> {
        let mut query = Query::new_with_owner(
            self,
            request,
            context,
            cancellation,
            selected.map(|selected| selected.owner_version.as_str()),
        )?;
        query.indexed = selected;
        query.page()
    }
}

pub(crate) fn prepare_indexed_selection(
    snapshot: &OwnedScopeRead,
    view: &MetadataView,
    request: &HistoryWindowRequest,
    context: &HistoryResolvedContext,
    cancellation: &HistoryWindowCancellation,
) -> Result<Option<IndexedSelection>> {
    snapshot.with_state(view.business_projection(), |reader| {
        select_scope(reader, view, request, context, cancellation)
    })
}

#[cfg(test)]
pub(crate) fn capture_indexed_selection(
    engine: &Engine,
    view: &MetadataView,
    request: &HistoryWindowRequest,
    context: &HistoryResolvedContext,
    cancellation: &HistoryWindowCancellation,
) -> Result<Option<IndexedSelection>> {
    engine.with_history_scope(
        request.file_id.as_str(),
        context.history_tree_id.as_str(),
        |reader| select_scope(reader, view, request, context, cancellation),
    )
}

fn select_scope(
    reader: &ScopeReader<'_, '_>,
    view: &MetadataView,
    request: &HistoryWindowRequest,
    context: &HistoryResolvedContext,
    cancellation: &HistoryWindowCancellation,
) -> Result<IndexedSelection> {
    cancellation.checkpoint()?;
    // Resolve the same minimum-ID owner as the fallback, without an
    // unrelated-pool scan. Do not stop early: the whole stream attests it.
    let mut owner = None::<&crate::FileVersion>;
    for version in reader.versions() {
        cancellation.checkpoint()?;
        let version = version?;
        if owner.is_none_or(|old| version.version_id < old.version_id) {
            owner = Some(version);
        }
    }
    let owner = owner.ok_or_else(|| invalid("scoped history has no owner version"))?;
    let query = Query::new_with_owner(
        view,
        request,
        context,
        cancellation,
        Some(&owner.version_id),
    )?;
    let page = match request.collection {
        HistoryCollection::Nodes => {
            let position = if let Some(anchor) = request.anchor_node_id.as_ref() {
                let node = query.node(anchor.as_str())?;
                let mut first = None::<&crate::Commit>;
                if let nodes::Node::Content(version) = node {
                    for entry in reader.commit_entries() {
                        cancellation.checkpoint()?;
                        let (commit, entry) = entry?;
                        if entry.version_id == version.version_id
                            && first.is_none_or(|old| commit.sequence < old.sequence)
                        {
                            first = Some(commit);
                        }
                    }
                }
                Position::At((Reverse(node.sequence(first)), node.id()))
            } else {
                query.position(|position| match position {
                    HistoryPosition::Node { sequence, node_id } => {
                        Ok((Reverse(*sequence), node_id.as_str()))
                    }
                    _ => Err(invalid("node cursor position required")),
                })?
            };
            let selected = query.select_nodes_from(
                reader.nodes().map(|node| {
                    node.map(|node| match node {
                        ScopeNode::Content(v) => nodes::Node::Content(v),
                        ScopeNode::Path(v) => nodes::Node::Path(v),
                        ScopeNode::Metadata(v) => nodes::Node::Metadata(v),
                    })
                }),
                |batch| {
                    for entry in reader.commit_entries() {
                        cancellation.checkpoint()?;
                        let (commit, entry) = entry?;
                        if let Some((_, first)) = batch.get_mut(entry.version_id.as_str())
                            && first.is_none_or(|old| commit.sequence < old.sequence)
                        {
                            *first = Some(commit);
                        }
                    }
                    Ok(())
                },
                position,
            )?;
            CapturedPage::Nodes(Captured {
                entries: selected
                    .entries
                    .into_iter()
                    .map(|(_, (node, commit))| NodePick {
                        id: node.id().to_owned(),
                        first_commit: commit.map(|commit| commit.commit_id.clone()),
                    })
                    .collect(),
                has_before: selected.has_before,
                has_after: selected.has_after,
            })
        }
        HistoryCollection::Versions => {
            let position = query.position(|position| match position {
                HistoryPosition::Version {
                    sequence,
                    version_id,
                } => Ok((*sequence, version_id.as_str())),
                _ => Err(invalid("version cursor position required")),
            })?;
            let mut selector = Selector::new(position, request.limit.get())?;
            for version in reader.versions() {
                cancellation.checkpoint()?;
                let version = version?;
                selector.offer(
                    (version.version_sequence, version.version_id.as_str()),
                    version,
                );
            }
            let selected = selector.finish()?;
            // At most the admitted page is retained. Complete the scoped entry
            // stream even after all selected versions found a first commit:
            // counts certify absence too, and later IDs can have lower sequences.
            let mut first = selected
                .entries
                .iter()
                .map(|(_, version)| (version.version_id.as_str(), None::<&crate::Commit>))
                .collect::<BTreeMap<_, _>>();
            for entry in reader.commit_entries() {
                cancellation.checkpoint()?;
                let (commit, entry) = entry?;
                if let Some(found) = first.get_mut(entry.version_id.as_str())
                    && found.is_none_or(|old| commit.sequence < old.sequence)
                {
                    *found = Some(commit);
                }
            }
            CapturedPage::Versions(Captured {
                entries: selected
                    .entries
                    .into_iter()
                    .map(|(_, version)| NodePick {
                        id: version.version_id.clone(),
                        first_commit: first[version.version_id.as_str()]
                            .map(|commit| commit.commit_id.clone()),
                    })
                    .collect(),
                has_before: selected.has_before,
                has_after: selected.has_after,
            })
        }
        _ => unreachable!("invariant: indexed collection was matched before reading"),
    };
    Ok(IndexedSelection {
        owner_version: owner.version_id.clone(),
        page,
    })
}

impl<'a> Query<'a> {
    pub(in crate::engine::history_window) fn indexed_version_commit(
        &self,
        version: &str,
    ) -> Option<Option<&'a crate::Commit>> {
        let CapturedPage::Versions(selected) = &self.indexed?.page else {
            return None;
        };
        let selected = selected
            .entries
            .iter()
            .find(|selected| selected.id == version)
            .expect("invariant: version summary belongs to its captured page");
        Some(selected.first_commit.as_ref().map(|id| {
            self.state
                .commits
                .get(id)
                .expect("invariant: first commit belongs to the captured immutable observation")
        }))
    }

    pub(super) fn restore_nodes(&self, selected: &Captured<NodePick>) -> Result<NodeSelection<'a>> {
        let entries = selected
            .entries
            .iter()
            .map(|pick| {
                let node = self.node(&pick.id)?;
                let commit = pick.first_commit.as_ref().map(|id| {
                    self.state
                        .commits
                        .get(id)
                        .expect("invariant: captured commit belongs to this immutable observation")
                });
                Ok(((Reverse(node.sequence(commit)), node.id()), (node, commit)))
            })
            .collect::<Result<_>>()?;
        Ok(Selection {
            entries,
            has_before: selected.has_before,
            has_after: selected.has_after,
        })
    }

    pub(super) fn restore_versions(
        &self,
        selected: &Captured<NodePick>,
    ) -> Result<Selection<(u64, &'a str), &'a crate::FileVersion>> {
        let entries = selected
            .entries
            .iter()
            .map(|pick| {
                let version = self.version(&pick.id)?;
                Ok((
                    (version.version_sequence, version.version_id.as_str()),
                    version,
                ))
            })
            .collect::<Result<_>>()?;
        Ok(Selection {
            entries,
            has_before: selected.has_before,
            has_after: selected.has_after,
        })
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/engine/history_window_indexed_tests.rs"]
mod tests;
