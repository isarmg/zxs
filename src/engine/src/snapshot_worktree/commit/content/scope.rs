//! A scoped DISK producer may borrow only this selected, physically held
//! worktree occurrence. None of these views is a stored record, a complete
//! disk inventory, a writer, or a substitute for the final Candidate.
use super::material::PreparedOriginalContent;
use crate::business::file_draft::worktree::failure;
use crate::directory_worktree::ledger::{Binding, Checkpoint, HeldWorktreeLedger, LedgerError};
use crate::directory_worktree::store::SparseWorktreeBinding;
use crate::engine::PlanningContext;
use crate::persist::layout::{snapshot as node_layout, worktree as ledger_layout};
use crate::persist::snapshot_worktree::{HostEvidence, content_batch::{Input, Output}};
use crate::snapshot_worktree::{RegistrationError, Result};
use crate::working_tree::snapshot::{HeldSnapshotNode, SnapshotEntry, SnapshotFile, SnapshotNodeBinding};
use platform_io::fs_safety::EntryName;
use storage_contract::{ContentHash, OperationId, Sequence, VersionId};
use storage_contract::snapshot_worktree::{SnapshotHost, WorktreeHandle};
use storage_contract::working_tree::{DirectoryId, LocalEntityIdentity};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NodeImage { Source, Destination }

/// One actual leaf in one actual parent. Relative names may repeat in
/// different files areas. The parent and immutable registration cannot.
pub(crate) struct PhysicalLeaf {
    pub(crate) parent: LocalEntityIdentity,
    pub(crate) name: EntryName,
    pub(crate) entity: LocalEntityIdentity,
    pub(crate) bytes: u64,
    pub(crate) hash: ContentHash,
}

pub(crate) enum BodyLocation {
    Present(PhysicalLeaf),
    /// The destination leaf does not yet exist. Its inode must subsequently
    /// move from this exact sparse leaf, not be reconstructed from its hash.
    ReservedMove { held: PhysicalLeaf, destination_parent: LocalEntityIdentity, destination_name: EntryName },
}

pub(crate) enum LedgerStage { Original { entity: LocalEntityIdentity }, ProspectiveCommit, ProspectiveCompleted }

pub(crate) enum ContentMaterial<'a> {
    Manifest { image: NodeImage, binding: SnapshotNodeBinding, operation: OperationId,
        file: PhysicalLeaf, canonical: &'a [u8] },
    /// Keeps every exact original path and FILE selector. Aliases and
    /// SnapshotOnly/empty directories are not dropped when bodies coincide.
    Entry { image: NodeImage, binding: SnapshotNodeBinding, entry: &'a SnapshotEntry },
    Body { image: NodeImage, binding: SnapshotNodeBinding, material: SnapshotFile,
        operation: OperationId, location: BodyLocation },
    Selected { original: &'a Input, result: &'a Output },
    Frame { stage: LedgerStage, checkpoint: Checkpoint, parent: LocalEntityIdentity,
        name: EntryName, bytes: &'a [u8], hash: ContentHash },
}

pub(crate) struct OriginalContentScope<'a> {
    owner: &'a PreparedOriginalContent,
    ledger: HeldWorktreeLedger<'a>,
}

impl PreparedOriginalContent {
    pub(crate) fn material_scope(&self, ctx: &PlanningContext<'_>) -> Result<OriginalContentScope<'_>> {
        self.require_before(ctx)?;
        self.original_material_loan()
    }
    fn original_material_loan(&self) -> Result<OriginalContentScope<'_>> {
        let ledger = HeldWorktreeLedger::reopen(&self.originals.source_node, self.plan.ledger, self.plan.before,
            |_, _| Err(LedgerError::PublicationPending)).map_err(RegistrationError::Ledger)?;
        Ok(OriginalContentScope { owner: self, ledger })
    }
    /// When an owner has consumed this loan, repeat precisely the original
    /// physical join before ACCEPT. Replacing an unchanged body/manifest/frame
    /// with an equal-byte different inode invalidates the entire planned root.
    pub(super) fn recheck_material_scope(&self) -> Result<()> {
        if self.scope_entities.get().is_some() {
            self.original_material_loan()?.visit(|_| Ok(())).map_err(RegistrationError::Publication)?;
        }
        Ok(())
    }

}

impl OriginalContentScope<'_> {
    pub(crate) fn host(&self) -> (&HostEvidence, SnapshotHost) { (&self.owner.plan.host, self.owner.plan.root) }
    pub(crate) fn worktree(&self) -> (WorktreeHandle, SparseWorktreeBinding, Binding) {
        (self.owner.plan.request.target, self.owner.plan.instance, self.owner.plan.ledger)
    }
    pub(crate) fn operations(&self) -> (OperationId, OperationId) {
        (self.owner.plan.request.operation_id, self.owner.plan.publish_operation)
    }
    pub(crate) fn nodes(&self) -> (SnapshotNodeBinding, DirectoryId, SnapshotNodeBinding) {
        (self.owner.plan.source_binding, self.owner.result.prepared_node.preparation, self.owner.result.prepared_node.binding)
    }
    pub(crate) fn checkpoints(&self) -> (Checkpoint, Checkpoint, Checkpoint) {
        (self.owner.plan.before, self.owner.result.committing, self.owner.result.completed)
    }
    pub(crate) fn revision(&self) -> &storage_contract::snapshot_worktree::WorktreeRevision {
        &self.owner.plan.request.expected
    }

    /// Source before result. Frames are in original chain order, then Commit,
    /// then Completed. At most one media frame or two prospective bounded
    /// frames is materialized; bodies stream through the native verifier.
    pub(crate) fn visit(&self, mut emit: impl FnMut(ContentMaterial<'_>) -> crate::Result<()>) -> crate::Result<()> {
        let mut entities = Vec::new();
        self.visit_raw(|row| {
            match &row {
                ContentMaterial::Manifest { file, .. } | ContentMaterial::Body { location: BodyLocation::Present(file), .. } =>
                    entities.push(file.entity),
                ContentMaterial::Body { location: BodyLocation::ReservedMove { held, .. }, .. } => entities.push(held.entity),
                ContentMaterial::Frame { stage: LedgerStage::Original { entity }, .. } => entities.push(*entity),
                _ => {}
            }
            emit(row)
        })?;
        // Structural order, full addresses, bytes and parent bindings above
        // derive from the same admitted original and frozen plan on every call.
        // Only physical leaf identities can otherwise vary across equal bytes.
        let entities = entities.into_boxed_slice();
        if let Err(actual) = self.owner.scope_entities.set(entities)
            && self.owner.scope_entities.get().map(AsRef::as_ref) != Some(actual.as_ref())
        { return Err(failure(RegistrationError::PublicationChanged)); }
        Ok(())
    }

    fn visit_raw(&self, mut emit: impl FnMut(ContentMaterial<'_>) -> crate::Result<()>) -> crate::Result<()> {
        let owner = self.owner;
        self.visit_node(NodeImage::Source, &owner.originals.source_node, &mut emit)?;
        self.visit_node(NodeImage::Destination, owner.node.staged_node(), &mut emit)?;
        for (original, result) in owner.plan.rows.iter().zip(&owner.result.rows) {
            emit(ContentMaterial::Selected { original, result })?;
        }
        let parent = owner.plan.ledger.ledger_entity();
        let mut previous = None;
        let mut total = 0usize;
        for i in 0..=owner.plan.before.sequence.get() {
            let sequence = Sequence::new(i);
            let (checkpoint, entity, bytes) = self.ledger.observed_content_frame(sequence, previous)
                .map_err(|e| failure(RegistrationError::Ledger(e)))?;
            total = total.checked_add(bytes.len()).ok_or_else(|| failure(RegistrationError::Ledger(LedgerError::Budget)))?;
            if total > self.ledger.admitted_bytes() || (i == 0 && checkpoint != owner.plan.ledger.initial_checkpoint())
                || (sequence == owner.plan.before.sequence && checkpoint != owner.plan.before)
            { return Err(failure(RegistrationError::Ledger(LedgerError::Chain))); }
            previous = Some(checkpoint.commitment);
            emit(ContentMaterial::Frame { stage: LedgerStage::Original { entity }, checkpoint, parent,
                name: ledger_layout::frame(sequence), hash: ContentHash::from_digest(blake3::hash(&bytes)), bytes: &bytes })?;
        }
        if total != self.ledger.admitted_bytes() { return Err(failure(RegistrationError::Ledger(LedgerError::Chain))); }
        let frames = self.ledger.prospective_content_frames(&owner.plan.events(&owner.result.rows))
            .map_err(|e| failure(RegistrationError::Ledger(e)))?;
        for (((checkpoint, bytes), expected), stage) in frames.into_iter()
            .zip([owner.result.committing, owner.result.completed]).zip([LedgerStage::ProspectiveCommit, LedgerStage::ProspectiveCompleted]) {
            if checkpoint != expected { return Err(failure(RegistrationError::Ledger(LedgerError::Chain))); }
            emit(ContentMaterial::Frame { stage, checkpoint, parent, name: ledger_layout::frame(checkpoint.sequence),
                hash: ContentHash::from_digest(blake3::hash(&bytes)), bytes: &bytes })?;
        }
        Ok(())
    }

    fn visit_node(&self, image: NodeImage, node: &HeldSnapshotNode,
        emit: &mut impl FnMut(ContentMaterial<'_>) -> crate::Result<()>) -> crate::Result<()> {
        let manifest = node.manifest();
        let binding = node.binding();
        let canonical = manifest.bytes();
        let (parent, entity) = node.observed_manifest_entity().map_err(|e| failure(RegistrationError::NodeReadback(e)))?;
        emit(ContentMaterial::Manifest { image, binding, operation: manifest.operation_id(), canonical,
            file: PhysicalLeaf { parent, name: node_layout::MANIFEST.clone(), entity, bytes: canonical.len() as u64,
                hash: ContentHash::from_digest(blake3::hash(canonical)) } })?;
        for entry in manifest.entries() { emit(ContentMaterial::Entry { image, binding, entry })?; }
        let moved = self.owner.plan.rows.iter().map(|r| (r.edit.material.version_id, r))
            .collect::<std::collections::BTreeMap<VersionId, _>>();
        for (version, material) in manifest.files() {
            let (operation, location) = if let Some(input) = moved.get(version).filter(|_| image == NodeImage::Destination) {
                let edit = &input.edit;
                let file = self.owner.originals.sparse.open_material(&edit.file_edit().material)
                    .map_err(|e| failure(RegistrationError::EditInstance(e)))?;
                let actual = crate::working_tree::records::local_entity(platform_io::fs_safety::linux::FileIdentity::from_metadata(
                    &file.metadata().map_err(|e| failure(RegistrationError::CommitRead(e)))?));
                if actual != edit.entity || *material != edit.material { return Err(failure(RegistrationError::CommitScope)); }
                (input.edit_operation, BodyLocation::ReservedMove { held: PhysicalLeaf { parent: self.owner.plan.instance.worktree_entity(),
                    name: edit.leaf.entry().clone(), entity: actual, bytes: material.size, hash: material.content_hash },
                    destination_parent: binding.files_entity(), destination_name: node_layout::file(&manifest.payloads()[version]) })
            } else {
                let (name, parent, entity) = node.observed_body_entity(*version)
                    .map_err(|e| failure(RegistrationError::NodeReadback(e)))?;
                (manifest.operation_id(), BodyLocation::Present(PhysicalLeaf { parent, name, entity, bytes: material.size,
                    hash: material.content_hash }))
            };
            emit(ContentMaterial::Body { image, binding, material: *material, operation, location })?;
        }
        Ok(())
    }
}
