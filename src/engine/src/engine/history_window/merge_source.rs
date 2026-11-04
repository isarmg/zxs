//! On-demand branch provenance; ordinary history pages never build this index.
use super::*;
use crate::business::file_ancestry::{
    self, FileAncestry, MAX_ANCESTRY_HEADERS, MAX_ANCESTRY_NODES, SearchBudget,
};
use std::collections::BTreeMap;

pub(super) fn resolve(query: &query::Query<'_>, source: &str) -> Result<HistoryBranchSummary> {
    query.cancel.checkpoint()?;
    let source_node = query.node(source)?;
    let mut headers = 0;
    let author = match source_node {
        nodes::Node::Metadata(node) => Some(node.branch_id.as_str()),
        nodes::Node::Path(node) => Some(node.branch_id.as_str()),
        nodes::Node::Content(_) => None,
    };
    if let Some(id) = author.filter(|id| query.allowed_branch(id)) {
        return summary(query, &query.state.branches[id], &mut headers);
    }
    let scope = query.scope();
    // Distinct real heads are bounded by the admitted graph, even when many
    // branches share the same frozen head. No candidate gets a separate DFS.
    let mut heads = BTreeMap::new();
    let mut too_many_heads = false;
    for branch in query.state.branches.values() {
        if headers == MAX_ANCESTRY_HEADERS {
            return Err(file_ancestry::exhausted());
        }
        headers += 1;
        query.cancel.checkpoint()?;
        if !query.allowed_branch(&branch.branch_id) {
            continue;
        }
        let commit = &query.state.commits[&branch.head_commit_id];
        let mut head = branch.file_node_id.as_deref();
        if head.is_none() {
            for entry in &commit.entries {
                if headers == MAX_ANCESTRY_HEADERS {
                    return Err(file_ancestry::exhausted());
                }
                headers += 1;
                query.cancel.checkpoint()?;
                if entry.kind == crate::NamespaceEntryKind::File && entry.file_id == scope.file {
                    head = Some(entry.version_id.as_str());
                    break;
                }
            }
        }
        if let Some(head) = head {
            // Check every cheap direct proof before admitting a potentially
            // large ancestry graph. This is independent of candidate ordering.
            if head == source {
                return summary(query, branch, &mut headers);
            }
            let head = query.node(head)?.id();
            if heads.contains_key(head) {
                continue;
            }
            if heads.len() == MAX_ANCESTRY_NODES {
                too_many_heads = true;
            } else {
                heads.insert(head, branch.branch_id.as_str());
            }
        }
    }
    if too_many_heads {
        return Err(file_ancestry::exhausted());
    }
    let index = FileAncestry::new(query.state, scope.file, scope.history, || {
        query.cancel.checkpoint()
    })?;
    if let Some(branch) = index.source_branch(source, heads, &mut SearchBudget::read(), || {
        query.cancel.checkpoint()
    })? {
        return summary(query, &query.state.branches[branch], &mut headers);
    }
    Err(Error::new(
        storage_contract::ErrorCode::FileBranchRequired,
        "historical node has no independently owned source branch; create a branch from this node first",
    ))
}

// Source selection shares this budget with all candidate headers and fallback
// head entries. Ordinary history pages retain their existing projection policy.
fn summary<'a>(
    query: &query::Query<'a>,
    branch: &'a crate::Branch,
    headers: &mut usize,
) -> Result<HistoryBranchSummary> {
    query.branch_summary_with_entry_checkpoint(branch, || {
        if *headers == MAX_ANCESTRY_HEADERS {
            return Err(file_ancestry::exhausted());
        }
        *headers += 1;
        Ok(())
    })
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/engine/history_merge_source_tests.rs"]
mod tests;
