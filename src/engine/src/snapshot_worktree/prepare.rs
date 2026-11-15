//! The only current host preparation entrance uses the actual Engine and its
//! strict permanent index. This is not yet a public snapshot/write endpoint.
use crate::container_storage::namespace_host::PreparedNamespaceHost;
use crate::snapshot_worktree::registry::ReceiptIndex;
use crate::snapshot_worktree::{RegistrationError, Result};
use crate::Engine;
use std::sync::Arc;
use storage_contract::snapshot_worktree::SnapshotNamespace;
use storage_contract::working_tree::RootId;
use storage_contract::{Generation, OperationId};

pub(crate) fn prepare_namespace_host(
    engine: &mut Engine,
    source: &SnapshotNamespace,
    root: RootId,
    generation: Generation,
    operation: OperationId,
) -> Result<PreparedNamespaceHost> {
    // Retain this actual allocation, not an equal State copy, so the index can
    // borrow it while the Engine's one mutable operation guard remains held.
    let state = Arc::clone(&engine.state);
    let index = ReceiptIndex::capture(&state)?;
    PreparedNamespaceHost::prepare(
        engine, &index.directories(), source, root, generation, operation,
    ).map_err(RegistrationError::NamespacePreparation)
}
