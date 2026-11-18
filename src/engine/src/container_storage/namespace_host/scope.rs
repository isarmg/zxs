//! Reuse the original namespace host walk for scoped candidate preparation.
//! No path alone or merely matching inode selects an ordinary host.
use super::*;
use crate::engine::PlanningContext;
use storage_contract::snapshot_worktree::SnapshotHost;
use storage_contract::working_tree::LocalEntityIdentity;

pub(crate) struct ScopedHostChain {
    common: DiskRoot, common_entity: LocalEntityIdentity, directories: Vec<DiskRoot>,
    records: Vec<NamespaceDirectoryBinding>, _permit: crate::engine::PayloadProofReservation,
}
impl ScopedHostChain {
    pub(crate) fn open(ctx: &PlanningContext<'_>, root: &RegisteredNamespaceRoot<'_>,
        requested: SnapshotHost, evidence: &HostEvidence) -> Result<Self> {
        let b = root.binding();
        if b.registration != evidence.registration || b.root_id != requested.root_id
            || b.generation != requested.expected_root_generation || b.root_entity != evidence.root_entity
            || b.roots.common_data != evidence.common_data || evidence.host().directory_id != requested.directory_id
        { return Err(NamespaceHostError::Changed); }
        let view = ctx.scope_host_tree(evidence.namespace).map_err(NamespaceHostError::Source)?;
        let components = components_in(evidence.namespace, view)?;
        if components.len() != evidence.directories.len() { return Err(NamespaceHostError::Directory); }
        let permit = crate::engine::payload_proof_budget().reserve(components.len() + 1)
            .ok_or_else(|| NamespaceHostError::Source(crate::engine::scope_closure::ScopeClosureError::Budget.into()))?;
        let mut parent = evidence.common_data.directory_id;
        for (part, record) in components.iter().zip(&evidence.directories) {
            require_component(b.root_id, parent, part, record)?;
            parent = record.directory.directory_id;
        }
        let held = root.common().map_err(NamespaceHostError::Source)?;
        let common_entity = evidence.common_data.entity;
        let common = held.private_child_matching(held.path(), file_identity(common_entity)).map_err(NamespaceHostError::Source)?;
        let chain = Self { directories: open_chain(&common, &evidence.directories)?, common, common_entity,
            records: evidence.directories.clone(), _permit: permit };
        chain.require(held)?;
        Ok(chain)
    }
    pub(crate) fn held(&self) -> &DiskRoot { self.directories.last().unwrap_or(&self.common) }
    pub(crate) fn steps(&self) -> &[NamespaceDirectoryBinding] { &self.records }
    pub(crate) fn require(&self, admitted_common: &DiskRoot) -> Result<()> {
        if self.directories.len() != self.records.len()
            || self.common.held_identity().map_err(NamespaceHostError::Physical)? != file_identity(self.common_entity)
            || admitted_common.held_identity().map_err(NamespaceHostError::Physical)? != file_identity(self.common_entity) {
            return Err(NamespaceHostError::Changed);
        }
        verify_chain(admitted_common, &self.records, &self.directories)
    }
}
