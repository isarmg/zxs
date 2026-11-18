//! A reservation and a freshly reopened physical occurrence, not a receipt
//! shape, are needed to prepare a partial node. The guard keeps both original
//! and prospective checkpoints distinct through the native decision.
use super::{Originals, plan::Projection};
use crate::directory_worktree::ledger::{Checkpoint, HeldWorktreeLedger, LedgerError, content_checkpoints};
use crate::engine::{BudgetedOriginalContent, PlanningContext, Prepared};
use crate::persist::snapshot_worktree::content_batch::{Input, Output, Publication, Reservation};
use crate::snapshot_worktree::registry::FrozenContent;
use crate::snapshot_worktree::{ReceiptIndex, RegistrationError, Result};
use crate::working_tree::snapshot::{AdmittedSnapshotManifest, HeldContentNode, HeldSnapshotNode, PinnedSnapshotRepository};
use crate::{Engine, State};
use std::sync::Arc;
use storage_contract::Principal;
use storage_contract::working_tree::{DirectoryId, MaterialCommitment};

pub(crate) struct OriginalContentInput {
    originals: Originals,
    plan: Reservation,
    commitment: MaterialCommitment,
    preparation: DirectoryId,
    projection: Projection,
    checkpoints: [Checkpoint; 2],
    principal: Principal,
}

impl OriginalContentInput {
    pub(in crate::snapshot_worktree) fn capture(engine: &Engine, index: &ReceiptIndex<'_>,
        frozen: &FrozenContent<'_>, principal: &Principal) -> Result<Self> {
        if !frozen.belongs_to(principal) { return Err(RegistrationError::Digest(frozen.plan().request.operation_id)); }
        let originals = Originals::retained(engine, index, frozen)?;
        let projection = originals.project_reserved(&engine.planning_context(), principal, frozen.plan())?;
        let ledger = HeldWorktreeLedger::reopen(&originals.source_node, originals.ledger, originals.before,
            |_, _| Err(LedgerError::PublicationPending)).map_err(RegistrationError::Ledger)?;
        if ledger.instance() != frozen.plan().instance { return Err(RegistrationError::CommitScope); }
        let events = frozen.plan().events(&projection.rows);
        let checkpoints = ledger.preview_content(&events).map_err(RegistrationError::Ledger)?;
        if checkpoints != content_checkpoints(frozen.plan().before, &events).map_err(RegistrationError::Ledger)? {
            return Err(RegistrationError::CommitScope);
        }
        drop(ledger);
        let preparation = DirectoryId::generate().map_err(RegistrationError::Allocation)?;
        if preparation == frozen.plan().root.directory_id || index.directories().contains_id(preparation) {
            return Err(RegistrationError::Identity);
        }
        Ok(Self { originals, projection, plan: frozen.plan().clone(), commitment: frozen.commitment(),
            preparation, checkpoints, principal: principal.clone() })
    }

    pub(crate) fn sources(&self) -> (&HeldSnapshotNode, &PinnedSnapshotRepository,
        &crate::directory_worktree::store::HeldSparseWorktree) {
        (&self.originals.source_node, &self.originals.repository, &self.originals.sparse)
    }
    pub(crate) fn preparation(&self) -> DirectoryId { self.preparation }
    pub(crate) fn rows(&self) -> &[Input] { &self.plan.rows }
    pub(crate) fn manifest(&self) -> &AdmittedSnapshotManifest { &self.projection.manifest }
    pub(crate) fn prepared(&self) -> &[Prepared] { &self.projection.versions }
    pub(crate) fn state(&self) -> &State { &self.projection.state }
    pub(crate) fn plan(&self) -> &Reservation { &self.plan }

    pub(crate) fn require_context(&self, ctx: &PlanningContext<'_>) -> Result<()> {
        self.originals.require_context(ctx)?;
        self.originals.repository.require_held_source(&self.originals.source_node)
            .map_err(RegistrationError::NodeReadback)?;
        for row in &self.originals.rows { row.container.verify().map_err(RegistrationError::Publication)?; }
        Ok(())
    }

    pub(crate) fn directory_bytes(&self) -> Result<u64> {
        let dirty = self.plan.rows.iter().map(|r| r.edit.material.version_id).collect::<std::collections::BTreeSet<_>>();
        let mut bytes = self.projection.manifest.bytes().len() as u64;
        for (version, file) in self.projection.manifest.files() {
            if !dirty.contains(version) {
                bytes = bytes.checked_add(file.size).ok_or(RegistrationError::CommitScope)?;
            }
        }
        let events = self.plan.events(&self.projection.rows);
        let frames = crate::directory_worktree::ledger::content_frame_bytes(self.plan.before, &events)
            .map_err(RegistrationError::Ledger)?;
        bytes.checked_add(frames).ok_or(RegistrationError::CommitScope)
    }

    /// Encode-only maximum. This is never an actual PreparedNode/binding,
    /// never enters State and can never authorize adopting an on-disk inode.
    pub(crate) fn result_budget(&self) -> Result<usize> {
        use data_serialization::Serialize;
        #[derive(Serialize)]
        struct Node<'a, T> { preparation: DirectoryId, binding: T,
            entries: &'a [crate::working_tree::snapshot::SnapshotEntry] }
        #[derive(Serialize)]
        struct Body<'a, T> { reserve: storage_contract::OperationId, reservation: MaterialCommitment,
            prepared_node: Node<'a, T>, rows: &'a [Output], committing: Checkpoint, completed: Checkpoint }
        #[derive(Serialize)]
        struct Record<T> { kind: &'static str, record: T }
        #[derive(Serialize)]
        struct Envelope<T> { snapshot_worktree: Record<T>, timestamp: String, membership_epoch: String,
            version_protection: Vec<crate::persist::file_draft::Protection> }
        let version_protection = self.plan.rows.iter().zip(&self.projection.rows).map(|(i,o)|
            crate::persist::file_draft::content_protection_for_owner(i.owner, &o.content)
                .map_err(RegistrationError::Publication)).collect::<Result<Vec<_>>>()?;
        let body = Body { reserve: self.plan.request.operation_id, reservation: self.commitment,
            prepared_node: Node { preparation: self.preparation, binding: self.plan.source_binding.budget_encoding(
                self.manifest().reference()), entries: self.manifest().entries() },
            rows: &self.projection.rows, committing: self.checkpoints[0], completed: self.checkpoints[1] };
        storage_contract::canonical_json(&Envelope { snapshot_worktree: Record { kind: "publish_content_batch", record: body },
            timestamp: u64::MAX.to_string(), membership_epoch: u64::MAX.to_string(), version_protection })
            .map(|bytes| bytes.len()).map_err(|e| RegistrationError::Codec(
                crate::persist::snapshot_worktree::CodecError::Encoding(e)))
    }

    pub(crate) fn stage(self, context: &PlanningContext<'_>, permit: BudgetedOriginalContent)
        -> Result<(PreparedOriginalContent, State, Vec<Prepared>)>
    {
        self.require_context(context)?;
        let node = HeldContentNode::stage(&self, &permit).map_err(RegistrationError::NodePreparation)?;
        let result = Publication { reserve: self.plan.request.operation_id, reservation: self.commitment,
            prepared_node: node.facts(), rows: self.projection.rows, committing: self.checkpoints[0], completed: self.checkpoints[1] };
        let held = PreparedOriginalContent { originals: self.originals, plan: self.plan, result,
            principal: self.principal, node, _permit: permit };
        held.require_before(context)?;
        Ok((held, self.projection.state, self.projection.versions))
    }
}

pub(crate) struct PreparedOriginalContent {
    originals: Originals,
    plan: Reservation,
    result: Publication,
    principal: Principal,
    node: HeldContentNode,
    _permit: BudgetedOriginalContent,
}

impl PreparedOriginalContent {
    pub(crate) fn result(&self) -> &Publication { &self.result }
    pub(crate) fn plan(&self) -> &Reservation { &self.plan }
    pub(crate) fn principal(&self) -> &Principal { &self.principal }
    pub(crate) fn require_before(&self, ctx: &PlanningContext<'_>) -> Result<()> {
        self.originals.require_context(ctx)?;
        let sparse = crate::directory_worktree::store::HeldSparseWorktree::reopen(
            &self.originals.source_node, self.plan.instance).map_err(RegistrationError::EditInstance)?;
        self.node.require_before(&self.originals.source_node, &self.originals.repository, &sparse,
            self.plan.rows.iter().map(|r| &r.edit)).map_err(RegistrationError::NodeReadback)?;
        for (row, expected) in self.originals.rows.iter().zip(&self.plan.rows) {
            let source = self.originals.state.versions.get(&expected.original.version_id.to_string())
                .ok_or(RegistrationError::Source)?;
            row.container.verify_version_body(source).map_err(RegistrationError::Publication)?;
        }
        Ok(())
    }
    pub(crate) fn matches_source(&self, state: &Arc<State>) -> bool { Arc::ptr_eq(&self.originals.state, state) }
}
