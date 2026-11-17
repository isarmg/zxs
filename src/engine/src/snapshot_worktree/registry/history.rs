//! Read only independently published nodes from the admitted original index.
//! Page accumulation is bounded; index admission and scanning are not O(page).
use super::{NodePlan, Publication, ReceiptIndex};
use crate::snapshot_worktree::{RegistrationError, Result};
use std::collections::BTreeMap;
use storage_contract::Sequence;
use storage_contract::snapshot_worktree::history::{
    SNAPSHOT_HISTORY_PAGE_SIZE, SnapshotHistoryCursor, SnapshotHistoryNode,
};
use storage_contract::snapshot_worktree::{NamespaceHostAddress, SnapshotHandle, SnapshotHost};
use storage_contract::working_tree::{RootBinding, SnapshotRepositoryId};

impl ReceiptIndex<'_> {
    pub(in crate::snapshot_worktree) fn history_at_root(
        &self,
        namespace: NamespaceHostAddress,
        host: SnapshotHost,
        binding: &RootBinding,
        cursor: Option<SnapshotHistoryCursor>,
    ) -> Result<(Vec<SnapshotHistoryNode>, Option<SnapshotHistoryCursor>)> {
        match self.root_registration(namespace, host, binding)? {
            Some(original) => self.history_page(original.plan.allocated.repository, cursor),
            None if cursor.is_none() => Ok((Vec::new(), None)),
            None => Err(RegistrationError::HistoryCursor),
        }
    }

    fn history_page(
        &self,
        repository: SnapshotRepositoryId,
        cursor: Option<SnapshotHistoryCursor>,
    ) -> Result<(Vec<SnapshotHistoryNode>, Option<SnapshotHistoryCursor>)> {
        let sequence = Sequence::new(self.source.sequence);
        let before = match cursor {
            Some(cursor) => {
                if cursor.sequence != sequence || cursor.after.repository_id != repository {
                    return Err(RegistrationError::HistoryCursor);
                }
                // A client cannot skip using a reserve, another root's node, or
                // a real publication combined with different branch/node IDs.
                self.node(cursor.after)?;
                Some(self.node_sequence(cursor.after)?)
            }
            None => None,
        };
        let mut newest = BTreeMap::new();
        let mut visit = |plan: &NodePlan, parent: Option<SnapshotHandle>| -> Result<()> {
            let snapshot = plan.handle();
            if snapshot.repository_id != repository {
                return Ok(());
            }
            let published = self.node_sequence(snapshot)?;
            if before.is_some_and(|before| published >= before) {
                return Ok(());
            }
            newest.insert(
                published,
                SnapshotHistoryNode {
                    snapshot,
                    branch_name: plan.branch_name.clone(),
                    parent,
                    is_head: self.branch_heads.get(&(repository, snapshot.branch_id))
                        == Some(&snapshot),
                },
            );
            // One look-ahead record determines whether another page exists.
            if newest.len() > SNAPSHOT_HISTORY_PAGE_SIZE + 1 {
                newest.pop_first();
            }
            Ok(())
        };
        for original in self.nodes.values() {
            if matches!(original.publication, Publication::Complete(_)) {
                visit(&original.plan, None)?;
            }
        }
        for commit in self.commits.values() {
            if matches!(commit.node, Publication::Complete(_)) {
                visit(&commit.node_plan, Some(commit.plan.source))?;
            }
        }
        for working in self.working.values() {
            if let Some((node, parent)) = working.history_node() {
                visit(node.plan, Some(parent))?;
            }
        }
        let has_more = newest.len() > SNAPSHOT_HISTORY_PAGE_SIZE;
        let nodes: Vec<_> = newest
            .into_values()
            .rev()
            .take(SNAPSHOT_HISTORY_PAGE_SIZE)
            .collect();
        let next_cursor = nodes
            .last()
            .filter(|_| has_more)
            .map(|last| SnapshotHistoryCursor {
                sequence,
                after: last.snapshot,
            });
        Ok((nodes, next_cursor))
    }

    fn node_sequence(&self, snapshot: SnapshotHandle) -> Result<Sequence> {
        self.source
            .operations
            .get(&snapshot.publication)
            .map(|receipt| Sequence::new(receipt.sequence))
            .ok_or(RegistrationError::Source)
    }
}
