//! Physical node input from the real held directory/FILE plan, never a path.
use crate::engine::PlanningContext;
use crate::snapshot_worktree::drafts::{DirectoryDraftError, DirectoryNodeInput};
use crate::working_tree::snapshot::{
    AdmittedSnapshotManifest, HeldSnapshotNode, SnapshotNodeBinding,
    SnapshotManifestError, SnapshotPreparationError, SnapshotReference,
};
use storage_contract::{DiskId, OperationId};
use std::sync::Arc;

#[derive(Debug, error_definition::Error)]
pub(crate) enum DirectoryPhysicalError {
    #[error("checking the exact directory and selected FILE sources")]
    Source(#[source] DirectoryDraftError),
    #[error("directory preparation requires the same aggregate physical admission")]
    Admission(#[source] crate::Error),
    #[error("decoding the registered directory owner identity")]
    Identity(#[source] storage_contract::Error),
    #[error("the complete directory material exceeds its byte budget")]
    Budget,
    #[error("retaining the exact canonical directory manifest")]
    Manifest(#[source] SnapshotManifestError),
    #[error("preparing the independently complete directory node")]
    Preparation(#[source] SnapshotPreparationError),
}

type Result<T> = std::result::Result<T, DirectoryPhysicalError>;

/// A charge description. It does not reserve capacity or authorize creation.
pub(crate) struct DirectoryBodyCost {
    payload_bytes: u64,
    manifest_bytes: u64,
    total_bytes: u64,
    payload_count: usize,
}

impl DirectoryBodyCost {
    pub(crate) fn payload_bytes(&self) -> u64 { self.payload_bytes }
    pub(crate) fn manifest_bytes(&self) -> u64 { self.manifest_bytes }
    pub(crate) fn total_bytes(&self) -> u64 { self.total_bytes }
    pub(crate) fn payload_count(&self) -> usize { self.payload_count }
    // Changed nodes retain node/files/worktrees. Unchanged material reopens
    // those three for the same complete readback, so both retain the bound.
    pub(crate) fn retained_descriptors(&self) -> usize { 3 }
}

pub(crate) struct DirectoryPhysicalInput {
    input: DirectoryNodeInput,
    manifest: AdmittedSnapshotManifest,
    owner: DiskId,
    cost: DirectoryBodyCost,
    unchanged: bool,
}

impl DirectoryPhysicalInput {
    pub(in crate::snapshot_worktree::drafts) fn new(
        input: DirectoryNodeInput,
        manifest: AdmittedSnapshotManifest,
        context: &PlanningContext<'_>,
    ) -> Result<Self> {
        input.require_context(context).map_err(DirectoryPhysicalError::Source)?;
        input.bind_proposed_manifest(&manifest).map_err(DirectoryPhysicalError::Source)?;
        let unchanged = manifest.entries() == input.source().original().manifest().entries();
        let manifest = if unchanged {
            input.source().original().manifest().clone()
        } else {
            manifest
        };
        let payload_bytes = if unchanged { 0 } else { manifest.payloads().keys().try_fold(
            0u64, |sum, version| {
            sum.checked_add(manifest.files()[version].size)
                .ok_or(DirectoryPhysicalError::Budget)
        })? };
        // Admission limits canonical bytes to MAX_MANIFEST_BYTES (4 MiB).
        let manifest_bytes = if unchanged { 0 } else { manifest.encoded_len() as u64 };
        let total_bytes = payload_bytes.checked_add(manifest_bytes)
            .ok_or(DirectoryPhysicalError::Budget)?;
        let cost = DirectoryBodyCost {
            payload_bytes, manifest_bytes, total_bytes,
            payload_count: if unchanged { 0 } else { manifest.payloads().len() },
        };
        let owner = input.source().directory().host().registration.disk_id.parse()
            .map_err(DirectoryPhysicalError::Identity)?;
        Ok(Self { input, manifest, owner, cost, unchanged })
    }

    pub(crate) fn owner(&self) -> DiskId { self.owner }
    pub(crate) fn operation(&self) -> OperationId { self.input.source().directory().operation() }
    pub(crate) fn is_unchanged(&self) -> bool { self.unchanged }
    pub(crate) fn reference(&self) -> SnapshotReference { self.manifest.reference() }
    pub(crate) fn original_binding(&self) -> SnapshotNodeBinding {
        self.input.source().original().binding()
    }
    pub(crate) fn cost(&self) -> &DirectoryBodyCost { &self.cost }
    pub(crate) fn manifest(&self) -> &AdmittedSnapshotManifest { &self.manifest }

    pub(crate) fn require_context(&self, context: &PlanningContext<'_>) -> Result<()> {
        context.require_manual_directory_pool().map_err(DirectoryPhysicalError::Admission)?;
        self.input.require_context(context).map_err(DirectoryPhysicalError::Source)
    }

    /// Only the publication budget phase issues this permit after FILE and
    /// directory costs plus prospective metadata admission. The complete radix
    /// graph is checked again once the real node feedback closes the receipt.
    pub(crate) fn prepare(
        self: Arc<Self>,
        context: &PlanningContext<'_>,
        permit: crate::engine::BudgetedDirectoryPreparation,
    ) -> Result<PreparedDirectoryMaterial> {
        self.require_context(context)?;
        permit.require_input(context, &self).map_err(DirectoryPhysicalError::Admission)?;
        if self.unchanged {
            self.input.require_prepared(self.input.source().original())
                .map_err(DirectoryPhysicalError::Source)?;
            return Ok(PreparedDirectoryMaterial { input: self, node: None, permit });
        }
        // Only the immutable bounded manifest value is copied. The actual
        // source occurrence, held parent and budget permit retain this same Arc.
        let manifest = self.manifest.clone();
        let node = self.input.prepare_node(manifest, &permit)
            .map_err(DirectoryPhysicalError::Preparation)?;
        self.input.require_context(context).map_err(DirectoryPhysicalError::Source)?;
        self.input.require_prepared(&node).map_err(DirectoryPhysicalError::Source)?;
        Ok(PreparedDirectoryMaterial { input: self, node: Some(node), permit })
    }
}

/// An actual EXCL node, or the same complete original node for no-change.
/// No registration or success result
/// is supplied until the caller incorporates its binding into the one decision.
pub(crate) struct PreparedDirectoryMaterial {
    input: Arc<DirectoryPhysicalInput>,
    node: Option<HeldSnapshotNode>,
    // Declared last: the accounted descriptors close before their lease.
    permit: crate::engine::BudgetedDirectoryPreparation,
}

impl PreparedDirectoryMaterial {
    pub(crate) fn require_input(&self, input: &Arc<DirectoryPhysicalInput>) -> Result<()> {
        if !Arc::ptr_eq(&self.input, input) {
            return Err(DirectoryPhysicalError::Source(DirectoryDraftError::SourceChanged));
        }
        self.require_material()
    }

    fn node(&self) -> &HeldSnapshotNode {
        match &self.node {
            Some(node) => node,
            None => self.input.input.source().original(),
        }
    }

    pub(crate) fn binding(&self) -> SnapshotNodeBinding { self.node().binding() }
    pub(crate) fn manifest(&self) -> &AdmittedSnapshotManifest { self.node().manifest() }
    pub(crate) fn owner(&self) -> DiskId { self.input.owner() }
    pub(crate) fn require_context(&self, context: &PlanningContext<'_>) -> Result<()> {
        self.input.require_context(context)?;
        self.require_material()
    }
    /// PREPARE itself advances the slot. This observes only the same retained
    /// entities; the enclosing Candidate must separately bind State and epoch.
    pub(crate) fn require_material(&self) -> Result<()> {
        self.permit.require_node(self.node().binding().reference(), self.input.original_binding())
            .map_err(DirectoryPhysicalError::Admission)?;
        self.input.input.require_prepared(self.node()).map_err(DirectoryPhysicalError::Source)
    }
}
