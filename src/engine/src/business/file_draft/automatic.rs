//! The private producer of automatic-node requests. A public Commit command,
//! file name, matching bytes, or mode flag cannot supply an original save.
use super::{DraftError, PublishedContent, invalid, origin::{NodeOrigin, Reason}};
use crate::{Error, OperationResult, Result, State};
use data_serialization::{Deserialize, Serialize};
use storage_contract::file_draft::{AutomaticBody, DraftLocation, FileWorkingState, SaveController,
    automatic::{AutomaticNodeDecision, SaveDecision}};
use storage_contract::{Generation, OperationId, Principal, Sequence, Timestamp};

pub(crate) const MESSAGE: &str = "Automatic working save";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Saved {
    pub(crate) epoch: Generation,
    pub(crate) decision: SaveDecision,
    digest: String,
}

impl Saved {
    pub(crate) fn capture(state: &State, target: &DraftLocation, original: &OperationResult,
        principal: &Principal) -> Result<Self>
    {
        let record = state.file_containers.get(&target.file).ok_or_else(invalid)?;
        let controller = record.working.automatic_controller().ok_or_else(stale)?;
        let save = *controller.pending_save().ok_or_else(stale)?;
        let result = Self { epoch: controller.epoch(), decision: save, digest: original.request_digest.clone() };
        result.require_original(state, target, original, principal)?;
        Ok(result)
    }
    pub(crate) fn reason(&self) -> Reason {
        Reason::AutomaticSave { epoch: self.epoch, save: self.decision }
    }
    pub(crate) fn operation(&self, target: &DraftLocation) -> Result<OperationId> {
        // This is a task ID, never a content identity. Every call additionally
        // binds principal, complete native request, original decision and source.
        let bytes = storage_contract::canonical_json(&("ordinary.file.automatic.publish", target, self))?;
        let hash = blake3::hash(&bytes);
        let mut id = [0; 16]; id.copy_from_slice(&hash.as_bytes()[..16]);
        Ok(OperationId::from_random(id))
    }
    pub(crate) fn require_original(&self, state: &State, target: &DraftLocation,
        original: &OperationResult, principal: &Principal) -> Result<()>
    {
        let record = state.file_containers.get(&target.file).ok_or_else(invalid)?;
        let controller = record.working.automatic_controller().ok_or_else(stale)?;
        let save = self.decision;
        if controller.epoch() != self.epoch || controller.protection().is_some()
            || controller.pending_save() != Some(&save) || record.working.revision() != save.revision
            || self.digest != original.request_digest || original.operation_id != save.operation
            || original.sequence != save.sequence.get()
            || state.operation_principals.get(&save.operation).map(String::as_str) != Some(principal.as_str())
        { return Err(stale()); }
        let mode = SaveController::Automatic { epoch: self.epoch };
        if let Some(unborn) = record.working.unborn() {
            let decoded = crate::persist::file_draft::unborn_save::decode(original)?;
            let request = decoded.request();
            if &request.target != target || request.controller != mode
                || request.expected.next().map_err(|e| Error::FileDraft(DraftError::RevisionExhausted(e)))? != save.revision
                || decoded.timestamp() != save.server_time
                || !matches!(unborn.body(), storage_contract::file_draft::UnbornBody::Saved { manifest } if manifest == &request.manifest)
                || decoded.receipt(save.operation, save.sequence, principal)? != *original
            { return Err(invalid()); }
        } else {
            let decoded = crate::persist::file_draft::decode(state, original)?;
            if original.kind != "file.draft.write" || decoded.outcome().request.target() != target
                || decoded.outcome().working != record.working || decoded.timestamp() != save.server_time
                || decoded.receipt(state, save.operation, original.sequence, principal.as_str())? != *original
            { return Err(invalid()); }
        }
        Ok(())
    }

    /// The admitted predecessor controller contains the original successful save
    /// even when a later visible degraded outcome has different health metadata.
    /// Hot selection separately requires the complete original receipt cursor.
    pub(crate) fn require_fence(&self, state: &State, target: &DraftLocation,
        principal: &str, operation: OperationId) -> Result<()> {
        let record = state.file_containers.get(&target.file).ok_or_else(invalid)?;
        let controller = record.working.automatic_controller().ok_or_else(stale)?;
        let visible = state.operations.get(&self.decision.operation).ok_or_else(invalid)?;
        if controller.epoch() != self.epoch || controller.protection().is_some()
            || controller.pending_save() != Some(&self.decision) || record.working.revision() != self.decision.revision
            || self.operation(target)? != operation || self.digest != visible.request_digest
            || self.decision.sequence.get() != visible.sequence
            || state.operation_principals.get(&self.decision.operation).map(String::as_str) != Some(principal)
        { return Err(stale()); }
        Ok(())
    }

    pub(crate) fn after(&self, state: &State, target: &DraftLocation, content: Option<&PublishedContent>,
        principal: &str, operation: OperationId, sequence: Sequence, time: Timestamp)
        -> Result<(FileWorkingState, Option<NodeOrigin>)>
    {
        self.require_fence(state, target, principal, operation)?;
        let working = &state.file_containers.get(&target.file).ok_or_else(invalid)?.working;
        let revision = working.revision().next().map_err(|e| Error::FileDraft(DraftError::RevisionExhausted(e)))?;
        let prior = working.automatic_controller().ok_or_else(stale)?;
        let cause = content.map(|body| NodeOrigin::from_content(state, target, body, self.reason(),
            operation, sequence, time)).transpose()?;
        let controller = match &cause {
            Some(origin) => prior.published(AutomaticNodeDecision { epoch: self.epoch,
                save: self.decision, node: origin.node, operation, sequence, published_at: time }, revision)
                .map_err(|e| Error::FileDraft(DraftError::Controller(e)))?,
            None if working.unborn().is_none() => prior.consumed_manually(), // equal-base, no node
            None => return Err(invalid()),
        };
        Ok((FileWorkingState::Automatic { revision, controller, body: AutomaticBody::Clean {} }, cause))
    }
}

pub(crate) fn stale() -> Error { Error::FileDraft(DraftError::SourceChanged) }
