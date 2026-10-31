//! Finite auxiliary-page facts from the same complete scoped streams.
use super::*;

fn captured<K, T>(selected: Selection<K, T>, id: impl Fn(T) -> String) -> Captured<String> {
    Captured {
        entries: selected
            .entries
            .into_iter()
            .map(|(_, value)| id(value))
            .collect(),
        has_before: selected.has_before,
        has_after: selected.has_after,
    }
}

pub(super) fn select(reader: &ScopeReader<'_, '_>, query: &Query<'_>) -> Result<CapturedPage> {
    Ok(match &query.request.collection {
        HistoryCollection::Branches => {
            let position = query.position(|position| match position {
                HistoryPosition::Branch { branch_id } => Ok(branch_id.as_str()),
                _ => Err(invalid("branch cursor position required")),
            })?;
            let mut selector = Selector::new(position, query.request.limit.get())?;
            for branch in reader.branches() {
                query.cancel.checkpoint()?;
                let branch = branch?;
                if branch.state != crate::BranchState::Migrated {
                    selector.offer(branch.branch_id.as_str(), branch);
                }
            }
            CapturedPage::Branches(captured(selector.finish()?, |branch| {
                branch.branch_id.clone()
            }))
        }
        HistoryCollection::Merges => {
            let position = query.position(|position| match position {
                HistoryPosition::Merge { sequence, merge_id } => {
                    Ok((Reverse(*sequence), merge_id.as_str()))
                }
                _ => Err(invalid("merge cursor position required")),
            })?;
            let mut selector = Selector::new(position, query.request.limit.get())?;
            for merge in reader.merges() {
                query.cancel.checkpoint()?;
                let merge = merge?;
                selector.offer((Reverse(merge.sequence), merge.merge_id.as_str()), merge);
            }
            CapturedPage::Merges(captured(selector.finish()?, |merge| merge.merge_id.clone()))
        }
        HistoryCollection::NodeParents { node_id } => {
            query.node(node_id.as_str())?;
            let position = query.position(|position| match position {
                HistoryPosition::Parent { parent_node_id } => Ok(parent_node_id.as_str()),
                _ => Err(invalid("parent cursor position required")),
            })?;
            let mut selector = Selector::new(position, query.request.limit.get())?;
            nodes::visit_parents_from(
                query.state,
                query.scope(),
                node_id.as_str(),
                query.cancel,
                reader.merges(),
                |parent| {
                    selector.offer(parent, parent);
                    Ok(())
                },
            )?;
            CapturedPage::Parents(captured(selector.finish()?, str::to_owned))
        }
        HistoryCollection::NodeField { node_id, field } => {
            let nodes::Node::Content(version) = query.node(node_id.as_str())? else {
                return Err(invalid("indexed field requires a content node"));
            };
            if *field == HistoryField::CreatedAt {
                return Err(invalid(
                    "creation field does not require indexed commit facts",
                ));
            }
            let mut first = None::<&crate::Commit>;
            for entry in reader.commit_entries() {
                query.cancel.checkpoint()?;
                let (commit, entry) = entry?;
                if entry.version_id == version.version_id
                    && first.is_none_or(|old| commit.sequence < old.sequence)
                {
                    first = Some(commit);
                }
            }
            CapturedPage::Field {
                node: version.version_id.clone(),
                first_commit: first.map(|c| c.commit_id.clone()),
            }
        }
        _ => unreachable!("invariant: auxiliary selection matched indexed collection"),
    })
}

impl<'a> Query<'a> {
    pub(in crate::engine::history_window) fn indexed_branches(
        &self,
    ) -> Option<Selection<&'a str, &'a crate::Branch>> {
        let CapturedPage::Branches(selected) = &self.indexed?.page else {
            return None;
        };
        Some(Selection {
            entries: selected
                .entries
                .iter()
                .map(|id| {
                    let branch = &self.state.branches[id];
                    (branch.branch_id.as_str(), branch)
                })
                .collect(),
            has_before: selected.has_before,
            has_after: selected.has_after,
        })
    }
    pub(in crate::engine::history_window) fn indexed_merges(
        &self,
    ) -> Option<Selection<(Reverse<u64>, &'a str), &'a crate::FileBranchMerge>> {
        let CapturedPage::Merges(selected) = &self.indexed?.page else {
            return None;
        };
        Some(Selection {
            entries: selected
                .entries
                .iter()
                .map(|id| {
                    let merge = &self.state.file_branch_merges[id];
                    ((Reverse(merge.sequence), merge.merge_id.as_str()), merge)
                })
                .collect(),
            has_before: selected.has_before,
            has_after: selected.has_after,
        })
    }
    pub(in crate::engine::history_window) fn indexed_parents(&self) -> Option<Selection<&str, ()>> {
        let CapturedPage::Parents(selected) = &self.indexed?.page else {
            return None;
        };
        Some(Selection {
            entries: selected
                .entries
                .iter()
                .map(|id| (id.as_str(), ()))
                .collect(),
            has_before: selected.has_before,
            has_after: selected.has_after,
        })
    }
    pub(in crate::engine::history_window) fn indexed_field_commit(
        &self,
        version: &str,
    ) -> Option<Option<&'a crate::Commit>> {
        let CapturedPage::Field { node, first_commit } = &self.indexed?.page else {
            return None;
        };
        assert_eq!(
            node, version,
            "invariant: field fact belongs to its admitted node"
        );
        Some(first_commit.as_ref().map(|id| &self.state.commits[id]))
    }
}
