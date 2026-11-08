//! SYSTEM-only selection of one remaining terminal registration. This is a
//! durable unfinished decision, not a registry successor, Stop, D3 or receipt.
use super::*;
use crate::{DiskRegistryCommitment, object_hash};
use data_serialization::{Deserialize, Serialize};

pub(super) const INTENT: &str = "retirement-intent.json";
pub(super) const SELECTED: &str = "retirement-selected.json";
const DIGEST_KIND: &str = "terminal-registration-retirement-intent";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "terminal-registration-retirement")]
    Retirement,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Source {
    pool_id: String,
    terminal_operation_id: OperationId,
    terminal_authority_hash: String,
    registry: DiskRegistryCommitment,
    #[serialization(with = "storage_contract::u64_string")]
    sequence: u64,
    #[serialization(with = "storage_contract::u64_string")]
    topology_generation: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Intent {
    format: Format,
    operation_id: OperationId,
    principal: String,
    source: Source,
    target: DiskRegistrationIdentity,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Selection {
    format: Format,
    operation_id: OperationId,
    intent_digest: String,
}

/// Captured from a live, authenticated no-voter owner. No field can be replaced
/// by an external caller. The same owner revalidates all fields before staging.
#[derive(Clone, Debug)]
pub struct TerminalRetirementPlan {
    intent: Intent,
    digest: String,
}
impl TerminalRetirementPlan {
    pub fn operation_id(&self) -> OperationId {
        self.intent.operation_id
    }
    pub fn principal(&self) -> &str {
        &self.intent.principal
    }
    pub fn target(&self) -> &DiskRegistrationIdentity {
        &self.intent.target
    }
    pub fn digest(&self) -> &str {
        &self.digest
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalRetirementPhase {
    IntentStored,
    DecisionStored,
}
/// Explicitly unfinished: no OperationResult, Published state or new registry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalRetirementStatus {
    pub operation_id: OperationId,
    pub principal: String,
    pub target: DiskRegistrationIdentity,
    pub intent_digest: String,
    pub phase: TerminalRetirementPhase,
}
struct Pending {
    plan: TerminalRetirementPlan,
    selected: bool,
}
impl Pending {
    fn status(&self) -> TerminalRetirementStatus {
        TerminalRetirementStatus {
            operation_id: self.plan.operation_id(),
            principal: self.plan.principal().to_owned(),
            target: self.plan.target().clone(),
            intent_digest: self.plan.digest.clone(),
            phase: if self.selected {
                TerminalRetirementPhase::DecisionStored
            } else {
                TerminalRetirementPhase::IntentStored
            },
        }
    }
}
impl TerminalManagement {
    /// Capture an exact remaining registration under current terminal authority.
    /// A pending decision must be recovered by its own ID/digest, not replaced.
    pub fn preview_terminal_retirement(
        &self,
        disk_id: &str,
        principal: &str,
        operation_id: OperationId,
    ) -> Result<TerminalRetirementPlan> {
        self.store.verify_writer(&self._locks)?;
        self.require_no_terminal_retirement()?;
        Engine::require_actor(principal)?;
        let target = self
            .registered_disks()
            .find(|disk| disk.disk_id == disk_id)
            .ok_or_else(|| {
                failure(
                    ErrorCode::NotFound,
                    "terminal retirement target is not currently registered",
                )
            })?;
        let intent = Intent {
            format: Format::Retirement,
            operation_id,
            principal: principal.into(),
            source: self.retirement_source(),
            target: target.identity(),
        };
        self.validate_retirement(&intent)?;
        Ok(TerminalRetirementPlan {
            digest: object_hash(DIGEST_KIND, &intent)?,
            intent,
        })
    }
    /// Persist the unchanged original selection only. No target is opened or
    /// removed. Metadata resumption remains fenced until a successor protocol
    /// explicitly consumes this unfinished decision; this slice has no cancel.
    pub fn stage_terminal_retirement(
        &mut self,
        plan: &TerminalRetirementPlan,
    ) -> Result<TerminalRetirementStatus> {
        self.store.verify_writer(&self._locks)?;
        self.validate_retirement(&plan.intent)?;
        if object_hash(DIGEST_KIND, &plan.intent)? != plan.digest {
            return Err(failure(
                ErrorCode::ObjectCorrupt,
                "terminal retirement plan digest differs",
            ));
        }
        if let Some(pending) = self.load_terminal_retirement()? {
            if pending.plan.intent != plan.intent || pending.plan.digest != plan.digest {
                return Err(failure(
                    ErrorCode::IdempotencyKeyReused,
                    "another exact terminal retirement is already pending",
                ));
            }
        }
        let directory = self.store.operation();
        // The immutable intent is durable before the selection. Repeating these
        // writes re-establishes fsync barriers after an uncertain earlier return.
        directory.write_json(&directory.path().join(INTENT), &plan.intent, false)?;
        let stored = self.load_terminal_retirement()?.ok_or_else(|| {
            failure(
                ErrorCode::RecoveryRequired,
                "retirement intent disappeared after publication",
            )
        })?;
        if stored.plan.intent != plan.intent || stored.plan.digest != plan.digest {
            return Err(failure(
                ErrorCode::LogFork,
                "retirement intent changed before selection",
            ));
        }
        self.store.verify_writer(&self._locks)?;
        let selection = Selection {
            format: Format::Retirement,
            operation_id: plan.operation_id(),
            intent_digest: plan.digest.clone(),
        };
        directory.write_json(&directory.path().join(SELECTED), &selection, false)?;
        let pending = self.load_terminal_retirement()?.ok_or_else(|| {
            failure(
                ErrorCode::RecoveryRequired,
                "selected retirement disappeared",
            )
        })?;
        if !pending.selected {
            return Err(failure(
                ErrorCode::RecoveryRequired,
                "retirement selection is not readable",
            ));
        }
        self.store.verify_writer(&self._locks)?;
        Ok(pending.status())
    }
    /// Cold continuation can only restage the saved actor/operation/digest.
    /// It neither supplies a new target nor reports deregistration complete.
    pub fn resume_terminal_retirement(
        &mut self,
        principal: &str,
        operation_id: OperationId,
        intent_digest: &str,
    ) -> Result<TerminalRetirementStatus> {
        Engine::require_actor(principal)?;
        let pending = self.load_terminal_retirement()?.ok_or_else(|| {
            failure(
                ErrorCode::NotFound,
                "original terminal retirement was not found",
            )
        })?;
        if pending.plan.principal() != principal
            || pending.plan.operation_id() != operation_id
            || pending.plan.digest() != intent_digest
        {
            return Err(failure(
                ErrorCode::IdempotencyKeyReused,
                "terminal retirement recovery differs from its exact original actor, operation or digest",
            ));
        }
        self.stage_terminal_retirement(&pending.plan)
    }
    /// Reads bounded SYSTEM records and revalidates source/selection. Absence of
    /// the intent with a selected marker is corruption, never an empty request.
    pub fn terminal_retirement_status(&self) -> Result<Option<TerminalRetirementStatus>> {
        self.store.verify_writer(&self._locks)?;
        Ok(self
            .load_terminal_retirement()?
            .map(|pending| pending.status()))
    }
    pub(in crate::engine) fn require_no_terminal_retirement(&self) -> Result<()> {
        if self.load_terminal_retirement()?.is_some() {
            return Err(failure(
                ErrorCode::RecoveryRequired,
                "an unfinished terminal retirement blocks metadata resumption; preserve and recover its exact original decision",
            ));
        }
        Ok(())
    }
    pub(super) fn validate_terminal_retirement(&self) -> Result<()> {
        self.load_terminal_retirement().map(|_| ())
    }
    fn retirement_source(&self) -> Source {
        Source {
            pool_id: self.original_configuration().pool_id.clone(),
            terminal_operation_id: self.verified.authority.operation_id(),
            terminal_authority_hash: self.store.authority_hash().into(),
            registry: self.registry().commitment().clone(),
            sequence: self.state.sequence,
            topology_generation: self.state.topology_generation,
        }
    }
    fn validate_retirement(&self, intent: &Intent) -> Result<()> {
        Engine::require_actor(&intent.principal)?;
        if intent.source != self.retirement_source() {
            return Err(failure(
                ErrorCode::GenerationMismatch,
                "terminal retirement source differs from the authenticated current authority/registry boundary",
            ));
        }
        if self.operation(intent.operation_id).is_some() {
            return Err(failure(
                ErrorCode::IdempotencyKeyReused,
                "terminal retirement ID already belongs to permanent history",
            ));
        }
        let registered = self
            .registered_disks()
            .find(|disk| disk.disk_id == intent.target.disk_id)
            .ok_or_else(|| {
                failure(
                    ErrorCode::DiskIdentityMismatch,
                    "terminal retirement target is no longer a retained registration",
                )
            })?;
        if registered.identity() != intent.target || registered.metadata_member {
            return Err(failure(
                ErrorCode::DiskIdentityMismatch,
                "terminal retirement must select the exact retained no-voter binding",
            ));
        }
        Ok(())
    }
    fn load_terminal_retirement(&self) -> Result<Option<Pending>> {
        let directory = self.store.operation();
        let intent = store::optional::<Intent>(directory, &directory.path().join(INTENT))?;
        let selected = store::optional::<Selection>(directory, &directory.path().join(SELECTED))?;
        let Some(intent) = intent else {
            if selected.is_some() {
                return Err(failure(
                    ErrorCode::ObjectCorrupt,
                    "terminal retirement selection has no exact original intent",
                ));
            }
            return Ok(None);
        };
        self.validate_retirement(&intent)?;
        let digest = object_hash(DIGEST_KIND, &intent)?;
        if let Some(selection) = &selected
            && (selection.operation_id != intent.operation_id || selection.intent_digest != digest)
        {
            return Err(failure(
                ErrorCode::LogFork,
                "terminal retirement selection differs from its original intent",
            ));
        }
        Ok(Some(Pending {
            plan: TerminalRetirementPlan { intent, digest },
            selected: selected.is_some(),
        }))
    }
}
pub(super) fn require_no_evidence(store: &TerminalStore) -> Result<()> {
    let directory = store.operation();
    // At this early resume gate we must not treat malformed evidence as absent.
    // The ordinary terminal owner performs full semantic validation afterwards.
    for name in [INTENT, SELECTED] {
        if directory
            .open_optional_read_regular(&directory.path().join(name))?
            .is_some()
        {
            return Err(failure(
                ErrorCode::RecoveryRequired,
                "terminal retirement evidence forbids metadata resumption before its exact successor is resolved",
            ));
        }
    }
    Ok(())
}
fn failure(code: ErrorCode, message: &'static str) -> Error {
    Error::new(code, message)
}
#[cfg(test)]
#[path = "../../../../../../test/engine/unit/engine/terminal_retirement_decision_tests.rs"]
mod tests;
