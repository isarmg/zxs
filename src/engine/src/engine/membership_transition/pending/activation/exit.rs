//! Prescribed lifecycle generations. Control decisions, registry graphs and the
//! frozen source precede D3; the final publication never appears in its own input.
use super::*;
use crate::engine::disk_registry::published::occurrence::FrozenDetachSource;
use crate::engine::disk_registry::{
    DiskRegistrationIdentity, DiskRegistryCommitment, RegistryExitEffect, RegistryExitRequest,
    VerifiedDiskRegistry,
};

mod runtime;
pub(in crate::engine) mod stop_plan;
pub(in crate::engine) use runtime::PendingDiskExit;
pub(in crate::engine::membership_transition) mod replay;
mod verify;

/// Metadata-only progress, never inferred from online member counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskExitPhase {
    Fenced,
    Activating,
    Published,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskExitStatus {
    pub operation_id: OperationId,
    pub transition_hash: String,
    pub target: DiskRegistrationIdentity,
    pub action: MembershipExitAction,
    pub phase: DiskExitPhase,
}
impl Engine {
    /// A prepared/stopped runtime must resume activation directly; rebuilding
    /// old control evidence at that point would touch a forbidden directory.
    pub fn disk_exit_status(&self) -> Result<Option<DiskExitStatus>> {
        let (intent, phase) = if let Some(fence) = &self.membership_fence {
            let activating = self.pending_disk_exit.as_ref().is_some_and(|pending| {
                pending.generation.intent.operation_id == fence.intent.operation_id
                    && pending.generation.intent.effect == fence.intent.effect
            });
            (
                &fence.intent,
                if activating {
                    DiskExitPhase::Activating
                } else {
                    DiskExitPhase::Fenced
                },
            )
        } else if let Some(generation) = self.config.generation()
            && let Some(core) = generation.portable.exit_core()
        {
            let published = self
                .current_namespaces
                .as_ref()
                .is_some_and(|view| view.registry() == core.registry())
                && self
                    .state
                    .operations
                    .contains_key(&generation.intent.operation_id);
            (
                &generation.intent,
                if published {
                    DiskExitPhase::Published
                } else {
                    DiskExitPhase::Activating
                },
            )
        } else {
            return Ok(None);
        };
        let Some(effect) = &intent.effect else {
            return Ok(None);
        };
        Ok(Some(DiskExitStatus {
            operation_id: intent.operation_id,
            transition_hash: object_hash(INTENT_KIND, intent)?,
            target: effect.identity(&intent.origin.config().pool_id),
            action: effect.action,
            phase,
        }))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
enum ExitFormat {
    #[serialization(rename = "disk-exit-generation")]
    DiskExit,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct GraphObject {
    kind: String,
    value: json_codec::Value,
}

/// The exact graph is carried with the portable authority so recovery does not
/// read a stopped root, trust a local cache, or reconstruct missing authority.
/// The existing MAX_METADATA bundle limit is checked before any exit is applied.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(in crate::engine) struct ExitCore {
    format: ExitFormat,
    pub(in crate::engine::membership_transition) joint: PortableJoint,
    pub(in crate::engine::membership_transition) archive: ArchiveRoot,
    predecessor_registry: DiskRegistryCommitment,
    #[serialization(default, skip_serializing_if = "Option::is_none")]
    continuation: Option<super::registry::RegistryContinuation>,
    registry: DiskRegistryCommitment,
    #[serialization(with = "storage_contract::u64_string")]
    source_business_generation: u64,
    #[serialization(with = "storage_contract::u64_string")]
    source_topology_generation: u64,
    objects: BTreeMap<String, GraphObject>,
}

impl ExitCore {
    pub(in crate::engine::membership_transition) fn registry(&self) -> &DiskRegistryCommitment {
        &self.registry
    }

    pub(super) fn registry_parent(&self) -> Option<&DiskRegistryCommitment> {
        self.continuation
            .as_ref()
            .map(super::registry::RegistryContinuation::parent)
    }

    fn predecessor(&self, intent: &TransitionIntent) -> Result<VerifiedDiskRegistry> {
        let boundary = intent.boundary.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::PublicationInvalid,
                "exit requires its frozen boundary",
            )
        })?;
        match &self.continuation {
            Some(continuation) => {
                if continuation.registry() != &self.predecessor_registry
                    || continuation.source_business_generation() != self.source_business_generation
                    || continuation.source_topology_generation() != self.source_topology_generation
                {
                    return Err(Error::new(
                        ErrorCode::PublicationInvalid,
                        "exit source differs from its registry continuation",
                    ));
                }
                continuation.verify(intent, intent.origin.config())
            }
            None => VerifiedDiskRegistry::verify_frozen_source(
                intent.origin.config(),
                intent.origin.predecessor(),
                &boundary.snapshot_hash,
                boundary.sequence,
                self.source_topology_generation,
                &self.predecessor_registry,
                |kind, hash| self.read(kind, hash),
            ),
        }
    }

    pub(super) fn verified_registry(&self) -> Result<VerifiedDiskRegistry> {
        let intent = self.intent()?;
        self.predecessor(&intent)?
            .verify_successor(&self.registry, |kind, hash| self.read(kind, hash))
    }

    fn read(&self, kind: &'static str, hash: &str) -> Result<Vec<u8>> {
        let object = self.objects.get(hash).ok_or_else(|| {
            Error::new(
                ErrorCode::ObjectCorrupt,
                "exit authority graph object is missing",
            )
        })?;
        if object.kind != kind {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "exit graph object kind differs",
            ));
        }
        let prepared = PreparedMetadataObject::new(kind, &object.value, MAX_METADATA)?;
        if prepared.hash != hash {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "exit graph object hash differs",
            ));
        }
        Ok(prepared.bytes().to_vec())
    }

    pub(in crate::engine::membership_transition) fn intent(&self) -> Result<TransitionIntent> {
        let verified = self.joint.verify()?;
        let intent = verified.intent;
        self.archive.verify_scope(&intent)?;
        let effect = intent.effect.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::PublicationInvalid,
                "exit authority lacks an exit effect",
            )
        })?;
        effect.validate(intent.origin.config(), &intent.target)?;
        if object_hash("membership-control-archive", &self.archive)?
            != verified.proposal.control_archive_root
        {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "exit archive differs from joint authority",
            ));
        }
        intent.boundary.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "exit requires an authenticated source publication",
            )
        })?;
        let predecessor = self.predecessor(&intent)?;
        let successor =
            predecessor.verify_successor(&self.registry, |kind, hash| self.read(kind, hash))?;
        let request = successor
            .pending_exit()
            .expect("invariant: verified successor has exit");
        let hash = object_hash(INTENT_KIND, &intent)?;
        if request.target != effect.identity(&intent.origin.config().pool_id)
            || request.operation_id != intent.operation_id
            || request.principal != intent.principal
            || request.intent_hash != hash
            || request.request_digest != hash
            || !matches!(
                (&effect.action, &request.effect),
                (
                    MembershipExitAction::PlannedOffline,
                    RegistryExitEffect::Disable
                ) | (
                    MembershipExitAction::Deregister,
                    RegistryExitEffect::Detach { .. }
                )
            )
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "exit registry differs from exact intent",
            ));
        }
        Ok(intent)
    }

    pub(super) fn registry_from_source(&self, state: &State) -> Result<VerifiedDiskRegistry> {
        let intent = self.intent()?;
        let boundary = intent
            .boundary
            .as_ref()
            .expect("invariant: exit has source publication");
        if state.sequence != boundary.sequence
            || state.business_generation != self.source_business_generation
            || state.topology_generation != self.source_topology_generation
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "exit source state differs from frozen boundary",
            ));
        }
        let predecessor = self.predecessor(&intent)?;
        if let Some(continuation) = &self.continuation {
            continuation.verify_source(&intent, state)?;
        }
        let successor =
            predecessor.verify_successor(&self.registry, |kind, hash| self.read(kind, hash))?;
        if self.continuation.is_none()
            && let RegistryExitEffect::Detach {
                occurrence_mask_hash,
            } = &successor
                .pending_exit()
                .expect("invariant: exit successor")
                .effect
        {
            let source = FrozenDetachSource::capture_state(
                state,
                intent.origin.config(),
                Some(&boundary.snapshot_hash),
                intent.origin.predecessor(),
                &predecessor,
                &successor
                    .pending_exit()
                    .expect("invariant: exit successor")
                    .target,
            )
            .map_err(projection_error)?;
            source
                .verify_mask(occurrence_mask_hash, |kind, hash| self.read(kind, hash))
                .map_err(projection_error)?;
        }
        Ok(successor)
    }

    fn prepare(engine: &Engine, joint: PortableJoint, archive: ArchiveRoot) -> Result<Self> {
        let verified = joint.verify()?;
        let intent = &verified.intent;
        let effect = intent.effect.as_ref().ok_or_else(|| {
            Error::new(ErrorCode::ConfigConflict, "pending decision is not an exit")
        })?;
        let boundary = intent.boundary.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "exit has no source publication",
            )
        })?;
        if engine.state.sequence != boundary.sequence
            || engine.snapshot.as_ref() != Some(&boundary.snapshot_hash)
            || engine.config.generation_hash() != intent.origin.predecessor()
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "live exit source advanced beyond its fence",
            ));
        }
        let mut objects = BTreeMap::new();
        let continuation = match engine
            .config
            .generation()
            .map(|generation| generation.verified_registry())
            .transpose()?
            .flatten()
        {
            Some(previous) => {
                if engine
                    .current_namespaces
                    .as_ref()
                    .is_none_or(|view| view.registry() != previous.commitment())
                {
                    return Err(Error::new(
                        ErrorCode::RecoveryRequired,
                        "exit continuation lacks its admitted source namespace",
                    ));
                }
                Some(super::registry::RegistryContinuation::prepare(
                    &previous,
                    intent,
                    &engine.state,
                    intent.origin.config(),
                )?)
            }
            None => None,
        };
        let prepared = match &continuation {
            Some(core) => core
                .verify(intent, intent.origin.config())?
                .prepared_copy()?,
            None => engine.prepare_disk_registry_base()?,
        };
        add_objects(&mut objects, prepared.objects())?;
        let predecessor = prepared.verify_with(|kind, hash| read_objects(&objects, kind, hash))?;
        let target = effect.identity(&engine.config.pool_id);
        let registry_effect = match effect.action {
            MembershipExitAction::PlannedOffline => RegistryExitEffect::Disable,
            MembershipExitAction::Deregister => {
                let source = match &continuation {
                    Some(_) => FrozenDetachSource::capture_current(
                        &engine.state,
                        &engine.config,
                        &boundary.snapshot_hash,
                        intent
                            .origin
                            .predecessor()
                            .expect("invariant: continued registry parent"),
                        engine
                            .current_namespaces
                            .as_deref()
                            .expect("invariant: admitted namespace checked"),
                        &predecessor,
                        &target,
                    )
                    .map_err(projection_error)?,
                    None => FrozenDetachSource::capture(engine, &predecessor, &target)
                        .map_err(projection_error)?,
                };
                let mask = source.prepare_mask().map_err(projection_error)?;
                add_objects(&mut objects, mask.objects())?;
                source
                    .verify_mask(mask.hash(), |kind, hash| read_objects(&objects, kind, hash))
                    .map_err(projection_error)?;
                RegistryExitEffect::Detach {
                    occurrence_mask_hash: mask.hash().to_owned(),
                }
            }
        };
        let transition = object_hash(INTENT_KIND, intent)?;
        let successor = predecessor.prepare_exit(RegistryExitRequest {
            target,
            operation_id: intent.operation_id,
            principal: intent.principal.clone(),
            request_digest: transition.clone(),
            intent_hash: transition,
            effect: registry_effect,
        })?;
        add_objects(&mut objects, successor.objects())?;
        let core = Self {
            format: ExitFormat::DiskExit,
            joint,
            archive,
            predecessor_registry: predecessor.commitment().clone(),
            continuation,
            registry: successor.commitment().clone(),
            source_business_generation: engine.state.business_generation,
            source_topology_generation: engine.state.topology_generation,
            objects,
        };
        core.registry_from_source(&engine.state)?;
        Ok(core)
    }
}

pub(super) fn read_objects(
    objects: &BTreeMap<String, GraphObject>,
    kind: &'static str,
    hash: &str,
) -> Result<Vec<u8>> {
    let object = objects
        .get(hash)
        .ok_or_else(|| Error::new(ErrorCode::ObjectCorrupt, "exit graph page is missing"))?;
    if object.kind != kind {
        return Err(Error::new(
            ErrorCode::ObjectCorrupt,
            "exit graph kind differs",
        ));
    }
    Ok(json_codec::to_vec(&object.value)?)
}
pub(super) fn add_objects<'a>(
    objects: &mut BTreeMap<String, GraphObject>,
    values: impl Iterator<Item = (&'static str, &'a str, &'a [u8])>,
) -> Result<()> {
    for (kind, hash, bytes) in values {
        let value = decode_strict(bytes)?;
        let object = GraphObject {
            kind: kind.to_owned(),
            value,
        };
        if let Some(previous) = objects.insert(hash.to_owned(), object.clone())
            && (previous.kind != object.kind || previous.value != object.value)
        {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "exit graph hash collision",
            ));
        }
    }
    Ok(())
}
fn projection_error(error: impl std::error::Error + Send + Sync + 'static) -> Error {
    Error::from(std::io::Error::other(error))
        .context(ErrorCode::PublicationInvalid, "exit namespace proof")
}

/// Live admission requires exact D3, actual retained journals, durable Stop
/// restrictions and physical I/O closure. Archived and preflight admission are
/// validation-only and can never install a live Engine namespace.
/// It is not serializable and no caller can manufacture its fields.
enum PublicationAdmission {
    Live,
    Archived,
    Preflight,
}

pub(in crate::engine) struct AdmittedDiskExitPublication {
    admission: PublicationAdmission,
    registry: DiskRegistryCommitment,
    occurrence_mask_hash: Option<String>,
    source_snapshot: String,
    source_sequence: u64,
    source_business_generation: u64,
    published_snapshot: String,
    published_sequence: u64,
}
impl AdmittedDiskExitPublication {
    pub(in crate::engine) fn permits_engine_install(&self) -> bool {
        matches!(self.admission, PublicationAdmission::Live)
    }
    pub(in crate::engine) fn registry(&self) -> &DiskRegistryCommitment {
        &self.registry
    }
    pub(in crate::engine) fn occurrence_mask_hash(&self) -> Option<&str> {
        self.occurrence_mask_hash.as_deref()
    }
    pub(in crate::engine) fn source_snapshot(&self) -> &str {
        &self.source_snapshot
    }
    pub(in crate::engine) fn source_sequence(&self) -> u64 {
        self.source_sequence
    }
    pub(in crate::engine) fn source_business_generation(&self) -> u64 {
        self.source_business_generation
    }
    pub(in crate::engine) fn published_snapshot(&self) -> &str {
        &self.published_snapshot
    }
    pub(in crate::engine) fn published_sequence(&self) -> u64 {
        self.published_sequence
    }
}

#[cfg(test)]
#[path = "../../../../../../../test/engine/unit/engine/disk_exit_publication_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../../../../test/engine/unit/engine/membership_retained_exit_adversarial_tests.rs"]
mod retained_exit_adversarial_tests;
