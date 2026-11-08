//! Exact sequential management-only registry and occurrence-mask successors.
//! Prepared namespaces are not active. No target pathname is resolved in recovery.
use super::*;
mod portable;
mod stopped;
use crate::{
    RegistryExitEffect, RegistryExitRequest,
    disk::MAX_METADATA,
    engine::disk_lifecycle::bootstrap::store::PreparedStop,
    engine::disk_registry::published::occurrence::{FrozenDetachSource, ProjectionError},
    journal::PreparedMetadataObject,
};
pub use portable::{TerminalRetirementArchive, TerminalRetirementCheckpoint};
use std::collections::BTreeMap;
pub(super) use stopped::EXISTING;

pub(super) const SUCCESSOR: &str = "retirement-successor.json";
pub(super) const STOP: &str = "retirement-stop.json";
pub(super) const COMPLETE: &str = "retirement-complete.json";
const SUCCESSOR_KIND: &str = "terminal-retirement-registry-successor";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct GraphObject {
    kind: String,
    value: json_codec::Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
enum SuccessorFormat {
    #[serialization(rename = "terminal-retirement-masked-registry")]
    MaskedRegistry,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Successor {
    format: SuccessorFormat,
    intent_digest: String,
    registry: DiskRegistryCommitment,
    occurrence_mask_hash: String,
    objects: BTreeMap<String, GraphObject>,
}
struct VerifiedSuccessor {
    registry: TerminalRegistry,
    namespaces: PreparedCurrentNamespaces,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct StopPlan {
    intent_digest: String,
    successor_hash: String,
    stop: PreparedStop,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Completion {
    format: Format,
    intent_digest: String,
    successor_hash: String,
    result: OperationResult,
}

pub(in crate::engine::disk_lifecycle::terminal) struct Completed {
    pub(in crate::engine::disk_lifecycle::terminal) registry: TerminalRegistry,
    namespaces: PreparedCurrentNamespaces,
    result: OperationResult,
    principal: String,
    plan: TerminalRetirementPlan,
    completion_hash: String,
    // D3 is process ownership too, not a stored assertion that an FD is closed.
    _stop: RecoveredDiskIo,
}

/// Constructed only from the held SYSTEM owner, its exact selected request and
/// the fully reverified immutable successor. Bootstrap bytes alone cannot make it.
pub(in crate::engine) struct VerifiedRetirementStopSource {
    target: DiskRegistrationIdentity,
    operation: OperationId,
    digest: IntentDigest,
    epoch: Option<String>,
}
impl VerifiedRetirementStopSource {
    pub(in crate::engine) fn identity(&self) -> &DiskRegistrationIdentity {
        &self.target
    }
    pub(in crate::engine) fn operation_id(&self) -> OperationId {
        self.operation
    }
    pub(in crate::engine) fn digest(&self) -> IntentDigest {
        self.digest
    }
    pub(in crate::engine) fn epoch(&self) -> Option<&str> {
        self.epoch.as_deref()
    }
    fn closed(&self) -> Result<RecoveredDiskIo> {
        RecoveredDiskIo::admit_terminal(&VerifiedTerminalStop {
            identity: self.target.clone(),
            admission_epoch: self.epoch.clone(),
        })
    }
}
impl Successor {
    fn read(&self, kind: &'static str, hash: &str) -> Result<Vec<u8>> {
        let object = self.objects.get(hash).ok_or_else(|| {
            failure(
                ErrorCode::ObjectCorrupt,
                "terminal retirement registry object missing",
            )
        })?;
        if object.kind != kind {
            return Err(failure(
                ErrorCode::ObjectCorrupt,
                "retirement registry object kind differs",
            ));
        }
        let prepared = PreparedMetadataObject::new(kind, &object.value, MAX_METADATA)?;
        if prepared.hash != hash {
            return Err(failure(
                ErrorCode::ObjectCorrupt,
                "retirement registry object hash differs",
            ));
        }
        Ok(prepared.bytes().to_vec())
    }
    fn hash(&self) -> Result<String> {
        Ok(PreparedMetadataObject::new(SUCCESSOR_KIND, self, MAX_METADATA)?.hash)
    }
}
impl TerminalManagement {
    pub fn completed_terminal_retirement(&self) -> Option<&OperationResult> {
        self.retirement_completion
            .as_ref()
            .map(|completed| &completed.result)
            .or_else(|| {
                self.retirement_history
                    .last()
                    .map(HistoricalCompleted::result)
            })
    }

    pub(super) fn require_retirement_stop_target(
        &self,
        target: &DiskRegistrationIdentity,
    ) -> Result<()> {
        let bootstrap = open_store(self.store.system().path())?;
        let locator = bootstrap
            .reload(&target.pool_id)
            .map_err(management_error)?;
        if self
            .existing_retirement_stop(&locator, target, self.registry())?
            .is_none()
        {
            locator
                .require_terminal_retirement_target(target)
                .map_err(management_error)?;
        }
        Ok(())
    }
    fn selected_retirement(
        &self,
        plan: &TerminalRetirementPlan,
    ) -> Result<VerifiedRetirementStopSource> {
        self.store.verify_writer(&self._locks)?;
        let pending = self.load_terminal_retirement()?.ok_or_else(|| {
            failure(
                ErrorCode::RecoveryRequired,
                "retirement lost its original selected request",
            )
        })?;
        if !pending.selected
            || pending.plan.intent != plan.intent
            || pending.plan.digest != plan.digest
        {
            return Err(failure(
                ErrorCode::LogFork,
                "retirement successor differs from the durable original selection",
            ));
        }
        Ok(VerifiedRetirementStopSource {
            target: plan.target().clone(),
            operation: plan.operation_id(),
            digest: IntentDigest::parse(plan.digest()).map_err(management_error)?,
            epoch: self
                .verified
                .authority
                .io_admission_epoch(&plan.target().disk_id)?,
        })
    }
    fn retirement_namespace_source<'a>(
        &'a self,
        plan: &TerminalRetirementPlan,
    ) -> Result<FrozenDetachSource<'a>> {
        FrozenDetachSource::capture_terminal(
            &self.verified.state,
            self.original_configuration(),
            self.registry_for_retirement_plan(plan),
            self.namespaces_for_retirement_plan(plan),
            plan.target(),
        )
        .map_err(projection_error)
    }
    fn prepare_retirement_successor(&self, plan: &TerminalRetirementPlan) -> Result<Successor> {
        let source = self.retirement_namespace_source(plan)?;
        let mask = source.prepare_mask().map_err(projection_error)?;
        let prepared = self
            .registry_for_retirement_plan(plan)
            .prepare_management_retirement(registry_request(plan, mask.hash()))?;
        let mut objects = BTreeMap::new();
        for (kind, hash, bytes) in prepared.objects().chain(mask.objects()) {
            let object = GraphObject {
                kind: kind.into(),
                value: crate::decode_strict(bytes)?,
            };
            if objects
                .insert(hash.to_owned(), object.clone())
                .is_some_and(|old| old != object)
            {
                return Err(failure(
                    ErrorCode::LogFork,
                    "terminal successor object collision",
                ));
            }
        }
        let successor = Successor {
            format: SuccessorFormat::MaskedRegistry,
            intent_digest: plan.digest.clone(),
            registry: prepared.commitment().clone(),
            occurrence_mask_hash: mask.hash().into(),
            objects,
        };
        successor.hash()?; // Bound the complete immutable envelope before staging.
        Ok(successor)
    }
    fn verify_retirement_successor(
        &self,
        plan: &TerminalRetirementPlan,
        successor: &Successor,
    ) -> Result<VerifiedSuccessor> {
        verify_successor(
            &self.verified,
            self.retirement_base_registry(),
            &self.retirement_base_namespaces,
            plan,
            successor,
        )
    }
    /// Complete only the captured original no-voter registration. A later
    /// request must extend a completed native predecessor. Previously stopped
    /// lifetimes require their original independent authority, never a flag.
    /// This does not admit active namespaces or metadata resumption.
    pub fn complete_terminal_retirement(
        &mut self,
        plan: &TerminalRetirementPlan,
    ) -> Result<OperationResult> {
        self.store.verify_writer(&self._locks)?;
        if let Some(result) =
            self.historical_retirement_result(plan.principal(), plan.operation_id(), plan.digest())?
        {
            return Ok(result.clone());
        }
        self.validate_retirement_proposal(plan)?;
        if object_hash(DIGEST_KIND, &plan.intent)? != plan.digest {
            return Err(failure(
                ErrorCode::ObjectCorrupt,
                "retirement completion plan digest differs",
            ));
        }
        // All pure successor/budget/overflow checks precede the first intent.
        let successor = self.prepare_retirement_successor(plan)?;
        let result = retirement_result_for(plan, &successor)?;
        if self.starts_new_retirement_frame(plan) || self.load_terminal_retirement()?.is_none() {
            self.require_retirement_stop_target(plan.target())?;
        }
        self.stage_terminal_retirement(plan)?;
        let source = self.selected_retirement(plan)?;
        let directory = self.retirement_directory();
        directory.write_json(&directory.path().join(SUCCESSOR), &successor, false)?;
        let installed: Successor = store::optional(directory, &directory.path().join(SUCCESSOR))?
            .ok_or_else(|| {
            failure(
                ErrorCode::RecoveryRequired,
                "retirement successor disappeared",
            )
        })?;
        self.verify_retirement_successor(plan, &installed)?;
        if installed.hash()? != successor.hash()? {
            return Err(failure(
                ErrorCode::LogFork,
                "installed retirement successor changed",
            ));
        }
        self.synchronize_retirement_stop(plan, &successor, &source)?;
        let closure = source.closed()?;
        let complete = Completion {
            format: Format::Retirement,
            intent_digest: plan.digest.clone(),
            successor_hash: successor.hash()?,
            result: result.clone(),
        };
        self.store.verify_writer(&self._locks)?;
        directory.write_json(&directory.path().join(COMPLETE), &complete, false)?;
        let completed = self.read_retirement_completion()?.ok_or_else(|| {
            failure(
                ErrorCode::RecoveryRequired,
                "retirement completion disappeared after publication",
            )
        })?;
        self.install_retirement_completion(completed)?;
        // Keep the first live exclusion until the installed completion owns its
        // independently admitted stop too. No target stat/open occurs here.
        drop(closure);
        Ok(result)
    }
    /// Exact permanent receipt, reauthenticated from SYSTEM and live exclusion.
    /// A root helper plan or saved JSON Completed bit is not a result authority.
    pub fn terminal_retirement_result(
        &self,
        principal: &str,
        operation: OperationId,
        digest: &str,
    ) -> Result<Option<OperationResult>> {
        Engine::require_actor(principal)?;
        self.store.verify_writer(&self._locks)?;
        if let Some(result) = self.historical_retirement_result(principal, operation, digest)? {
            return Ok(Some(result.clone()));
        }
        let Some(pending) = self.load_terminal_retirement()? else {
            return Ok(None);
        };
        if pending.plan.operation_id() != operation && self.retirement_completion.is_some() {
            return Ok(None);
        }
        if pending.plan.principal() != principal
            || pending.plan.operation_id() != operation
            || pending.plan.digest() != digest
        {
            return Err(failure(
                ErrorCode::IdempotencyKeyReused,
                "retirement query differs from original actor/operation/digest",
            ));
        }
        let complete = self.read_retirement_completion()?;
        if complete.is_some() && self.retirement_completion.is_none() {
            return Err(failure(
                ErrorCode::RecoveryRequired,
                "retirement marker exists but this owner must recover original durability barriers",
            ));
        }
        Ok(complete.map(|complete| complete.result))
    }
    /// Reconstruct the private typed plan, without allowing target/source edits.
    pub fn original_terminal_retirement(
        &self,
        principal: &str,
        operation: OperationId,
        digest: &str,
    ) -> Result<TerminalRetirementPlan> {
        Engine::require_actor(principal)?;
        self.store.verify_writer(&self._locks)?;
        if let Some(plan) = self.historical_retirement_plan(principal, operation, digest)? {
            return Ok(plan.clone());
        }
        let pending = self
            .load_terminal_retirement()?
            .ok_or_else(|| failure(ErrorCode::NotFound, "original retirement not found"))?;
        if pending.plan.operation_id() != operation && self.retirement_completion.is_some() {
            return Err(failure(
                ErrorCode::NotFound,
                "requested terminal retirement has not started",
            ));
        }
        if pending.plan.principal() != principal
            || pending.plan.operation_id() != operation
            || pending.plan.digest() != digest
        {
            return Err(failure(
                ErrorCode::IdempotencyKeyReused,
                "original retirement actor/operation/digest differs",
            ));
        }
        Ok(pending.plan)
    }
    pub(super) fn restore_retirement_frame(&mut self) -> Result<()> {
        if let Some(held) = self.read_retirement_completion()? {
            let pending = self.load_terminal_retirement()?.ok_or_else(|| {
                failure(
                    ErrorCode::RecoveryRequired,
                    "completed retirement lost its original request",
                )
            })?;
            // A visible rename after an uncertain return is not proof of fsync.
            // Cold admission repeats original barriers, never a cached flag.
            self.complete_terminal_retirement(&pending.plan)?;
            drop(held);
        }
        Ok(())
    }
    fn install_retirement_completion(&mut self, completed: Completed) -> Result<()> {
        let mut state = (*self.retirement_base_state).clone();
        state.sequence = completed.result.sequence;
        state.topology_generation = state.topology_generation.checked_add(1).ok_or_else(|| {
            failure(
                ErrorCode::ResourceLimit,
                "terminal retirement topology exhausted",
            )
        })?;
        state
            .operations
            .insert(completed.result.operation_id, completed.result.clone());
        state
            .operation_principals
            .insert(completed.result.operation_id, completed.principal.clone());
        self.state = Arc::new(state);
        self.retirement_completion = Some(completed);
        Ok(())
    }
    pub(super) fn read_retirement_completion(&self) -> Result<Option<Completed>> {
        let directory = self.retirement_directory();
        let successor = store::optional::<Successor>(directory, &directory.path().join(SUCCESSOR))?;
        let has_stop = directory
            .open_optional_read_regular(&directory.path().join(STOP))?
            .is_some()
            || directory
                .open_optional_read_regular(&directory.path().join(EXISTING))?
                .is_some();
        let complete = store::optional::<Completion>(directory, &directory.path().join(COMPLETE))?;
        let Some(successor) = successor else {
            if has_stop || complete.is_some() {
                return Err(failure(
                    ErrorCode::ObjectCorrupt,
                    "retirement Stop/completion lacks its successor",
                ));
            }
            return Ok(None);
        };
        let pending = self.load_terminal_retirement()?.ok_or_else(|| {
            failure(
                ErrorCode::ObjectCorrupt,
                "retirement successor lacks its original intent",
            )
        })?;
        let plan = &pending.plan;
        let source = self.selected_retirement(plan)?;
        let verified = self.verify_retirement_successor(plan, &successor)?;
        let Some(stop) = self.verify_retirement_stop_material(plan, &successor, &source)? else {
            if complete.is_some() {
                return Err(failure(
                    ErrorCode::ObjectCorrupt,
                    "retirement completion lacks its exact Stop plan",
                ));
            }
            return Ok(None);
        };
        let Some(complete) = complete else {
            return Ok(None);
        };
        self.require_durable_retirement_stop(&stop, plan, &source)?;
        let expected = Completion {
            format: Format::Retirement,
            intent_digest: plan.digest.clone(),
            successor_hash: successor.hash()?,
            result: retirement_result_for(plan, &successor)?,
        };
        if complete != expected {
            return Err(failure(
                ErrorCode::LogFork,
                "retirement completion differs from its exact native successor/result",
            ));
        }
        Ok(Some(Completed {
            registry: verified.registry,
            namespaces: verified.namespaces,
            completion_hash: object_hash("terminal-retirement-completion", &complete)?,
            result: complete.result,
            principal: plan.principal().into(),
            plan: plan.clone(),
            _stop: source.closed()?,
        }))
    }
}
fn require_stop(
    stop: &StopPlan,
    plan: &TerminalRetirementPlan,
    successor: &Successor,
) -> Result<()> {
    if stop.intent_digest != plan.digest || stop.successor_hash != successor.hash()? {
        return Err(failure(
            ErrorCode::LogFork,
            "retirement Stop plan differs from its exact successor",
        ));
    }
    Ok(())
}
pub(super) fn has_material(directory: &DiskRoot) -> Result<bool> {
    for name in [SUCCESSOR, STOP, EXISTING, COMPLETE] {
        if directory
            .open_optional_read_regular(&directory.path().join(name))?
            .is_some()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

impl Completed {
    pub(super) fn plan(&self) -> &TerminalRetirementPlan {
        &self.plan
    }
    pub(super) fn hash(&self) -> &str {
        &self.completion_hash
    }
    pub(super) fn namespaces(&self) -> &PreparedCurrentNamespaces {
        &self.namespaces
    }
}

/// Historical outcome and its live exclusion, without retaining another full
/// registry/State clone for every removed member.
pub(in crate::engine::disk_lifecycle::terminal) struct HistoricalCompleted {
    plan: TerminalRetirementPlan,
    result: OperationResult,
    _stop: RecoveredDiskIo,
}
impl HistoricalCompleted {
    pub(super) fn plan(&self) -> &TerminalRetirementPlan {
        &self.plan
    }
    pub(super) fn result(&self) -> &OperationResult {
        &self.result
    }
}
pub(super) struct CompletedPredecessor {
    pub(super) registry: TerminalRegistry,
    pub(super) namespaces: PreparedCurrentNamespaces,
    pub(super) historical: HistoricalCompleted,
}
impl Completed {
    pub(super) fn into_history(self) -> CompletedPredecessor {
        CompletedPredecessor {
            registry: self.registry,
            namespaces: self.namespaces,
            historical: HistoricalCompleted {
                plan: self.plan,
                result: self.result,
                _stop: self._stop,
            },
        }
    }
}
fn registry_request(
    plan: &TerminalRetirementPlan,
    occurrence_mask_hash: &str,
) -> RegistryExitRequest {
    RegistryExitRequest {
        target: plan.target().clone(),
        operation_id: plan.operation_id(),
        principal: plan.principal().into(),
        request_digest: plan.digest().into(),
        intent_hash: plan.digest().into(),
        effect: RegistryExitEffect::Detach {
            occurrence_mask_hash: occurrence_mask_hash.into(),
        },
    }
}
fn projection_error(error: ProjectionError) -> Error {
    Error::from(std::io::Error::other(error)).context(
        ErrorCode::PublicationInvalid,
        "terminal retirement must preserve the complete inherited namespace and exact occurrence removal",
    )
}

fn verify_successor(
    verified: &VerifiedTerminalAuthority,
    predecessor: &TerminalRegistry,
    namespaces: &PreparedCurrentNamespaces,
    plan: &TerminalRetirementPlan,
    successor: &Successor,
) -> Result<VerifiedSuccessor> {
    if successor.intent_digest != plan.digest {
        return Err(failure(
            ErrorCode::LogFork,
            "terminal successor names another retirement",
        ));
    }
    successor.hash()?;
    let registry = predecessor.verify_management_retirement(
        registry_request(plan, &successor.occurrence_mask_hash),
        &successor.registry,
        |kind, hash| successor.read(kind, hash),
    )?;
    let source = FrozenDetachSource::capture_terminal(
        &verified.state,
        verified.authority.original(),
        predecessor,
        namespaces,
        plan.target(),
    )
    .map_err(projection_error)?;
    let namespaces = source
        .verify_mask(&successor.occurrence_mask_hash, |kind, hash| {
            successor.read(kind, hash)
        })
        .and_then(|mask| mask.prepare_terminal_retirement_namespaces(&registry))
        .map_err(projection_error)?;
    Ok(VerifiedSuccessor {
        registry,
        namespaces,
    })
}

fn retirement_result_for(
    plan: &TerminalRetirementPlan,
    successor: &Successor,
) -> Result<OperationResult> {
    let sequence = plan.intent.source.sequence.checked_add(1).ok_or_else(|| {
        failure(
            ErrorCode::ResourceLimit,
            "terminal retirement sequence exhausted",
        )
    })?;
    let topology = plan
        .intent
        .source
        .topology_generation
        .checked_add(1)
        .ok_or_else(|| {
            failure(
                ErrorCode::ResourceLimit,
                "terminal retirement topology exhausted",
            )
        })?;
    Ok(OperationResult {
        operation_id: plan.operation_id(),
        kind: "disk-deregister".into(),
        commit_id: None,
        history_tree_id: None,
        branch_id: None,
        commit_state: storage_contract::CommitState::Published,
        protection_at_publish: storage_contract::ProtectionAtPublish::NotApplicable,
        current_health: storage_contract::CurrentHealth::Unknown,
        request_digest: plan.digest.clone(),
        sequence,
        result: json_codec::json!({
            "authority_kind":"terminal-retirement", "authority":successor.hash()?,
            "disk_id":plan.target().disk_id, "slot_id":plan.target().slot_id,
            "binding_generation":plan.target().binding_generation.to_string(),
            "registration":"detached", "target_io":"closed", "active_metadata_members":0,
            "topology_generation":topology.to_string(), "registry":successor.registry,
            "occurrence_mask_hash":successor.occurrence_mask_hash,
            "namespace_projection_verified":true,
            "active_namespace_admitted":false, "metadata_resumption_supported":false,
        }),
    })
}
