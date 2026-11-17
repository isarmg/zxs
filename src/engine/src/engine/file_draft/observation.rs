//! Bounded set of original native publications, not a search of current paths,
//! historical version names or mutable controller.last_node descriptions.
use crate::{Engine, Error, Result};
use storage_contract::automatic_observation::{self as wire, NodeKey, Page, Position, Request, Row};
use storage_contract::snapshot_worktree::NamespaceDirectoryId;
use storage_contract::{FileId, Sequence};
use std::collections::BTreeMap;

impl Engine {
    pub fn automatic_node_page(&self, request: &Request) -> Result<(Page, Option<FileId>)> {
        request.validate().map_err(|_| Error::new(storage_contract::ErrorCode::InvalidRequest,
            "invalid automatic publication window"))?;
        let stale = || Error::new(storage_contract::ErrorCode::PreparationStale,
            "automatic publication window must be refreshed");
        let context = self.planning_context();
        let observed = Sequence::new(context.state.sequence);
        let after = match request.position {
            Position::First => None,
            Position::After { observed: prior, last } if prior == observed => Some(last),
            Position::After { .. } => return Err(stale()),
        };
        let mut exact_predecessor = after.is_none();
        let mut rows = BTreeMap::<NodeKey, Row>::new();
        for visible in context.state.operations.values() {
            if !matches!(visible.kind.as_str(), "file.draft.commit" | crate::persist::file_draft::unborn_first::KIND) { continue; }
            let (original, _) = context.original_receipts.ok_or_else(super::business::invalid)?
                .file_decision(&context, visible.operation_id)?;
            let Some(node) = super::codec::observed_node(original)? else { continue; };
            if !matches!(node.reason, super::business::origin::Reason::AutomaticSave { .. })
                || node.target.repository != request.scope.repository
                || node.published_at < request.scope.start || node.published_at >= request.scope.end
            { continue; }
            if let NamespaceDirectoryId::Entry(directory) = request.scope.directory
                && !node.ancestors.contains(&directory) { continue; }
            let key = NodeKey { file: node.target.file, node: node.node };
            if after == Some(key) { exact_predecessor = true; continue; }
            if after.is_some_and(|last| key < last) { continue; }
            if rows.len() > wire::PAGE_LIMIT && rows.last_key_value().is_some_and(|(last, _)| key > *last) {
                continue;
            }
            let row = Row { key, operation: node.operation, sequence: node.sequence, published_at: node.published_at };
            if let Some(earlier) = rows.insert(key, row.clone()) && earlier != row {
                return Err(super::business::invalid());
            }
            if rows.len() > wire::PAGE_LIMIT + 1 { rows.pop_last(); }
        }
        if !exact_predecessor { return Err(stale()); }
        let lookahead = if rows.len() > wire::PAGE_LIMIT {
            rows.pop_last().map(|(key, _)| key.file)
        } else { None };
        let next = lookahead.and_then(|_| rows.last_key_value().map(|(key, _)| *key));
        let page = Page { request: *request, observed, rows: rows.into_values().collect(), next };
        page.validate(request).map_err(|_| super::business::invalid())?;
        Ok((page, lookahead))
    }
}
