//! The first native reservation is affirmative evidence of an obligation,
//! not of a physically installed repository (and not of an empty host).
use super::{record::*, *};
use crate::{
    engine::scope_closure::{OrdinalBuilder, ordinal::KeyedBuilder},
    persist::snapshot_worktree::{NODE_RESERVE, NodeReservation, Record},
    snapshot_worktree::{InitialDirectoryMaterial, ReceiptIndex},
    working_tree::snapshot::AdmittedSnapshotManifest,
};
use platform_io::fs_safety::linux::FileIdentity;
use storage_contract::{Principal, snapshot_worktree::InitializeSnapshot};

pub(in crate::engine) struct PendingRepository {
    host: BoundHost,
    frozen: FrozenDirectory,
}
impl PendingRepository {
    /// Actual predecision plan, including every path of the native manifest
    /// recipe. The previous index validates the entire ordinary transition;
    /// the same operation/sequence is retained without any enclosing hash.
    pub(in crate::engine) fn planned(
        context: &PlanningContext<'_>, next: &crate::State, receipt: &crate::OperationResult,
        principal: &Principal, plan: &NodeReservation,
    ) -> Result<Self> {
        let index = context.snapshot_receipts().map_err(crate::business::file_draft::worktree::failure)?;
        let record = Record::ReserveNode(plan.clone());
        index.require_planned(next, receipt, principal, &record)
            .map_err(crate::business::file_draft::worktree::failure)?;
        let commitment = record.commitment().map_err(|_| reject())?;
        drop(record);
        let manifest = AdmittedSnapshotManifest::prepare(plan.allocated.repository, plan.allocated.node,
            plan.request.operation_id, plan.entries.clone()).map_err(|_| reject())?;
        check(manifest.reference() == plan.material && context.next_slot == receipt.sequence)?;
        let facts = InitialDirectoryMaterial { original: receipt, request: &plan.request, host: &plan.host,
            allocated: plan.allocated, branch_name: &plan.branch_name, manifest: &manifest, commitment };
        Self::from_facts(context, facts)
    }
    /// Cold source obtains the same facts only from an original receipt
    /// index of this exact context, never by examining the host's contents.
    pub(in crate::engine) fn original(
        context: &PlanningContext<'_>, repo: SnapshotRepositoryId,
    ) -> Result<Self> {
        let index: ReceiptIndex<'_> = context.snapshot_receipts().map_err(crate::business::file_draft::worktree::failure)?;
        let facts = index.initial_pending_material(repo).map_err(crate::business::file_draft::worktree::failure)?;
        Self::from_facts(context, facts)
    }
    pub(super) fn from_facts(context: &PlanningContext<'_>, facts: InitialDirectoryMaterial<'_>) -> Result<Self> {
        let InitialDirectoryMaterial { original, request, host, allocated, branch_name, manifest, commitment } = facts;
        let InitializeSnapshot { host: requested, source, operation_id } = request;
        let orig = origin(original)?;
        let reserve = orig.operation_id;
        check(original.kind == NODE_RESERVE && *operation_id == reserve && allocated.completion != reserve
            && manifest.repository_id() == allocated.repository && manifest.node_id() == allocated.node
            && manifest.operation_id() == reserve && source.branch_id == host.namespace.branch_id)?;
        let host = BoundHost::initial(context, *requested, host)?;
        let mut rows = KeyedBuilder::new(MAX_ROWS);
        host.append_steps(orig, &mut rows)?;
        rows.push(&Key::Branch { branch_id: allocated.branch }, &Row { source: orig,
            value: Branch { branch_id: allocated.branch, name: branch_name, creation_operation: reserve,
                state: BranchState::Pending { reservation: reserve } } })?;
        rows.push(&Key::Pending { reserve_operation: reserve }, &Row { source: orig, value: Pending {
            record_kind: NODE_RESERVE, commitment, target: Target::InitialNode { allocated, material: manifest.reference() },
            binding: LocalBinding::NotBound {},
        } })?;
        for (index, entry) in manifest.entries().iter().enumerate() {
            let key = Key::PendingItem { reserve_operation: reserve, role: ItemRole::NodeEntry,
                source_ordinal: Sequence::new(index as u64), item_ordinal: Sequence::new(0) };
            rows.push(&key, &Row { source: orig, value: PlannedEntry { entry } })?;
        }
        // Positive recipe, no *originally bound* repository body. May not be
        // reported as an intact installed repo, no matter what a path contains.
        let frozen = FrozenDirectory::new(allocated.repository, &host, reserve, rows.finish()?,
            OrdinalBuilder::new(0).finish()?)?;
        frozen.bytes()?;
        Ok(Self { host, frozen })
    }
    pub(in crate::engine) fn require(&self, config: &ValidatedConfig) -> Result<()> { self.host.check(config) }
    pub(in crate::engine) fn root(&self) -> MaterialCommitment { self.frozen.descriptor.hash() }
    pub(in crate::engine) fn reservation(&self) -> OperationId { self.frozen.header.initialization_operation_id }
    pub(in crate::engine) fn bytes(&self) -> Result<usize> { self.frozen.bytes() }
    pub(in crate::engine) fn host_entity(&self) -> FileIdentity {
        crate::working_tree::records::file_identity(self.host.native.host_entity)
    }
    /// Emits precisely the positive known reservation row, never the other
    /// kind used for a physically complete repository.
    pub(in crate::engine) fn append_to(&self, disk_rows: &mut OrdinalBuilder) -> Result<()> {
        let h = &self.host.native;
        let reserve = self.frozen.header.initialization_operation_id;
        disk_rows.push(&InventoryRow {
            object: Object::DirectoryRepositoryReservation { repository_id: self.frozen.header.repository_id,
                directory_id: h.directory_id, reserve_operation: reserve, material: &self.frozen.material },
            ownership: Ownership::Original { registration: &h.registration },
            location: Location::Anchor { root_id: h.root_id, directory_id: h.directory_id },
            lifecycle: Lifecycle::Pending { reservation: reserve }, entity: h.host_entity, operation_id: reserve,
        })
    }
    pub(in crate::engine) fn stage_original(&self, config: &ValidatedConfig) -> Result<()> {
        self.frozen.stage(&self.host, config)
    }
    pub(in crate::engine) fn verify_original(&self, config: &ValidatedConfig) -> Result<()> {
        self.frozen.verify(&self.host, config)
    }
}
