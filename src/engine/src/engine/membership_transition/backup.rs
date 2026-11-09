//! Native all-original-member source barrier for an explicit backup cutover.
//! The effective Config remains the original one. No target is a voter or an
//! active member, and ordinary Add/Joint activation cannot consume this origin.
use super::*;
mod generation;
mod registry;
use crate::engine::backup_preparation::BackupSourceFenceEvidence;
pub(in crate::engine::membership_transition) use generation::BackupCore;
pub(in crate::engine) use generation::PreparedBackupAuthority;
pub(crate) use generation::PreparedBackupFirstCandidate;
pub(in crate::engine) use registry::{
    AdmittedBackupDecision, BackupChoiceContext, BackupFrozenControl,
    PreparedBackupFirstPublication,
};
pub use registry::{
    BackupFirstFinalizeReport, BackupFirstPublicationHash, BackupFirstPublicationReport,
    BackupRegistryDecisionHash,
};

pub(super) fn verify_intent(intent: &TransitionIntent) -> Result<()> {
    let Origin::BackupCutover { config, evidence } = &intent.origin else {
        return Err(Error::new(
            ErrorCode::ConfigConflict,
            "source barrier has no backup cutover origin",
        ));
    };
    let boundary = intent.boundary.as_ref().ok_or_else(|| {
        Error::new(
            ErrorCode::RecoveryRequired,
            "backup source fence requires its exact published boundary",
        )
    })?;
    if intent.fence_sequence <= boundary.sequence
        || intent.fence_sequence == u64::MAX
        || intent.effect.is_some()
        || intent.fence_scope.is_some()
        || !intent.added_roots.is_empty()
        || canonical_json(config)? != canonical_json(&intent.target)?
    {
        return Err(Error::new(
            ErrorCode::ConfigConflict,
            "backup source fence cannot replace the original Config, voters, registration or roots",
        ));
    }
    evidence.validate(
        config,
        intent.operation_id,
        &intent.principal,
        boundary.sequence,
        &boundary.snapshot_hash,
    )
}

impl Engine {
    /// Freeze the complete original generation only after every selected target
    /// has independently verified its original cutover inputs. Every old member
    /// durably records the same immutable intent. Source data is never deleted.
    ///
    /// This is a potentially long-lived **whole-pool write stop**, not completed
    /// migration. Failures may have fenced part of the original membership;
    /// retry only this operation/actor, or use the certified original-quorum
    /// cancellation protocol. Final target activation is not implemented.
    pub fn begin_backup_cutover_fence(
        &mut self,
        operation_id: OperationId,
        principal: &str,
    ) -> Result<MembershipFenceStatus> {
        Self::require_actor(principal)?;
        self.require_no_commit_preparation()?;
        if self.membership_fence.is_some() {
            self.require_backup_fence_request(operation_id, principal)?;
            return self.persist_membership_fence();
        }
        let prepared = self.prepare_backup_source_fence(operation_id, principal)?;
        if self.state.operations.contains_key(&operation_id) {
            return Err(Error::new(
                ErrorCode::IdempotencyKeyReused,
                "backup source fence operation already has a permanent business receipt",
            ));
        }
        let members = self.members();
        self.validate_fence_members(&members)?;
        if self.journals.values().any(JournalWriter::needs_recovery) {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "backup source fence needs every original journal at a clean boundary",
            ));
        }
        let boundary = self.handoff_boundary(&members)?;
        let next_slot = self.next_slot.checked_add(1).ok_or_else(|| {
            Error::new(
                ErrorCode::ResourceLimit,
                "backup source fence sequence exhausted",
            )
        })?;
        let intent = TransitionIntent {
            operation_id,
            principal: principal.into(),
            origin: Origin::BackupCutover {
                config: self.config.clone(),
                evidence: Box::new(prepared.evidence.clone()),
            },
            target: self.config.clone(),
            added_roots: BTreeMap::new(),
            effect: None,
            fence_scope: None,
            boundary,
            fence_sequence: self.next_slot,
        };
        verify_intent(&intent)?;
        let object = PreparedMetadataObject::new(INTENT_KIND, &intent, MAX_METADATA)?;
        prepared.recheck_targets()?;
        // Conservative local stop precedes any uncertain object/journal write.
        // Cold admission derives the same stop from even a partial native fence.
        self.blocked = true;
        self.metadata_integrity.invalidate();
        self.published_state_base = None;
        self.next_slot = next_slot;
        self.membership_fence = Some(AdmittedFence {
            intent,
            hash: object.hash,
            receipts: BTreeMap::new(),
            prepared_roots: BTreeMap::new(),
        });
        let result = self.persist_membership_fence()?;
        prepared.recheck_targets()?;
        Ok(result)
    }

    fn require_backup_fence_request(
        &self,
        operation_id: OperationId,
        principal: &str,
    ) -> Result<()> {
        let fence = self.membership_fence.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "no native backup source fence is admitted",
            )
        })?;
        if !matches!(&fence.intent.origin, Origin::BackupCutover { .. })
            || fence.intent.operation_id != operation_id
            || fence.intent.principal != principal
        {
            return Err(Error::new(
                ErrorCode::IdempotencyKeyReused,
                "the pending source fence binds another operation, actor or authority kind",
            ));
        }
        verify_intent(&fence.intent)?;
        self.verify_membership_fence_prefix(Some(self.state.sequence), self.snapshot.as_deref())
    }

    /// Read-only capability for an isolated target copy. Only the same original
    /// native barrier can supply this evidence; a SYSTEM progress/task file
    /// cannot authorize it. It deliberately grants no target writer authority.
    pub(in crate::engine) fn fenced_backup_inputs(
        &self,
        operation_id: OperationId,
        principal: &str,
    ) -> Result<(String, BackupSourceFenceEvidence)> {
        self.require_backup_fence_request(operation_id, principal)?;
        let fence = self
            .membership_fence
            .as_ref()
            .expect("invariant: exact backup fence admitted");
        let Origin::BackupCutover { config, evidence } = &fence.intent.origin else {
            unreachable!("invariant: exact backup origin checked");
        };
        if !self.blocked
            || self.current_namespaces.is_some()
            || canonical_json(config)? != canonical_json(&self.config)?
            || object_hash("backup-prepared-state", self.state.as_ref())? != evidence.state_hash()
            || !self
                .membership_fence_status()
                .expect("invariant: native fence present")
                .all_members_fenced()
        {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "target installation requires the complete original native barrier and unchanged source State; no targets opened",
            ));
        }
        self.validate_fence_members(&fence.intent.fence_members_at(&self.config)?)?;
        Ok((fence.hash.clone(), (**evidence).clone()))
    }

    /// Actual admitted native fence receipts, never the target preparation
    /// progress cache. A quorum-fenced source does not authorize new targets.
    pub fn backup_cutover_fence_status(&self) -> Option<MembershipFenceStatus> {
        self.membership_fence
            .as_ref()
            .filter(|fence| matches!(&fence.intent.origin, Origin::BackupCutover { .. }))?;
        self.membership_fence_status()
    }
}

/// Parsed inputs alone cannot select this origin. Only the live target verifier
/// can construct its in-memory capability before begin_backup_cutover_fence.
pub(super) type Evidence = BackupSourceFenceEvidence;
