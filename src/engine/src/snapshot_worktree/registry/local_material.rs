//! Only the independent receipt index may identify a held/reserved original.
//! These are borrowed projections of the existing native records, not a second
//! registration or a handle minted from a disk scan.
use super::*;
use crate::working_tree::snapshot::AdmittedSnapshotManifest;
use storage_contract::{BranchName, snapshot_worktree::InitializeSnapshot};

pub(crate) struct InitialDirectoryMaterial<'a> {
    pub(crate) original: &'a crate::OperationResult,
    pub(crate) request: &'a InitializeSnapshot,
    pub(crate) host: &'a HostEvidence,
    pub(crate) allocated: NodeAllocation,
    pub(crate) branch_name: &'a BranchName,
    pub(crate) manifest: &'a AdmittedSnapshotManifest,
    pub(crate) commitment: MaterialCommitment,
}
impl ReceiptIndex<'_> {
    /// Before the first native node is bound nothing can derive a worktree
    /// from it. A head, selected completion or an already-created instance
    /// contradicts this particular proof rather than widening its scope.
    pub(crate) fn initial_pending_material(
        &self, repository: SnapshotRepositoryId,
    ) -> Result<InitialDirectoryMaterial<'_>> {
        let mut nodes = self.nodes.values().filter(|n| n.plan.allocated.repository == repository);
        let node = nodes.next().ok_or(RegistrationError::Source)?;
        if nodes.next().is_some() || !matches!(node.publication, Publication::Reserved)
            || self.branch_heads.keys().any(|(r, _)| *r == repository)
            || self.worktrees.values().any(|w| w.plan.request.source.repository_id == repository)
            || self.source.operations.contains_key(&node.plan.allocated.completion)
        { return Err(RegistrationError::Source); }
        Ok(InitialDirectoryMaterial { original: node.receipt.original, request: &node.plan.request,
            host: &node.plan.host, allocated: node.plan.allocated, branch_name: &node.plan.branch_name,
            manifest: &node.manifest, commitment: node.commitment })
    }
}
