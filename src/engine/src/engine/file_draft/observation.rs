//! Bounded set of original native publications, not a search of current paths,
//! historical version names or mutable controller.last_node descriptions.
use crate::{Engine, Error, Result};
use storage_contract::automatic_observation::{self as wire, NodeKey, Page, Position, Request, Row};
use storage_contract::snapshot_worktree::NamespaceDirectoryId;
use storage_contract::{FileId, Sequence};


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
        let mut rows = Vec::<Row>::with_capacity(wire::PAGE_LIMIT);
        let mut lookahead = None;
        if let Some(originals) = context.original_receipts {
            for (key, operation) in originals.automatic_after(&context, after)? {
                let (original, _) = originals.file_decision(&context, operation)?;
                let node = super::codec::observed_node(original)?.ok_or_else(super::business::invalid)?;
                if !matches!(node.reason, super::business::origin::Reason::AutomaticSave { .. })
                    || key != (NodeKey { file: node.target.file, node: node.node })
                { return Err(super::business::invalid()); }
                if node.target.repository != request.scope.repository || node.published_at < request.scope.start
                    || node.published_at >= request.scope.end { continue; }
                if let NamespaceDirectoryId::Entry(directory) = request.scope.directory
                    && !node.ancestors.contains(&directory) { continue; }
                if after == Some(key) { exact_predecessor = true; continue; }
                if rows.len() == wire::PAGE_LIMIT { lookahead = Some(key.file); break; }
                rows.push(Row { key, operation, sequence: node.sequence, published_at: node.published_at });
            }
        }
        if !exact_predecessor { return Err(stale()); }
        let next = lookahead.and_then(|_| rows.last().map(|row| row.key));
        let page = Page { request: *request, observed, rows, next };
        page.validate(request).map_err(|_| super::business::invalid())?;
        Ok((page, lookahead))
    }
}
