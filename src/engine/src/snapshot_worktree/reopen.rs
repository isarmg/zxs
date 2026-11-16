use crate::container_storage::namespace_host::NamespaceHost;
use crate::directory_worktree::session::{ReopenedWorktree, WorktreeSession};
use crate::snapshot_worktree::registry::{PublishedWorktree, ReceiptIndex};
use crate::snapshot_worktree::{RegistrationError, Result};
use crate::working_tree::snapshot::HeldSnapshotNode;

impl ReceiptIndex<'_> {
    /// Only this exact Engine's admitted receipts may name an existing node.
    /// Pending reserves fail before host I/O; neither a complete local manifest
    /// nor a matching pathname can upgrade an incomplete SYSTEM decision.
    pub(super) fn reopen_node(
        &self,
        engine: &crate::Engine,
        handle: storage_contract::snapshot_worktree::SnapshotHandle,
    ) -> Result<HeldSnapshotNode> {
        if !std::ptr::eq(self.source(), engine.state()) {
            return Err(RegistrationError::Source);
        }
        let published = self.node(handle)?;
        let source = NamespaceHost::registered(engine, published.host().namespace)
            .map_err(RegistrationError::NamespaceHost)?;
        let requested = published.initialization().host;
        source.reopen_registered(
            requested.root_id, requested.expected_root_generation,
            published.host(), published.binding(),
        )
            .map_err(RegistrationError::NamespaceHost)
    }
}

impl PublishedWorktree<'_> {
    /// The Engine consumer first opens the original node with the current
    /// authorized host and this record's independent node binding. This method
    /// then consumes the exact SYSTEM-decided ledger checkpoint, never a local
    /// newest-frame search. A later writable registration must retain its own
    /// typed checkpoint transition rather than broaden this initial read path.
    pub(super) fn reopen<'a>(&self, source: &'a HeldSnapshotNode) -> Result<WorktreeSession<'a>> {
        if source.binding() != self.source().binding() {
            return Err(RegistrationError::Source);
        }
        // Only a separately completed SYSTEM publication can verify a later
        // FILE/directory completion frame. An unregistered local tail cannot.
        let reopened = WorktreeSession::reopen(
            source,
            self.binding(),
            self.checkpoint(),
            |publication, files| self.require_publication(publication, files),
        ).map_err(RegistrationError::Ledger)?;
        let ReopenedWorktree::Ready(session) = reopened else {
            return Err(RegistrationError::Source);
        };
        let reserved = self.reservation();
        let worktree = session.worktree();
        if session.instance().operation() != reserved.request.operation_id
            || worktree.source().branch != reserved.request.source.branch_id
            || worktree.mount().parent != reserved.request.parent_directory_id
            || worktree.mount().name.as_str() != reserved.request.name.as_str()
            || worktree.revision() != self.revision()
        {
            return Err(RegistrationError::Source);
        }
        Ok(session)
    }
}
